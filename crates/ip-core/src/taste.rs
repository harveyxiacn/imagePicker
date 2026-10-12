//! Personalised scoring (docs/03 §3.3).
//!
//! The user's explicit ratings and pick/reject flags are recorded as *preference labels*. At
//! training time they become pairwise preferences (A is preferred over B): any two labelled
//! photos of a session whose levels differ by at least one star, and a picked photo against its
//! unlabelled burst mates (half weight). A RankNet-style linear model (pairwise logistic
//! regression, L2 regularised, plain Rust) learns a score from
//! `[SigLIP embedding | score measures | scene one-hot]`.
//!
//! The model is only used when it orders held-out pairs (20% of the photos) clearly better than
//! the base `ai_score`. When active the stored score is `(1-α)·base + α·user` with α growing
//! from 0 to 0.6 with the number of labels. `photo.base_score` always keeps the unfused score.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::analysis::scoring::{score_photo, star_rating, FaceFeat, PhotoFeat, Ranked, Reason};
use crate::analysis::store::{self, FACE_FEAT_COLS};
use crate::analysis::vecs::decode_f16;
use crate::catalog::now_ms;
use crate::error::Result;
use crate::events::Event;
use crate::model::{PatchRequest, PhotoUpdate};
use crate::Core;

/// New labels between two automatic retrainings.
pub const RETRAIN_EVERY: i64 = 50;
/// Fewest labelled (analysed) photos worth training on.
pub const MIN_LABELLED_PHOTOS: usize = 30;
/// Fewest held-out pairs for the activation decision to mean anything.
pub const MIN_HOLDOUT_PAIRS: usize = 20;
pub const ALPHA_MAX: f64 = 0.6;
/// Label count at which α reaches [`ALPHA_MAX`].
pub const ALPHA_FULL_AT: f64 = 200.0;
const MAX_PAIRS: usize = 20_000;
const MAX_SAMPLES: usize = 3_000;
const EPOCHS: usize = 250;
const L2: f32 = 0.02;
const SCENES: [&str; 8] = [
    "portrait",
    "group",
    "landscape",
    "architecture",
    "night",
    "food",
    "pet",
    "other",
];
const MEASURES: usize = 9;

/// Fusion weight for `labels` labels.
pub fn alpha_for(labels: i64) -> f64 {
    (ALPHA_MAX * (labels as f64 / ALPHA_FULL_AT)).clamp(0.0, ALPHA_MAX)
}

// ------------------------------------------------------------------ features & model

/// `[embedding (dim) | 9 score measures | 8 scene one-hot]`.
pub fn features(feat: &PhotoFeat, emb: Option<&[f32]>, dim: usize) -> Vec<f32> {
    let mut v = Vec::with_capacity(dim + MEASURES + SCENES.len());
    match emb {
        Some(e) if e.len() == dim => v.extend_from_slice(e),
        _ => v.resize(dim, 0.0),
    }
    let sc = score_photo(feat);
    for m in [
        feat.sharpness,
        feat.exposure,
        feat.aesthetic,
        feat.iqa,
        feat.noise,
        feat.mean_luminance,
        feat.clipped_highlights,
        feat.crushed_shadows,
        sc.face_score,
    ] {
        v.push(m.unwrap_or(0.0) as f32);
    }
    for s in SCENES {
        v.push(if sc.effective_scene == s { 1.0 } else { 0.0 });
    }
    v
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub dim: usize,
    pub w: Vec<f32>,
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
    /// Mean / spread of the raw model score over the training photos.
    pub s_mu: f64,
    pub s_sd: f64,
    /// Mean / spread of the base score over the same photos (the user score is mapped onto
    /// the base score's scale so that the two can be blended).
    pub q_mu: f64,
    pub q_sd: f64,
}

impl Model {
    fn raw(&self, x: &[f32]) -> f64 {
        let mut s = 0.0f64;
        for (i, xi) in x.iter().enumerate().take(self.w.len()) {
            s += (self.w[i] * (xi - self.mean[i]) / self.std[i]) as f64;
        }
        s
    }

    /// The user's taste as a 0..1 score on the scale of the base score.
    pub fn user_score(&self, feat: &PhotoFeat, emb: Option<&[f32]>) -> f64 {
        let x = features(feat, emb, self.dim);
        let z = (self.raw(&x) - self.s_mu) / self.s_sd.max(1e-9);
        (self.q_mu + self.q_sd * z).clamp(0.0, 1.0)
    }
}

/// Blends the user score into one burst's base ranking and recomputes ranks and stars.
pub fn fuse(mut ranked: Vec<Ranked>, user: &HashMap<i64, f64>, alpha: f64) -> Vec<Ranked> {
    for r in &mut ranked {
        let base = r.base_q;
        let u = user.get(&r.photo_id).copied().unwrap_or(base);
        let fused = ((1.0 - alpha) * base + alpha * u).clamp(0.0, 1.0);
        let delta = fused - base;
        r.q = fused;
        if delta.abs() >= 1e-4 {
            r.contributions
                .push(crate::analysis::scoring::Contribution {
                    key: "taste".into(),
                    label_key: "score.taste".into(),
                    delta: (delta * 10_000.0).round() / 10_000.0,
                });
        }
    }
    ranked.sort_by(|a, b| {
        b.q.partial_cmp(&a.q)
            .unwrap()
            .then(a.rank.cmp(&b.rank))
            .then(a.photo_id.cmp(&b.photo_id))
    });
    let n = ranked.len();
    for (i, r) in ranked.iter_mut().enumerate() {
        r.rank = i;
        r.reasons.retain(|x| x.key != "best_in_group");
        if n >= 2 && i == 0 {
            r.reasons.insert(
                0,
                Reason {
                    key: "best_in_group".into(),
                    params: json!({"size": n}),
                },
            );
        }
        r.ai_rating = star_rating(r.q, n >= 2 && i == 0, r.hard_issue);
    }
    ranked
}

// ------------------------------------------------------------------ training

struct Pair {
    w: usize,
    l: usize,
    weight: f32,
}

struct Sample {
    photo_id: i64,
    feat: PhotoFeat,
    emb: Option<Vec<f32>>,
    base: f64,
    session: i64,
    burst: Option<i64>,
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Largest eigenvalue of `(1/n) Σ x xᵀ` by power iteration (for a safe step size).
fn top_eigen(x: &[Vec<f32>]) -> f32 {
    let d = x.first().map(Vec::len).unwrap_or(0);
    if d == 0 {
        return 1.0;
    }
    let mut v: Vec<f32> = (0..d).map(|i| 1.0 + (i % 7) as f32 * 0.1).collect();
    let mut lambda = 1.0;
    for _ in 0..12 {
        let norm = dot(&v, &v).sqrt().max(1e-12);
        v.iter_mut().for_each(|e| *e /= norm);
        let mut next = vec![0.0f32; d];
        for row in x {
            let p = dot(row, &v) / x.len() as f32;
            for (n, e) in next.iter_mut().zip(row) {
                *n += p * e;
            }
        }
        lambda = dot(&next, &v);
        v = next;
    }
    lambda.max(1e-3)
}

/// Pairwise logistic regression by full-batch gradient descent. Per-epoch cost is
/// O(photos x dims): pair gradients are first folded into one coefficient per photo.
fn fit(x: &[Vec<f32>], pairs: &[Pair]) -> Vec<f32> {
    let d = x.first().map(Vec::len).unwrap_or(0);
    let mut w = vec![0.0f32; d];
    if pairs.is_empty() {
        return w;
    }
    let total: f32 = pairs.iter().map(|p| p.weight).sum();
    let lr = 1.0 / (0.5 * top_eigen(x) + L2);
    let mut s = vec![0.0f32; x.len()];
    let mut g = vec![0.0f32; x.len()];
    for _ in 0..EPOCHS {
        for (si, row) in s.iter_mut().zip(x) {
            *si = dot(&w, row);
        }
        g.iter_mut().for_each(|e| *e = 0.0);
        for p in pairs {
            let c = -p.weight * sigmoid(s[p.l] - s[p.w]) / total;
            g[p.w] += c;
            g[p.l] -= c;
        }
        let mut grad: Vec<f32> = w.iter().map(|wi| L2 * wi).collect();
        for (gi, row) in g.iter().zip(x) {
            if *gi != 0.0 {
                for (a, e) in grad.iter_mut().zip(row) {
                    *a += gi * e;
                }
            }
        }
        for (wi, gi) in w.iter_mut().zip(&grad) {
            *wi -= lr * gi;
        }
    }
    w
}

fn accuracy(pairs: &[Pair], score: impl Fn(usize) -> f64) -> f64 {
    if pairs.is_empty() {
        return 0.0;
    }
    let mut hit = 0.0;
    for p in pairs {
        let (a, b) = (score(p.w), score(p.l));
        hit += if a > b {
            1.0
        } else if a == b {
            0.5
        } else {
            0.0
        };
    }
    hit / pairs.len() as f64
}

fn is_holdout(photo_id: i64) -> bool {
    ((photo_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 33).is_multiple_of(5)
}

/// Preference level of a photo from its labels (`None` = no usable label).
fn level_of(rating: Option<f64>, flag: Option<f64>) -> Option<f64> {
    match (rating, flag) {
        (_, Some(f)) if f < 0.0 => Some(0.0),
        (Some(r), Some(_)) => Some(r + 1.0),
        (Some(r), None) => Some(r),
        (None, Some(_)) => Some(4.5),
        (None, None) => None,
    }
}

struct Gathered {
    samples: Vec<Sample>,
    /// Preference level per sample (`None` for burst mates without a label).
    levels: Vec<Option<f64>>,
    pairs: Vec<Pair>,
}

fn load_samples(conn: &Connection, ids: &[i64]) -> Result<Vec<Sample>> {
    let mut out = Vec::new();
    for chunk in ids.chunks(400) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut faces = store::load_faces_feat(
            conn,
            &format!(
                "SELECT {FACE_FEAT_COLS} FROM face f WHERE f.photo_id IN ({ph}) ORDER BY f.id"
            ),
            chunk,
        )?;
        let mut st = conn.prepare(&format!(
            "SELECT p.id, p.scene_type, a.sharpness, a.exposure, a.noise, a.iqa, a.aesthetic,
                    a.clipped_highlights, a.crushed_shadows, a.mean_luminance, a.embedding,
                    COALESCE(p.base_score, p.ai_score), p.burst_id,
                    COALESCE((SELECT MIN(session_id) FROM session_photo WHERE photo_id=p.id), 0)
             FROM photo p JOIN analysis a ON a.photo_id=p.id WHERE p.id IN ({ph})"
        ))?;
        let rows = st
            .query_map(params_from_iter(chunk.iter()), |r| {
                Ok(Sample {
                    photo_id: r.get(0)?,
                    feat: PhotoFeat {
                        photo_id: r.get(0)?,
                        scene_type: r.get(1)?,
                        sharpness: r.get(2)?,
                        exposure: r.get(3)?,
                        noise: r.get(4)?,
                        iqa: r.get(5)?,
                        aesthetic: r.get(6)?,
                        clipped_highlights: r.get(7)?,
                        crushed_shadows: r.get(8)?,
                        mean_luminance: r.get(9)?,
                        faces: Vec::new(),
                        ..Default::default()
                    },
                    emb: r
                        .get::<_, Option<Vec<u8>>>(10)?
                        .map(|b| decode_f16(&b))
                        .filter(|e| !e.is_empty()),
                    base: r.get::<_, Option<f64>>(11)?.unwrap_or(0.5),
                    burst: r.get(12)?,
                    session: r.get(13)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for mut s in rows {
            s.feat.faces = faces.remove(&s.photo_id).unwrap_or_default();
            out.push(s);
        }
    }
    Ok(out)
}

fn gather(conn: &Connection) -> Result<Gathered> {
    // labels -> level per photo
    let mut ratings: BTreeMap<i64, f64> = BTreeMap::new();
    let mut flags: BTreeMap<i64, f64> = BTreeMap::new();
    {
        let mut st = conn.prepare("SELECT photo_id, kind, value FROM preference_label")?;
        let rows = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, f64>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (id, kind, v) in rows {
            if kind == "rating" {
                ratings.insert(id, v);
            } else {
                flags.insert(id, v);
            }
        }
    }
    let mut level: BTreeMap<i64, f64> = BTreeMap::new();
    for id in ratings.keys().chain(flags.keys()) {
        if let Some(l) = level_of(ratings.get(id).copied(), flags.get(id).copied()) {
            level.insert(*id, l);
        }
    }
    // bounded, deterministic subset of the labelled photos
    let mut ids: Vec<i64> = level.keys().copied().collect();
    if ids.len() > MAX_SAMPLES {
        let stride = ids.len().div_ceil(MAX_SAMPLES);
        ids = ids.into_iter().step_by(stride).collect();
    }
    let mut samples = load_samples(conn, &ids)?;
    // unlabelled mates of picked photos (implicit negatives)
    let picked_bursts: HashSet<i64> = samples
        .iter()
        .filter(|s| flags.get(&s.photo_id).is_some_and(|f| *f > 0.0))
        .filter_map(|s| s.burst)
        .collect();
    let mut mate_ids = Vec::new();
    for b in &picked_bursts {
        let mut st = conn.prepare("SELECT id FROM photo WHERE burst_id=?1")?;
        for id in st.query_map([b], |r| r.get::<_, i64>(0))? {
            let id = id?;
            if !level.contains_key(&id) {
                mate_ids.push(id);
            }
        }
    }
    samples.extend(load_samples(conn, &mate_ids)?);
    let levels: Vec<Option<f64>> = samples
        .iter()
        .map(|s| level.get(&s.photo_id).copied())
        .collect();

    // pairs; only between photos on the same side of the train/holdout split
    let mut pairs = Vec::new();
    let n = samples.len();
    for i in 0..n {
        for j in 0..n {
            if i == j || is_holdout(samples[i].photo_id) != is_holdout(samples[j].photo_id) {
                continue;
            }
            match (levels[i], levels[j]) {
                (Some(a), Some(b)) if samples[i].session == samples[j].session && a - b >= 1.0 => {
                    pairs.push(Pair {
                        w: i,
                        l: j,
                        weight: 1.0,
                    })
                }
                (Some(_), None)
                    if samples[i].burst.is_some()
                        && samples[i].burst == samples[j].burst
                        && flags.get(&samples[i].photo_id).is_some_and(|f| *f > 0.0) =>
                {
                    pairs.push(Pair {
                        w: i,
                        l: j,
                        weight: 0.5,
                    })
                }
                _ => {}
            }
        }
    }
    if pairs.len() > MAX_PAIRS {
        let stride = pairs.len().div_ceil(MAX_PAIRS);
        pairs = pairs.into_iter().step_by(stride).collect();
    }
    Ok(Gathered {
        samples,
        levels,
        pairs,
    })
}

/// Outcome of one training run.
#[derive(Debug, Clone)]
pub struct TrainReport {
    pub labels: i64,
    pub pairs: usize,
    pub holdout_pairs: usize,
    pub model_accuracy: Option<f64>,
    pub base_accuracy: Option<f64>,
    pub active: bool,
    pub alpha: f64,
}

fn most_common_dim(samples: &[Sample]) -> usize {
    let mut counts: HashMap<usize, usize> = HashMap::new();
    for s in samples {
        if let Some(e) = &s.emb {
            *counts.entry(e.len()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(d, c)| (*c, *d))
        .map(|(d, _)| d)
        .unwrap_or(0)
}

fn standardise(x: &mut [Vec<f32>], mean: &[f32], std: &[f32]) {
    for row in x {
        for (i, v) in row.iter_mut().enumerate() {
            *v = (*v - mean[i]) / std[i];
        }
    }
}

fn pearson(xs: &[f64], ys: &[f64]) -> f64 {
    let n = xs.len() as f64;
    if n < 3.0 {
        return 0.0;
    }
    let (mx, my) = (xs.iter().sum::<f64>() / n, ys.iter().sum::<f64>() / n);
    let (mut sxy, mut sxx, mut syy) = (0.0, 0.0, 0.0);
    for (x, y) in xs.iter().zip(ys) {
        sxy += (x - mx) * (y - my);
        sxx += (x - mx).powi(2);
        syy += (y - my).powi(2);
    }
    if sxx < 1e-12 || syy < 1e-12 {
        0.0
    } else {
        sxy / (sxx * syy).sqrt()
    }
}

/// Simple derived traits (i18n keys) from how the labelled photos' levels correlate with
/// measurable properties.
fn derive_traits(g: &Gathered) -> Vec<Value> {
    let rows: Vec<(&Sample, f64)> = g
        .samples
        .iter()
        .zip(&g.levels)
        .filter_map(|(s, l)| l.map(|l| (s, l)))
        .collect();
    if rows.len() < 10 {
        return vec![];
    }
    let mut found: Vec<(f64, Value)> = Vec::new();
    let mut corr_of = |key_pos: &str, key_neg: Option<&str>, f: &dyn Fn(&Sample) -> Option<f64>| {
        let (xs, ys): (Vec<f64>, Vec<f64>) = rows
            .iter()
            .filter_map(|(s, l)| f(s).map(|v| (v, *l)))
            .unzip();
        let c = pearson(&xs, &ys);
        let key = if c >= 0.3 {
            Some(key_pos)
        } else if c <= -0.3 {
            key_neg
        } else {
            None
        };
        if let Some(k) = key {
            found.push((
                c.abs(),
                json!({"key": k, "params": {"strength": (c.abs() * 100.0).round() / 100.0}}),
            ));
        }
    };
    corr_of("taste.prefers_bright", Some("taste.prefers_dark"), &|s| {
        s.feat.mean_luminance
    });
    corr_of("taste.prefers_sharp", None, &|s| s.feat.sharpness);
    corr_of("taste.prefers_aesthetic", None, &|s| s.feat.aesthetic);
    corr_of("taste.prefers_smiles", None, &|s| {
        let smiles: Vec<f64> = s
            .feat
            .faces
            .iter()
            .filter(|f: &&FaceFeat| f.is_subject)
            .filter_map(|f| f.smile)
            .collect();
        (!smiles.is_empty()).then(|| smiles.iter().sum::<f64>() / smiles.len() as f64)
    });
    // scene: clearly higher mean level than the rest
    let overall = rows.iter().map(|r| r.1).sum::<f64>() / rows.len() as f64;
    let mut by_scene: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for (s, l) in &rows {
        let sc = score_photo(&s.feat).effective_scene.to_string();
        by_scene.entry(sc).or_default().push(*l);
    }
    for (scene, ls) in by_scene {
        if ls.len() >= 5 {
            let m = ls.iter().sum::<f64>() / ls.len() as f64;
            if m - overall >= 1.0 {
                found.push((
                    (m - overall) / 5.0,
                    json!({"key": "taste.prefers_scene", "params": {"scene": scene}}),
                ));
            }
        }
    }
    found.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    found.into_iter().take(4).map(|f| f.1).collect()
}

/// Gathers the labels, trains, decides on activation and stores the result. `rescore` is
/// needed afterwards when the returned report differs from the state before.
pub fn train(conn: &mut Connection) -> Result<TrainReport> {
    let labels: i64 = conn.query_row("SELECT COUNT(*) FROM preference_label", [], |r| r.get(0))?;
    let g = gather(conn)?;
    let mut report = TrainReport {
        labels,
        pairs: g.pairs.len(),
        holdout_pairs: 0,
        model_accuracy: None,
        base_accuracy: None,
        active: false,
        alpha: 0.0,
    };
    let labelled = g.levels.iter().filter(|l| l.is_some()).count();
    let dim = most_common_dim(&g.samples);
    let mut model: Option<Model> = None;
    if labelled >= MIN_LABELLED_PHOTOS && !g.pairs.is_empty() {
        let raw: Vec<Vec<f32>> = g
            .samples
            .iter()
            .map(|s| features(&s.feat, s.emb.as_deref(), dim))
            .collect();
        let d = raw[0].len();
        let nf = raw.len() as f32;
        let mean: Vec<f32> = (0..d)
            .map(|i| raw.iter().map(|r| r[i]).sum::<f32>() / nf)
            .collect();
        let std: Vec<f32> = (0..d)
            .map(|i| {
                let var = raw.iter().map(|r| (r[i] - mean[i]).powi(2)).sum::<f32>() / nf;
                if var.sqrt() < 1e-3 {
                    1.0
                } else {
                    var.sqrt()
                }
            })
            .collect();
        let mut x = raw;
        standardise(&mut x, &mean, &std);
        let (train_idx, hold_idx): (Vec<usize>, Vec<usize>) =
            (0..g.pairs.len()).partition(|i| !is_holdout(g.samples[g.pairs[*i].w].photo_id));
        let pick = |idx: &[usize]| -> Vec<Pair> {
            idx.iter()
                .map(|i| Pair {
                    w: g.pairs[*i].w,
                    l: g.pairs[*i].l,
                    weight: g.pairs[*i].weight,
                })
                .collect()
        };
        let (train_pairs, hold_pairs) = (pick(&train_idx), pick(&hold_idx));
        report.holdout_pairs = hold_pairs.len();
        if !train_pairs.is_empty() && !hold_pairs.is_empty() {
            let w = fit(&x, &train_pairs);
            let scores: Vec<f64> = x.iter().map(|r| dot(&w, r) as f64).collect();
            let model_acc = accuracy(&hold_pairs, |i| scores[i]);
            let base_acc = accuracy(&hold_pairs, |i| g.samples[i].base);
            report.model_accuracy = Some(model_acc);
            report.base_accuracy = Some(base_acc);
            let hold_photos: HashSet<usize> = hold_pairs.iter().flat_map(|p| [p.w, p.l]).collect();
            let margin = (1.5 / (hold_photos.len() as f64).sqrt()).max(0.05);
            if hold_pairs.len() >= MIN_HOLDOUT_PAIRS
                && model_acc >= 0.6
                && model_acc >= base_acc + margin
            {
                // refit on everything for the final weights
                let all: Vec<Pair> = g
                    .pairs
                    .iter()
                    .map(|p| Pair {
                        w: p.w,
                        l: p.l,
                        weight: p.weight,
                    })
                    .collect();
                let w = fit(&x, &all);
                let s: Vec<f64> = x.iter().map(|r| dot(&w, r) as f64).collect();
                let q: Vec<f64> = g.samples.iter().map(|s| s.base).collect();
                let stat = |v: &[f64]| {
                    let m = v.iter().sum::<f64>() / v.len() as f64;
                    let sd =
                        (v.iter().map(|e| (e - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
                    (m, sd)
                };
                let (s_mu, s_sd) = stat(&s);
                let (q_mu, q_sd) = stat(&q);
                model = Some(Model {
                    dim,
                    w,
                    mean,
                    std,
                    s_mu,
                    s_sd: s_sd.max(1e-6),
                    q_mu,
                    q_sd: q_sd.max(0.05),
                });
            }
        }
    }
    report.active = model.is_some();
    report.alpha = if report.active {
        alpha_for(labels)
    } else {
        0.0
    };
    let traits = derive_traits(&g);
    conn.execute(
        "INSERT INTO taste_state(id, model, labels_at_train, active, alpha, holdout_accuracy,
                                 base_accuracy, pairs, traits, trained_at)
         VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,?9)
         ON CONFLICT(id) DO UPDATE SET model=?1, labels_at_train=?2, active=?3, alpha=?4,
           holdout_accuracy=?5, base_accuracy=?6, pairs=?7, traits=?8, trained_at=?9",
        params![
            model
                .as_ref()
                .map(|m| serde_json::to_string(m).unwrap_or_default()),
            labels,
            report.active as i64,
            report.alpha,
            report.model_accuracy,
            report.base_accuracy,
            report.pairs as i64,
            Value::Array(traits).to_string(),
            now_ms()
        ],
    )?;
    Ok(report)
}

/// The active model and its α, if there is one.
pub fn load_active(conn: &Connection) -> Result<Option<(Model, f64)>> {
    let row: Option<(Option<String>, f64)> = conn
        .query_row(
            "SELECT model, alpha FROM taste_state WHERE id=1 AND active=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(row.and_then(|(m, a)| {
        m.and_then(|m| serde_json::from_str::<Model>(&m).ok())
            .map(|m| (m, a))
    }))
}

// ------------------------------------------------------------------ labels

fn upsert(conn: &Connection, photo_id: i64, kind: &str, value: f64, source: &str) -> Result<usize> {
    Ok(conn
        .prepare_cached(
            "INSERT INTO preference_label(photo_id, kind, value, source, created_at)
             SELECT ?1,?2,?3,?4,?5 WHERE EXISTS (SELECT 1 FROM photo WHERE id=?1)
             ON CONFLICT(photo_id, kind) DO UPDATE SET value=?3, source=?4, created_at=?5
               WHERE value != ?3",
        )?
        .execute(params![photo_id, kind, value, source, now_ms()])?)
}

fn remove(conn: &Connection, photo_id: i64, kind: &str) -> Result<usize> {
    Ok(conn
        .prepare_cached("DELETE FROM preference_label WHERE photo_id=?1 AND kind=?2")?
        .execute(params![photo_id, kind])?)
}

/// Records the explicit rating / flag of a user patch. Returns how many labels changed.
pub fn record_patch(conn: &Connection, req: &PatchRequest) -> Result<usize> {
    // one transaction: a select-all patch used to commit once per photo
    let tx = conn.unchecked_transaction()?;
    let conn = &*tx;
    let mut changed = 0;
    for id in &req.ids {
        match req.user_rating {
            Some(Some(r)) if r >= 1 => changed += upsert(conn, *id, "rating", r as f64, "patch")?,
            Some(_) => changed += remove(conn, *id, "rating")?,
            None => {}
        }
        match req.flag {
            Some(f) if f != 0 => changed += upsert(conn, *id, "flag", f as f64, "patch")?,
            Some(_) => changed += remove(conn, *id, "flag")?,
            None => {}
        }
    }
    tx.commit()?;
    Ok(changed)
}

/// Records the ratings a user accepted from the AI.
pub fn record_accept(conn: &Connection, updates: &[PhotoUpdate]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    let conn = &*tx;
    let mut changed = 0;
    for u in updates {
        if let Some(r) = u.user_rating.filter(|r| *r >= 1) {
            changed += upsert(conn, u.id, "rating", r as f64, "accept_ai")?;
        }
    }
    tx.commit()?;
    Ok(changed)
}

// ------------------------------------------------------------------ status & Core API

#[derive(Debug, Clone, Serialize)]
pub struct TasteOut {
    pub labels: i64,
    pub active: bool,
    pub alpha: f64,
    pub holdout_accuracy: Option<f64>,
    /// Pairwise accuracy of the base score on the same held-out pairs.
    pub base_accuracy: Option<f64>,
    /// Pairs the last training used.
    pub pairs: i64,
    pub traits: Vec<Value>,
    pub updated_at: Option<i64>,
}

pub fn status(conn: &Connection) -> Result<TasteOut> {
    let labels: i64 = conn.query_row("SELECT COUNT(*) FROM preference_label", [], |r| r.get(0))?;
    #[allow(clippy::type_complexity)]
    let row: Option<(
        i64,
        f64,
        Option<f64>,
        Option<f64>,
        i64,
        Option<String>,
        Option<i64>,
    )> = conn
        .query_row(
            "SELECT active, alpha, holdout_accuracy, base_accuracy, pairs, traits, trained_at
             FROM taste_state WHERE id=1",
            [],
            |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            },
        )
        .optional()?;
    Ok(match row {
        Some((active, alpha, hold, base, pairs, traits, at)) => TasteOut {
            labels,
            active: active != 0,
            alpha: if active != 0 { alpha } else { 0.0 },
            holdout_accuracy: hold,
            base_accuracy: base,
            pairs,
            traits: traits
                .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
                .unwrap_or_default(),
            updated_at: at,
        },
        None => TasteOut {
            labels,
            active: false,
            alpha: 0.0,
            holdout_accuracy: None,
            base_accuracy: None,
            pairs: 0,
            traits: vec![],
            updated_at: None,
        },
    })
}

impl Core {
    pub async fn taste(&self) -> Result<TasteOut> {
        self.db.call(|c| status(c)).await
    }

    /// Called after labels were written: starts a background retraining every
    /// [`RETRAIN_EVERY`] new labels.
    pub(crate) fn taste_after_labels(self: &Arc<Self>, changed: usize) {
        if changed == 0 {
            return;
        }
        let core = self.clone();
        tokio::spawn(async move {
            let due = core
                .db
                .call(|c| {
                    let labels: i64 =
                        c.query_row("SELECT COUNT(*) FROM preference_label", [], |r| r.get(0))?;
                    let at: i64 = c
                        .query_row(
                            "SELECT labels_at_train FROM taste_state WHERE id=1",
                            [],
                            |r| r.get(0),
                        )
                        .optional()?
                        .unwrap_or(0);
                    Ok((labels - at).abs() >= RETRAIN_EVERY)
                })
                .await
                .unwrap_or(false);
            if due {
                if let Err(e) = core.retrain_taste().await {
                    tracing::warn!(error = %e, "taste retraining failed");
                }
            }
        });
    }

    /// Trains now (the background trigger and the CLI / tests share this), rescoring every
    /// analysed photo when the model in use changed, and broadcasts `taste.updated`.
    pub async fn retrain_taste(self: &Arc<Self>) -> Result<TasteOut> {
        let _g = self.taste_lock.lock().await;
        let before_active = self.db.call(|c| Ok(load_active(c)?.is_some())).await?;
        let report = self.db.call(train).await?;
        if before_active || report.active {
            self.rescore_everything().await?;
        }
        let out = self.taste().await?;
        self.events.emit(Event::TasteUpdated {
            labels: out.labels,
            active: out.active,
            alpha: out.alpha,
        });
        Ok(out)
    }

    /// `POST /api/taste/reset`: forgets every label and the model; scores go back to the base.
    pub async fn reset_taste(self: &Arc<Self>) -> Result<()> {
        let _g = self.taste_lock.lock().await;
        let was_active = self.db.call(|c| Ok(load_active(c)?.is_some())).await?;
        self.db
            .call(|c| {
                c.execute("DELETE FROM preference_label", [])?;
                c.execute("DELETE FROM taste_state", [])?;
                Ok(())
            })
            .await?;
        if was_active {
            self.rescore_everything().await?;
        }
        self.events.emit(Event::TasteUpdated {
            labels: 0,
            active: false,
            alpha: 0.0,
        });
        Ok(())
    }

    /// Re-runs scoring for all bursts (picks up the current taste model) and tells clients.
    async fn rescore_everything(&self) -> Result<()> {
        let (touched, sessions) = self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT id FROM burst")?;
                let bursts = st
                    .query_map([], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                drop(st);
                let touched = store::rescore_bursts(c, &bursts)?;
                let mut by_session: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
                for chunk in touched.chunks(500) {
                    let ph = vec!["?"; chunk.len()].join(",");
                    let mut st = c.prepare(&format!(
                        "SELECT session_id, photo_id FROM session_photo WHERE photo_id IN ({ph})"
                    ))?;
                    let rows = st
                        .query_map(params_from_iter(chunk.iter()), |r| {
                            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                        })?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    for (s, p) in rows {
                        by_session.entry(s).or_default().push(p);
                    }
                }
                Ok((touched, by_session))
            })
            .await?;
        let _ = touched;
        for (session_id, ids) in sessions {
            for chunk in ids.chunks(5000) {
                self.events.emit(Event::AnalysisUpdated {
                    session_id,
                    ids: chunk.to_vec(),
                });
            }
        }
        Ok(())
    }
}
