//! Quality score, issue tags, star mapping and in-group ranking (docs/03 §3.2). Pure functions.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::vecs::cosine;

// ------------------------------------------------------------------ thresholds
pub const EYES_CLOSED_BELOW: f64 = 0.45;
pub const BLURRY_BELOW: f64 = 0.35;
pub const BLURRY_SEVERE_BELOW: f64 = 0.20;
pub const OVEREXPOSED_CLIPPED: f64 = 0.10;
pub const OVEREXPOSED_SEVERE_CLIPPED: f64 = 0.25;
pub const OVEREXPOSED_MEAN: f64 = 0.82;
pub const OVEREXPOSED_SEVERE_MEAN: f64 = 0.90;
/// A bright frame that keeps this much centre detail is high-key (white backdrop, snow, paper),
/// not blown out; below [`OVEREXPOSED_SEVERE_MEAN`] it is not flagged.
pub const OVEREXPOSED_DETAIL_BELOW: f64 = 0.45;
pub const UNDEREXPOSED_CRUSHED: f64 = 0.40;
pub const UNDEREXPOSED_SEVERE_CRUSHED: f64 = 0.60;
pub const UNDEREXPOSED_MEAN: f64 = 0.12;
pub const UNDEREXPOSED_SEVERE_MEAN: f64 = 0.07;
/// Same idea for dark frames: a low mean with this much detail is low-key / night (neon, stage,
/// candle light); only [`UNDEREXPOSED_SEVERE_MEAN`] or crushed shadows still flag it.
pub const UNDEREXPOSED_DETAIL_BELOW: f64 = 0.40;
/// Immerkaer noise is 0.12-0.49 on the noisy frames of the synthetic library and at most 0.38 on
/// textured clean ones (foliage, water); see `bench/eval-library.py`.
pub const NOISY_AT: f64 = 0.22;
pub const NOISY_AT_NIGHT: f64 = 0.30;
/// A burst member this much less sharp than the burst's sharpest frame is flagged blurry
/// (motion blur on the subject barely moves the whole-frame measure, so a fixed threshold misses it).
pub const BLURRY_IN_BURST_RATIO: f64 = 0.80;

pub const PENALTY_CLOSED_EYES: f64 = 0.25;
pub const PENALTY_SEVERE_BLUR: f64 = 0.30;
pub const PENALTY_SEVERE_EXPOSURE: f64 = 0.20;
/// Light penalty for near-duplicates of a better photo with no distinct merit.
pub const PENALTY_REDUNDANT: f64 = 0.03;
pub const REDUNDANT_COS: f32 = 0.93;
pub const GROUP_BEST_BONUS: f64 = 0.5;
pub const HARD_ISSUE_STAR_CAP: f64 = 2.0;

// ------------------------------------------------------------------ issues
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Issue {
    ClosedEyes,
    Blurry,
    Overexposed,
    Underexposed,
    Noisy,
    Tilted,
}

impl Issue {
    pub const ALL: [Issue; 6] = [
        Issue::ClosedEyes,
        Issue::Blurry,
        Issue::Overexposed,
        Issue::Underexposed,
        Issue::Noisy,
        Issue::Tilted,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::ClosedEyes => "closed_eyes",
            Self::Blurry => "blurry",
            Self::Overexposed => "overexposed",
            Self::Underexposed => "underexposed",
            Self::Noisy => "noisy",
            Self::Tilted => "tilted",
        }
    }

    /// Bit in `photo.issues` (docs/05: 1 closed eyes, 2 blur, 4 over, 8 under, 16 tilt, 32 bystander;
    /// 64 = noisy is an extension).
    pub fn bit(self) -> i64 {
        match self {
            Self::ClosedEyes => 1,
            Self::Blurry => 2,
            Self::Overexposed => 4,
            Self::Underexposed => 8,
            Self::Tilted => 16,
            Self::Noisy => 64,
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|i| i.key() == s)
    }

    pub fn from_mask(mask: i64) -> Vec<Issue> {
        Self::ALL
            .into_iter()
            .filter(|i| mask & i.bit() != 0)
            .collect()
    }

    pub fn to_mask(issues: &[Issue]) -> i64 {
        issues.iter().fold(0, |m, i| m | i.bit())
    }
}

// ------------------------------------------------------------------ features
#[derive(Debug, Clone, Default)]
pub struct FaceFeat {
    pub id: i64,
    pub person_id: Option<i64>,
    pub bbox: [f64; 4],
    pub eyes_open: Option<f64>,
    pub smile: Option<f64>,
    pub gaze: Option<f64>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub sharpness: Option<f64>,
    pub is_subject: bool,
}

#[derive(Debug, Clone, Default)]
pub struct PhotoFeat {
    pub photo_id: i64,
    pub sharpness: Option<f64>,
    pub exposure: Option<f64>,
    pub noise: Option<f64>,
    pub iqa: Option<f64>,
    pub aesthetic: Option<f64>,
    pub clipped_highlights: Option<f64>,
    pub crushed_shadows: Option<f64>,
    pub mean_luminance: Option<f64>,
    pub scene_type: Option<String>,
    pub faces: Vec<FaceFeat>,
}

fn clamp01(x: f64) -> f64 {
    x.clamp(0.0, 1.0)
}

/// Expression quality of one face in 0-1 from whatever indicators exist (weights renormalised).
pub fn expression_score(f: &FaceFeat) -> Option<f64> {
    let mut parts: Vec<(f64, f64)> = Vec::new();
    if let Some(e) = f.eyes_open {
        parts.push((0.40, clamp01(e)));
    }
    if let Some(s) = f.smile {
        parts.push((0.20, 0.4 + 0.6 * clamp01(s)));
    }
    if let Some(g) = f.gaze {
        parts.push((0.15, clamp01(g)));
    }
    if f.yaw.is_some() || f.pitch.is_some() {
        let yaw = f.yaw.unwrap_or(0.0).abs();
        let pitch = f.pitch.unwrap_or(0.0).abs();
        let pen = ((yaw - 15.0).max(0.0) / 45.0 + (pitch - 15.0).max(0.0) / 60.0).min(1.0);
        parts.push((0.10, 1.0 - pen));
    }
    if let Some(s) = f.sharpness {
        parts.push((0.15, clamp01(s)));
    }
    let w: f64 = parts.iter().map(|p| p.0).sum();
    if w <= 0.0 {
        return None;
    }
    Some(parts.iter().map(|(w, v)| w * v).sum::<f64>() / w)
}

/// `is_subject` per face of one photo (docs/03 §4.2): weight = sqrt(area) x centredness x
/// sharpness x (named bonus) x looking; a face is a subject when its weight is at least 45% of the
/// photo's strongest face and it covers at least 0.15% of the frame.
pub fn subject_flags(faces: &[(FaceFeat, bool /* person named */)]) -> Vec<bool> {
    let weights: Vec<(f64, f64)> = faces
        .iter()
        .map(|(f, named)| {
            let area = (f.bbox[2] * f.bbox[3]).max(0.0);
            let (cx, cy) = (f.bbox[0] + f.bbox[2] / 2.0, f.bbox[1] + f.bbox[3] / 2.0);
            let d =
                ((cx - 0.5).powi(2) + (cy - 0.5).powi(2)).sqrt() / std::f64::consts::FRAC_1_SQRT_2;
            let center = (1.0 - 1.2 * d).clamp(0.2, 1.0);
            let sharp = 0.3 + 0.7 * clamp01(f.sharpness.unwrap_or(0.6));
            let look = 0.7 + 0.3 * clamp01(f.gaze.unwrap_or(0.5));
            let named = if *named { 1.25 } else { 1.0 };
            (area, area.sqrt() * center * sharp * look * named)
        })
        .collect();
    let max_w = weights.iter().map(|w| w.1).fold(0.0, f64::max);
    weights
        .iter()
        .map(|(area, w)| *area >= 0.0015 && *w >= 0.45 * max_w)
        .collect()
}

// ------------------------------------------------------------------ per-photo score
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contribution {
    pub key: String,
    pub label_key: String,
    pub delta: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reason {
    pub key: String,
    pub params: Value,
}

#[derive(Debug, Clone)]
pub struct PhotoScore {
    pub photo_id: i64,
    /// Composite 0-1 before the group adjustment.
    pub q: f64,
    pub face_score: Option<f64>,
    /// (component key, value 0-1, normalised weight)
    pub components: Vec<(&'static str, f64, f64)>,
    pub issues: Vec<Issue>,
    pub hard_issue: bool,
    pub contributions: Vec<Contribution>,
    pub reasons: Vec<Reason>,
    pub effective_scene: &'static str,
    /// The photo's sharpness measure, also when the scene does not weigh it (burst comparison).
    pub sharpness: Option<f64>,
}

fn scene_class(feat: &PhotoFeat) -> &'static str {
    match feat.scene_type.as_deref() {
        Some("portrait") => "portrait",
        Some("group") => "group",
        Some("landscape") => "landscape",
        Some("architecture") => "architecture",
        Some("night") => "night",
        Some("food") => "food",
        Some("pet") => "pet",
        Some("other") => "other",
        _ => {
            let subjects = feat.faces.iter().filter(|f| f.is_subject).count();
            match subjects {
                0 => "other",
                1 | 2 => "portrait",
                _ => "group",
            }
        }
    }
}

fn weights(scene: &str) -> &'static [(&'static str, f64)] {
    match scene {
        "portrait" | "group" => &[
            ("face", 0.40),
            ("sharpness", 0.25),
            ("aesthetic", 0.20),
            ("exposure", 0.15),
        ],
        "landscape" | "architecture" => &[
            ("aesthetic", 0.40),
            ("technical", 0.30),
            ("composition", 0.20),
            ("exposure", 0.10),
        ],
        "night" => &[
            ("aesthetic", 0.25),
            ("sharpness", 0.35),
            ("technical", 0.20),
            ("exposure", 0.15),
        ],
        _ => &[
            ("aesthetic", 0.30),
            ("sharpness", 0.30),
            ("technical", 0.25),
            ("exposure", 0.15),
        ],
    }
}

pub fn detect_issues(feat: &PhotoFeat, scene: &str) -> (Vec<Issue>, Vec<Issue>) {
    let mut issues = Vec::new();
    let mut hard = Vec::new();
    let subjects: Vec<&FaceFeat> = feat.faces.iter().filter(|f| f.is_subject).collect();
    if subjects
        .iter()
        .any(|f| f.eyes_open.map(|e| e < EYES_CLOSED_BELOW).unwrap_or(false))
    {
        issues.push(Issue::ClosedEyes);
        hard.push(Issue::ClosedEyes);
    }
    if let Some(s) = feat.sharpness {
        if s < BLURRY_BELOW {
            issues.push(Issue::Blurry);
            if s < BLURRY_SEVERE_BELOW {
                hard.push(Issue::Blurry);
            }
        }
    }
    // Without a sharpness measure, assume the detail is gone (the old behaviour).
    let detail = feat.sharpness.unwrap_or(0.0);
    let over = (feat.clipped_highlights.unwrap_or(0.0) >= OVEREXPOSED_CLIPPED
        || feat.mean_luminance.unwrap_or(0.0) > OVEREXPOSED_MEAN)
        && (detail < OVEREXPOSED_DETAIL_BELOW
            || feat.mean_luminance.unwrap_or(0.0) > OVEREXPOSED_SEVERE_MEAN);
    if over {
        issues.push(Issue::Overexposed);
        if feat.clipped_highlights.unwrap_or(0.0) >= OVEREXPOSED_SEVERE_CLIPPED
            || feat.mean_luminance.unwrap_or(0.0) > OVEREXPOSED_SEVERE_MEAN
        {
            hard.push(Issue::Overexposed);
        }
    }
    if scene != "night" {
        let under = feat.crushed_shadows.unwrap_or(0.0) >= UNDEREXPOSED_CRUSHED
            || feat
                .mean_luminance
                .map(|m| {
                    m < UNDEREXPOSED_SEVERE_MEAN
                        || (m < UNDEREXPOSED_MEAN && detail < UNDEREXPOSED_DETAIL_BELOW)
                })
                .unwrap_or(false);
        if under {
            issues.push(Issue::Underexposed);
            if feat.crushed_shadows.unwrap_or(0.0) >= UNDEREXPOSED_SEVERE_CRUSHED
                || feat
                    .mean_luminance
                    .map(|m| m < UNDEREXPOSED_SEVERE_MEAN)
                    .unwrap_or(false)
            {
                hard.push(Issue::Underexposed);
            }
        }
    }
    let noisy_at = if scene == "night" {
        NOISY_AT_NIGHT
    } else {
        NOISY_AT
    };
    if feat.noise.map(|n| n >= noisy_at).unwrap_or(false) {
        issues.push(Issue::Noisy);
    }
    (issues, hard)
}

pub fn score_photo(feat: &PhotoFeat) -> PhotoScore {
    let scene = scene_class(feat);
    let subjects: Vec<&FaceFeat> = feat.faces.iter().filter(|f| f.is_subject).collect();
    let face_scores: Vec<f64> = subjects
        .iter()
        .filter_map(|f| expression_score(f))
        .collect();
    let face_score = (!face_scores.is_empty())
        .then(|| face_scores.iter().sum::<f64>() / face_scores.len() as f64);

    let noise_eff = feat
        .noise
        .map(|n| if scene == "night" { n * 0.5 } else { n });
    let technical = feat.iqa.or(noise_eff.map(|n| 1.0 - n));
    let value = |key: &str| -> Option<f64> {
        match key {
            "face" => face_score,
            "sharpness" => feat.sharpness,
            "aesthetic" => feat.aesthetic,
            "exposure" => feat.exposure,
            "technical" => technical,
            _ => None, // composition: not measured in M2
        }
        .map(clamp01)
    };
    let present: Vec<(&'static str, f64, f64)> = weights(scene)
        .iter()
        .filter_map(|(k, w)| value(k).map(|v| (*k, v, *w)))
        .collect();
    let wsum: f64 = present.iter().map(|p| p.2).sum();
    let components: Vec<(&'static str, f64, f64)> = present
        .into_iter()
        .map(|(k, v, w)| (k, v, if wsum > 0.0 { w / wsum } else { 0.0 }))
        .collect();

    let (issues, hard) = detect_issues(feat, scene);
    let mut contributions: Vec<Contribution> = components
        .iter()
        .map(|(k, v, w)| Contribution {
            key: (*k).to_string(),
            label_key: format!("score.{k}"),
            delta: round4(w * (v - 0.5)),
        })
        .collect();
    let mut q = if components.is_empty() {
        0.5
    } else {
        components.iter().map(|(_, v, w)| v * w).sum()
    };
    for h in &hard {
        let p = match h {
            Issue::ClosedEyes => PENALTY_CLOSED_EYES,
            Issue::Blurry => PENALTY_SEVERE_BLUR,
            _ => PENALTY_SEVERE_EXPOSURE,
        };
        q -= p;
        contributions.push(Contribution {
            key: h.key().to_string(),
            label_key: format!("issue.{}", h.key()),
            delta: -p,
        });
    }
    contributions.sort_by(|a, b| b.delta.abs().partial_cmp(&a.delta.abs()).unwrap());

    let mut reasons = Vec::new();
    for i in &issues {
        let params = if *i == Issue::ClosedEyes {
            let ids: Vec<Value> = subjects
                .iter()
                .filter(|f| f.eyes_open.map(|e| e < EYES_CLOSED_BELOW).unwrap_or(false))
                .map(|f| f.person_id.map(Value::from).unwrap_or(Value::Null))
                .collect();
            let first = ids.iter().find(|v| !v.is_null()).cloned();
            json!({"count": ids.len(), "person_id": first})
        } else {
            json!({})
        };
        reasons.push(Reason {
            key: i.key().to_string(),
            params,
        });
    }
    if feat.aesthetic.map(|a| a >= 0.75).unwrap_or(false) {
        reasons.push(Reason {
            key: "strong_aesthetic".into(),
            params: json!({}),
        });
    }
    if face_score.map(|f| f >= 0.85).unwrap_or(false) {
        reasons.push(Reason {
            key: "good_expression".into(),
            params: json!({}),
        });
    }

    PhotoScore {
        photo_id: feat.photo_id,
        q: clamp01(q),
        face_score,
        components,
        issues,
        hard_issue: !hard.is_empty(),
        contributions,
        reasons,
        effective_scene: scene,
        sharpness: feat.sharpness,
    }
}

fn round4(x: f64) -> f64 {
    (x * 10_000.0).round() / 10_000.0
}

// ------------------------------------------------------------------ stars
/// Piecewise mapping of the composite score to whole stars.
pub fn base_stars(q: f64) -> f64 {
    match q {
        q if q < 0.20 => 0.0,
        q if q < 0.35 => 1.0,
        q if q < 0.50 => 2.0,
        q if q < 0.65 => 3.0,
        q if q < 0.80 => 4.0,
        _ => 5.0,
    }
}

/// Absolute stars (+0.5 for the group's best, never above 5) capped at 2 stars with a hard issue.
pub fn star_rating(q: f64, group_best: bool, hard_issue: bool) -> f64 {
    let mut s = base_stars(q);
    if group_best {
        s = (s + GROUP_BEST_BONUS).min(5.0);
    }
    if hard_issue {
        s = s.min(HARD_ISSUE_STAR_CAP);
    }
    s
}

// ------------------------------------------------------------------ in-group ranking
pub struct BurstMember<'a> {
    pub score: PhotoScore,
    pub taken_at: Option<i64>,
    pub emb: Option<&'a [f32]>,
}

#[derive(Debug, Clone)]
pub struct Ranked {
    pub photo_id: i64,
    pub rank: usize,
    pub burst_size: usize,
    /// Composite after the redundancy adjustment.
    pub q: f64,
    /// The composite before personalisation (equals `q` unless a taste model is fused in).
    pub base_q: f64,
    pub hard_issue: bool,
    pub ai_rating: f64,
    pub issues: Vec<Issue>,
    pub contributions: Vec<Contribution>,
    pub reasons: Vec<Reason>,
    pub face_score: Option<f64>,
}

/// Ranks the members of one burst (best first) and produces final score, stars and reasons.
pub fn rank_burst(members: Vec<BurstMember<'_>>) -> Vec<Ranked> {
    let n = members.len();
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| {
        let (ma, mb) = (&members[a], &members[b]);
        mb.score
            .q
            .partial_cmp(&ma.score.q)
            .unwrap()
            .then_with(|| {
                mb.score
                    .face_score
                    .unwrap_or(0.0)
                    .partial_cmp(&ma.score.face_score.unwrap_or(0.0))
                    .unwrap()
            })
            .then_with(|| ma.taken_at.cmp(&mb.taken_at))
            .then_with(|| ma.score.photo_id.cmp(&mb.score.photo_id))
    });
    let sharpest = members
        .iter()
        .filter_map(|m| m.score.sharpness)
        .fold(f64::NAN, f64::max);
    let mut out = Vec::with_capacity(n);
    for (rank, &mi) in order.iter().enumerate() {
        let m = &members[mi];
        let mut q = m.score.q;
        let mut contributions = m.score.contributions.clone();
        let mut reasons = m.score.reasons.clone();
        let mut issues = m.score.issues.clone();
        let blurrier = m
            .score
            .sharpness
            .is_some_and(|s| s < BLURRY_IN_BURST_RATIO * sharpest);
        if n >= 2 && blurrier && !issues.contains(&Issue::Blurry) {
            issues.push(Issue::Blurry);
            issues.sort();
            reasons.push(Reason {
                key: Issue::Blurry.key().to_string(),
                params: json!({}),
            });
        }
        if n >= 2 && rank == 0 {
            reasons.insert(
                0,
                Reason {
                    key: "best_in_group".into(),
                    params: json!({"size": n}),
                },
            );
            let second = &members[order[1]].score;
            if let (Some(a), Some(b)) = (m.score.face_score, second.face_score) {
                if a - b >= 0.1 {
                    reasons.push(Reason {
                        key: "best_expression".into(),
                        params: json!({}),
                    });
                }
            }
            if let (Some(a), Some(b)) = (comp(&m.score, "sharpness"), comp(second, "sharpness")) {
                if a - b >= 0.1 {
                    reasons.push(Reason {
                        key: "sharpest_in_group".into(),
                        params: json!({}),
                    });
                }
            }
        }
        if rank >= 1 {
            // redundancy: very similar to a better photo and nothing clearly better than it
            let redundant = order[..rank].iter().any(|&bi| {
                let b = &members[bi];
                let similar = match (m.emb, b.emb) {
                    (Some(x), Some(y)) => cosine(x, y) >= REDUNDANT_COS,
                    _ => false,
                };
                similar
                    && m.score.components.iter().all(|(k, v, _)| {
                        comp(&b.score, k).map(|bv| *v <= bv + 0.05).unwrap_or(true)
                    })
            });
            if redundant {
                q = (q - PENALTY_REDUNDANT).max(0.0);
                contributions.push(Contribution {
                    key: "redundant".into(),
                    label_key: "score.redundant".into(),
                    delta: -PENALTY_REDUNDANT,
                });
                reasons.push(Reason {
                    key: "redundant_in_group".into(),
                    params: json!({}),
                });
            }
        }
        let ai_rating = star_rating(q, n >= 2 && rank == 0, m.score.hard_issue);
        out.push(Ranked {
            photo_id: m.score.photo_id,
            rank,
            burst_size: n,
            q,
            base_q: q,
            hard_issue: m.score.hard_issue,
            ai_rating,
            issues,
            contributions,
            reasons,
            face_score: m.score.face_score,
        });
    }
    out
}

fn comp(s: &PhotoScore, key: &str) -> Option<f64> {
    s.components.iter().find(|c| c.0 == key).map(|c| c.1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn face(eyes: f64, subject: bool) -> FaceFeat {
        FaceFeat {
            id: 1,
            bbox: [0.4, 0.3, 0.2, 0.3],
            eyes_open: Some(eyes),
            smile: Some(0.8),
            gaze: Some(0.9),
            yaw: Some(3.0),
            pitch: Some(2.0),
            sharpness: Some(0.9),
            is_subject: subject,
            ..Default::default()
        }
    }

    fn good(id: i64) -> PhotoFeat {
        PhotoFeat {
            photo_id: id,
            sharpness: Some(0.9),
            exposure: Some(0.9),
            noise: Some(0.05),
            iqa: Some(0.8),
            aesthetic: Some(0.8),
            clipped_highlights: Some(0.0),
            crushed_shadows: Some(0.0),
            mean_luminance: Some(0.45),
            scene_type: Some("landscape".into()),
            faces: vec![],
        }
    }

    #[test]
    fn star_thresholds() {
        let at = |q| base_stars(q);
        assert_eq!(
            [0.1, 0.2, 0.34, 0.35, 0.49, 0.5, 0.64, 0.65, 0.79, 0.8, 1.0].map(at),
            [0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 5.0, 5.0]
        );
    }

    #[test]
    fn group_best_bonus_and_cap() {
        assert_eq!(star_rating(0.70, false, false), 4.0);
        assert_eq!(star_rating(0.70, true, false), 4.5);
        assert_eq!(star_rating(0.90, true, false), 5.0); // never above 5
        assert_eq!(star_rating(0.90, true, true), 2.0); // hard issue caps at 2
        assert_eq!(star_rating(0.40, true, true), 2.0);
        assert_eq!(star_rating(0.10, true, true), 0.5);
    }

    #[test]
    fn clean_photo_scores_high_without_issues() {
        let s = score_photo(&good(1));
        assert!(s.q > 0.75, "{}", s.q);
        assert!(s.issues.is_empty());
        assert!(!s.hard_issue);
        // weights are renormalised over the available components (composition missing)
        let wsum: f64 = s.components.iter().map(|c| c.2).sum();
        assert!((wsum - 1.0).abs() < 1e-9);
        assert!(s
            .contributions
            .iter()
            .all(|c| c.label_key.starts_with("score.")));
    }

    #[test]
    fn closed_eyes_on_a_subject_is_a_hard_issue() {
        let mut f = good(1);
        f.scene_type = Some("portrait".into());
        f.faces = vec![face(0.9, true), face(0.2, true)];
        let s = score_photo(&f);
        assert!(s.issues.contains(&Issue::ClosedEyes));
        assert!(s.hard_issue);
        assert!(s
            .contributions
            .iter()
            .any(|c| c.key == "closed_eyes" && (c.delta + PENALTY_CLOSED_EYES).abs() < 1e-9));
        let r = s.reasons.iter().find(|r| r.key == "closed_eyes").unwrap();
        assert_eq!(r.params["count"], 1);
        // star cap
        assert_eq!(star_rating(s.q, true, s.hard_issue), 2.0);

        // closed eyes of a bystander do not count
        f.faces = vec![face(0.9, true), face(0.2, false)];
        assert!(!score_photo(&f).issues.contains(&Issue::ClosedEyes));
        // and the penalty lowers the score compared with open eyes
        f.faces = vec![face(0.9, true), face(0.9, true)];
        let open = score_photo(&f).q;
        f.faces = vec![face(0.9, true), face(0.2, true)];
        assert!(open > score_photo(&f).q + 0.2);
    }

    #[test]
    fn technical_issues_are_detected() {
        let mut f = good(1);
        f.sharpness = Some(0.1);
        f.clipped_highlights = Some(0.3);
        f.mean_luminance = Some(0.9);
        f.noise = Some(0.7);
        let s = score_photo(&f);
        for i in [Issue::Blurry, Issue::Overexposed, Issue::Noisy] {
            assert!(s.issues.contains(&i), "{i:?}");
        }
        assert!(s.hard_issue); // severe blur + severe overexposure
        assert_eq!(
            Issue::to_mask(&s.issues),
            Issue::Blurry.bit() | Issue::Overexposed.bit() | Issue::Noisy.bit()
        );
        assert_eq!(Issue::from_mask(Issue::to_mask(&s.issues)), {
            let mut v = s.issues.clone();
            v.sort();
            v
        });
        // night photos are not flagged underexposed and tolerate noise
        let mut n = good(2);
        n.scene_type = Some("night".into());
        n.mean_luminance = Some(0.05);
        n.crushed_shadows = Some(0.7);
        n.noise = Some((NOISY_AT + NOISY_AT_NIGHT) / 2.0);
        let s = score_photo(&n);
        assert!(!s.issues.contains(&Issue::Underexposed));
        assert!(!s.issues.contains(&Issue::Noisy));
        n.scene_type = Some("landscape".into());
        assert!(score_photo(&n).issues.contains(&Issue::Noisy));
    }

    #[test]
    fn bright_or_dark_frames_with_detail_are_not_exposure_errors() {
        // white backdrop portrait: clipped background, sharp subject
        let mut f = good(1);
        f.clipped_highlights = Some(0.35);
        f.mean_luminance = Some(0.75);
        f.sharpness = Some(0.6);
        assert!(!score_photo(&f).issues.contains(&Issue::Overexposed));
        // the same brightness with the detail gone is blown out
        f.sharpness = Some(0.1);
        assert!(score_photo(&f).issues.contains(&Issue::Overexposed));
        // extremely bright is overexposed however much detail is left
        f.sharpness = Some(0.9);
        f.mean_luminance = Some(0.95);
        assert!(score_photo(&f).issues.contains(&Issue::Overexposed));

        // neon street at night (no scene classifier): dark but detailed
        let mut d = good(2);
        d.scene_type = None;
        d.mean_luminance = Some(0.10);
        d.crushed_shadows = Some(0.1);
        d.sharpness = Some(0.5);
        assert!(!score_photo(&d).issues.contains(&Issue::Underexposed));
        d.sharpness = Some(0.1);
        assert!(score_photo(&d).issues.contains(&Issue::Underexposed));
        d.sharpness = Some(0.5);
        d.mean_luminance = Some(0.05);
        assert!(score_photo(&d).issues.contains(&Issue::Underexposed));
    }

    #[test]
    fn blurrier_burst_frames_are_flagged() {
        let e = [1.0f32, 0.0];
        let (a, ea) = member(1, 0.70, &e);
        let (b, eb) = member(2, 0.50, &e); // motion blur: 0.71 x the sharpest frame
        let (c, ec) = member(3, 0.62, &e); // 0.89 x: an ordinary variation
        assert!(b.issues.is_empty(), "above the absolute threshold");
        let r = rank_burst(vec![
            BurstMember {
                score: a,
                taken_at: Some(1),
                emb: Some(&ea),
            },
            BurstMember {
                score: b,
                taken_at: Some(2),
                emb: Some(&eb),
            },
            BurstMember {
                score: c,
                taken_at: Some(3),
                emb: Some(&ec),
            },
        ]);
        let issues = |id| &r.iter().find(|x| x.photo_id == id).unwrap().issues;
        assert!(issues(2).contains(&Issue::Blurry));
        assert!(!issues(1).contains(&Issue::Blurry));
        assert!(!issues(3).contains(&Issue::Blurry));
        let blurred = r.iter().find(|x| x.photo_id == 2).unwrap();
        assert!(!blurred.hard_issue, "relative blur is a tag, not a penalty");
        assert!(blurred.reasons.iter().any(|x| x.key == "blurry"));
    }

    #[test]
    fn missing_components_degrade_gracefully() {
        let f = PhotoFeat {
            photo_id: 1,
            sharpness: Some(0.8),
            exposure: Some(0.8),
            noise: Some(0.1),
            ..Default::default()
        };
        let s = score_photo(&f);
        let keys: Vec<&str> = s.components.iter().map(|c| c.0).collect();
        assert!(keys.contains(&"sharpness") && keys.contains(&"technical"));
        assert!(!keys.contains(&"aesthetic"));
        assert!(s.q > 0.7);
        let nothing = score_photo(&PhotoFeat::default());
        assert_eq!(nothing.q, 0.5);
    }

    #[test]
    fn expression_score_orders_faces() {
        let a = expression_score(&face(0.95, true)).unwrap();
        let b = expression_score(&face(0.1, true)).unwrap();
        assert!(a > b + 0.3);
        assert!(expression_score(&FaceFeat::default()).is_none());
        let sharp_only = FaceFeat {
            sharpness: Some(0.5),
            ..Default::default()
        };
        assert_eq!(expression_score(&sharp_only), Some(0.5));
    }

    #[test]
    fn subject_selection_drops_small_background_faces() {
        let mk = |x, y, w, h| {
            (
                FaceFeat {
                    bbox: [x, y, w, h],
                    sharpness: Some(0.8),
                    ..Default::default()
                },
                false,
            )
        };
        let flags = subject_flags(&[
            mk(0.4, 0.3, 0.2, 0.3),    // big, centred
            mk(0.35, 0.35, 0.15, 0.2), // similar size
            mk(0.9, 0.9, 0.04, 0.05),  // tiny, in the corner
        ]);
        assert_eq!(flags, vec![true, true, false]);
        // a lone tiny face is not a subject either
        assert_eq!(subject_flags(&[mk(0.5, 0.5, 0.02, 0.03)]), vec![false]);
    }

    fn member(id: i64, q_sharp: f64, emb: &[f32]) -> (PhotoScore, Vec<f32>) {
        let mut f = good(id);
        f.scene_type = None; // "other": sharpness counts
        f.sharpness = Some(q_sharp);
        (score_photo(&f), emb.to_vec())
    }

    #[test]
    fn burst_ranking_and_group_best_bonus() {
        let embs = [[1.0f32, 0.0], [0.5, 0.866], [0.0, 1.0]];
        let data: Vec<_> = [(1, 0.5), (2, 0.95), (3, 0.7)]
            .iter()
            .zip(embs.iter())
            .map(|((id, s), e)| member(*id, *s, e))
            .collect();
        let members: Vec<BurstMember> = data
            .iter()
            .map(|(s, e)| BurstMember {
                score: s.clone(),
                taken_at: Some(s.photo_id),
                emb: Some(e.as_slice()),
            })
            .collect();
        let r = rank_burst(members);
        assert_eq!(
            r.iter().map(|x| x.photo_id).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert_eq!(r[0].rank, 0);
        assert_eq!(r[0].burst_size, 3);
        assert!(r[0].reasons.iter().any(|x| x.key == "best_in_group"));
        assert!(r[0].reasons.iter().any(|x| x.key == "sharpest_in_group"));
        // the best photo got the bonus: half a star above its absolute stars
        assert_eq!(
            r[0].ai_rating,
            base_stars(r[0].q) + 0.5_f64.min(5.0 - base_stars(r[0].q))
        );
        assert!(r[1].ai_rating <= base_stars(r[1].q));
    }

    #[test]
    fn near_duplicates_are_lightly_penalised() {
        let e = [1.0f32, 0.0];
        let (a, ea) = member(1, 0.9, &e);
        let (b, eb) = member(2, 0.88, &e); // no distinct merit, identical embedding
        let members = vec![
            BurstMember {
                score: a.clone(),
                taken_at: Some(1),
                emb: Some(&ea),
            },
            BurstMember {
                score: b.clone(),
                taken_at: Some(2),
                emb: Some(&eb),
            },
        ];
        let r = rank_burst(members);
        assert_eq!(r[1].photo_id, 2);
        assert!((r[1].q - (b.q - PENALTY_REDUNDANT)).abs() < 1e-9);
        assert!(r[1].reasons.iter().any(|x| x.key == "redundant_in_group"));
        assert!((r[0].q - a.q).abs() < 1e-12);
    }

    #[test]
    fn single_photo_gets_no_bonus() {
        let (a, ea) = member(1, 0.9, &[1.0, 0.0]);
        let q = a.q;
        let r = rank_burst(vec![BurstMember {
            score: a,
            taken_at: None,
            emb: Some(&ea),
        }]);
        assert_eq!(r[0].ai_rating, base_stars(q));
        assert!(r[0].reasons.iter().all(|x| x.key != "best_in_group"));
    }
}
