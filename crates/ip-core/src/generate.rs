//! Generative editing (docs/api-contract-m5.md sections B, C, E): best take (face swap from a
//! burst mate), inpainting (bystander / stroke removal) and enhancement (denoise, face
//! restoration).
//!
//! Every generative task asks the AI worker for an RGBA patch, stores it as a photo-specific
//! asset (`edit::patch`) and records it as a `patch` op in the photo's edit stack, then sends
//! `edits.updated` followed by the matching `*.done` event. Patches are never copied to other
//! photos by sync / presets.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ip_worker_client::{
    BestTakeComposeRequest, EnhanceRequest, InpaintRequest, MaskPhoto, MaskRequest,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use crate::analysis::map_worker_err;
use crate::analysis::scoring::Issue;
use crate::analysis::store as astore;
use crate::analysis::types::{BurstFacesOut, Face};
use crate::catalog::{self, PhotoRef};
use crate::edit::sync::rank;
use crate::edit::{ops_of, store};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

/// A candidate face is not composable when its head pose differs from the base face's by more
/// than this many degrees (yaw or pitch).
pub const POSE_LIMIT_DEG: f64 = 25.0;
/// Most choices one best-take request may carry.
pub const MAX_CHOICES: usize = 32;
/// Long edge of the removal mask handed to `inpaint.run` (analysis resolution).
pub const INPAINT_MASK_EDGE: u32 = 1024;
/// Edge feather stored on generated patches (fraction of the rect's short side).
const PATCH_FEATHER: f32 = 0.08;
/// A person gets a new face only when a composable face of theirs beats the base face by more
/// than this (expression score 0-1): smaller gains are not worth the compositing risk. The base
/// choice uses the same margin ("good enough" = within this of the person's best).
pub const MIN_GAIN: f64 = 0.04;
/// Base choice: cost per unit of expression a person stays below their best (beyond
/// [`MIN_GAIN`]) in the composite; closed eyes that cannot be replaced cost about two face
/// replacements.
pub const BELOW_BEST_COST: f64 = 6.0;
/// Base choice: cost of a person of the burst who is missing from the frame (nobody can be
/// pasted in).
pub const MISSING_COST: f64 = 3.0;
/// Base choice: the best-ranked frame whose cost is within this of the cheapest frame becomes
/// the base, i.e. leaving a better-ranked frame must save more than a quarter of a full-size
/// face replacement.
pub const BASE_SWITCH_MARGIN: f64 = 0.25;
/// Photo issues compositing cannot repair: frames with one are bases of last resort.
const BASE_BLOCKING_ISSUES: [Issue; 3] = [Issue::Blurry, Issue::Overexposed, Issue::Underexposed];

// ------------------------------------------------------------------ wire types

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BestTakeCandidate {
    pub photo_id: i64,
    pub face_id: i64,
    pub expression_score: Option<f64>,
    pub composable: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BestTakePerson {
    pub track_id: i64,
    pub person_id: Option<i64>,
    pub person_name: Option<String>,
    pub base_face_id: i64,
    pub candidates: Vec<BestTakeCandidate>,
    /// Photo with this person's best expression among the base and its composable candidates
    /// (the base itself when nothing beats it by more than [`MIN_GAIN`]).
    pub best_photo_id: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BestTakePlan {
    pub base_photo_id: i64,
    pub people: Vec<BestTakePerson>,
    /// Why this base (additive to the M5 contract); `None` for a burst without photos.
    pub base_choice: Option<BaseChoice>,
}

/// How a frame would do as the base: the work "best for everyone" leaves on it.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BaseFrame {
    pub photo_id: i64,
    /// Faces "best for everyone" would paste into this frame.
    pub replacements: usize,
    /// People whose expression would stay more than [`MIN_GAIN`] below their best (no good
    /// enough composable face).
    pub below_best: usize,
    /// People of the burst (subjects seen in at least half of the frames) missing here.
    pub missing: usize,
    /// Issues compositing cannot repair (`blurry`, `overexposed`, `underexposed`).
    pub issues: Vec<String>,
    /// Replacements, leftover flaws and missing people weighted by face size (lower is better).
    pub cost: f64,
}

/// Why the plan's base was chosen.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BaseChoice {
    /// `auto` (the rule below) or `manual` (the caller's `base_photo_id`).
    pub mode: String,
    /// `group_best` (the burst's best photo is also the best base), `group_best_issue`,
    /// `fewer_missing`, `fewer_replacements`, `fewer_below_best`, `less_work` (smaller faces to
    /// replace), or `manual`.
    pub reason: String,
    /// The burst's best-ranked photo.
    pub group_best_photo_id: i64,
    /// The frame the rule picks (the base unless `manual`).
    pub auto_photo_id: i64,
    /// Every frame of the burst, best-ranked first.
    pub frames: Vec<BaseFrame>,
}

/// Body of `POST /api/bursts/{id}/besttake/auto` (optional).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BestTakeAutoBody {
    /// Compose onto this frame instead of the automatically chosen base.
    #[serde(default)]
    pub base_photo_id: Option<i64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct BestTakeChoice {
    pub base_face_id: i64,
    pub source_photo_id: i64,
    pub source_face_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BestTakeRequest {
    pub base_photo_id: i64,
    pub choices: Vec<BestTakeChoice>,
}

/// One entry of `besttake.done`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BestTakeResult {
    pub base_face_id: i64,
    pub ok: bool,
    pub warnings: Vec<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Bystander {
    pub face_id: i64,
    pub bbox: [f64; 4],
}

/// A removal brush stroke: a polyline with round caps in normalised upright coordinates;
/// `radius` is a fraction of the image width.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
    pub radius: f32,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct InpaintBody {
    #[serde(default)]
    pub bystanders: bool,
    #[serde(default)]
    pub face_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub strokes: Option<Vec<Stroke>>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnhanceBody {
    pub op: String,
    #[serde(default)]
    pub strength: Option<f64>,
}

// ------------------------------------------------------------------ plan (pure)

fn score_key(s: Option<f64>) -> f64 {
    s.unwrap_or(-1.0)
}

/// Largest yaw/pitch difference between two faces (degrees); unknown angles count as 0.
pub fn pose_delta(a: &Face, b: &Face) -> f64 {
    let d = |x: Option<f64>, y: Option<f64>| match (x, y) {
        (Some(x), Some(y)) => (x - y).abs(),
        _ => 0.0,
    };
    d(a.yaw, b.yaw).max(d(a.pitch, b.pitch))
}

/// Expression score for replacements and the base choice (unknown = 0).
fn expr(f: &Face) -> f64 {
    f.expression_score.unwrap_or(0.0).clamp(0.0, 1.0)
}

/// One face track of the burst, frames in rank order.
struct TrackFaces<'a> {
    faces: Vec<Option<&'a Face>>,
    /// Face size (sqrt of the mean area) relative to the largest track's.
    weight: f64,
    /// Best expression anywhere in the burst.
    best: f64,
    /// A subject seen in at least two frames and at least half of them: missing it counts.
    expected: bool,
}

fn track_faces(out: &BurstFacesOut) -> Vec<TrackFaces<'_>> {
    let n = out.photo_ids.len();
    let mut tracks: Vec<TrackFaces<'_>> = out
        .tracks
        .iter()
        .map(|t| {
            let faces: Vec<Option<&Face>> = out
                .photo_ids
                .iter()
                .map(|p| t.cells.get(&p.to_string()).and_then(Option::as_ref))
                .collect();
            let present: Vec<&Face> = faces.iter().flatten().copied().collect();
            let area = present
                .iter()
                .map(|f| (f.bbox[2] * f.bbox[3]).max(0.0))
                .sum::<f64>()
                / present.len().max(1) as f64;
            TrackFaces {
                weight: area.sqrt(),
                best: present.iter().map(|f| expr(f)).fold(0.0, f64::max),
                expected: present.iter().any(|f| f.is_subject)
                    && present.len() >= 2
                    && 2 * present.len() >= n,
                faces,
            }
        })
        .collect();
    let largest = tracks.iter().map(|t| t.weight).fold(0.0, f64::max);
    for t in &mut tracks {
        t.weight = if largest > 0.0 {
            t.weight / largest
        } else {
            1.0
        };
    }
    tracks
}

/// The face "best for everyone" pastes over the track's face in frame `base`: the best
/// composable face of another frame (ties: better rank) when it beats the base face by more
/// than [`MIN_GAIN`]. `(frame index, expression)`.
fn replacement(faces: &[Option<&Face>], base: usize) -> Option<(usize, f64)> {
    let b = faces[base]?;
    let mut best: Option<(usize, f64)> = None;
    for (i, f) in faces.iter().enumerate() {
        let Some(f) = f else {
            continue;
        };
        if i == base || pose_delta(b, f) > POSE_LIMIT_DEG {
            continue;
        }
        let better = match best {
            Some((_, s)) => expr(f) > s,
            None => true,
        };
        if better {
            best = Some((i, expr(f)));
        }
    }
    best.filter(|(_, s)| *s > expr(b) + MIN_GAIN)
}

/// Every frame as a candidate base (rank order) with its unrounded cost.
fn base_frames(
    out: &BurstFacesOut,
    tracks: &[TrackFaces<'_>],
    issues: &HashMap<i64, i64>,
) -> Vec<(BaseFrame, f64)> {
    out.photo_ids
        .iter()
        .enumerate()
        .map(|(i, &photo_id)| {
            let (mut replacements, mut below_best, mut missing, mut cost) = (0, 0, 0, 0.0);
            for t in tracks {
                let Some(face) = t.faces[i] else {
                    if t.expected {
                        missing += 1;
                        cost += t.weight * MISSING_COST;
                    }
                    continue;
                };
                let mut fin = expr(face);
                if let Some((_, s)) = replacement(&t.faces, i) {
                    replacements += 1;
                    cost += t.weight;
                    fin = s;
                }
                let gap = t.best - fin - MIN_GAIN;
                if gap > 1e-9 {
                    below_best += 1;
                    cost += t.weight * BELOW_BEST_COST * gap;
                }
            }
            let mask = issues.get(&photo_id).copied().unwrap_or(0);
            let frame = BaseFrame {
                photo_id,
                replacements,
                below_best,
                missing,
                issues: BASE_BLOCKING_ISSUES
                    .iter()
                    .filter(|x| mask & x.bit() != 0)
                    .map(|x| x.key().to_string())
                    .collect(),
                cost: (cost * 10_000.0).round() / 10_000.0,
            };
            (frame, cost)
        })
        .collect()
}

/// Index of the automatic base: among the frames without a blocking issue (all frames when
/// every one has one), the best-ranked whose cost is within [`BASE_SWITCH_MARGIN`] of the
/// cheapest.
fn pick_base(frames: &[(BaseFrame, f64)]) -> usize {
    let clean: Vec<usize> = (0..frames.len())
        .filter(|&i| frames[i].0.issues.is_empty())
        .collect();
    let pool = if clean.is_empty() {
        (0..frames.len()).collect()
    } else {
        clean
    };
    let cheapest = pool
        .iter()
        .map(|&i| frames[i].1)
        .fold(f64::INFINITY, f64::min);
    pool.into_iter()
        .find(|&i| frames[i].1 <= cheapest + BASE_SWITCH_MARGIN)
        .unwrap_or(0)
}

/// The main reason `chosen` beat the group's best frame.
fn base_reason(chosen: &BaseFrame, group_best: &BaseFrame) -> &'static str {
    if chosen.photo_id == group_best.photo_id {
        "group_best"
    } else if !group_best.issues.is_empty() && chosen.issues.is_empty() {
        "group_best_issue"
    } else if chosen.missing < group_best.missing {
        "fewer_missing"
    } else if chosen.replacements < group_best.replacements {
        "fewer_replacements"
    } else if chosen.below_best < group_best.below_best {
        "fewer_below_best"
    } else {
        "less_work"
    }
}

/// Builds the best-take plan of a burst from its face tracks (`burst_faces`, frames in rank
/// order) and the photos' `issues` masks. The base is `manual` when given, else the frame that
/// leaves the least compositing work and risk (docs/03 section 5): frames with a blocking issue
/// (blur, over/underexposure) only when all have one; cost = per face to replace its size
/// relative to the largest face, plus [`BELOW_BEST_COST`] per unit of expression someone stays
/// below their best, plus [`MISSING_COST`] per missing person; the best-ranked frame within
/// [`BASE_SWITCH_MARGIN`] of the cheapest wins, so the group's best stays the base unless
/// another frame clearly saves work. Every track with a face in the base becomes a person entry
/// whose candidates are the faces of the same track in the other photos, best expression first.
/// `None` when the burst has no photos or `manual` is not one of them.
pub fn plan_from_tracks(
    out: &BurstFacesOut,
    issues: &HashMap<i64, i64>,
    manual: Option<i64>,
) -> Option<BestTakePlan> {
    let group_best = *out.photo_ids.first()?;
    let tracks = track_faces(out);
    let frames = base_frames(out, &tracks, issues);
    let auto = pick_base(&frames);
    let at = match manual {
        Some(id) => out.photo_ids.iter().position(|p| *p == id)?,
        None => auto,
    };
    let base = out.photo_ids[at];
    let mut people = Vec::new();
    for (t, tf) in out.tracks.iter().zip(&tracks) {
        let Some(base_face) = tf.faces[at] else {
            continue;
        };
        let mut candidates: Vec<BestTakeCandidate> = Vec::new();
        for (i, f) in tf.faces.iter().enumerate() {
            let Some(f) = f else {
                continue;
            };
            if i == at {
                continue;
            }
            let composable = pose_delta(base_face, f) <= POSE_LIMIT_DEG;
            candidates.push(BestTakeCandidate {
                photo_id: out.photo_ids[i],
                face_id: f.id,
                expression_score: f.expression_score,
                composable,
                reason: (!composable).then(|| "large_pose_change".to_string()),
            });
        }
        // stable: ties keep the burst's rank order
        candidates.sort_by(|a, b| {
            score_key(b.expression_score)
                .partial_cmp(&score_key(a.expression_score))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        people.push(BestTakePerson {
            track_id: t.track_id,
            person_id: t.person_id,
            person_name: t.person_name.clone(),
            base_face_id: base_face.id,
            candidates,
            best_photo_id: replacement(&tf.faces, at)
                .map(|(i, _)| out.photo_ids[i])
                .unwrap_or(base),
        });
    }
    let (mode, reason) = match manual {
        Some(_) => ("manual", "manual"),
        None => ("auto", base_reason(&frames[auto].0, &frames[0].0)),
    };
    Some(BestTakePlan {
        base_photo_id: base,
        people,
        base_choice: Some(BaseChoice {
            mode: mode.to_string(),
            reason: reason.to_string(),
            group_best_photo_id: group_best,
            auto_photo_id: out.photo_ids[auto],
            frames: frames.into_iter().map(|(f, _)| f).collect(),
        }),
    })
}

/// The choices `besttake/auto` makes: per person whose best take is another photo, that photo's
/// face.
pub fn auto_choices(plan: &BestTakePlan) -> Vec<BestTakeChoice> {
    plan.people
        .iter()
        .filter(|p| p.best_photo_id != plan.base_photo_id)
        .filter_map(|p| {
            let c = p
                .candidates
                .iter()
                .find(|c| c.photo_id == p.best_photo_id && c.composable)?;
            Some(BestTakeChoice {
                base_face_id: p.base_face_id,
                source_photo_id: c.photo_id,
                source_face_id: c.face_id,
            })
        })
        .collect()
}

// ------------------------------------------------------------------ mask building (pure)

/// An 8-bit removal mask (255 = remove).
#[derive(Debug, Clone)]
pub struct MaskCanvas {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl MaskCanvas {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width * height) as usize],
        }
    }

    /// Canvas for an upright image of `w` x `h` whose long edge is `edge`.
    pub fn for_image(w: u32, h: u32, edge: u32) -> Self {
        let (w, h) = (w.max(1) as f64, h.max(1) as f64);
        let s = edge as f64 / w.max(h);
        Self::new(
            ((w * s).round() as u32).max(1),
            ((h * s).round() as u32).max(1),
        )
    }

    pub fn count(&self) -> usize {
        self.data.iter().filter(|v| **v > 127).count()
    }

    /// Bounding box of the set pixels, normalised `[x, y, w, h]`.
    pub fn bounds(&self) -> Option<[f32; 4]> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..self.height {
            for x in 0..self.width {
                if self.data[(y * self.width + x) as usize] > 127 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0 != u32::MAX).then(|| {
            let (w, h) = (self.width as f32, self.height as f32);
            [
                x0 as f32 / w,
                y0 as f32 / h,
                (x1 + 1 - x0) as f32 / w,
                (y1 + 1 - y0) as f32 / h,
            ]
        })
    }

    /// Draws round-capped polylines (`radius` is a fraction of the width).
    pub fn draw_strokes(&mut self, strokes: &[Stroke]) {
        let long = self.width as f32;
        let (wf, hf) = (self.width as f32, self.height as f32);
        for s in strokes {
            let r = (s.radius * long).max(1.0);
            let pts: Vec<[f32; 2]> = s.points.iter().map(|p| [p[0] * wf, p[1] * hf]).collect();
            if pts.len() == 1 {
                self.capsule(pts[0], pts[0], r);
            }
            for w in pts.windows(2) {
                self.capsule(w[0], w[1], r);
            }
        }
    }

    /// Fills every pixel whose centre is within `r` of the segment `a`-`b` (a capsule).
    fn capsule(&mut self, a: [f32; 2], b: [f32; 2], r: f32) {
        let x0 = ((a[0].min(b[0]) - r).floor().max(0.0)) as u32;
        let y0 = ((a[1].min(b[1]) - r).floor().max(0.0)) as u32;
        let x1 = ((a[0].max(b[0]) + r).ceil().min(self.width as f32 - 1.0)).max(0.0) as u32;
        let y1 = ((a[1].max(b[1]) + r).ceil().min(self.height as f32 - 1.0)).max(0.0) as u32;
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let len2 = dx * dx + dy * dy;
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let t = if len2 > 0.0 {
                    (((px - a[0]) * dx + (py - a[1]) * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (cx, cy) = (a[0] + t * dx, a[1] + t * dy);
                if (px - cx).powi(2) + (py - cy).powi(2) <= r * r {
                    self.data[(y * self.width + x) as usize] = 255;
                }
            }
        }
    }

    /// ORs `other` (any resolution, nearest-neighbour resampled) into the canvas.
    pub fn union(&mut self, other: &image::GrayImage) {
        let (ow, oh) = (other.width().max(1), other.height().max(1));
        for y in 0..self.height {
            let sy = ((y as u64 * oh as u64) / self.height as u64).min(oh as u64 - 1) as u32;
            for x in 0..self.width {
                let sx = ((x as u64 * ow as u64) / self.width as u64).min(ow as u64 - 1) as u32;
                if other.get_pixel(sx, sy).0[0] > 127 {
                    self.data[(y * self.width + x) as usize] = 255;
                }
            }
        }
    }

    /// ORs a normalised rectangle `[x, y, w, h]` into the canvas.
    pub fn fill_rect(&mut self, r: [f32; 4]) {
        let (wf, hf) = (self.width as f32, self.height as f32);
        let x0 = (r[0].clamp(0.0, 1.0) * wf).floor() as u32;
        let y0 = (r[1].clamp(0.0, 1.0) * hf).floor() as u32;
        let x1 = (((r[0] + r[2]).clamp(0.0, 1.0)) * wf).ceil() as u32;
        let y1 = (((r[1] + r[3]).clamp(0.0, 1.0)) * hf).ceil() as u32;
        for y in y0..y1.min(self.height) {
            for x in x0..x1.min(self.width) {
                self.data[(y * self.width + x) as usize] = 255;
            }
        }
    }

    /// Grows the mask by `r` pixels (square structuring element, separable).
    pub fn dilate(&mut self, r: u32) {
        if r == 0 {
            return;
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let r = r as usize;
        let mut tmp = vec![0u8; w * h];
        for y in 0..h {
            // sliding "last set pixel" scan, left to right then right to left
            let row = &self.data[y * w..(y + 1) * w];
            let out = &mut tmp[y * w..(y + 1) * w];
            let mut last: Option<usize> = None;
            for x in 0..w {
                if row[x] > 127 {
                    last = Some(x);
                }
                if last.is_some_and(|l| x - l <= r) {
                    out[x] = 255;
                }
            }
            let mut last: Option<usize> = None;
            for x in (0..w).rev() {
                if row[x] > 127 {
                    last = Some(x);
                }
                if last.is_some_and(|l| l - x <= r) {
                    out[x] = 255;
                }
            }
        }
        let mut res = vec![0u8; w * h];
        for x in 0..w {
            let mut last: Option<usize> = None;
            for y in 0..h {
                if tmp[y * w + x] > 127 {
                    last = Some(y);
                }
                if last.is_some_and(|l| y - l <= r) {
                    res[y * w + x] = 255;
                }
            }
            let mut last: Option<usize> = None;
            for y in (0..h).rev() {
                if tmp[y * w + x] > 127 {
                    last = Some(y);
                }
                if last.is_some_and(|l| l - y <= r) {
                    res[y * w + x] = 255;
                }
            }
        }
        self.data = res;
    }

    pub fn save_png(&self, path: &Path) -> Result<()> {
        let img = image::GrayImage::from_raw(self.width, self.height, self.data.clone())
            .ok_or_else(|| CoreError::Internal(anyhow::anyhow!("bad mask buffer")))?;
        img.save_with_format(path, image::ImageFormat::Png)
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("write mask: {e}")))
    }
}

/// Dilation of bystander masks, as a fraction of the mask's long edge (about 0.6 %).
pub const BYSTANDER_DILATE: f32 = 0.006;

/// Box that stands in for a person whose mask could not be generated: a body-sized region under
/// and around the face.
pub fn body_box(face: [f64; 4]) -> [f32; 4] {
    let [x, y, w, h] = face.map(|v| v as f32);
    let x0 = (x - 0.5 * w).max(0.0);
    let y0 = (y - 0.2 * h).max(0.0);
    [
        x0,
        y0,
        (x + 1.5 * w).min(1.0) - x0,
        (y + 4.0 * h).min(1.0) - y0,
    ]
}

// ------------------------------------------------------------------ patch ops

fn mkphoto(r: &PhotoRef) -> MaskPhoto {
    MaskPhoto {
        photo_id: r.id,
        path: r.path.to_string_lossy().into_owned(),
        orientation: r.orientation,
    }
}

/// Clamps a worker-reported rect into the valid patch range.
fn clamp_rect(r: [f32; 4]) -> [f32; 4] {
    let x = r[0].clamp(0.0, 1.0);
    let y = r[1].clamp(0.0, 1.0);
    let w = r[2].clamp(0.0001, 1.0).min(1.0 - x).max(0.0001);
    let h = r[3].clamp(0.0001, 1.0).min(1.0 - y).max(0.0001);
    [x, y, w, h]
}

fn patch_op(
    kind: &str,
    asset: &str,
    rect: [f32; 4],
    feather: f32,
    amount: f32,
) -> Map<String, Value> {
    let rect = clamp_rect(rect);
    let mut m = Map::new();
    m.insert("type".into(), json!("patch"));
    m.insert("kind".into(), json!(kind));
    m.insert("asset".into(), json!(asset));
    m.insert("rect".into(), json!(rect));
    m.insert("feather".into(), json!(feather));
    m.insert("amount".into(), json!(amount));
    m.insert("enabled".into(), json!(true));
    m
}

fn is_patch(op: &Value, kind: &str) -> bool {
    op.get("type").and_then(Value::as_str) == Some("patch")
        && op.get("kind").and_then(Value::as_str) == Some(kind)
}

/// Whether `old` is the slot `new` takes over. A best take is tracked by person (or by the base
/// face when the face has no person); denoise replaces denoise; face restoration replaces all
/// face restoration; inpainting always appends.
fn same_slot(old: &Value, new: &Value) -> bool {
    let kind = new.get("kind").and_then(Value::as_str).unwrap_or("");
    if !is_patch(old, kind) {
        return false;
    }
    match kind {
        "best_take" => match new.get("person_id").and_then(Value::as_i64) {
            Some(p) => old.get("person_id").and_then(Value::as_i64) == Some(p),
            None => {
                old.get("person_id").and_then(Value::as_i64).is_none()
                    && old.get("base_face_id").and_then(Value::as_i64)
                        == new.get("base_face_id").and_then(Value::as_i64)
            }
        },
        "denoise" | "face_restore" => true,
        _ => false,
    }
}

/// `old` without the slots `new` takes over, then `new`, in canonical pipeline order.
pub fn replace_patches(old: Vec<Value>, new: Vec<Value>) -> Vec<Value> {
    let mut out: Vec<Value> = old
        .into_iter()
        .filter(|o| !new.iter().any(|n| same_slot(o, n)))
        .collect();
    out.extend(new);
    out.sort_by_key(rank);
    out
}

// ------------------------------------------------------------------ Core API

fn reason_of(e: &CoreError) -> String {
    e.to_string()
}

/// Worker failures after which trying the next item of the same task is pointless.
fn is_fatal(e: &CoreError) -> bool {
    matches!(
        e,
        CoreError::ModelsMissing(_) | CoreError::WorkerUnavailable(_) | CoreError::WorkerTimeout(_)
    )
}

impl Core {
    /// 409 when `models.list` reports a model of `feature` (`besttake`, `inpaint`, `enhance`)
    /// as not installed, 503/504 when the worker cannot be reached.
    /// 409 `models_missing` up front when the models `feature` (narrowed by `path`, e.g.
    /// `("inpaint", &["lama"])`) needs are not installed. Prefers the worker's exact
    /// `<feature>_models` list; falls back to `required_for` tags, ignoring optional models
    /// (e.g. the SDXL "pro" pack must not block LaMa inpainting).
    pub(crate) async fn preflight_models(&self, feature: &str, path: &[&str]) -> Result<()> {
        let listing = self.worker.models_list().await.map_err(map_worker_err)?;
        let named = listing.named_models(feature, path);
        let missing: Vec<String> = listing
            .models
            .iter()
            .filter(|m| {
                !m.installed
                    && match &named {
                        Some(ids) => ids.contains(&m.id),
                        None => !m.optional && m.required_for.iter().any(|s| s == feature),
                    }
            })
            .map(|m| m.id.clone())
            .collect();
        if missing.is_empty() {
            Ok(())
        } else {
            Err(CoreError::ModelsMissing(missing))
        }
    }

    fn gen_dir(&self, task_id: &str) -> PathBuf {
        self.dirs.gen.join(task_id)
    }

    fn task_progress(
        &self,
        task_id: &str,
        kind: &str,
        done: i64,
        total: i64,
        state: &str,
        error: Option<String>,
    ) {
        self.events.emit(Event::TaskProgress {
            task_id: task_id.to_string(),
            kind: kind.to_string(),
            done,
            total,
            state: state.to_string(),
            error,
        });
    }

    /// Current edit ops of a photo (empty when unedited).
    async fn current_ops(&self, photo_id: i64) -> Result<Vec<Value>> {
        let cur = self
            .db
            .call(move |c| {
                catalog::photo_ref(c, photo_id)?;
                store::current(c, photo_id)
            })
            .await?;
        Ok(cur.map(|c| ops_of(&c.stack)).unwrap_or_default())
    }

    /// Saves `new` patch ops into the photo's stack (replacing the slots they take over) and
    /// sends `edits.updated`.
    async fn apply_patches(self: &Arc<Self>, photo_id: i64, new: Vec<Value>) -> Result<()> {
        let r = self
            .db
            .call(move |c| catalog::photo_ref(c, photo_id))
            .await?;
        let old = self.current_ops(photo_id).await?;
        let ops = replace_patches(old, new);
        let changed = self.persist_stacks_quiet(vec![(r, ops)]).await?;
        self.refresh_edits(changed).await;
        Ok(())
    }

    // -------------------------------------------------------------- best take

    /// `GET /api/bursts/{id}/besttake[?base_photo_id=]`: the plan on the automatically chosen
    /// base, or on `base` (422 when it is not a photo of the burst).
    pub async fn besttake_plan(&self, burst_id: i64, base: Option<i64>) -> Result<BestTakePlan> {
        let (out, issues) = self
            .db
            .call(move |c| {
                let out = astore::burst_faces(c, burst_id)?;
                let mut st =
                    c.prepare("SELECT id, COALESCE(issues, 0) FROM photo WHERE burst_id=?1")?;
                let issues = st
                    .query_map([burst_id], |r| {
                        Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
                    })?
                    .collect::<rusqlite::Result<HashMap<i64, i64>>>()?;
                Ok((out, issues))
            })
            .await?;
        if let Some(b) = base.filter(|b| !out.photo_ids.contains(b)) {
            return Err(CoreError::Unprocessable(format!(
                "photo {b} is not in burst {burst_id}"
            )));
        }
        Ok(
            plan_from_tracks(&out, &issues, base).unwrap_or(BestTakePlan {
                base_photo_id: 0,
                people: Vec::new(),
                base_choice: None,
            }),
        )
    }

    /// `POST /api/besttake`: validates the choices and starts the task.
    pub async fn besttake_start(self: &Arc<Self>, req: BestTakeRequest) -> Result<String> {
        if req.choices.is_empty() {
            return Err(CoreError::bad_request("choices must not be empty"));
        }
        if req.choices.len() > MAX_CHOICES {
            return Err(CoreError::bad_request(format!(
                "at most {MAX_CHOICES} choices per request"
            )));
        }
        let mut seen = HashSet::new();
        for c in &req.choices {
            if !seen.insert(c.base_face_id) {
                return Err(CoreError::Unprocessable(format!(
                    "base face {} appears twice in choices",
                    c.base_face_id
                )));
            }
        }
        let base_id = req.base_photo_id;
        let choices = req.choices.clone();
        let (base, faces) = self
            .db
            .call(move |c| {
                let base = catalog::photo_ref(c, base_id)?;
                let mut faces = Vec::new();
                for ch in &choices {
                    let bf = astore::get_face(c, ch.base_face_id)?;
                    let sf = astore::get_face(c, ch.source_face_id)?;
                    let src = catalog::photo_ref(c, ch.source_photo_id)?;
                    if bf.photo_id != base_id {
                        return Err(CoreError::Unprocessable(format!(
                            "face {} is not in photo {base_id}",
                            ch.base_face_id
                        )));
                    }
                    if sf.photo_id != ch.source_photo_id {
                        return Err(CoreError::Unprocessable(format!(
                            "face {} is not in photo {}",
                            ch.source_face_id, ch.source_photo_id
                        )));
                    }
                    if ch.source_photo_id == base_id {
                        return Err(CoreError::Unprocessable(
                            "the source photo must differ from the base photo".into(),
                        ));
                    }
                    faces.push((bf, sf, src));
                }
                Ok((base, faces))
            })
            .await?;
        self.preflight_models("besttake", &[]).await?;
        let task_id = format!("besttake-{}", self.next_task_seq());
        let items: Vec<(BestTakeChoice, Face, Face, PhotoRef)> = req
            .choices
            .into_iter()
            .zip(faces)
            .map(|(c, (bf, sf, src))| (c, bf, sf, src))
            .collect();
        self.task_progress(&task_id, "besttake", 0, items.len() as i64, "running", None);
        let (core, tid) = (self.clone(), task_id.clone());
        tokio::spawn(async move { core.run_besttake(tid, base, items).await });
        Ok(task_id)
    }

    /// `POST /api/bursts/{id}/besttake/auto`: everyone's best onto the plan's base (or `base`).
    pub async fn besttake_auto(
        self: &Arc<Self>,
        burst_id: i64,
        base: Option<i64>,
    ) -> Result<String> {
        let plan = self.besttake_plan(burst_id, base).await?;
        let choices = auto_choices(&plan);
        if choices.is_empty() {
            // nothing to improve: a task that finishes at once, so clients follow one flow
            let task_id = format!("besttake-{}", self.next_task_seq());
            let (core, tid, base) = (self.clone(), task_id.clone(), plan.base_photo_id);
            self.task_progress(&task_id, "besttake", 0, 0, "running", None);
            tokio::spawn(async move {
                core.events.emit(Event::BestTakeDone {
                    photo_id: base,
                    results: Vec::new(),
                });
                core.task_progress(&tid, "besttake", 0, 0, "done", None);
            });
            return Ok(task_id);
        }
        self.besttake_start(BestTakeRequest {
            base_photo_id: plan.base_photo_id,
            choices,
        })
        .await
    }

    async fn run_besttake(
        self: Arc<Self>,
        task_id: String,
        base: PhotoRef,
        items: Vec<(BestTakeChoice, Face, Face, PhotoRef)>,
    ) {
        let total = items.len() as i64;
        let dir = self.gen_dir(&task_id);
        let mut results: Vec<BestTakeResult> = Vec::new();
        let mut new_ops: Vec<(usize, Value)> = Vec::new();
        let mut fatal: Option<String> = None;
        for (i, (choice, bf, sf, src)) in items.iter().enumerate() {
            let fail = |reason: String| BestTakeResult {
                base_face_id: choice.base_face_id,
                ok: false,
                warnings: Vec::new(),
                reason: Some(reason),
            };
            if let Some(why) = &fatal {
                results.push(fail(why.clone()));
                continue;
            }
            // composability is judged against the base actually posted
            if pose_delta(bf, sf) > POSE_LIMIT_DEG {
                results.push(fail("large_pose_change".to_string()));
                self.task_progress(&task_id, "besttake", i as i64 + 1, total, "running", None);
                continue;
            }
            let req = BestTakeComposeRequest {
                base: mkphoto(&base),
                source: mkphoto(src),
                base_face: bf.bbox,
                source_face: sf.bbox,
                out_dir: dir.join(i.to_string()).to_string_lossy().into_owned(),
                allow_download: false,
            };
            match self
                .worker
                .besttake_compose(&req)
                .await
                .map_err(map_worker_err)
            {
                Err(e) => {
                    let why = reason_of(&e);
                    if is_fatal(&e) {
                        fatal = Some(why.clone());
                    }
                    results.push(fail(why));
                }
                Ok(resp) => {
                    let (Some(patch), Some(rect)) = (resp.patch.clone(), resp.rect) else {
                        results.push(fail(
                            resp.reason.unwrap_or_else(|| "reason_unknown".to_string()),
                        ));
                        self.task_progress(
                            &task_id,
                            "besttake",
                            i as i64 + 1,
                            total,
                            "running",
                            None,
                        );
                        continue;
                    };
                    let (svc, base_id, face_id) = (self.render.clone(), base.id, bf.id);
                    let stored = tokio::task::spawn_blocking(move || {
                        svc.patches
                            .import(base_id, "bt", &face_id.to_string(), Path::new(&patch))
                    })
                    .await
                    .map_err(|e| CoreError::Internal(anyhow::anyhow!("patch import failed: {e}")))
                    .and_then(|r| r);
                    match stored {
                        Err(e) => results.push(fail(reason_of(&e))),
                        Ok((asset, _)) => {
                            let mut op = patch_op("best_take", &asset, rect, PATCH_FEATHER, 1.0);
                            if let Some(p) = bf.person_id {
                                op.insert("person_id".into(), json!(p));
                            }
                            op.insert("source_photo_id".into(), json!(src.id));
                            // additive (not in the typed op): which base face this replaces
                            op.insert("base_face_id".into(), json!(bf.id));
                            new_ops.push((results.len(), Value::Object(op)));
                            results.push(BestTakeResult {
                                base_face_id: choice.base_face_id,
                                ok: true,
                                warnings: resp.quality.map(|q| q.warnings).unwrap_or_default(),
                                reason: None,
                            });
                        }
                    }
                }
            }
            self.task_progress(&task_id, "besttake", i as i64 + 1, total, "running", None);
        }
        if !new_ops.is_empty() {
            let ops: Vec<Value> = new_ops.iter().map(|(_, o)| o.clone()).collect();
            if let Err(e) = self.apply_patches(base.id, ops).await {
                for (idx, _) in &new_ops {
                    results[*idx].ok = false;
                    results[*idx].warnings.clear();
                    results[*idx].reason = Some(reason_of(&e));
                }
            }
        }
        let ok = results.iter().filter(|r| r.ok).count();
        let error =
            (ok == 0 && !results.is_empty()).then(|| results[0].reason.clone().unwrap_or_default());
        self.events.emit(Event::BestTakeDone {
            photo_id: base.id,
            results,
        });
        let state = if error.is_some() { "failed" } else { "done" };
        self.task_progress(&task_id, "besttake", ok as i64, total, state, error);
        let _ = std::fs::remove_dir_all(dir);
    }

    // -------------------------------------------------------------- inpaint

    /// `GET /api/assets/{photo_id}/{asset}`: the PNG of a patch asset.
    pub async fn asset_png(&self, photo_id: i64, asset: &str) -> Result<Vec<u8>> {
        self.db
            .call(move |c| catalog::photo_ref(c, photo_id).map(|_| ()))
            .await?;
        let (svc, asset) = (self.render.clone(), asset.to_string());
        tokio::task::spawn_blocking(move || svc.patches.read_png(photo_id, &asset))
            .await
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("asset read failed: {e}")))?
    }

    /// `GET /api/photos/{id}/bystanders`: the non-subject faces.
    pub async fn bystanders(&self, photo_id: i64) -> Result<Vec<Bystander>> {
        self.db
            .call(move |c| {
                catalog::photo_ref(c, photo_id)?;
                Ok(astore::faces_of_photo(c, photo_id)?
                    .into_iter()
                    .filter(|f| !f.is_subject)
                    .map(|f| Bystander {
                        face_id: f.id,
                        bbox: f.bbox,
                    })
                    .collect())
            })
            .await
    }

    /// `POST /api/photos/{id}/inpaint`.
    pub async fn inpaint_start(
        self: &Arc<Self>,
        photo_id: i64,
        body: InpaintBody,
    ) -> Result<String> {
        let model = body.model.clone().unwrap_or_else(|| "lama".to_string());
        if model != "lama" && model != "sdxl" {
            return Err(CoreError::Unprocessable(
                "model must be lama or sdxl".into(),
            ));
        }
        let strokes = body.strokes.clone().unwrap_or_default();
        if strokes.len() > 100 {
            return Err(CoreError::Unprocessable("at most 100 strokes".into()));
        }
        for s in &strokes {
            if s.points.is_empty() || s.points.len() > 4096 {
                return Err(CoreError::Unprocessable(
                    "a stroke needs 1..4096 points".into(),
                ));
            }
            if s.points.iter().any(|p| {
                !p[0].is_finite()
                    || !p[1].is_finite()
                    || p[0] < -0.05
                    || p[0] > 1.05
                    || p[1] < -0.05
                    || p[1] > 1.05
            }) {
                return Err(CoreError::Unprocessable(
                    "stroke points must be normalised coordinates within 0..1".into(),
                ));
            }
            if !s.radius.is_finite() || !(0.0005..=0.25).contains(&s.radius) {
                return Err(CoreError::Unprocessable(
                    "stroke radius must be within 0.0005..0.25".into(),
                ));
            }
        }
        let want_faces = body.face_ids.clone().unwrap_or_default();
        let by = body.bystanders;
        let (r, faces) = self
            .db
            .call(move |c| {
                let r = catalog::photo_ref(c, photo_id)?;
                let all = astore::faces_of_photo(c, photo_id)?;
                let mut chosen: Vec<Face> = Vec::new();
                for id in &want_faces {
                    let f = all.iter().find(|f| f.id == *id).ok_or_else(|| {
                        CoreError::Unprocessable(format!("face {id} is not in photo {photo_id}"))
                    })?;
                    chosen.push(f.clone());
                }
                if by {
                    for f in all.iter().filter(|f| !f.is_subject) {
                        if !chosen.iter().any(|c| c.id == f.id) {
                            chosen.push(f.clone());
                        }
                    }
                }
                Ok((r, chosen))
            })
            .await?;
        if faces.is_empty() && strokes.is_empty() {
            return Err(CoreError::Unprocessable(if body.bystanders {
                "the photo has no bystanders; give face_ids or strokes".into()
            } else {
                "give bystanders, face_ids or strokes".to_string()
            }));
        }
        self.preflight_models("inpaint", &[model.as_str()]).await?;
        let task_id = format!("inpaint-{}", self.next_task_seq());
        self.task_progress(
            &task_id,
            "inpaint",
            0,
            faces.len() as i64 + 1,
            "running",
            None,
        );
        let (core, tid) = (self.clone(), task_id.clone());
        tokio::spawn(async move {
            let out = core
                .run_inpaint(&tid, r.clone(), faces, strokes, model)
                .await;
            let (ok, reason) = match out {
                Ok(()) => (true, None),
                Err(e) => (false, Some(reason_of(&e))),
            };
            core.events.emit(Event::InpaintDone {
                photo_id: r.id,
                ok,
                reason: reason.clone(),
            });
            let state = if ok { "done" } else { "failed" };
            core.task_progress(&tid, "inpaint", 1, 1, state, reason);
            let _ = std::fs::remove_dir_all(core.gen_dir(&tid));
        });
        Ok(task_id)
    }

    /// Builds the removal mask: bystander faces become `person` masks from the worker, strokes
    /// are rasterised, everything is unioned and dilated slightly.
    pub async fn build_removal_mask(
        &self,
        r: &PhotoRef,
        faces: &[Face],
        strokes: &[Stroke],
        dir: &Path,
        progress: &(dyn Fn(i64) + Sync),
    ) -> Result<MaskCanvas> {
        let (w, h) = match (r.width, r.height) {
            (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
            _ => {
                let (svc, rr) = (self.render.clone(), r.clone());
                let p = tokio::task::spawn_blocking(move || svc.proxy(&rr, INPAINT_MASK_EDGE))
                    .await
                    .map_err(|e| CoreError::Internal(anyhow::anyhow!("decode failed: {e}")))??;
                (p.width, p.height)
            }
        };
        let mut canvas = MaskCanvas::for_image(w, h, INPAINT_MASK_EDGE);
        for (i, f) in faces.iter().enumerate() {
            let fdir = dir.join(format!("face_{}", f.id));
            std::fs::create_dir_all(&fdir)?;
            let req = MaskRequest {
                photo: mkphoto(r),
                targets: vec!["person".to_string()],
                person_bbox: Some(f.bbox),
                size: crate::edit::service::MASK_SIZE,
                out_dir: fdir.to_string_lossy().into_owned(),
                allow_download: false,
            };
            let resp = self
                .worker
                .mask_generate(&req)
                .await
                .map_err(map_worker_err)?;
            match (resp.masks.get("person"), resp.skipped.get("person")) {
                (Some(p), _) => {
                    let img = image::open(p)
                        .map_err(|e| {
                            CoreError::Internal(anyhow::anyhow!("unreadable person mask: {e}"))
                        })?
                        .to_luma8();
                    // The matting keeps the photo's main subject, so small background people
                    // often come back as an empty mask: treat that like a missing mask.
                    if img.pixels().any(|px| px.0[0] > 127) {
                        canvas.union(&img);
                    } else {
                        canvas.fill_rect(body_box(f.bbox));
                    }
                }
                (None, Some(why)) if why == "model_unavailable" => {
                    return Err(CoreError::ModelsMissing(vec!["mask.person".to_string()]));
                }
                // no usable mask: remove a body-sized box around the face instead
                _ => canvas.fill_rect(body_box(f.bbox)),
            }
            progress(i as i64 + 1);
        }
        // person masks hug the silhouette: grow them a little; brush strokes are taken as drawn
        if !faces.is_empty() {
            let r =
                ((canvas.width.max(canvas.height) as f32 * BYSTANDER_DILATE).round() as u32).max(2);
            canvas.dilate(r);
        }
        canvas.draw_strokes(strokes);
        Ok(canvas)
    }

    async fn run_inpaint(
        self: &Arc<Self>,
        task_id: &str,
        r: PhotoRef,
        faces: Vec<Face>,
        strokes: Vec<Stroke>,
        model: String,
    ) -> Result<()> {
        let dir = self.gen_dir(task_id);
        std::fs::create_dir_all(&dir)?;
        let total = faces.len() as i64 + 1;
        let (core, tid) = (self.clone(), task_id.to_string());
        let canvas = self
            .build_removal_mask(&r, &faces, &strokes, &dir, &move |done| {
                core.task_progress(&tid, "inpaint", done, total, "running", None)
            })
            .await?;
        if canvas.count() == 0 {
            return Err(CoreError::Unprocessable("the removal mask is empty".into()));
        }
        let mask_path = dir.join("mask.png");
        canvas.save_png(&mask_path)?;
        let resp = self
            .worker
            .inpaint_run(&InpaintRequest {
                photo: mkphoto(&r),
                mask: mask_path.to_string_lossy().into_owned(),
                model,
                out_dir: dir.join("out").to_string_lossy().into_owned(),
                allow_download: false,
            })
            .await
            .map_err(map_worker_err)?;
        let (Some(patch), Some(rect)) = (resp.patch, resp.rect) else {
            return Err(CoreError::Internal(anyhow::anyhow!(
                "inpaint.run returned no patch"
            )));
        };
        let (svc, id) = (self.render.clone(), r.id);
        let (asset, _) = tokio::task::spawn_blocking(move || {
            svc.patches.import(id, "inp", "", Path::new(&patch))
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("patch import failed: {e}")))??;
        let op = patch_op("inpaint", &asset, rect, PATCH_FEATHER, 1.0);
        self.apply_patches(r.id, vec![Value::Object(op)]).await
    }

    // -------------------------------------------------------------- enhance

    /// `POST /api/photos/{id}/enhance`.
    pub async fn enhance_start(
        self: &Arc<Self>,
        photo_id: i64,
        body: EnhanceBody,
    ) -> Result<String> {
        let op = match body.op.as_str() {
            "denoise" | "face_restore" => body.op.clone(),
            "upscale" => {
                return Err(CoreError::Unprocessable(
                    "upscale is only available through export (upscale: 2|4)".into(),
                ))
            }
            _ => {
                return Err(CoreError::Unprocessable(
                    "op must be denoise or face_restore".into(),
                ))
            }
        };
        let strength = body.strength.unwrap_or(0.5);
        if !strength.is_finite() || !(0.0..=1.0).contains(&strength) {
            return Err(CoreError::Unprocessable(
                "strength must be within 0..1".into(),
            ));
        }
        let (r, faces) = self
            .db
            .call(move |c| {
                let r = catalog::photo_ref(c, photo_id)?;
                Ok((r, astore::faces_of_photo(c, photo_id)?))
            })
            .await?;
        if op == "face_restore" && faces.is_empty() {
            return Err(CoreError::Unprocessable("the photo has no faces".into()));
        }
        self.preflight_models("enhance", &[op.as_str()]).await?;
        let task_id = format!("enhance-{}", self.next_task_seq());
        self.task_progress(&task_id, "enhance", 0, 1, "running", None);
        let (core, tid) = (self.clone(), task_id.clone());
        tokio::spawn(async move {
            let out = core.run_enhance(&tid, &r, &op, strength, &faces).await;
            let (ok, reason) = match out {
                Ok(()) => (true, None),
                Err(e) => (false, Some(reason_of(&e))),
            };
            core.events.emit(Event::EnhanceDone {
                photo_id: r.id,
                op: op.clone(),
                ok,
                reason: reason.clone(),
            });
            let state = if ok { "done" } else { "failed" };
            core.task_progress(&tid, "enhance", ok as i64, 1, state, reason);
            let _ = std::fs::remove_dir_all(core.gen_dir(&tid));
        });
        Ok(task_id)
    }

    async fn run_enhance(
        self: &Arc<Self>,
        task_id: &str,
        r: &PhotoRef,
        op: &str,
        strength: f64,
        faces: &[Face],
    ) -> Result<()> {
        let dir = self.gen_dir(task_id);
        let resp = self
            .worker
            .enhance_run(&EnhanceRequest {
                photo: mkphoto(r),
                op: op.to_string(),
                strength,
                scale: None,
                faces: (op == "face_restore").then(|| faces.iter().map(|f| f.bbox).collect()),
                out_dir: dir.to_string_lossy().into_owned(),
                allow_download: false,
            })
            .await
            .map_err(map_worker_err)?;
        let mut pieces: Vec<(String, [f32; 4])> = Vec::new();
        match op {
            "denoise" => {
                let p = resp.patch.ok_or_else(|| {
                    CoreError::Internal(anyhow::anyhow!("enhance.run returned no patch"))
                })?;
                pieces.push((p, resp.rect.unwrap_or([0.0, 0.0, 1.0, 1.0])));
            }
            _ => {
                for p in resp.patches {
                    pieces.push((p.patch, p.rect));
                }
                if pieces.is_empty() {
                    return Err(CoreError::Conflict("no face could be restored".into()));
                }
            }
        }
        let (svc, id) = (self.render.clone(), r.id);
        let kind = op.to_string();
        let assets = tokio::task::spawn_blocking(move || -> Result<Vec<(String, [f32; 4])>> {
            pieces
                .into_iter()
                .map(|(p, rect)| {
                    let prefix = if kind == "denoise" { "dn" } else { "fr" };
                    svc.patches
                        .import(id, prefix, "", Path::new(&p))
                        .map(|(a, _)| (a, rect))
                })
                .collect()
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("patch import failed: {e}")))??;
        let ops: Vec<Value> = assets
            .iter()
            .map(|(a, rect)| Value::Object(patch_op(op, a, *rect, PATCH_FEATHER, 1.0)))
            .collect();
        self.apply_patches(r.id, ops).await
    }
}
