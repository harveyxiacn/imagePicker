//! Personalised scoring: "learn my taste" (docs/03 §3.3, taste v2).
//!
//! Every explicit pick / reject / star rating the user makes in the UI is a *preference label*
//! (`preference_label`, `source = 'patch'`). Decisions the app makes itself (assistant actions,
//! "accept AI ratings") are never labels. Per photo the label becomes a class: reject flag or
//! ≤ 2 stars = negative, pick flag or ≥ 4 stars = positive, 3 stars = neutral (ignored). Labels
//! come from every session of the catalog.
//!
//! The model is an L2-regularised logistic regression over
//! `[interpretable measures | issue bits | scene one-hot | SigLIP embedding]`, standardised,
//! trained in plain Rust (deterministic: fixed iteration count, rayon over fixed-size chunks
//! summed in order). It needs [`MIN_PER_CLASS`] positives and negatives, and is only switched on
//! when its 5-fold cross-validated AUC beats the generic score (`base_score`) by a margin.
//!
//! When active, every analysed photo gets a cached taste score (`taste_score`, the probability
//! the user keeps it) that the grid can sort by, and the AI score is blended with it as before:
//! `(1-α)·base + α·user`, α growing from 0 to 0.6 with the number of labels.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rayon::prelude::*;
use rusqlite::{params, params_from_iter, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::analysis::scoring::{score_photo, star_rating, Issue, PhotoFeat, Ranked, Reason};
use crate::analysis::store::{self, FACE_FEAT_COLS};
use crate::analysis::vecs::decode_f16;
use crate::catalog::now_ms;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::model::{PatchRequest, PhotoUpdate};
use crate::Core;

/// Version of the feature vector; a stored model of another version is ignored and retrained.
pub const FEATURE_VERSION: i64 = 2;
/// New decisions that trigger an immediate background retraining.
pub const RETRAIN_EVERY: i64 = 50;
/// New decisions that trigger a retraining once the user has paused for [`RETRAIN_IDLE_MS`].
pub const RETRAIN_IDLE_MIN: i64 = 10;
/// Quiet time after the last decision before the idle retraining (end of a culling pass).
pub const RETRAIN_IDLE_MS: u64 = 120_000;
/// Fewest positives and fewest negatives before anything is trained (cold start).
pub const MIN_PER_CLASS: usize = 30;
pub const ALPHA_MAX: f64 = 0.6;
/// Label count at which α reaches [`ALPHA_MAX`].
pub const ALPHA_FULL_AT: f64 = 200.0;
/// Lowest cross-validated AUC at which the model may be used at all.
pub const MIN_AUC: f64 = 0.6;
/// Most labelled photos evaluated by cross-validation / used for the final fit.
const MAX_EVAL: usize = 4_000;
const MAX_FIT: usize = 20_000;
const FOLDS: u64 = 5;
const ITERS: usize = 150;
/// L2 strengths tried by cross-validation (first wins ties: the strongest regularisation).
const LAMBDAS: [f32; 3] = [0.3, 0.03, 0.003];
/// Rows per gradient chunk; the chunking (not the thread count) fixes the summation order.
const CHUNK: usize = 256;
const PAR_MIN_ROWS: usize = 4 * CHUNK;
/// Bursts rescored per database call by `rescore_everything` (~1-2k photos).
const RESCORE_BURSTS_PER_CALL: usize = 300;

/// Interpretable features, in vector order (the issue bits follow [`Issue::ALL`]).
pub const NAMED: [&str; 21] = [
    "sharpness",
    "exposure",
    "aesthetic",
    "technical",
    "noise",
    "brightness",
    "highlights",
    "shadows",
    "expression",
    "eyes_open",
    "smile",
    "composition",
    "faces",
    "closed_eyes",
    "blurry",
    "overexposed",
    "underexposed",
    "noisy",
    "tilted",
    "bystander",
    "bad_expression",
];
pub const SCENES: [&str; 8] = [
    "portrait",
    "group",
    "landscape",
    "architecture",
    "night",
    "food",
    "pet",
    "other",
];
/// Length of the fixed (non-embedding) part of the feature vector.
pub const N_FIXED: usize = NAMED.len() + SCENES.len();

/// Fusion weight for `labels` labels.
pub fn alpha_for(labels: i64) -> f64 {
    (ALPHA_MAX * (labels as f64 / ALPHA_FULL_AT)).clamp(0.0, ALPHA_MAX)
}

/// Name of feature `i` of the fixed part (`scene_<x>` for the scene one-hot).
pub fn feature_name(i: usize) -> String {
    if i < NAMED.len() {
        NAMED[i].to_string()
    } else {
        format!("scene_{}", SCENES[i - NAMED.len()])
    }
}

// ------------------------------------------------------------------ features & model

/// What the model sees of one photo.
pub struct Input<'a> {
    pub feat: &'a PhotoFeat,
    /// `photo.issues` bit mask.
    pub issues: i64,
    pub emb: Option<&'a [f32]>,
}

/// `[NAMED | SCENES | embedding (dim)]`; missing values are NaN (imputed with the mean).
pub fn raw_features(inp: &Input, dim: usize) -> Vec<f32> {
    let f = inp.feat;
    let sc = score_photo(f);
    let subjects: Vec<_> = f.faces.iter().filter(|x| x.is_subject).collect();
    let eyes = subjects
        .iter()
        .filter_map(|x| x.eyes_open)
        .fold(None, |a: Option<f64>, e| Some(a.map_or(e, |a| a.min(e))));
    let smiles: Vec<f64> = subjects.iter().filter_map(|x| x.smile).collect();
    let smile = (!smiles.is_empty()).then(|| smiles.iter().sum::<f64>() / smiles.len() as f64);
    let nan = |v: Option<f64>| v.map(|v| v as f32).unwrap_or(f32::NAN);
    let mut v = Vec::with_capacity(N_FIXED + dim);
    for m in [
        f.sharpness,
        f.exposure,
        f.aesthetic,
        f.iqa,
        f.noise,
        f.mean_luminance,
        f.clipped_highlights,
        f.crushed_shadows,
        sc.face_score,
        eyes,
        smile,
        f.composition,
    ] {
        v.push(nan(m));
    }
    v.push(subjects.len().min(5) as f32);
    for i in Issue::ALL {
        v.push(if inp.issues & i.bit() != 0 { 1.0 } else { 0.0 });
    }
    for s in SCENES {
        v.push(if sc.effective_scene == s { 1.0 } else { 0.0 });
    }
    match inp.emb {
        Some(e) if dim > 0 && e.len() == dim => v.extend_from_slice(e),
        _ => v.resize(N_FIXED + dim, f32::NAN),
    }
    v
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// A trained model (stored as JSON in `taste_state.model`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Model {
    pub feature_version: i64,
    /// Embedding dimension the model was trained with (0 = none).
    pub dim: usize,
    /// Weights on the standardised features (`N_FIXED + dim`).
    pub w: Vec<f32>,
    pub b: f32,
    pub mean: Vec<f32>,
    pub std: Vec<f32>,
    pub lambda: f32,
    /// Mean / spread of the logit over the training photos.
    pub s_mu: f64,
    pub s_sd: f64,
    /// Mean / spread of the base score over the same photos (the user score is mapped onto
    /// the base score's scale so that the two can be blended).
    pub q_mu: f64,
    pub q_sd: f64,
}

/// One feature's share of a photo's taste logit.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Contribution {
    /// A [`NAMED`] feature, `scene_<x>`, or `look` (the embedding: overall look and content).
    pub feature: String,
    /// Change of the logit against an average photo.
    pub delta: f64,
    /// `high` / `low`: whether this photo has more or less of the feature than average.
    pub direction: &'static str,
}

fn standardise(raw: &[f32], mean: &[f32], std: &[f32]) -> Vec<f32> {
    raw.iter()
        .zip(mean.iter().zip(std))
        .map(|(x, (m, s))| if x.is_nan() { 0.0 } else { (x - m) / s })
        .collect()
}

impl Model {
    fn z(&self, inp: &Input) -> Vec<f32> {
        standardise(&raw_features(inp, self.dim), &self.mean, &self.std)
    }

    pub fn logit(&self, inp: &Input) -> f64 {
        (dot(&self.w, &self.z(inp)) + self.b) as f64
    }

    /// Probability (0..1) that the user keeps this photo.
    pub fn taste_score(&self, inp: &Input) -> f64 {
        sigmoid(self.logit(inp) as f32) as f64
    }

    /// The user's taste as a 0..1 score on the scale of the base score (for the fusion).
    pub fn user_score(&self, inp: &Input) -> f64 {
        let z = (self.logit(inp) - self.s_mu) / self.s_sd.max(1e-9);
        (self.q_mu + self.q_sd * z).clamp(0.0, 1.0)
    }

    /// The strongest reasons for and against (up to 3 each), largest first.
    pub fn explain(&self, inp: &Input) -> Vec<Contribution> {
        let z = self.z(inp);
        let mut items: Vec<Contribution> = Vec::new();
        for (i, name) in NAMED.iter().enumerate() {
            let d = (self.w[i] * z[i]) as f64;
            items.push(Contribution {
                feature: name.to_string(),
                delta: d,
                direction: if z[i] >= 0.0 { "high" } else { "low" },
            });
        }
        let scene_sum: f32 = (NAMED.len()..N_FIXED).map(|i| self.w[i] * z[i]).sum();
        if let Some(i) = (NAMED.len()..N_FIXED).find(|&i| z[i] > 0.0) {
            items.push(Contribution {
                feature: feature_name(i),
                delta: scene_sum as f64,
                direction: "high",
            });
        }
        if self.dim > 0 {
            let look: f32 = (N_FIXED..self.w.len()).map(|i| self.w[i] * z[i]).sum();
            items.push(Contribution {
                feature: "look".into(),
                delta: look as f64,
                direction: if look >= 0.0 { "high" } else { "low" },
            });
        }
        items.retain(|c| c.delta.abs() >= 0.05);
        let round = |c: &mut Contribution| c.delta = (c.delta * 1000.0).round() / 1000.0;
        let mut pos: Vec<Contribution> = items.iter().filter(|c| c.delta > 0.0).cloned().collect();
        let mut neg: Vec<Contribution> = items.iter().filter(|c| c.delta < 0.0).cloned().collect();
        pos.sort_by(|a, b| b.delta.total_cmp(&a.delta));
        neg.sort_by(|a, b| a.delta.total_cmp(&b.delta));
        pos.truncate(3);
        neg.truncate(3);
        pos.extend(neg);
        pos.iter_mut().for_each(round);
        pos
    }
}

/// Blends the user score into one burst's base ranking and recomputes ranks and stars.
pub fn fuse(mut ranked: Vec<Ranked>, user: &HashMap<i64, f64>, alpha: f64) -> Vec<Ranked> {
    for r in &mut ranked {
        let base = if r.base_q.is_finite() { r.base_q } else { 0.0 };
        let u = user
            .get(&r.photo_id)
            .copied()
            .filter(|u| u.is_finite())
            .unwrap_or(base);
        let alpha = if alpha.is_finite() { alpha } else { 0.0 };
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
        b.q.total_cmp(&a.q)
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

// ------------------------------------------------------------------ training (pure)

/// One labelled photo.
pub struct Sample {
    pub photo_id: i64,
    pub feat: PhotoFeat,
    pub issues: i64,
    pub emb: Option<Vec<f32>>,
    /// The generic score (`base_score`, never a taste-blended `ai_score`): the baseline.
    /// `None` = never scored; such photos are left out of the baseline numbers.
    pub base: Option<f64>,
    /// Cross-validation group: the burst, else the duplicate group, else the photo itself, so
    /// near-identical frames never end up on both sides of a fold.
    pub group: i64,
}

impl Sample {
    pub fn input(&self) -> Input<'_> {
        Input {
            feat: &self.feat,
            issues: self.issues,
            emb: self.emb.as_deref(),
        }
    }
}

/// Cross-validation group key of a photo (burst, else duplicate group, else the photo).
pub fn group_key(photo_id: i64, burst: Option<i64>, dup_group: Option<i64>) -> i64 {
    match (burst, dup_group) {
        (Some(b), _) => b.wrapping_mul(4).wrapping_add(1),
        (None, Some(d)) => d.wrapping_mul(4).wrapping_add(2),
        (None, None) => photo_id.wrapping_mul(4).wrapping_add(3),
    }
}

/// Deterministic per-photo hash (splitmix64): folds and subsampling.
fn mix(id: i64) -> u64 {
    let mut z = (id as u64).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// At most `cap` of `items` (photo id, class): the smaller class keeps up to half the budget,
/// each class picked in hash order. The result is sorted by photo id.
fn stratified(items: &[(i64, bool)], cap: usize) -> Vec<(i64, bool)> {
    if items.len() <= cap {
        return items.to_vec();
    }
    let mut pos: Vec<(i64, bool)> = items.iter().copied().filter(|x| x.1).collect();
    let mut neg: Vec<(i64, bool)> = items.iter().copied().filter(|x| !x.1).collect();
    pos.sort_by_key(|x| (mix(x.0), x.0));
    neg.sort_by_key(|x| (mix(x.0), x.0));
    let (small, big) = if pos.len() <= neg.len() {
        (&mut pos, &mut neg)
    } else {
        (&mut neg, &mut pos)
    };
    small.truncate(cap / 2);
    big.truncate(cap - small.len());
    let mut out: Vec<(i64, bool)> = small.iter().chain(big.iter()).copied().collect();
    out.sort_by_key(|x| x.0);
    out
}

/// Standardised design matrix with a trailing constant column (the bias).
struct Mat {
    d: usize,
    x: Vec<f32>,
}

impl Mat {
    fn row(&self, i: usize) -> &[f32] {
        &self.x[i * self.d..(i + 1) * self.d]
    }

    /// `Σ coef(i, row_i) · row_i` over `idx`, summed per fixed chunk and then in chunk order
    /// (identical result whether the chunks run in parallel or not).
    fn accumulate(&self, idx: &[usize], coef: &(dyn Fn(usize, &[f32]) -> f32 + Sync)) -> Vec<f32> {
        let part = |chunk: &[usize]| {
            let mut g = vec![0.0f32; self.d];
            for &i in chunk {
                let r = self.row(i);
                let c = coef(i, r);
                if c != 0.0 {
                    for (a, e) in g.iter_mut().zip(r) {
                        *a += c * e;
                    }
                }
            }
            g
        };
        let parts: Vec<Vec<f32>> = if idx.len() >= PAR_MIN_ROWS {
            idx.par_chunks(CHUNK).map(part).collect()
        } else {
            idx.chunks(CHUNK).map(part).collect()
        };
        let mut out = vec![0.0f32; self.d];
        for p in parts {
            for (a, e) in out.iter_mut().zip(&p) {
                *a += e;
            }
        }
        out
    }
}

/// Class-balanced sample weights (mean 1) for `idx`.
fn balanced(idx: &[usize], y: &[bool]) -> (f32, f32) {
    let n = idx.len() as f32;
    let p = idx.iter().filter(|&&i| y[i]).count().max(1) as f32;
    let q = (idx.len() as f32 - p).max(1.0);
    (n / (2.0 * p), n / (2.0 * q))
}

/// L2-regularised logistic regression (bias unpenalised) by Nesterov-accelerated gradient
/// descent with a fixed number of iterations. Returns `d` weights, the last one the bias.
fn fit(m: &Mat, idx: &[usize], y: &[bool], lambda: f32) -> Vec<f32> {
    let d = m.d;
    let n = idx.len().max(1) as f32;
    let (wp, wn) = balanced(idx, y);
    let sw = |i: usize| if y[i] { wp } else { wn };
    // step size from the largest eigenvalue of the weighted second-moment matrix
    let mut v: Vec<f32> = (0..d).map(|i| 1.0 + (i % 7) as f32 * 0.1).collect();
    let mut eig = 1.0f32;
    for _ in 0..20 {
        let norm = dot(&v, &v).sqrt().max(1e-12);
        v.iter_mut().for_each(|e| *e /= norm);
        let vv = v.clone();
        let next = m.accumulate(idx, &|i, r| sw(i) * dot(r, &vv) / n);
        eig = dot(&next, &v).max(1e-3);
        v = next;
    }
    let lr = 1.0 / (0.25 * eig + lambda);
    let grad = |w: &[f32]| -> Vec<f32> {
        let mut g = m.accumulate(idx, &|i, r| {
            let t = if y[i] { 1.0 } else { 0.0 };
            sw(i) * (sigmoid(dot(w, r)) - t) / n
        });
        for (gi, wi) in g.iter_mut().zip(w).take(d - 1) {
            *gi += lambda * wi;
        }
        g
    };
    let mut w = vec![0.0f32; d];
    let mut look = w.clone();
    let mut t = 1.0f32;
    for _ in 0..ITERS {
        let g = grad(&look);
        let next: Vec<f32> = look.iter().zip(&g).map(|(a, b)| a - lr * b).collect();
        let t_next = (1.0 + (1.0 + 4.0 * t * t).sqrt()) / 2.0;
        let k = (t - 1.0) / t_next;
        look = next.iter().zip(&w).map(|(a, b)| a + k * (a - b)).collect();
        w = next;
        t = t_next;
    }
    w
}

/// Area under the ROC curve (ties count half); `None` without both classes.
pub fn auc(scores: &[f64], y: &[bool]) -> Option<f64> {
    let p = y.iter().filter(|v| **v).count();
    let q = y.len() - p;
    if p == 0 || q == 0 {
        return None;
    }
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|&a, &b| scores[a].total_cmp(&scores[b]));
    let mut rank_sum = 0.0;
    let mut i = 0;
    while i < order.len() {
        let mut j = i;
        while j + 1 < order.len() && scores[order[j + 1]] == scores[order[i]] {
            j += 1;
        }
        let avg = (i + j) as f64 / 2.0 + 1.0;
        for &k in &order[i..=j] {
            if y[k] {
                rank_sum += avg;
            }
        }
        i = j + 1;
    }
    Some((rank_sum - (p * (p + 1)) as f64 / 2.0) / (p as f64 * q as f64))
}

/// Accuracy when the top-`positives` scores are called positive (comparable across scores of
/// any scale, so the model and the base score are judged the same way).
fn rank_accuracy(scores: &[f64], y: &[bool]) -> f64 {
    let p = y.iter().filter(|v| **v).count();
    let mut order: Vec<usize> = (0..scores.len()).collect();
    order.sort_by(|&a, &b| scores[b].total_cmp(&scores[a]).then(a.cmp(&b)));
    let hit = order
        .iter()
        .enumerate()
        .filter(|(rank, i)| (*rank < p) == y[**i])
        .count();
    hit as f64 / scores.len().max(1) as f64
}

/// Quality numbers of the last training (stored as JSON in `taste_state.metrics`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Metrics {
    /// Cross-validated AUC of the taste model.
    pub auc: f64,
    /// AUC of the generic score on the same photos (the baseline).
    pub base_auc: f64,
    pub accuracy: f64,
    pub base_accuracy: f64,
    /// How much the model had to beat the baseline by.
    pub margin: f64,
    pub eval_positives: usize,
    pub eval_negatives: usize,
    pub folds: u64,
    pub lambda: f32,
    /// Embedding dimension used (0 = none).
    pub dim: usize,
    /// Photos of the final fit (0 when the model was not good enough).
    pub fit_photos: usize,
    pub train_ms: u64,
}

/// Labelled photos ready for training.
pub struct TrainData {
    pub samples: Vec<Sample>,
    pub y: Vec<bool>,
    /// All usable labelled photos (the samples may be a subset).
    pub positives: usize,
    pub negatives: usize,
}

/// What one training produced.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// `Some` only when the model beat the baseline.
    pub model: Option<Model>,
    pub metrics: Option<Metrics>,
    pub traits: Vec<Value>,
    pub positives: usize,
    pub negatives: usize,
}

fn most_common_dim(samples: &[Sample]) -> usize {
    let mut counts: BTreeMap<usize, usize> = BTreeMap::new();
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

/// Column means / standard deviations of `rows` ignoring missing values.
fn column_stats(rows: &[&Vec<f32>]) -> (Vec<f32>, Vec<f32>) {
    let d = rows.first().map(|r| r.len()).unwrap_or(0);
    let mut mean = vec![0.0f32; d];
    let mut std = vec![1.0f32; d];
    for (j, (mj, sj)) in mean.iter_mut().zip(std.iter_mut()).enumerate() {
        let vals: Vec<f64> = rows
            .iter()
            .map(|r| r[j])
            .filter(|v| !v.is_nan())
            .map(|v| v as f64)
            .collect();
        if vals.is_empty() {
            continue;
        }
        let m = vals.iter().sum::<f64>() / vals.len() as f64;
        let var = vals.iter().map(|v| (v - m).powi(2)).sum::<f64>() / vals.len() as f64;
        *mj = m as f32;
        *sj = if var.sqrt() < 1e-4 {
            1.0
        } else {
            var.sqrt() as f32
        };
    }
    (mean, std)
}

/// Design matrix of `rows` standardised with `mean` / `std`, plus the bias column.
fn design(rows: &[&Vec<f32>], mean: &[f32], std: &[f32]) -> Mat {
    let d = mean.len() + 1;
    let mut x = Vec::with_capacity(rows.len() * d);
    for r in rows {
        x.extend(standardise(r, mean, std));
        x.push(1.0);
    }
    Mat { d, x }
}

/// Strongest interpretable weights as `{key: likes|dislikes, params: {feature, strength}}`.
fn traits_of(w: &[f32]) -> Vec<Value> {
    let mut v: Vec<(f32, usize)> = (0..N_FIXED)
        .map(|i| (w[i], i))
        .filter(|(x, _)| x.abs() > 1e-3)
        .collect();
    let max = v.iter().map(|x| x.0.abs()).fold(0.0f32, f32::max);
    v.retain(|x| x.0.abs() >= 0.25 * max);
    v.sort_by(|a, b| b.0.abs().total_cmp(&a.0.abs()).then(a.1.cmp(&b.1)));
    v.truncate(4);
    v.into_iter()
        .map(|(x, i)| {
            json!({
                "key": if x > 0.0 { "likes" } else { "dislikes" },
                "params": {"feature": feature_name(i), "strength": (x.abs() as f64 * 100.0).round() / 100.0}
            })
        })
        .collect()
}

/// Cold-start gate, cross-validation against the baseline and (when good enough) the final
/// fit. Folds are grouped by burst / duplicate group and the standardisation is fitted inside
/// each fold. Pure and deterministic apart from `train_ms`.
pub fn train(data: &TrainData) -> Outcome {
    let t0 = Instant::now();
    let mut out = Outcome {
        model: None,
        metrics: None,
        traits: vec![],
        positives: data.positives,
        negatives: data.negatives,
    };
    let pos = data.y.iter().filter(|v| **v).count();
    let neg = data.y.len() - pos;
    if data.positives < MIN_PER_CLASS || data.negatives < MIN_PER_CLASS || pos == 0 || neg == 0 {
        return out;
    }
    let s = &data.samples;
    let y = &data.y;
    let dim = most_common_dim(s);
    let raws: Vec<Vec<f32>> = s.iter().map(|x| raw_features(&x.input(), dim)).collect();
    let fold: Vec<u64> = s.iter().map(|x| mix(x.group) % FOLDS).collect();

    // cross-validation on (a subset of) the samples
    let items: Vec<(i64, bool)> = s.iter().zip(y).map(|(x, c)| (x.photo_id, *c)).collect();
    let row_of: HashMap<i64, usize> = s.iter().enumerate().map(|(i, x)| (x.photo_id, i)).collect();
    let eval: Vec<usize> = stratified(&items, MAX_EVAL)
        .iter()
        .map(|(id, _)| row_of[id])
        .collect();
    let y_eval: Vec<bool> = eval.iter().map(|&i| y[i]).collect();
    let eval_rows: Vec<&Vec<f32>> = eval.iter().map(|&i| &raws[i]).collect();
    let mut oof = vec![vec![0.0f64; eval.len()]; LAMBDAS.len()];
    for f in 0..FOLDS {
        let train_local: Vec<usize> = (0..eval.len()).filter(|&k| fold[eval[k]] != f).collect();
        if train_local.is_empty() {
            continue;
        }
        let fit_rows: Vec<&Vec<f32>> = train_local.iter().map(|&k| eval_rows[k]).collect();
        let (mean, std) = column_stats(&fit_rows);
        let m = design(&eval_rows, &mean, &std);
        for (l, lambda) in LAMBDAS.iter().enumerate() {
            let w = fit(&m, &train_local, &y_eval, *lambda);
            for (k, &i) in eval.iter().enumerate() {
                if fold[i] == f {
                    oof[l][k] = dot(&w, m.row(k)) as f64;
                }
            }
        }
    }
    let mut best = 0;
    let mut best_auc = f64::NEG_INFINITY;
    for (l, scores) in oof.iter().enumerate() {
        let a = auc(scores, &y_eval).unwrap_or(0.5);
        if a > best_auc {
            best_auc = a;
            best = l;
        }
    }
    let (model_auc, lambda) = (best_auc, LAMBDAS[best]);
    // the baseline only where a generic score exists
    let with_base: Vec<usize> = (0..eval.len())
        .filter(|&k| s[eval[k]].base.is_some())
        .collect();
    let base_scores: Vec<f64> = with_base
        .iter()
        .map(|&k| s[eval[k]].base.unwrap_or(0.0))
        .collect();
    let base_y: Vec<bool> = with_base.iter().map(|&k| y_eval[k]).collect();
    let base_auc = auc(&base_scores, &base_y).unwrap_or(0.5);
    let base_acc = if base_scores.is_empty() {
        0.5
    } else {
        rank_accuracy(&base_scores, &base_y)
    };
    let ep = y_eval.iter().filter(|v| **v).count();
    let en = y_eval.len() - ep;
    let margin = (0.3 / (ep.min(en).max(1) as f64).sqrt()).max(0.02);
    let r4 = |v: f64| (v * 10_000.0).round() / 10_000.0;
    let mut metrics = Metrics {
        auc: r4(model_auc),
        base_auc: r4(base_auc),
        accuracy: r4(rank_accuracy(&oof[best], &y_eval)),
        base_accuracy: r4(base_acc),
        margin: r4(margin),
        eval_positives: ep,
        eval_negatives: en,
        folds: FOLDS,
        lambda,
        dim,
        fit_photos: 0,
        train_ms: 0,
    };
    if model_auc >= MIN_AUC && model_auc - base_auc >= margin {
        let rows: Vec<&Vec<f32>> = raws.iter().collect();
        let (mean, std) = column_stats(&rows);
        let m = design(&rows, &mean, &std);
        drop(rows);
        let d = m.d;
        let all: Vec<usize> = (0..s.len()).collect();
        let w = fit(&m, &all, y, lambda);
        let logits: Vec<f64> = all.iter().map(|&i| dot(&w, m.row(i)) as f64).collect();
        let stat = |v: &[f64]| {
            if v.is_empty() {
                return (0.5, 0.15);
            }
            let mu = v.iter().sum::<f64>() / v.len() as f64;
            let sd = (v.iter().map(|e| (e - mu).powi(2)).sum::<f64>() / v.len() as f64).sqrt();
            (mu, sd)
        };
        let (s_mu, s_sd) = stat(&logits);
        let bases: Vec<f64> = s.iter().filter_map(|x| x.base).collect();
        let (q_mu, q_sd) = stat(&bases);
        out.traits = traits_of(&w);
        metrics.fit_photos = s.len();
        out.model = Some(Model {
            feature_version: FEATURE_VERSION,
            dim,
            b: w[d - 1],
            w: w[..d - 1].to_vec(),
            mean,
            std,
            lambda,
            s_mu,
            s_sd: s_sd.max(1e-6),
            q_mu,
            q_sd: q_sd.max(0.05),
        });
    }
    metrics.train_ms = t0.elapsed().as_millis() as u64;
    out.metrics = Some(metrics);
    out
}

// ------------------------------------------------------------------ catalog access

/// Class of a photo from its labels: flag first, then stars (3 stars = neutral = `None`).
pub fn class_of(rating: Option<f64>, flag: Option<f64>) -> Option<bool> {
    match (rating, flag) {
        (_, Some(f)) if f < 0.0 => Some(false),
        (_, Some(f)) if f > 0.0 => Some(true),
        (Some(r), _) if r >= 4.0 => Some(true),
        (Some(r), _) if r <= 2.0 => Some(false),
        _ => None,
    }
}

/// `(photo id, positive?)` of every analysed photo with an explicit user decision, by id.
pub fn labelled(conn: &Connection) -> Result<Vec<(i64, bool)>> {
    // A label only counts while it still agrees with the photo: a rating or flag changed since
    // (assistant, "accept AI", XMP sync, Lightroom, a pair-file edit) silences it, and an undo
    // that restores the value brings it back.
    let mut st = conn.prepare(&format!(
        "SELECT l.photo_id, l.kind, l.value FROM preference_label l JOIN photo p ON p.id=l.photo_id
         WHERE {AGREES} AND EXISTS (SELECT 1 FROM analysis a WHERE a.photo_id=l.photo_id)"
    ))?;
    let mut per: BTreeMap<i64, (Option<f64>, Option<f64>)> = BTreeMap::new();
    let rows = st.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, f64>(2)?,
        ))
    })?;
    for row in rows {
        let (id, kind, v) = row?;
        let e = per.entry(id).or_default();
        if kind == "rating" {
            e.0 = Some(v);
        } else {
            e.1 = Some(v);
        }
    }
    Ok(per
        .into_iter()
        .filter_map(|(id, (r, f))| class_of(r, f).map(|c| (id, c)))
        .collect())
}

/// SQL condition (`l` = preference_label, `p` = photo): a user label that still matches the
/// photo's current rating / flag.
const AGREES: &str = "l.source='patch' AND ((l.kind='rating' AND p.user_rating = l.value)
      OR (l.kind='flag' AND COALESCE(p.flag, 0) = l.value))";

/// Analysed photos as model inputs, in the order of `ids` (unanalysed ids are skipped).
pub fn load_samples(conn: &Connection, ids: &[i64]) -> Result<Vec<Sample>> {
    let mut by_id: HashMap<i64, Sample> = HashMap::new();
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
                    p.base_score, COALESCE(p.issues, 0), p.burst_id, p.dup_group_id,
                    a.tilt_deg, a.tilt_confidence, a.saliency
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
                        tilt_deg: r.get(15)?,
                        tilt_confidence: r.get(16)?,
                        saliency: r
                            .get::<_, Option<String>>(17)?
                            .and_then(|s| serde_json::from_str(&s).ok()),
                        composition: None,
                        faces: Vec::new(),
                    },
                    emb: r
                        .get::<_, Option<Vec<u8>>>(10)?
                        .map(|b| decode_f16(&b))
                        .filter(|e| !e.is_empty()),
                    base: r.get(11)?,
                    issues: r.get(12)?,
                    group: group_key(r.get(0)?, r.get(13)?, r.get(14)?),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for mut s in rows {
            s.feat.faces = faces.remove(&s.photo_id).unwrap_or_default();
            // exactly as `rescore_bursts` computes it (no train/serve skew)
            s.feat.composition =
                crate::analysis::composition::composition_score(&s.feat).map(|c| c.score);
            by_id.insert(s.photo_id, s);
        }
    }
    Ok(ids.iter().filter_map(|id| by_id.remove(id)).collect())
}

/// The labelled photos (a deterministic stratified subset of at most 20,000).
pub fn gather(conn: &Connection) -> Result<TrainData> {
    let items = labelled(conn)?;
    let positives = items.iter().filter(|x| x.1).count();
    let negatives = items.len() - positives;
    let picked = stratified(&items, MAX_FIT);
    let ids: Vec<i64> = picked.iter().map(|x| x.0).collect();
    let class: HashMap<i64, bool> = picked.into_iter().collect();
    let samples = load_samples(conn, &ids)?;
    let y = samples.iter().map(|s| class[&s.photo_id]).collect();
    Ok(TrainData {
        samples,
        y,
        positives,
        negatives,
    })
}

fn ensure_state_row(conn: &Connection) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO taste_state(id) VALUES(1)", [])?;
    Ok(())
}

/// Whether taste learning is switched on (default on).
pub fn is_enabled(conn: &Connection) -> Result<bool> {
    Ok(conn
        .query_row("SELECT enabled FROM taste_state WHERE id=1", [], |r| {
            r.get::<_, i64>(0)
        })
        .optional()?
        .is_none_or(|e| e != 0))
}

/// Writes a training outcome: the model (when good enough), metrics, traits; clears the cached
/// scores (the caller rescores). `consumed` new decisions are taken off the pending count.
pub fn store_outcome(conn: &Connection, out: &Outcome, consumed: i64) -> Result<()> {
    ensure_state_row(conn)?;
    let active = out.model.is_some();
    // α grows with the number of user labels, as in 1.0
    let labels = count_labels(conn)?;
    conn.execute(
        "UPDATE taste_state SET model=?1, active=?2, alpha=?3, holdout_accuracy=?4,
           base_accuracy=?5, metrics=?6, traits=?7, positives=?8, negatives=?9,
           pending_labels=MAX(0, pending_labels-?10), labels_at_train=?11, feature_version=?12,
           model_version=model_version+?13, pairs=0, trained_at=?14,
           rescore_pending=CASE WHEN active=1 OR ?2=1 THEN 1 ELSE rescore_pending END
         WHERE id=1",
        params![
            out.model
                .as_ref()
                .map(|m| serde_json::to_string(m).unwrap_or_default()),
            active as i64,
            if active { alpha_for(labels) } else { 0.0 },
            out.metrics.as_ref().map(|m| m.accuracy),
            out.metrics.as_ref().map(|m| m.base_accuracy),
            out.metrics
                .as_ref()
                .map(|m| serde_json::to_string(m).unwrap_or_default()),
            Value::Array(out.traits.clone()).to_string(),
            out.positives as i64,
            out.negatives as i64,
            consumed,
            labels,
            FEATURE_VERSION,
            active as i64,
            now_ms()
        ],
    )?;
    conn.execute("DELETE FROM taste_score", [])?;
    Ok(())
}

/// The model in use: active, switched on and of the current feature version.
pub struct Active {
    pub model: Model,
    pub alpha: f64,
    pub version: i64,
}

pub fn load_active(conn: &Connection) -> Result<Option<Active>> {
    let row: Option<(Option<String>, f64, i64)> = conn
        .query_row(
            "SELECT model, alpha, model_version FROM taste_state
             WHERE id=1 AND active=1 AND enabled=1 AND feature_version=?1",
            [FEATURE_VERSION],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    Ok(row.and_then(|(m, alpha, version)| {
        m.and_then(|m| serde_json::from_str::<Model>(&m).ok())
            .filter(|m| m.feature_version == FEATURE_VERSION)
            .map(|model| Active {
                model,
                alpha,
                version,
            })
    }))
}

// ------------------------------------------------------------------ labels

/// Who made a rating / flag change (`PATCH /api/photos` `source`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    /// The user, by hand (`source` absent or `"user"`): a training label.
    User,
    /// The app (assistant actions, the client's redo of one): never a label. The user's
    /// label of the photo stays but is silenced while it disagrees with the photo.
    Auto,
    /// The client undoing an app decision (`"undo"`): no new label; a silenced user label
    /// that matches the restored value counts again.
    Undo,
}

impl LabelSource {
    pub fn parse(s: Option<&str>) -> Result<Self> {
        match s {
            None | Some("user") => Ok(Self::User),
            Some("auto") => Ok(Self::Auto),
            Some("undo") => Ok(Self::Undo),
            Some(x) => Err(CoreError::bad_request(format!(
                "source must be user|auto|undo, not {x:?}"
            ))),
        }
    }
}

fn upsert(conn: &Connection, photo_id: i64, kind: &str, value: f64) -> Result<usize> {
    Ok(conn
        .prepare_cached(
            "INSERT INTO preference_label(photo_id, kind, value, source, created_at)
             SELECT ?1,?2,?3,'patch',?4 WHERE EXISTS (SELECT 1 FROM photo WHERE id=?1)
             ON CONFLICT(photo_id, kind) DO UPDATE SET value=?3, source='patch', created_at=?4,
               legacy=0
               WHERE value != ?3 OR source != 'patch'",
        )?
        .execute(params![photo_id, kind, value, now_ms()])?)
}

fn remove(conn: &Connection, photo_id: i64, kind: &str) -> Result<usize> {
    Ok(conn
        .prepare_cached("DELETE FROM preference_label WHERE photo_id=?1 AND kind=?2")?
        .execute(params![photo_id, kind])?)
}

/// The user labels of `ids` that currently agree with their photo (`(photo id, kind)`).
pub fn agreeing(
    conn: &Connection,
    ids: &[i64],
) -> Result<std::collections::HashSet<(i64, String)>> {
    let mut out = std::collections::HashSet::new();
    for chunk in ids.chunks(500) {
        let ph = vec!["?"; chunk.len()].join(",");
        let mut st = conn.prepare(&format!(
            "SELECT l.photo_id, l.kind FROM preference_label l JOIN photo p ON p.id=l.photo_id
             WHERE l.photo_id IN ({ph}) AND {AGREES}"
        ))?;
        for row in st.query_map(params_from_iter(chunk.iter()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })? {
            out.insert(row?);
        }
    }
    Ok(out)
}

/// Labels whose agreement changed between `before` and now (silenced or revived).
fn flips(
    conn: &Connection,
    ids: &[i64],
    before: &std::collections::HashSet<(i64, String)>,
) -> Result<usize> {
    let after = agreeing(conn, ids)?;
    Ok(before.symmetric_difference(&after).count())
}

fn add_pending(conn: &Connection, n: usize) -> Result<()> {
    if n > 0 {
        ensure_state_row(conn)?;
        conn.execute(
            "UPDATE taste_state SET pending_labels=pending_labels+?1 WHERE id=1",
            [n as i64],
        )?;
    }
    Ok(())
}

/// Records the rating / flag of a patch (call after the photos were written; `before` =
/// [`agreeing`] for `req.ids` before the write). The user's own changes become labels while
/// learning is on (while it is off nothing new is recorded; clearing still removes); the
/// app's changes and undos only silence or revive existing labels. Returns the number of
/// label changes, which are also added to the pending count (retraining trigger).
pub fn record_patch(
    conn: &Connection,
    req: &PatchRequest,
    source: LabelSource,
    before: &std::collections::HashSet<(i64, String)>,
) -> Result<usize> {
    if req.user_rating.is_none() && req.flag.is_none() {
        return Ok(0);
    }
    // one transaction: a select-all patch used to commit once per photo
    let tx = conn.unchecked_transaction()?;
    let conn = &*tx;
    let enabled = is_enabled(conn)?;
    let changed = if source == LabelSource::User {
        let mut changed = 0;
        for id in &req.ids {
            match req.user_rating {
                Some(Some(r)) if r >= 1 => {
                    if enabled {
                        changed += upsert(conn, *id, "rating", r as f64)?
                    }
                }
                Some(_) => changed += remove(conn, *id, "rating")?,
                None => {}
            }
            match req.flag {
                Some(f) if f != 0 => {
                    if enabled {
                        changed += upsert(conn, *id, "flag", f as f64)?
                    }
                }
                Some(_) => changed += remove(conn, *id, "flag")?,
                None => {}
            }
        }
        changed
    } else {
        flips(conn, &req.ids, before)?
    };
    if enabled {
        add_pending(conn, changed)?;
    }
    tx.commit()?;
    Ok(if enabled { changed } else { 0 })
}

/// "Accept AI ratings" is the app's own decision: no label is recorded; a user rating label it
/// overwrote is silenced (and counted as pending). `before` as for [`record_patch`].
pub fn record_accept(
    conn: &Connection,
    updates: &[PhotoUpdate],
    before: &std::collections::HashSet<(i64, String)>,
) -> Result<usize> {
    let ids: Vec<i64> = updates.iter().map(|u| u.id).collect();
    let n = flips(conn, &ids, before)?;
    if is_enabled(conn)? {
        add_pending(conn, n)?;
        Ok(n)
    } else {
        Ok(0)
    }
}

// ------------------------------------------------------------------ status

/// `GET /api/taste`.
#[derive(Debug, Clone, Serialize)]
pub struct TasteOut {
    /// Explicit user labels (ratings and flags) that still agree with their photo.
    pub labels: i64,
    /// Labels recorded before taste v2 (1.0 cannot tell the assistant's "reject the rest"
    /// flags from the user's; the UI says so and offers Forget).
    pub legacy_labels: i64,
    /// A training is running (`POST /api/taste/train` answers 202; `taste.updated` follows).
    pub training: bool,
    /// Analysed photos the user kept / rejected (3 stars and undecided photos don't count).
    pub positives: i64,
    pub negatives: i64,
    pub min_per_class: i64,
    /// Decisions since the last training.
    pub pending: i64,
    pub retrain_every: i64,
    pub enabled: bool,
    /// `disabled` | `collecting` (cold start) | `ready` (enough labels, not trained yet) |
    /// `not_better` (trained, did not beat the generic score) | `active`.
    pub state: &'static str,
    pub active: bool,
    pub alpha: f64,
    /// Cross-validated accuracy of the model / of the generic score on the same photos.
    pub holdout_accuracy: Option<f64>,
    pub base_accuracy: Option<f64>,
    pub metrics: Option<Metrics>,
    pub traits: Vec<Value>,
    /// Photos with a cached taste score.
    pub scored: i64,
    pub feature_version: i64,
    pub updated_at: Option<i64>,
}

/// User labels that currently agree with their photo.
fn count_labels(conn: &Connection) -> Result<i64> {
    Ok(conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM preference_label l JOIN photo p ON p.id=l.photo_id WHERE {AGREES}"
        ),
        [],
        |r| r.get(0),
    )?)
}

pub fn status(conn: &Connection) -> Result<TasteOut> {
    let labels = count_labels(conn)?;
    let legacy_labels: i64 = conn.query_row(
        "SELECT COUNT(*) FROM preference_label WHERE source='patch' AND legacy=1",
        [],
        |r| r.get(0),
    )?;
    let items = labelled(conn)?;
    let positives = items.iter().filter(|x| x.1).count() as i64;
    let negatives = items.len() as i64 - positives;
    #[allow(clippy::type_complexity)]
    let row: Option<(
        i64,
        i64,
        f64,
        Option<String>,
        Option<String>,
        Option<i64>,
        i64,
        i64,
    )> = conn
        .query_row(
            "SELECT enabled, active, alpha, metrics, traits, trained_at, pending_labels,
                    feature_version
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
                    r.get(7)?,
                ))
            },
        )
        .optional()?;
    let (enabled, active, alpha, metrics, traits, at, pending, fv) =
        row.unwrap_or((1, 0, 0.0, None, None, None, 0, 0));
    let enabled = enabled != 0;
    let current = fv == FEATURE_VERSION;
    let active = enabled && current && active != 0;
    let metrics: Option<Metrics> = metrics
        .filter(|_| current)
        .and_then(|m| serde_json::from_str(&m).ok());
    let scored: i64 = conn.query_row("SELECT COUNT(*) FROM taste_score", [], |r| r.get(0))?;
    let min = MIN_PER_CLASS as i64;
    let state = if !enabled {
        "disabled"
    } else if active {
        "active"
    } else if positives < min || negatives < min {
        "collecting"
    } else if metrics.is_some() {
        "not_better"
    } else {
        "ready"
    };
    Ok(TasteOut {
        labels,
        legacy_labels,
        training: false,
        positives,
        negatives,
        min_per_class: min,
        pending,
        retrain_every: RETRAIN_EVERY,
        enabled,
        state,
        active,
        alpha: if active { alpha } else { 0.0 },
        holdout_accuracy: metrics.as_ref().map(|m| m.accuracy),
        base_accuracy: metrics.as_ref().map(|m| m.base_accuracy),
        metrics,
        traits: if active {
            traits
                .and_then(|t| serde_json::from_str::<Vec<Value>>(&t).ok())
                .unwrap_or_default()
        } else {
            vec![]
        },
        scored,
        feature_version: FEATURE_VERSION,
        updated_at: at,
    })
}

/// `GET /api/taste/explain/{id}`.
#[derive(Debug, Clone, Serialize)]
pub struct TasteExplain {
    pub photo_id: i64,
    /// `None` when no model is in use or the photo is not analysed.
    pub taste_score: Option<f64>,
    pub base_score: Option<f64>,
    pub contributions: Vec<Contribution>,
}

pub fn explain(conn: &Connection, photo_id: i64) -> Result<TasteExplain> {
    let exists: Option<i64> = conn
        .query_row("SELECT id FROM photo WHERE id=?1", [photo_id], |r| r.get(0))
        .optional()?;
    if exists.is_none() {
        return Err(CoreError::not_found(format!("photo {photo_id}")));
    }
    let sample = load_samples(conn, &[photo_id])?.pop();
    let mut out = TasteExplain {
        photo_id,
        taste_score: None,
        base_score: sample.as_ref().and_then(|s| s.base),
        contributions: vec![],
    };
    if let (Some(a), Some(s)) = (load_active(conn)?, sample) {
        out.taste_score = Some((a.model.taste_score(&s.input()) * 10_000.0).round() / 10_000.0);
        out.contributions = a.model.explain(&s.input());
    }
    Ok(out)
}

/// Oldest catalog schema `--eval` can read (analysis.saliency arrived in v15).
pub const EVAL_MIN_SCHEMA: i64 = 15;

/// Read-only evaluation of a catalog file (`imagepicker taste --eval`): the same gathering
/// and cross-validation as a training. Nothing is written to the catalog; SQLite may still
/// create the `-shm` / `-wal` side files of a WAL catalog next to it.
pub fn evaluate_catalog(path: &std::path::Path) -> Result<Outcome> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let newest = crate::db::MIGRATIONS.len() as i64;
    if v < EVAL_MIN_SCHEMA {
        return Err(CoreError::bad_request(format!(
            "catalog schema v{v} is too old to evaluate (needs v{EVAL_MIN_SCHEMA}..v{newest}; \
             open it once with this version of the app to upgrade it)"
        )));
    }
    if v > newest {
        return Err(CoreError::bad_request(format!(
            "catalog schema v{v} is newer than this build (v{newest}); use a newer imagepicker"
        )));
    }
    Ok(train(&gather(&conn)?))
}

// ------------------------------------------------------------------ Core API

impl Core {
    pub async fn taste(&self) -> Result<TasteOut> {
        let mut out = self.db.call(|c| status(c)).await?;
        out.training = self.taste_training.load(Ordering::SeqCst);
        Ok(out)
    }

    pub async fn taste_explain(&self, photo_id: i64) -> Result<TasteExplain> {
        self.db.call(move |c| explain(c, photo_id)).await
    }

    fn emit_taste(&self, out: &TasteOut) {
        self.events.emit(Event::TasteUpdated {
            labels: out.labels,
            active: out.active,
            alpha: out.alpha,
        });
    }

    async fn emit_taste_now(&self) {
        if let Ok(out) = self.taste().await {
            self.emit_taste(&out);
        }
    }

    /// Called after label changes: retrains in the background right away after
    /// [`RETRAIN_EVERY`] new decisions, or after [`RETRAIN_IDLE_MIN`] once the user paused for
    /// `taste_idle_ms` (re-read while waiting, so tests can shorten it). Also retries a
    /// rescoring that failed earlier.
    pub(crate) fn taste_after_labels(self: &Arc<Self>, changed: usize) {
        if changed == 0 || tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let seq = self.taste_seq.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(core) = weak.upgrade() else { return };
            if let Err(e) = core.rescore_taste().await {
                tracing::warn!(error = %e, "taste rescoring failed");
            }
            let pending = core.taste_pending().await;
            if pending < RETRAIN_IDLE_MIN {
                return;
            }
            let min = if pending >= RETRAIN_EVERY {
                RETRAIN_EVERY
            } else {
                drop(core);
                let start = Instant::now();
                loop {
                    let Some(c) = weak.upgrade() else { return };
                    if c.taste_seq.load(Ordering::SeqCst) != seq {
                        return; // newer decisions: their task decides
                    }
                    let idle = Duration::from_millis(c.taste_idle_ms.load(Ordering::Relaxed));
                    drop(c);
                    let waited = start.elapsed();
                    if waited >= idle {
                        break;
                    }
                    tokio::time::sleep((idle - waited).min(Duration::from_millis(50))).await;
                }
                RETRAIN_IDLE_MIN
            };
            let Some(core) = weak.upgrade() else { return };
            if let Err(e) = core.retrain_taste_if(min).await {
                tracing::warn!(error = %e, "taste retraining failed");
            }
        });
    }

    /// New decisions since the last training (-1 when learning is switched off).
    async fn taste_pending(&self) -> i64 {
        self.db
            .call(|c| {
                Ok(c.query_row(
                    "SELECT pending_labels FROM taste_state WHERE id=1 AND enabled=1",
                    [],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?
                .unwrap_or(-1))
            })
            .await
            .unwrap_or(-1)
    }

    /// Retrains when at least `min` decisions are pending (checked under the training lock,
    /// so a burst of triggers trains once).
    async fn retrain_taste_if(self: &Arc<Self>, min: i64) -> Result<()> {
        let _g = self.taste_lock.lock().await;
        if self.taste_pending().await >= min {
            let out = self.retrain_locked().await?;
            self.emit_taste(&out);
        }
        Ok(())
    }

    /// Trains now and waits for it, rescoring included (CLI `--retrain`, tests). 400 while
    /// switched off.
    pub async fn retrain_taste(self: &Arc<Self>) -> Result<TasteOut> {
        let _g = self.taste_lock.lock().await;
        if !self.db.call(|c| is_enabled(c)).await? {
            return Err(CoreError::bad_request("taste learning is switched off"));
        }
        let out = self.retrain_locked().await?;
        self.emit_taste(&out);
        Ok(out)
    }

    /// `POST /api/taste/train`: starts a training in the background (202); `taste.updated`
    /// follows when it is done. 400 while switched off.
    pub async fn start_taste_training(self: &Arc<Self>) -> Result<TasteOut> {
        if !self.db.call(|c| is_enabled(c)).await? {
            return Err(CoreError::bad_request("taste learning is switched off"));
        }
        self.taste_training.store(true, Ordering::SeqCst);
        let core = self.clone();
        tokio::spawn(async move {
            if let Err(e) = core.retrain_taste().await {
                tracing::warn!(error = %e, "taste retraining failed");
                core.taste_training.store(false, Ordering::SeqCst);
                core.emit_taste_now().await;
            }
        });
        let mut out = self.taste().await?;
        out.training = true;
        Ok(out)
    }

    /// Gather on a database connection, fit on the blocking pool without holding one, store
    /// unless the user switched off / reset in the meantime, then rescore.
    async fn retrain_locked(self: &Arc<Self>) -> Result<TasteOut> {
        self.taste_training.store(true, Ordering::SeqCst);
        let r = self.retrain_inner().await;
        self.taste_training.store(false, Ordering::SeqCst);
        r?;
        self.taste().await
    }

    async fn retrain_inner(self: &Arc<Self>) -> Result<()> {
        let gen = self.taste_gen.load(Ordering::SeqCst);
        let (data, pending) = self
            .db
            .call(|c| {
                let pending: i64 = c
                    .query_row(
                        "SELECT pending_labels FROM taste_state WHERE id=1",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                Ok((gather(c)?, pending))
            })
            .await?;
        let outcome = tokio::task::spawn_blocking(move || train(&data))
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("taste training panicked: {e}")))?;
        if let Some(m) = &outcome.metrics {
            tracing::info!(
                auc = m.auc,
                base_auc = m.base_auc,
                active = outcome.model.is_some(),
                ms = m.train_ms,
                "taste trained"
            );
        }
        let stored = {
            let _s = self.taste_state_lock.lock().await;
            if self.taste_gen.load(Ordering::SeqCst) != gen {
                false // switched off or forgotten while fitting: drop the result
            } else {
                self.db
                    .call(move |c| {
                        if !is_enabled(c)? {
                            return Ok(false);
                        }
                        store_outcome(c, &outcome, pending)?;
                        Ok(true)
                    })
                    .await?
            }
        };
        if stored {
            self.rescore_taste().await?;
        }
        Ok(())
    }

    /// Runs a pending rescoring (`taste_state.rescore_pending`), one at a time. When it fails
    /// the flag is set again, so the next label change or the next start retries it.
    pub(crate) async fn rescore_taste(&self) -> Result<()> {
        let _r = self.taste_rescore_lock.lock().await;
        let due = self
            .db
            .call(|c| {
                let due: i64 = c
                    .query_row(
                        "SELECT rescore_pending FROM taste_state WHERE id=1",
                        [],
                        |r| r.get(0),
                    )
                    .optional()?
                    .unwrap_or(0);
                if due != 0 {
                    c.execute("UPDATE taste_state SET rescore_pending=0 WHERE id=1", [])?;
                }
                Ok(due != 0)
            })
            .await?;
        if !due {
            return Ok(());
        }
        if let Err(e) = self.rescore_everything().await {
            let _ = self
                .db
                .call(|c| {
                    c.execute("UPDATE taste_state SET rescore_pending=1 WHERE id=1", [])?;
                    Ok(())
                })
                .await;
            return Err(e);
        }
        Ok(())
    }

    fn spawn_taste_rescore(self: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak = Arc::downgrade(self);
        tokio::spawn(async move {
            let Some(core) = weak.upgrade() else { return };
            if let Err(e) = core.rescore_taste().await {
                tracing::warn!(error = %e, "taste rescoring failed");
            }
            core.emit_taste_now().await;
        });
    }

    /// `PATCH /api/taste {"enabled"}`: switching off stops recording decisions and training
    /// and removes the personalisation (labels and model are kept); switching on uses the
    /// kept model again, or trains one in the background. Never waits for a running training
    /// (its result is dropped); the rescoring runs in the background.
    pub async fn set_taste_enabled(self: &Arc<Self>, on: bool) -> Result<TasteOut> {
        let usable = {
            let _s = self.taste_state_lock.lock().await;
            self.taste_gen.fetch_add(1, Ordering::SeqCst);
            self.db
                .call(move |c| {
                    let used = load_active(c)?.is_some();
                    ensure_state_row(c)?;
                    c.execute("UPDATE taste_state SET enabled=?1 WHERE id=1", [on as i64])?;
                    if !on {
                        c.execute("DELETE FROM taste_score", [])?;
                    }
                    let usable = load_active(c)?.is_some();
                    if used != usable {
                        c.execute("UPDATE taste_state SET rescore_pending=1 WHERE id=1", [])?;
                    }
                    Ok(usable)
                })
                .await?
        };
        self.spawn_taste_rescore();
        if on && !usable && tokio::runtime::Handle::try_current().is_ok() {
            let weak = Arc::downgrade(self);
            tokio::spawn(async move {
                let Some(core) = weak.upgrade() else { return };
                if let Err(e) = core.retrain_taste_if(0).await {
                    tracing::warn!(error = %e, "taste retraining failed");
                }
            });
        }
        let out = self.taste().await?;
        self.emit_taste(&out);
        Ok(out)
    }

    /// `POST /api/taste/reset`: forgets every label, the model, its metrics and the cached
    /// scores; AI scores go back to the base (rescored in the background). `disable` also
    /// switches learning off (opt out). Never waits for a running training.
    pub async fn reset_taste(self: &Arc<Self>, disable: bool) -> Result<()> {
        {
            let _s = self.taste_state_lock.lock().await;
            self.taste_gen.fetch_add(1, Ordering::SeqCst);
            self.db
                .call(move |c| {
                    let tx = c.transaction()?;
                    tx.execute("DELETE FROM preference_label", [])?;
                    tx.execute("DELETE FROM taste_score", [])?;
                    tx.execute("INSERT OR IGNORE INTO taste_state(id) VALUES(1)", [])?;
                    tx.execute(
                        "UPDATE taste_state SET model=NULL, alpha=0, holdout_accuracy=NULL,
                           base_accuracy=NULL, metrics=NULL, traits=NULL, pairs=0, positives=0,
                           negatives=0, pending_labels=0, labels_at_train=0, trained_at=NULL,
                           rescore_pending=CASE WHEN active=1 THEN 1 ELSE rescore_pending END,
                           active=0,
                           enabled=CASE WHEN ?1 THEN 0 ELSE enabled END
                         WHERE id=1",
                        [disable],
                    )?;
                    tx.commit()?;
                    Ok(())
                })
                .await?;
        }
        self.spawn_taste_rescore();
        let out = self.taste().await?;
        self.emit_taste(&out);
        Ok(())
    }

    /// At startup: finishes a rescoring that did not complete, and retrains a model of an
    /// older feature version (taste v1) once, so its fused scores are replaced (or reverted).
    pub(crate) fn spawn_taste_upgrade(self_: &Arc<Self>) {
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let weak = Arc::downgrade(self_);
        tokio::spawn(async move {
            let Some(core) = weak.upgrade() else { return };
            if let Err(e) = core.rescore_taste().await {
                tracing::warn!(error = %e, "taste rescoring failed");
            }
            let stale = core
                .db
                .call(|c| {
                    Ok(c.query_row(
                        "SELECT enabled=1 AND feature_version!=?1 AND (active=1 OR EXISTS
                           (SELECT 1 FROM preference_label WHERE source='patch'))
                         FROM taste_state WHERE id=1",
                        [FEATURE_VERSION],
                        |r| r.get::<_, bool>(0),
                    )
                    .optional()?
                    .unwrap_or(false))
                })
                .await
                .unwrap_or(false);
            if stale {
                if let Err(e) = core.retrain_taste_if(0).await {
                    tracing::warn!(error = %e, "taste upgrade retraining failed");
                }
            }
        });
    }

    /// Re-runs scoring for all bursts (picks up the current taste model and writes the taste
    /// scores) and tells clients. One database call per chunk of bursts, so other requests are
    /// served in between even on a 100k-photo catalog.
    async fn rescore_everything(&self) -> Result<()> {
        let bursts = self
            .db
            .call(|c| {
                let mut st = c.prepare("SELECT id FROM burst ORDER BY id")?;
                let v = st
                    .query_map([], |r| r.get::<_, i64>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(v)
            })
            .await?;
        for chunk in bursts.chunks(RESCORE_BURSTS_PER_CALL) {
            let chunk = chunk.to_vec();
            let sessions = self
                .db
                .call(move |c| {
                    let touched = store::rescore_bursts(c, &chunk)?;
                    let mut by_session: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
                    for part in touched.chunks(500) {
                        let ph = vec!["?"; part.len()].join(",");
                        let mut st = c.prepare(&format!(
                            "SELECT session_id, photo_id FROM session_photo WHERE photo_id IN ({ph})"
                        ))?;
                        let rows = st
                            .query_map(params_from_iter(part.iter()), |r| {
                                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                            })?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        for (s, p) in rows {
                            by_session.entry(s).or_default().push(p);
                        }
                    }
                    Ok(by_session)
                })
                .await?;
            for (session_id, ids) in sessions {
                self.events.emit(Event::AnalysisUpdated { session_id, ids });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64) / ((1u64 << 31) as f64)
        }
    }

    /// `n` photos with an `dim`-dimensional embedding; `label(i, hidden, aesthetic)` decides.
    fn synth(
        n: usize,
        dim: usize,
        seed: u64,
        label: &dyn Fn(usize, f64, f64) -> bool,
    ) -> TrainData {
        let mut rng = Lcg(seed);
        let mut samples = Vec::new();
        let mut y = Vec::new();
        for i in 0..n {
            let hidden = rng.next();
            let aesthetic = rng.next();
            let mut emb: Vec<f32> = (0..dim).map(|_| rng.next() as f32 - 0.5).collect();
            emb[0] = hidden as f32;
            samples.push(Sample {
                photo_id: i as i64 + 1,
                feat: PhotoFeat {
                    photo_id: i as i64 + 1,
                    sharpness: Some(0.3 + 0.7 * rng.next()),
                    aesthetic: Some(aesthetic),
                    exposure: Some(rng.next()),
                    ..Default::default()
                },
                issues: if rng.next() < 0.1 { 2 } else { 0 },
                emb: Some(emb),
                base: Some(aesthetic),
                group: group_key(i as i64 + 1, None, None),
            });
            y.push(label(i, hidden, aesthetic));
        }
        let positives = y.iter().filter(|v| **v).count();
        TrainData {
            negatives: y.len() - positives,
            positives,
            samples,
            y,
        }
    }

    #[test]
    fn learns_a_planted_rule_that_the_generic_score_misses() {
        let data = synth(240, 16, 1, &|_, h, _| h > 0.5);
        let out = train(&data);
        let m = out.metrics.clone().unwrap();
        assert!(m.auc > 0.9, "{m:?}");
        assert!(m.base_auc < 0.7, "{m:?}");
        assert!(m.accuracy > m.base_accuracy, "{m:?}");
        assert_eq!(m.folds, FOLDS);
        let model = out.model.expect("beats the baseline");
        assert_eq!(model.feature_version, FEATURE_VERSION);
        assert_eq!(model.dim, 16);
        // scores follow the rule on the training photos
        let (mut hi, mut lo) = (vec![], vec![]);
        for (s, c) in data.samples.iter().zip(&data.y) {
            let p = model.taste_score(&s.input());
            assert!((0.0..=1.0).contains(&p));
            if *c {
                hi.push(p)
            } else {
                lo.push(p)
            }
        }
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        assert!(mean(&hi) > mean(&lo) + 0.3, "{} {}", mean(&hi), mean(&lo));
        // the rule lives in the embedding: the explanation names the overall look
        let top = &data.samples[data.y.iter().position(|c| *c).unwrap()];
        let ex = model.explain(&top.input());
        assert!(ex.len() <= 6);
        assert!(
            ex.iter().any(|c| c.feature == "look" && c.delta > 0.0),
            "{ex:?}"
        );
    }

    #[test]
    fn training_is_deterministic() {
        let data = synth(300, 24, 2, &|i, h, a| h + 0.3 * a > 0.6 || i % 17 == 0);
        let a = train(&data);
        let b = train(&data);
        assert_eq!(a.model, b.model);
        assert!(a.model.is_some());
        let (ma, mb) = (a.metrics.unwrap(), b.metrics.unwrap());
        assert_eq!(
            (ma.auc, ma.base_auc, ma.lambda),
            (mb.auc, mb.base_auc, mb.lambda)
        );
        assert_eq!(a.traits, b.traits);
    }

    #[test]
    fn parallel_chunks_sum_like_sequential_chunks() {
        let d = 5;
        let n = PAR_MIN_ROWS * 3 + 17;
        let mut rng = Lcg(3);
        let x: Vec<f32> = (0..n * d).map(|_| rng.next() as f32).collect();
        let m = Mat { d, x };
        let idx: Vec<usize> = (0..n).collect();
        let coef = |i: usize, r: &[f32]| (i % 3) as f32 * r[0];
        let par = m.accumulate(&idx, &coef);
        let mut seq = vec![0.0f32; d];
        for chunk in idx.chunks(CHUNK) {
            let mut g = vec![0.0f32; d];
            for &i in chunk {
                let r = m.row(i);
                let c = coef(i, r);
                if c != 0.0 {
                    for (a, e) in g.iter_mut().zip(r) {
                        *a += c * e;
                    }
                }
            }
            for (a, e) in seq.iter_mut().zip(&g) {
                *a += e;
            }
        }
        assert_eq!(par, seq);
    }

    #[test]
    fn cold_start_needs_both_classes() {
        // 29 negatives: nothing is trained, nothing is measured
        let mut data = synth(120, 8, 4, &|i, _, _| i >= 29);
        assert_eq!(data.negatives, 29);
        let out = train(&data);
        assert!(out.model.is_none() && out.metrics.is_none());
        assert_eq!((out.positives, out.negatives), (91, 29));
        // one more negative and it trains
        data.y[29] = false;
        data.negatives = 30;
        data.positives = 90;
        assert!(train(&data).metrics.is_some());
    }

    #[test]
    fn stays_off_when_not_better_than_the_generic_score() {
        // the user keeps exactly what the generic score likes: nothing to add
        let data = synth(240, 8, 5, &|_, _, a| a > 0.5);
        let out = train(&data);
        let m = out.metrics.unwrap();
        assert!(m.base_auc > 0.99, "{m:?}");
        assert!(out.model.is_none(), "{m:?}");
        assert!(out.traits.is_empty());
        // and pure noise does not switch on either
        let mut rng = Lcg(6);
        let coin: Vec<bool> = (0..240).map(|_| rng.next() > 0.5).collect();
        let noise = synth(240, 8, 7, &|i, _, _| coin[i]);
        let out = train(&noise);
        assert!(out.model.is_none(), "{:?}", out.metrics);
    }

    #[test]
    fn near_identical_frames_do_not_leak_across_folds() {
        // 120 bursts of two identical frames; whether a burst is kept is a coin flip, so there is
        // nothing to learn. Folds grouped by burst must not let the model "recognise" a frame's
        // twin from the training side.
        let mut rng = Lcg(9);
        let mut samples = Vec::new();
        let mut y = Vec::new();
        for b in 0..120i64 {
            let emb: Vec<f32> = (0..48).map(|_| rng.next() as f32 - 0.5).collect();
            let keep = rng.next() > 0.5;
            let aesthetic = rng.next();
            for k in 0..2 {
                let id = b * 2 + k + 1;
                samples.push(Sample {
                    photo_id: id,
                    feat: PhotoFeat {
                        photo_id: id,
                        aesthetic: Some(aesthetic),
                        ..Default::default()
                    },
                    issues: 0,
                    emb: Some(emb.clone()),
                    base: Some(aesthetic),
                    group: group_key(id, Some(1000 + b), None),
                });
                y.push(keep);
            }
        }
        let positives = y.iter().filter(|v| **v).count();
        let data = TrainData {
            negatives: y.len() - positives,
            positives,
            samples,
            y,
        };
        let out = train(&data);
        let m = out.metrics.unwrap();
        assert!(m.auc < 0.7, "{m:?}");
        assert!(out.model.is_none(), "{m:?}");
        // the folds really are per burst
        assert_eq!(
            mix(group_key(1, Some(1000), None)) % FOLDS,
            mix(group_key(2, Some(1000), None)) % FOLDS
        );
    }

    #[test]
    fn the_baseline_skips_photos_without_a_generic_score() {
        let mut data = synth(240, 8, 10, &|_, h, _| h > 0.5);
        for s in data.samples.iter_mut().step_by(3) {
            s.base = None;
        }
        let m = train(&data).metrics.unwrap();
        assert!(m.base_auc < 0.7 && m.auc > 0.9, "{m:?}");
    }

    #[test]
    fn fuse_survives_non_finite_scores() {
        use crate::analysis::scoring::Ranked;
        let r = |id: i64, q: f64| Ranked {
            photo_id: id,
            rank: id as usize,
            burst_size: 2,
            q,
            base_q: q,
            hard_issue: false,
            ai_rating: 0.0,
            issues: vec![],
            contributions: vec![],
            reasons: vec![],
            face_score: None,
        };
        let user: HashMap<i64, f64> = [(1, f64::NAN), (2, 0.9)].into_iter().collect();
        let out = fuse(vec![r(1, f64::NAN), r(2, 0.4)], &user, 0.5);
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|x| x.q.is_finite()));
        assert_eq!(out[0].photo_id, 2);
    }

    #[test]
    fn auc_and_accuracy() {
        assert_eq!(
            auc(&[0.1, 0.4, 0.35, 0.8], &[false, false, true, true]),
            Some(0.75)
        );
        assert_eq!(auc(&[0.5, 0.5], &[false, true]), Some(0.5));
        assert_eq!(auc(&[0.5, 0.6], &[true, true]), None);
        assert_eq!(
            rank_accuracy(&[0.9, 0.1, 0.8, 0.2], &[true, false, true, false]),
            1.0
        );
        assert_eq!(rank_accuracy(&[0.1, 0.9], &[true, false]), 0.0);
    }

    #[test]
    fn labels_map_to_classes() {
        assert_eq!(class_of(Some(5.0), None), Some(true));
        assert_eq!(class_of(Some(4.0), None), Some(true));
        assert_eq!(class_of(Some(3.0), None), None);
        assert_eq!(class_of(Some(2.0), None), Some(false));
        assert_eq!(class_of(Some(1.0), None), Some(false));
        // the flag wins over the stars
        assert_eq!(class_of(Some(5.0), Some(-1.0)), Some(false));
        assert_eq!(class_of(Some(1.0), Some(1.0)), Some(true));
        assert_eq!(class_of(None, None), None);
    }

    #[test]
    fn subsampling_is_bounded_and_keeps_the_rare_class() {
        let items: Vec<(i64, bool)> = (0..1000).map(|i| (i, i % 50 == 0)).collect();
        let s = stratified(&items, 100);
        assert_eq!(s.len(), 100);
        assert_eq!(s.iter().filter(|x| x.1).count(), 20);
        assert!(s.windows(2).all(|w| w[0].0 < w[1].0));
        assert_eq!(s, stratified(&items, 100));
        assert_eq!(stratified(&items[..10], 100).len(), 10);
    }

    #[test]
    fn traits_name_interpretable_features() {
        let mut w = vec![0.0f32; N_FIXED + 4];
        w[0] = 0.8; // sharpness
        w[13] = -0.6; // closed eyes
        w[NAMED.len() + 5] = 0.3; // food scenes
        w[N_FIXED] = 5.0; // embedding: never a trait
        let t = traits_of(&w);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0]["key"], "likes");
        assert_eq!(t[0]["params"]["feature"], "sharpness");
        assert_eq!(t[1]["key"], "dislikes");
        assert_eq!(t[1]["params"]["feature"], "closed_eyes");
        assert_eq!(t[2]["params"]["feature"], "scene_food");
    }

    /// `cargo test --release -p ip-core taste_training_speed -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn taste_training_speed() {
        for n in [1_000usize, 5_000, 20_000] {
            let data = synth(n, 768, 8, &|_, h, a| h + 0.2 * a > 0.6);
            let t = Instant::now();
            let out = train(&data);
            println!(
                "n={n} dim=768: {} ms (auc {:?}, active {})",
                t.elapsed().as_millis(),
                out.metrics.map(|m| m.auc),
                out.model.is_some()
            );
        }
    }
}
