//! Composition score 0-1 (docs/03 §3.4), computed from boxes the analysis already stored:
//! the subject faces, else the worker's saliency box, plus the horizon tilt. Pure functions.
//!
//! The anchor is the union box of the subject faces (living faces only), or the saliency box when
//! there is none; no anchor, no score. Four parts, each 0-1, combined with the weights below
//! (renormalised over the parts that exist):
//!
//! * `thirds` (0.35): proximity of the anchor to the rule-of-thirds lines. Per axis the better of
//!   a Gaussian (sigma 0.09) around the nearest third line and 0.8x a Gaussian (sigma 0.06)
//!   around the centre, so a centred subject scores a little less than a third-line one, a
//!   subject in a corner or on the edge much less; the score is the geometric mean of both axes.
//!   For faces the vertical anchor is the eye line (40% down the union box).
//! * `headroom` (0.15, faces at least 8% of the frame tall): the gap above the topmost head is
//!   ideal between 4% and 25% of the frame height; smaller is cut off (linear to 0), larger
//!   wastes the frame (linear down to 0.3 at 60%).
//! * `horizon` (0.20, only when the tilt is confident and at most [`TILTED_MAX_DEG`]): 1 at
//!   level, falling linearly to 0.15 at 6 degrees; a larger angle is perspective or intent and
//!   is not judged.
//! * `cutoff` (0.30): subject faces touching the frame edge are cut off: 1 - 0.8 x their share
//!   (the top edge is judged by headroom). Without faces, a saliency box touching an edge while
//!   covering less than 90% of the frame in both directions counts as one cut-off subject, with
//!   the smaller penalty 0.4.

use super::scoring::{PhotoFeat, TILTED_MAX_DEG, TILTED_MIN_CONFIDENCE};

pub const W_THIRDS: f64 = 0.35;
pub const W_HEADROOM: f64 = 0.15;
pub const W_HORIZON: f64 = 0.20;
pub const W_CUTOFF: f64 = 0.30;
const EDGE: f64 = 0.01;
const MIN_HEAD_HEIGHT: f64 = 0.08;
const LEVEL_FULL_PENALTY_DEG: f64 = 6.0;

fn gauss(d: f64, sigma: f64) -> f64 {
    (-0.5 * (d / sigma).powi(2)).exp()
}

/// How well one coordinate (0-1) sits on a third line or the centre.
pub fn axis_score(a: f64) -> f64 {
    let third = (a - 1.0 / 3.0).abs().min((a - 2.0 / 3.0).abs());
    gauss(third, 0.09).max(0.8 * gauss((a - 0.5).abs(), 0.06))
}

/// Rule-of-thirds proximity of an anchor point.
pub fn thirds_score(x: f64, y: f64) -> f64 {
    (axis_score(x.clamp(0.0, 1.0)) * axis_score(y.clamp(0.0, 1.0))).sqrt()
}

/// Headroom above the topmost head (fraction of the frame height).
pub fn headroom_score(gap: f64) -> f64 {
    if gap < 0.04 {
        (gap.max(0.0) / 0.04).clamp(0.0, 1.0)
    } else if gap <= 0.25 {
        1.0
    } else {
        (1.0 - 0.7 * (gap - 0.25) / 0.35).clamp(0.3, 1.0)
    }
}

/// 1 at level, 0.15 at [`LEVEL_FULL_PENALTY_DEG`] and beyond (until [`TILTED_MAX_DEG`]).
pub fn horizon_score(deg: f64) -> f64 {
    1.0 - 0.85 * (deg.abs() / LEVEL_FULL_PENALTY_DEG).min(1.0)
}

fn union(boxes: &[[f64; 4]]) -> [f64; 4] {
    let x0 = boxes.iter().map(|b| b[0]).fold(f64::MAX, f64::min);
    let y0 = boxes.iter().map(|b| b[1]).fold(f64::MAX, f64::min);
    let x1 = boxes.iter().map(|b| b[0] + b[2]).fold(f64::MIN, f64::max);
    let y1 = boxes.iter().map(|b| b[1] + b[3]).fold(f64::MIN, f64::max);
    [x0, y0, x1 - x0, y1 - y0]
}

fn touches_side_or_bottom(b: &[f64; 4]) -> bool {
    b[0] < EDGE || b[0] + b[2] > 1.0 - EDGE || b[1] + b[3] > 1.0 - EDGE
}

#[derive(Debug, Clone, PartialEq)]
pub struct Composition {
    pub score: f64,
    /// (part key, value 0-1, normalised weight)
    pub parts: Vec<(&'static str, f64, f64)>,
}

/// The score of a photo, `None` without an anchor (no living subject face, no saliency box).
/// Uses `faces` (subjects only), `saliency`, `tilt_deg` / `tilt_confidence`.
pub fn composition_score(feat: &PhotoFeat) -> Option<Composition> {
    let faces: Vec<[f64; 4]> = feat
        .faces
        .iter()
        .filter(|f| f.is_subject && !f.likely_artificial())
        .map(|f| f.bbox)
        .filter(|b| b[2] > 0.0 && b[3] > 0.0)
        .collect();
    let mut parts: Vec<(&'static str, f64, f64)> = Vec::new();
    if !faces.is_empty() {
        let u = union(&faces);
        parts.push((
            "thirds",
            thirds_score(u[0] + u[2] / 2.0, u[1] + 0.4 * u[3]),
            W_THIRDS,
        ));
        if u[3] >= MIN_HEAD_HEIGHT {
            parts.push(("headroom", headroom_score(u[1]), W_HEADROOM));
        }
        let cut = faces.iter().filter(|b| touches_side_or_bottom(b)).count();
        parts.push((
            "cutoff",
            1.0 - 0.8 * cut as f64 / faces.len() as f64,
            W_CUTOFF,
        ));
    } else {
        let b = feat.saliency.filter(|b| b[2] > 0.0 && b[3] > 0.0)?;
        parts.push((
            "thirds",
            thirds_score(b[0] + b[2] / 2.0, b[1] + b[3] / 2.0),
            W_THIRDS,
        ));
        let full = b[2] >= 0.9 && b[3] >= 0.9;
        let cut = !full && (touches_side_or_bottom(&b) || b[1] < EDGE);
        parts.push(("cutoff", if cut { 0.6 } else { 1.0 }, W_CUTOFF));
    }
    if let (Some(deg), Some(conf)) = (feat.tilt_deg, feat.tilt_confidence) {
        if conf >= TILTED_MIN_CONFIDENCE && deg.abs() <= TILTED_MAX_DEG {
            parts.push(("horizon", horizon_score(deg), W_HORIZON));
        }
    }
    let wsum: f64 = parts.iter().map(|p| p.2).sum();
    let parts: Vec<(&'static str, f64, f64)> = parts
        .into_iter()
        .map(|(k, v, w)| (k, v, w / wsum))
        .collect();
    let score = parts
        .iter()
        .map(|(_, v, w)| v * w)
        .sum::<f64>()
        .clamp(0.0, 1.0);
    Some(Composition { score, parts })
}

#[cfg(test)]
mod tests {
    use super::super::scoring::FaceFeat;
    use super::*;

    fn subject(b: [f64; 4]) -> FaceFeat {
        FaceFeat {
            bbox: b,
            is_subject: true,
            ..Default::default()
        }
    }

    fn feat(faces: Vec<FaceFeat>) -> PhotoFeat {
        PhotoFeat {
            faces,
            ..Default::default()
        }
    }

    fn score(f: &PhotoFeat) -> f64 {
        composition_score(f).unwrap().score
    }

    #[test]
    fn third_line_beats_corner_and_slightly_beats_centre() {
        let third = thirds_score(1.0 / 3.0, 1.0 / 3.0);
        let centre = thirds_score(0.5, 0.5);
        let corner = thirds_score(0.05, 0.05);
        assert!(third > 0.99);
        assert!(centre > 0.75 && centre < third, "{centre}");
        assert!(corner < 0.05, "{corner}");
        // halfway between centre and third is the worst place on an axis
        assert!(axis_score(0.42) < axis_score(0.5) && axis_score(0.42) < axis_score(1.0 / 3.0));
    }

    #[test]
    fn headroom_curve() {
        assert_eq!(headroom_score(0.0), 0.0);
        assert!((headroom_score(0.02) - 0.5).abs() < 1e-9);
        assert_eq!(headroom_score(0.1), 1.0);
        assert_eq!(headroom_score(0.25), 1.0);
        assert!((headroom_score(0.5) - 0.5).abs() < 1e-9);
        assert!(headroom_score(0.5) < headroom_score(0.3));
        assert_eq!(headroom_score(0.9), 0.3);
    }

    #[test]
    fn horizon_curve() {
        assert_eq!(horizon_score(0.0), 1.0);
        assert!((horizon_score(-3.0) - 0.575).abs() < 1e-9);
        assert!((horizon_score(6.0) - 0.15).abs() < 1e-9);
        assert!((horizon_score(9.0) - 0.15).abs() < 1e-9);
    }

    #[test]
    fn well_framed_portrait_beats_cropped_one() {
        // face on the left third line, eyes on the upper third, room above
        let good = feat(vec![subject([0.26, 0.25, 0.14, 0.2])]);
        // face jammed against the top-right corner
        let bad = feat(vec![subject([0.84, 0.0, 0.16, 0.2])]);
        let (g, b) = (score(&good), score(&bad));
        assert!(g > 0.85, "{g}");
        assert!(b < 0.45, "{b}");
        let parts = composition_score(&bad).unwrap().parts;
        let part = |k: &str| parts.iter().find(|p| p.0 == k).unwrap().1;
        assert_eq!(part("headroom"), 0.0);
        assert!((part("cutoff") - 0.2).abs() < 1e-9);
        let wsum: f64 = parts.iter().map(|p| p.2).sum();
        assert!((wsum - 1.0).abs() < 1e-9);
    }

    #[test]
    fn cutoff_counts_share_of_faces_and_ignores_top_edge() {
        let a = feat(vec![
            subject([0.2, 0.2, 0.15, 0.2]),
            subject([0.85, 0.2, 0.15, 0.2]),
        ]);
        let cut = composition_score(&a)
            .unwrap()
            .parts
            .iter()
            .find(|p| p.0 == "cutoff")
            .unwrap()
            .1;
        assert!((cut - 0.6).abs() < 1e-9, "{cut}");
        let top = feat(vec![subject([0.4, 0.0, 0.2, 0.3])]);
        let cut = composition_score(&top)
            .unwrap()
            .parts
            .iter()
            .find(|p| p.0 == "cutoff")
            .unwrap()
            .1;
        assert_eq!(cut, 1.0, "a head at the top is a headroom matter");
    }

    #[test]
    fn horizon_only_when_confident_and_in_range() {
        let mut f = feat(vec![subject([0.3, 0.2, 0.15, 0.2])]);
        let level = score(&f);
        let has = |f: &PhotoFeat| {
            composition_score(f)
                .unwrap()
                .parts
                .iter()
                .any(|p| p.0 == "horizon")
        };
        f.tilt_deg = Some(4.0);
        f.tilt_confidence = Some(0.1);
        assert!(!has(&f));
        f.tilt_confidence = Some(0.6);
        assert!(has(&f));
        assert!(score(&f) < level - 0.05, "{} {}", score(&f), level);
        f.tilt_deg = Some(15.0); // perspective / intent: not judged
        assert!(!has(&f));
        f.tilt_deg = Some(0.2); // level
        assert!(score(&f) >= level - 1e-9);
    }

    #[test]
    fn saliency_anchors_when_no_face_and_artificial_faces_do_not() {
        let mut f = PhotoFeat::default();
        assert!(composition_score(&f).is_none(), "no anchor, no score");
        f.saliency = Some([0.55, 0.25, 0.2, 0.3]);
        let on_third = score(&f);
        f.saliency = Some([0.0, 0.1, 0.3, 0.3]);
        let cut = score(&f);
        assert!(on_third > cut + 0.3, "{on_third} {cut}");
        // a statue face is no anchor, and no subject: the saliency box decides
        let mut statue = subject([0.4, 0.4, 0.2, 0.2]);
        statue.artificial = Some(0.9);
        let g = PhotoFeat {
            faces: vec![statue],
            saliency: Some([0.55, 0.25, 0.2, 0.3]),
            ..Default::default()
        };
        assert!((score(&g) - on_third).abs() < 1e-9);
        // a full-frame saliency box is not "cut off"
        f.saliency = Some([0.0, 0.0, 1.0, 1.0]);
        let parts = composition_score(&f).unwrap().parts;
        assert_eq!(parts.iter().find(|p| p.0 == "cutoff").unwrap().1, 1.0);
    }
}
