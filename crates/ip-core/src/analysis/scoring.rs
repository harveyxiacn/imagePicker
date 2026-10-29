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
/// Horizon tilt (`ip_lite::tilt`, the worker's `steps/tilt.py`): flagged when the dominant
/// straight lines are off level by at least this many degrees with at least this confidence.
/// Larger deviations are mostly perspective (receding streets, table edges) or intentional, so
/// they are not flagged. Calibrated on rotated library photos (`bench/eval-tilt.py`): see docs/03.
/// A tag only: a tilt is fixed by straightening, so it never lowers the score.
pub const TILTED_MIN_DEG: f64 = 2.0;
pub const TILTED_MAX_DEG: f64 = 10.0;
pub const TILTED_MIN_CONFIDENCE: f64 = 0.2;
/// A burst member whose centre-region sharpness (`PhotoFeat::sharpness_center`: the subject's
/// head, hands and body, not just the inner face) is below this share of the median of the other
/// frames is flagged blurry: motion blur on the subject leaves the absolute measure in the normal
/// range, so a fixed threshold misses it. The inner-face measure is not used here: it drops with a
/// blink or a head turn although nothing is blurred. Calibrated on the library's bursts
/// (`bench/eval-library.py`, docs/03 §3.2): motion-blur variants p50 0.81 of the median, the other
/// variants p1 0.87 / min 0.83 — generated frames differ far more than a real burst does.
pub const BLURRY_IN_BURST_RATIO: f64 = 0.85;

/// `bystander` tag (docs/03 §3.3): the photo has a non-subject face that is sharp enough to be
/// recognised (face sharpness at least [`BLURRY_BELOW`]) and sizeable (at least this share of the
/// frame, about 5.5% x 5.5%), and that is not on a statue, poster or screen. Smaller or blurrier
/// strangers are background, not something to retouch. A tag only, it never lowers the score.
pub const BYSTANDER_MIN_AREA: f64 = 0.003;
pub const BYSTANDER_MIN_SHARPNESS: f64 = BLURRY_BELOW;
/// `face.artificial` (the worker's `steps/liveness.py`, its `ARTIFICIAL_AT`) at or above this
/// means the face is on a statue / poster / screen / print: never a subject or a bystander.
pub const ARTIFICIAL_AT: f64 = 0.55;
/// `bad_expression` tag (mirrors the worker's `steps/faces.py::expression_flag`): a subject whose
/// mouth is open (jawOpen at least this) without a smile is mid-word, ... MediaPipe's jawOpen
/// stays small for speech: on the library's non-smiling subjects mid-sentence p25 0.04 / p50 0.11,
/// closed mouths p90 0.02 / p95 0.06 (`bench/eval-bystander.py`, docs/03 §3.1.2).
pub const TALKING_MOUTH_OPEN: f64 = 0.06;
/// ... and one whose eyes are half shut (`eyes_open` in `[HALF_BLINK_MIN, HALF_BLINK_MAX)`;
/// below it is `closed_eyes`) without a smile was caught blinking. A smile (at or above
/// [`EXPR_SMILE_BELOW`]) excuses both: laughing eyes and open-mouthed grins are fine.
pub const EXPR_SMILE_BELOW: f64 = 0.35;
pub const HALF_BLINK_MIN: f64 = EYES_CLOSED_BELOW;
pub const HALF_BLINK_MAX: f64 = 0.65;
/// No expression verdict for a face whose crop sharpness is below this (the blurry threshold):
/// the landmarks of a blurred face are guesses (mirrors the worker's `EXPR_MIN_SHARPNESS`).
pub const EXPR_MIN_SHARPNESS: f64 = BLURRY_BELOW;
/// Weight of the composition score in the portrait / group / night / other mixes; landscapes and
/// architecture weigh it 0.20 (renormalised with the other available components).
pub const COMPOSITION_WEIGHT: f64 = 0.08;

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
    /// A sharp, sizeable non-subject face that is a living person ([`is_tag_bystander`]).
    Bystander,
    /// A subject mid-word or half-blinking ([`bad_expression`]).
    BadExpression,
}

impl Issue {
    pub const ALL: [Issue; 8] = [
        Issue::ClosedEyes,
        Issue::Blurry,
        Issue::Overexposed,
        Issue::Underexposed,
        Issue::Noisy,
        Issue::Tilted,
        Issue::Bystander,
        Issue::BadExpression,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::ClosedEyes => "closed_eyes",
            Self::Blurry => "blurry",
            Self::Overexposed => "overexposed",
            Self::Underexposed => "underexposed",
            Self::Noisy => "noisy",
            Self::Tilted => "tilted",
            Self::Bystander => "bystander",
            Self::BadExpression => "bad_expression",
        }
    }

    /// Bit in `photo.issues` (docs/05: 1 closed eyes, 2 blur, 4 over, 8 under, 16 tilt, 32 bystander;
    /// 64 = noisy and 128 = bad expression are extensions).
    pub fn bit(self) -> i64 {
        match self {
            Self::ClosedEyes => 1,
            Self::Blurry => 2,
            Self::Overexposed => 4,
            Self::Underexposed => 8,
            Self::Tilted => 16,
            Self::Noisy => 64,
            Self::Bystander => 32,
            Self::BadExpression => 128,
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
    /// 0-1 confidence that the face is on a statue / poster / screen / print (`face.artificial`).
    pub artificial: Option<f64>,
    /// jawOpen blendshape, 0 closed ... 1 wide open.
    pub mouth_open: Option<f64>,
    /// `face.nonhuman`: the face belongs to a cluster that is mostly not recognised as a human
    /// face (a pet; docs/03 §4.3).
    pub nonhuman: bool,
}

impl FaceFeat {
    pub fn likely_artificial(&self) -> bool {
        self.artificial.is_some_and(|a| a >= ARTIFICIAL_AT)
    }

    /// Not a living person in the photo: on a statue / poster / screen, or not human at all.
    /// Such a face is never a subject or a bystander and has no expression verdict.
    pub fn not_a_person(&self) -> bool {
        self.nonhuman || self.likely_artificial()
    }
}

#[derive(Debug, Clone, Default)]
pub struct PhotoFeat {
    pub photo_id: i64,
    /// Headline sharpness: the faces' (area-weighted) when there are any, otherwise the frame's.
    pub sharpness: Option<f64>,
    /// Sharpness of the central 60% x 60% of the frame (`analysis.sharpness_center`, v18), the
    /// burst-relative blurry measure; `None` before v18.
    pub sharpness_center: Option<f64>,
    pub exposure: Option<f64>,
    pub noise: Option<f64>,
    pub iqa: Option<f64>,
    pub aesthetic: Option<f64>,
    pub clipped_highlights: Option<f64>,
    pub crushed_shadows: Option<f64>,
    pub mean_luminance: Option<f64>,
    /// Horizon tilt in degrees (> 0 = clockwise) and its 0..1 confidence.
    pub tilt_deg: Option<f64>,
    pub tilt_confidence: Option<f64>,
    /// `[x, y, w, h]` of the visual subject (worker saliency), the anchor without subject faces.
    pub saliency: Option<[f64; 4]>,
    /// Composition score 0-1 (`composition::composition_score`), filled in by the caller.
    pub composition: Option<f64>,
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

/// `is_subject` per face of one photo (docs/03 §4.2), in two passes:
/// 1. prominence: weight = sqrt(area) x centredness x sharpness x (named bonus) x looking; a face
///    is a subject when its weight is at least 45% of the photo's strongest face;
/// 2. group membership: a face standing next to a subject (centres at most 4 face widths apart
///    across and 1.5 face heights up / down) that is about as large (sqrt(area) at least 0.7x that
///    subject's and 0.5x the largest subject's) and about as sharp (sharpness factor at least 0.6x)
///    is a subject too, whatever its centredness; repeated along the row / cluster. People at the
///    ends of a group photo are members, not bystanders; a small far-away face, a much blurrier one
///    or one standing apart from the group stays a bystander. Pass 2 only adds subjects.
///
/// Every subject covers at least 0.15% of the frame.
pub fn subject_flags(faces: &[(FaceFeat, bool /* person named */)]) -> Vec<bool> {
    const MIN_AREA: f64 = 0.0015;
    const SIZE_RATIO: f64 = 0.7; // sqrt(area) vs the neighbouring subject
    const SIZE_FLOOR: f64 = 0.5; // sqrt(area) vs the largest subject: no drift along a chain
    const SHARP_RATIO: f64 = 0.6; // sharpness factor vs the neighbouring subject
    const DX_FACES: f64 = 4.0; // centre distance across, in face widths
    const DY_FACES: f64 = 1.5; // centre distance up / down, in face heights
    struct Face {
        area: f64,
        size: f64,
        sharp: f64,
        weight: f64,
        centre: (f64, f64),
        dims: (f64, f64),
    }
    let fs: Vec<Face> = faces
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
            Face {
                area,
                size: area.sqrt(),
                sharp,
                weight: area.sqrt() * center * sharp * look * named,
                centre: (cx, cy),
                dims: (f.bbox[2].max(0.0), f.bbox[3].max(0.0)),
            }
        })
        .collect();
    let max_w = fs.iter().map(|f| f.weight).fold(0.0, f64::max);
    let mut subject: Vec<bool> = fs
        .iter()
        .map(|f| f.area >= MIN_AREA && f.weight >= 0.45 * max_w)
        .collect();
    let largest = fs
        .iter()
        .zip(&subject)
        .filter(|(_, s)| **s)
        .map(|(f, _)| f.size)
        .fold(0.0, f64::max);
    let member_of = |a: &Face, s: &Face| {
        a.size >= SIZE_RATIO * s.size
            && a.sharp >= SHARP_RATIO * s.sharp
            && (a.centre.0 - s.centre.0).abs() <= DX_FACES * a.dims.0.max(s.dims.0)
            && (a.centre.1 - s.centre.1).abs() <= DY_FACES * a.dims.1.max(s.dims.1)
    };
    let mut grew = true;
    while grew {
        grew = false;
        for (i, f) in fs.iter().enumerate() {
            if subject[i] || f.area < MIN_AREA || f.size < SIZE_FLOOR * largest {
                continue;
            }
            if fs
                .iter()
                .zip(&subject)
                .any(|(s, is)| *is && member_of(f, s))
            {
                subject[i] = true;
                grew = true;
            }
        }
    }
    subject
}

/// [`subject_flags`] over the living faces only: a face on a statue, poster or screen, or a
/// non-human one ([`FaceFeat::not_a_person`]) is never a subject and does not take part in the
/// competition, so a big poster or the dog in the foreground cannot demote the people.
pub fn subject_flags_live(faces: &[(FaceFeat, bool)]) -> Vec<bool> {
    let live: Vec<(FaceFeat, bool)> = faces
        .iter()
        .filter(|(f, _)| !f.not_a_person())
        .cloned()
        .collect();
    let mut flags = subject_flags(&live).into_iter();
    faces
        .iter()
        .map(|(f, _)| !f.not_a_person() && flags.next().unwrap_or(false))
        .collect()
}

/// A sharp, sizeable, living non-subject face (the `bystander` tag, see
/// [`BYSTANDER_MIN_AREA`]); the same test picks the faces "remove bystanders" suggests.
pub fn is_tag_bystander(f: &FaceFeat) -> bool {
    !f.is_subject
        && !f.not_a_person()
        && f.bbox[2].max(0.0) * f.bbox[3].max(0.0) >= BYSTANDER_MIN_AREA
        && f.sharpness.unwrap_or(0.0) >= BYSTANDER_MIN_SHARPNESS
}

pub fn bystander_count(faces: &[FaceFeat]) -> usize {
    faces.iter().filter(|f| is_tag_bystander(f)).count()
}

/// Why a subject's expression is "not great", `None` when it is fine or unknown (no landmarks).
/// `"talking"`: mouth open at least [`TALKING_MOUTH_OPEN`]; `"half_blink"`: eyes between
/// [`HALF_BLINK_MIN`] and [`HALF_BLINK_MAX`]; both only without a smile ([`EXPR_SMILE_BELOW`])
/// and on a face sharp enough to read ([`EXPR_MIN_SHARPNESS`]).
pub fn bad_expression(f: &FaceFeat) -> Option<&'static str> {
    if !f.is_subject || f.not_a_person() {
        return None;
    }
    let smile = f.smile?;
    if smile >= EXPR_SMILE_BELOW || f.sharpness.is_some_and(|s| s < EXPR_MIN_SHARPNESS) {
        return None;
    }
    if f.mouth_open.is_some_and(|m| m >= TALKING_MOUTH_OPEN) {
        return Some("talking");
    }
    if f.eyes_open
        .is_some_and(|e| (HALF_BLINK_MIN..HALF_BLINK_MAX).contains(&e))
    {
        return Some("half_blink");
    }
    None
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
    /// Centre-region sharpness, the burst comparison's measure when every frame has it.
    pub sharpness_center: Option<f64>,
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
            ("composition", COMPOSITION_WEIGHT),
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
            ("composition", COMPOSITION_WEIGHT),
        ],
        _ => &[
            ("aesthetic", 0.30),
            ("sharpness", 0.30),
            ("technical", 0.25),
            ("exposure", 0.15),
            ("composition", COMPOSITION_WEIGHT),
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
    // Without a sharpness measure, assume the detail is gone (the old behaviour).
    let detail = feat.sharpness.unwrap_or(0.0);
    let over = (feat.clipped_highlights.unwrap_or(0.0) >= OVEREXPOSED_CLIPPED
        || feat.mean_luminance.unwrap_or(0.0) > OVEREXPOSED_MEAN)
        && (detail < OVEREXPOSED_DETAIL_BELOW
            || feat.mean_luminance.unwrap_or(0.0) > OVEREXPOSED_SEVERE_MEAN);
    let under = scene != "night"
        && (feat.crushed_shadows.unwrap_or(0.0) >= UNDEREXPOSED_CRUSHED
            || feat
                .mean_luminance
                .map(|m| {
                    m < UNDEREXPOSED_SEVERE_MEAN
                        || (m < UNDEREXPOSED_MEAN && detail < UNDEREXPOSED_DETAIL_BELOW)
                })
                .unwrap_or(false));
    // A blown-out or crushed frame has no detail left to measure: its low sharpness is the
    // exposure problem, which carries its own tag (and penalty), not blur.
    if let Some(s) = feat.sharpness.filter(|_| !over && !under) {
        if s < BLURRY_BELOW {
            issues.push(Issue::Blurry);
            if s < BLURRY_SEVERE_BELOW {
                hard.push(Issue::Blurry);
            }
        }
    }
    if over {
        issues.push(Issue::Overexposed);
        if feat.clipped_highlights.unwrap_or(0.0) >= OVEREXPOSED_SEVERE_CLIPPED
            || feat.mean_luminance.unwrap_or(0.0) > OVEREXPOSED_SEVERE_MEAN
        {
            hard.push(Issue::Overexposed);
        }
    }
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
    let noisy_at = if scene == "night" {
        NOISY_AT_NIGHT
    } else {
        NOISY_AT
    };
    if feat.noise.map(|n| n >= noisy_at).unwrap_or(false) {
        issues.push(Issue::Noisy);
    }
    if tilted_by(feat).is_some() {
        issues.push(Issue::Tilted);
    }
    if bystander_count(&feat.faces) > 0 {
        issues.push(Issue::Bystander);
    }
    if feat.faces.iter().any(|f| bad_expression(f).is_some()) {
        issues.push(Issue::BadExpression);
    }
    (issues, hard)
}

/// The tilt in degrees when it is confident and in the flagged range.
pub fn tilted_by(feat: &PhotoFeat) -> Option<f64> {
    let deg = feat.tilt_deg?;
    let confident = feat.tilt_confidence.unwrap_or(0.0) >= TILTED_MIN_CONFIDENCE;
    (confident && (TILTED_MIN_DEG..=TILTED_MAX_DEG).contains(&deg.abs())).then_some(deg)
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
            "composition" => feat.composition,
            _ => None,
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
        } else if let (Issue::Tilted, Some(deg)) = (i, tilted_by(feat)) {
            json!({"deg": (deg.abs() * 10.0).round() / 10.0})
        } else if *i == Issue::Bystander {
            json!({"count": bystander_count(&feat.faces)})
        } else if *i == Issue::BadExpression {
            let kinds: Vec<&str> = feat.faces.iter().filter_map(bad_expression).collect();
            let first = kinds.first().copied().unwrap_or("talking");
            json!({"count": kinds.len(), "kind": first})
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
        sharpness_center: feat.sharpness_center,
    }
}

/// Median of the values (mean of the middle two for an even count), `None` when empty.
fn median(mut v: Vec<f64>) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    v.sort_by(f64::total_cmp);
    let k = v.len() / 2;
    Some(if v.len() % 2 == 1 {
        v[k]
    } else {
        (v[k - 1] + v[k]) / 2.0
    })
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
    // burst-relative blur measure: the centre region when every frame has it (catalogs analysed
    // before v18 do not), else the headline sharpness
    let use_center = members.iter().all(|m| m.score.sharpness_center.is_some());
    let blur_measure: Vec<Option<f64>> = members
        .iter()
        .map(|m| {
            if use_center {
                m.score.sharpness_center
            } else {
                m.score.sharpness
            }
        })
        .collect();
    let mut out = Vec::with_capacity(n);
    for (rank, &mi) in order.iter().enumerate() {
        let m = &members[mi];
        let mut q = m.score.q;
        let mut contributions = m.score.contributions.clone();
        let mut reasons = m.score.reasons.clone();
        let mut issues = m.score.issues.clone();
        let others: Vec<f64> = (0..n)
            .filter(|&j| j != mi)
            .filter_map(|j| blur_measure[j])
            .collect();
        let exposure_issue =
            issues.contains(&Issue::Overexposed) || issues.contains(&Issue::Underexposed);
        let blurrier = blur_measure[mi]
            .zip(median(others))
            .is_some_and(|(s, med)| s < BLURRY_IN_BURST_RATIO * med);
        if n >= 2 && blurrier && !exposure_issue && !issues.contains(&Issue::Blurry) {
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
            ..Default::default()
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
        f.noise = Some(0.7);
        let s = score_photo(&f);
        for i in [Issue::Blurry, Issue::Noisy] {
            assert!(s.issues.contains(&i), "{i:?}");
        }
        assert!(s.hard_issue); // severe blur
        assert_eq!(
            Issue::to_mask(&s.issues),
            Issue::Blurry.bit() | Issue::Noisy.bit()
        );
        // blown out as well: the missing detail is the exposure problem, not blur
        f.clipped_highlights = Some(0.3);
        f.mean_luminance = Some(0.9);
        let s = score_photo(&f);
        assert!(s.hard_issue); // severe overexposure
        assert_eq!(
            Issue::to_mask(&s.issues),
            Issue::Overexposed.bit() | Issue::Noisy.bit()
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
        // no centre measure (a catalog from before v18): the headline sharpness is compared
        let (b, eb) = member(2, 0.50, &e); // motion blur: 0.76 x the median of the others
        let (c, ec) = member(3, 0.62, &e); // 1.03 x: an ordinary variation
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
    fn burst_blur_compares_the_centre_region_with_the_median_frame() {
        let e = [1.0f32, 0.0];
        // (id, face sharpness, centre sharpness)
        let frames = [
            (1, 0.75, 0.60),
            (2, 0.50, 0.59), // a blink: the inner face reads softer, nothing is blurred
            (3, 0.70, 0.45), // motion blur on head and hands: 0.76 x the median centre
            (4, 0.72, 0.58),
        ];
        let scored: Vec<(PhotoScore, Vec<f32>)> = frames
            .iter()
            .map(|&(id, face, center)| {
                let mut f = good(id);
                f.scene_type = None;
                f.sharpness = Some(face);
                f.sharpness_center = Some(center);
                (score_photo(&f), e.to_vec())
            })
            .collect();
        let r = rank_burst(
            scored
                .iter()
                .map(|(s, e)| BurstMember {
                    score: s.clone(),
                    taken_at: Some(s.photo_id),
                    emb: Some(e.as_slice()),
                })
                .collect(),
        );
        let blurry = |id| {
            r.iter()
                .find(|x| x.photo_id == id)
                .unwrap()
                .issues
                .contains(&Issue::Blurry)
        };
        assert!(!blurry(1) && !blurry(2) && !blurry(4));
        assert!(blurry(3));
        // one frame without the centre measure: the whole burst falls back to the headline value
        let mut old = scored.clone();
        old[0].0.sharpness_center = None;
        let r = rank_burst(
            old.iter()
                .map(|(s, e)| BurstMember {
                    score: s.clone(),
                    taken_at: Some(s.photo_id),
                    emb: Some(e.as_slice()),
                })
                .collect(),
        );
        let flagged: Vec<i64> = r
            .iter()
            .filter(|x| x.issues.contains(&Issue::Blurry))
            .map(|x| x.photo_id)
            .collect();
        assert_eq!(
            flagged,
            vec![2],
            "0.50 < 0.85 x 0.72 on the headline measure"
        );
    }

    #[test]
    fn exposure_explains_missing_detail() {
        // a blown-out frame has no detail left: overexposed, not blurry as well
        let mut f = good(1);
        f.sharpness = Some(0.05);
        f.clipped_highlights = Some(0.4);
        f.mean_luminance = Some(0.93);
        let s = score_photo(&f);
        assert!(s.issues.contains(&Issue::Overexposed));
        assert!(!s.issues.contains(&Issue::Blurry));
        assert!(s.hard_issue);
        // same for crushed shadows
        let mut d = good(2);
        d.sharpness = Some(0.02);
        d.mean_luminance = Some(0.03);
        let s = score_photo(&d);
        assert_eq!(s.issues, vec![Issue::Underexposed]);
        // a well-exposed soft frame is still blurry
        let mut b = good(3);
        b.sharpness = Some(0.1);
        assert!(score_photo(&b).issues.contains(&Issue::Blurry));
    }

    #[test]
    fn tilt_is_a_tag_with_the_angle() {
        let mut f = good(1);
        let base = score_photo(&f);
        f.tilt_deg = Some(-3.46);
        f.tilt_confidence = Some(0.5);
        let s = score_photo(&f);
        assert_eq!(s.issues, vec![Issue::Tilted]);
        assert!(!s.hard_issue);
        assert_eq!(s.q, base.q, "a tilt does not change the score");
        let r = s.reasons.iter().find(|r| r.key == "tilted").unwrap();
        assert_eq!(r.params, json!({"deg": 3.5}));
        // not confident, too small, or too large (perspective / intentional): no tag
        for (deg, conf) in [(-3.46, 0.1), (1.2, 0.9), (13.0, 0.9)] {
            f.tilt_deg = Some(deg);
            f.tilt_confidence = Some(conf);
            assert!(score_photo(&f).issues.is_empty(), "{deg} {conf}");
        }
        f.tilt_confidence = None;
        f.tilt_deg = Some(4.0);
        assert!(score_photo(&f).issues.is_empty());
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

    fn face_at(x: f64, y: f64, w: f64, h: f64, sharpness: f64) -> (FaceFeat, bool) {
        (
            FaceFeat {
                bbox: [x, y, w, h],
                sharpness: Some(sharpness),
                gaze: Some(0.8),
                ..Default::default()
            },
            false,
        )
    }

    #[test]
    fn a_row_of_similar_faces_are_all_subjects() {
        // 5 people side by side: the ends are far from the centre (centredness ~0.3, weight about
        // a third of the middle face's) but they belong to the group
        let row: Vec<_> = [0.02, 0.22, 0.44, 0.66, 0.86]
            .iter()
            .map(|&x| face_at(x, 0.35, 0.12, 0.2, 0.7))
            .collect();
        assert_eq!(subject_flags(&row), vec![true; 5]);
        // two rows (back row higher and a little smaller) and a slightly blurrier member
        let rows = [
            face_at(0.10, 0.45, 0.12, 0.2, 0.7),
            face_at(0.30, 0.45, 0.12, 0.2, 0.7),
            face_at(0.50, 0.45, 0.12, 0.2, 0.4),
            face_at(0.70, 0.45, 0.12, 0.2, 0.7),
            face_at(0.05, 0.22, 0.10, 0.17, 0.6),
            face_at(0.82, 0.22, 0.10, 0.17, 0.6),
        ];
        assert_eq!(subject_flags(&rows), vec![true; 6]);
    }

    #[test]
    fn strangers_stay_bystanders_next_to_a_group() {
        // a small far-away stranger right next to the subject
        let flags = subject_flags(&[
            face_at(0.4, 0.3, 0.2, 0.3, 0.8),
            face_at(0.63, 0.35, 0.05, 0.07, 0.8),
        ]);
        assert_eq!(flags, vec![true, false]);
        // a large centred subject and tiny people in the background
        let flags = subject_flags(&[
            face_at(0.35, 0.2, 0.3, 0.4, 0.8),
            face_at(0.05, 0.5, 0.03, 0.045, 0.3),
            face_at(0.75, 0.45, 0.035, 0.05, 0.5),
            face_at(0.9, 0.6, 0.03, 0.04, 0.2),
        ]);
        assert_eq!(flags, vec![true, false, false, false]);
        // a face as large as the subject but standing apart at the edge of the frame
        let flags = subject_flags(&[
            face_at(0.45, 0.35, 0.1, 0.18, 0.8),
            face_at(0.9, 0.4, 0.08, 0.15, 0.8),
        ]);
        assert_eq!(flags, vec![true, false]);
        // a much blurrier face next to the subject (someone passing by, out of focus)
        let flags = subject_flags(&[
            face_at(0.4, 0.3, 0.15, 0.25, 0.9),
            face_at(0.75, 0.35, 0.14, 0.24, 0.02),
        ]);
        assert_eq!(flags, vec![true, false]);
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

    // ---------------------------------------------------------------- P1-8: tags and composition
    fn stranger(sharp: f64, w: f64, h: f64) -> FaceFeat {
        FaceFeat {
            bbox: [0.75, 0.5, w, h],
            sharpness: Some(sharp),
            is_subject: false,
            ..Default::default()
        }
    }

    fn with_faces(faces: Vec<FaceFeat>) -> PhotoFeat {
        let mut f = good(1);
        f.scene_type = Some("portrait".into());
        f.faces = faces;
        f
    }

    #[test]
    fn bystander_tag_needs_a_sharp_sizeable_living_non_subject() {
        let tags = |f: &PhotoFeat| score_photo(f).issues;
        let main = face(0.9, true);
        // sharp and big enough: tagged, no penalty, count in the reason
        let f = with_faces(vec![main.clone(), stranger(0.8, 0.07, 0.1)]);
        let s = score_photo(&f);
        assert_eq!(s.issues, vec![Issue::Bystander]);
        assert!(!s.hard_issue);
        assert_eq!(
            s.reasons
                .iter()
                .find(|r| r.key == "bystander")
                .unwrap()
                .params,
            json!({"count": 1})
        );
        // too small, too blurry: background, not a bystander
        assert!(tags(&with_faces(vec![main.clone(), stranger(0.8, 0.04, 0.06)])).is_empty());
        assert!(tags(&with_faces(vec![main.clone(), stranger(0.2, 0.07, 0.1)])).is_empty());
        // a subject is no bystander; no faces, no tag
        assert!(tags(&with_faces(vec![main.clone(), face(0.9, true)])).is_empty());
        assert!(tags(&good(1)).is_empty());
        // the tag does not move the score
        let plain = with_faces(vec![main.clone()]);
        assert_eq!(score_photo(&plain).q, s.q);
    }

    #[test]
    fn bystander_tag_skips_faces_on_statues_and_posters() {
        let mut statue = stranger(0.8, 0.07, 0.1);
        statue.artificial = Some(0.9);
        let f = with_faces(vec![face(0.9, true), statue.clone()]);
        assert!(!score_photo(&f).issues.contains(&Issue::Bystander));
        // below the threshold it is a person again
        statue.artificial = Some(ARTIFICIAL_AT - 0.01);
        let f = with_faces(vec![face(0.9, true), statue.clone()]);
        assert!(score_photo(&f).issues.contains(&Issue::Bystander));
        // one living and one artificial stranger: counted once
        let mut poster = stranger(0.8, 0.09, 0.12);
        poster.artificial = Some(1.0);
        let f = with_faces(vec![face(0.9, true), statue, poster]);
        let s = score_photo(&f);
        assert_eq!(
            s.reasons
                .iter()
                .find(|r| r.key == "bystander")
                .unwrap()
                .params["count"],
            1
        );
    }

    #[test]
    fn nonhuman_faces_are_neither_subjects_nor_bystanders() {
        // the dog walked by the subject: a little bigger and as central as she is
        let dog = FaceFeat {
            bbox: [0.42, 0.5, 0.16, 0.2],
            sharpness: Some(0.8),
            nonhuman: true,
            ..Default::default()
        };
        let her = FaceFeat {
            bbox: [0.6, 0.3, 0.14, 0.18],
            sharpness: Some(0.8),
            ..Default::default()
        };
        let flags = subject_flags_live(&[(dog.clone(), false), (her, false)]);
        assert_eq!(flags, vec![false, true]);
        assert!(dog.not_a_person() && !dog.likely_artificial());
        assert!(!is_tag_bystander(&dog));
    }

    #[test]
    fn artificial_faces_take_no_part_in_the_subject_competition() {
        let mk = |x, y, w, h, art: Option<f64>| {
            (
                FaceFeat {
                    bbox: [x, y, w, h],
                    sharpness: Some(0.8),
                    artificial: art,
                    ..Default::default()
                },
                false,
            )
        };
        // a huge centred poster face next to two smaller living people
        let faces = [
            mk(0.3, 0.1, 0.4, 0.6, Some(0.9)),
            mk(0.05, 0.4, 0.12, 0.18, None),
            mk(0.8, 0.4, 0.12, 0.18, None),
        ];
        // plain subject_flags lets the poster demote everyone else ...
        assert_eq!(subject_flags(&faces), vec![true, false, false]);
        // ... the live variant does not
        assert_eq!(subject_flags_live(&faces), vec![false, true, true]);
        // identical to subject_flags when nothing is artificial
        let live = [
            mk(0.4, 0.3, 0.2, 0.3, None),
            mk(0.9, 0.9, 0.02, 0.03, Some(0.1)),
        ];
        assert_eq!(subject_flags_live(&live), subject_flags(&live));
    }

    fn talking(smile: f64, mouth: f64, eyes: f64) -> FaceFeat {
        FaceFeat {
            smile: Some(smile),
            mouth_open: Some(mouth),
            eyes_open: Some(eyes),
            ..face(eyes, true)
        }
    }

    #[test]
    fn bad_expression_tag_mapping() {
        let kind = bad_expression;
        assert_eq!(kind(&talking(0.05, 0.4, 0.9)), Some("talking"));
        assert_eq!(
            kind(&talking(0.05, TALKING_MOUTH_OPEN, 0.9)),
            Some("talking")
        );
        assert_eq!(kind(&talking(0.05, TALKING_MOUTH_OPEN - 0.01, 0.9)), None);
        assert_eq!(kind(&talking(0.8, 0.6, 0.9)), None, "laughing is fine");
        assert_eq!(kind(&talking(EXPR_SMILE_BELOW, 0.6, 0.9)), None);
        assert_eq!(kind(&talking(0.1, 0.0, 0.55)), Some("half_blink"));
        assert_eq!(kind(&talking(0.1, 0.0, HALF_BLINK_MIN)), Some("half_blink"));
        assert_eq!(
            kind(&talking(0.1, 0.0, HALF_BLINK_MIN - 0.01)),
            None,
            "closed_eyes instead"
        );
        assert_eq!(kind(&talking(0.1, 0.0, HALF_BLINK_MAX)), None);
        assert_eq!(kind(&talking(0.8, 0.0, 0.55)), None, "smiling eyes");
        // no landmarks, not a subject, statue: no verdict
        assert_eq!(
            kind(&FaceFeat {
                is_subject: true,
                ..Default::default()
            }),
            None
        );
        let mut other = talking(0.05, 0.5, 0.9);
        other.is_subject = false;
        assert_eq!(kind(&other), None);
        let mut art = talking(0.05, 0.5, 0.9);
        art.artificial = Some(0.9);
        assert_eq!(kind(&art), None);
        // a face too blurry to read: no verdict (the landmarks are guesses)
        let mut soft = talking(0.05, 0.5, 0.9);
        soft.sharpness = Some(EXPR_MIN_SHARPNESS - 0.01);
        assert_eq!(kind(&soft), None);
        soft.eyes_open = Some(0.55);
        soft.mouth_open = Some(0.0);
        assert_eq!(kind(&soft), None);
        soft.sharpness = Some(EXPR_MIN_SHARPNESS);
        assert_eq!(kind(&soft), Some("half_blink"));
    }

    #[test]
    fn bad_expression_is_a_tag_not_a_penalty() {
        let ok = with_faces(vec![talking(0.6, 0.0, 0.9)]);
        let bad = with_faces(vec![talking(0.05, 0.5, 0.9)]);
        let (a, b) = (score_photo(&ok), score_photo(&bad));
        assert!(!a.issues.contains(&Issue::BadExpression));
        assert_eq!(b.issues, vec![Issue::BadExpression]);
        assert!(!b.hard_issue);
        let r = b
            .reasons
            .iter()
            .find(|r| r.key == "bad_expression")
            .unwrap();
        assert_eq!(r.params, json!({"count": 1, "kind": "talking"}));
        // eyes closed AND mid-talk on two subjects: both tags, closed eyes first
        let both = with_faces(vec![talking(0.05, 0.5, 0.9), talking(0.6, 0.0, 0.1)]);
        assert_eq!(
            score_photo(&both).issues,
            vec![Issue::ClosedEyes, Issue::BadExpression]
        );
    }

    #[test]
    fn issue_bits_and_keys_round_trip() {
        assert_eq!(Issue::Bystander.bit(), 32);
        assert_eq!(Issue::BadExpression.bit(), 128);
        let all = Issue::ALL.to_vec();
        assert_eq!(Issue::from_mask(Issue::to_mask(&all)), all);
        let bits: std::collections::HashSet<i64> = all.iter().map(|i| i.bit()).collect();
        assert_eq!(bits.len(), all.len(), "bits are distinct");
        for i in all {
            assert_eq!(Issue::parse(i.key()), Some(i));
        }
    }

    #[test]
    fn composition_enters_the_score_with_a_modest_weight() {
        // without a composition value nothing changes (calibration of existing photos)
        let mut f = good(1);
        f.scene_type = Some("other".into());
        let base = score_photo(&f);
        assert!(base.components.iter().all(|c| c.0 != "composition"));
        // with one, its normalised weight is COMPOSITION_WEIGHT / (1.0 + COMPOSITION_WEIGHT)
        f.composition = Some(0.9);
        let s = score_photo(&f);
        let c = s.components.iter().find(|c| c.0 == "composition").unwrap();
        assert!((c.2 - COMPOSITION_WEIGHT / (1.0 + COMPOSITION_WEIGHT)).abs() < 1e-9);
        assert!(c.2 < 0.10);
        assert!(s.contributions.iter().any(|c| c.key == "composition"));
        // the swing between a bad and a good composition stays small
        let (mut lo, mut hi) = (f.clone(), f.clone());
        lo.composition = Some(0.1);
        hi.composition = Some(0.9);
        let swing = score_photo(&hi).q - score_photo(&lo).q;
        assert!(swing > 0.03 && swing < 0.07, "{swing}");
    }

    #[test]
    fn composition_changes_the_ranking_only_where_it_differs() {
        let mut a = good(1);
        let mut b = good(2);
        a.scene_type = Some("other".into());
        b.scene_type = Some("other".into());
        // b is a hair better technically, so it wins when the compositions are equal ...
        b.sharpness = Some(0.91);
        a.sharpness = Some(0.90);
        let order = |a: &PhotoFeat, b: &PhotoFeat| {
            let emb = |v: f32| vec![v, 1.0 - v];
            let (ea, eb) = (emb(1.0), emb(0.0));
            rank_burst(vec![
                BurstMember {
                    score: score_photo(a),
                    taken_at: Some(1),
                    emb: Some(&ea),
                },
                BurstMember {
                    score: score_photo(b),
                    taken_at: Some(2),
                    emb: Some(&eb),
                },
            ])
            .iter()
            .map(|r| r.photo_id)
            .collect::<Vec<_>>()
        };
        assert_eq!(order(&a, &b), vec![2, 1]);
        a.composition = Some(0.5);
        b.composition = Some(0.5);
        assert_eq!(
            order(&a, &b),
            vec![2, 1],
            "equal compositions keep the order"
        );
        // ... and loses when a is clearly better framed
        a.composition = Some(0.95);
        b.composition = Some(0.15);
        assert_eq!(order(&a, &b), vec![1, 2]);
        // a composition on only one photo is renormalised, not punished as a zero
        a.composition = None;
        b.composition = Some(0.9);
        let (sa, sb) = (score_photo(&a), score_photo(&b));
        assert!(sb.q > sa.q - 0.01);
    }
}
