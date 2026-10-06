//! Burst and scene segmentation (docs/03 §2.2). Pure functions over time-sorted items.

use super::vecs::cosine;

#[derive(Debug, Clone)]
pub struct GroupItem {
    pub id: i64,
    /// Capture time (ms); `None` when neither EXIF nor file time is known.
    pub t: Option<i64>,
    pub phash: Option<u64>,
    pub emb: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct GroupParams {
    /// `None` = adaptive from the gap distribution (default 3 s).
    pub t_burst_ms: Option<i64>,
    pub t_slow_ms: i64,
    pub theta_burst: f32,
    pub theta_strict: f32,
    /// A photo whose similarity to the burst's first photo falls below this starts a new burst.
    pub theta_drift: f32,
    pub w_embedding: f32,
    pub scene_gap_ms: i64,
    pub scene_cos: f32,
}

impl Default for GroupParams {
    fn default() -> Self {
        Self {
            t_burst_ms: None,
            t_slow_ms: 30_000,
            theta_burst: 0.85,
            theta_strict: 0.92,
            theta_drift: 0.75,
            w_embedding: 0.6,
            scene_gap_ms: 10 * 60_000,
            scene_cos: 0.7,
        }
    }
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

/// `0.6*cos(emb) + 0.4*(1 - hamming/64)`; falls back to whichever signal both photos have.
pub fn similarity(a: &GroupItem, b: &GroupItem, p: &GroupParams) -> f32 {
    let e = match (&a.emb, &b.emb) {
        (Some(x), Some(y)) if x.len() == y.len() && !x.is_empty() => Some(cosine(x, y)),
        _ => None,
    };
    let h = match (a.phash, b.phash) {
        (Some(x), Some(y)) => Some(1.0 - hamming(x, y) as f32 / 64.0),
        _ => None,
    };
    match (e, h) {
        (Some(e), Some(h)) => p.w_embedding * e + (1.0 - p.w_embedding) * h,
        (Some(e), None) => e,
        (None, Some(h)) => h,
        (None, None) => 0.0,
    }
}

/// Picks the burst interval from the valley of the log-gap distribution; 3 s without enough data.
pub fn adaptive_t_burst_ms(items: &[GroupItem]) -> i64 {
    const DEFAULT: i64 = 3_000;
    let mut gaps: Vec<f64> = items
        .windows(2)
        .filter_map(|w| Some((w[1].t? - w[0].t?).max(0) as f64))
        .map(|g| g.max(200.0))
        .filter(|g| *g <= 20_000.0)
        .collect();
    if gaps.len() < 20 {
        return DEFAULT;
    }
    gaps.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut best: Option<(f64, f64)> = None; // (ratio, geometric mid)
    for w in gaps.windows(2) {
        let (lo, hi) = (w[0], w[1]);
        if lo < 300.0 || hi > 10_000.0 || hi <= lo {
            continue;
        }
        let ratio = hi / lo;
        if best.map(|(r, _)| ratio > r).unwrap_or(true) {
            best = Some((ratio, (lo * hi).sqrt()));
        }
    }
    match best {
        Some((r, mid)) if r >= 1.5 => (mid as i64).clamp(1_000, 5_000),
        _ => DEFAULT,
    }
}

/// Splits time-sorted `items` into bursts; returns index lists (each non-empty, in order).
pub fn segment_bursts(items: &[GroupItem], p: &GroupParams) -> Vec<Vec<usize>> {
    if items.is_empty() {
        return Vec::new();
    }
    let t_burst = p.t_burst_ms.unwrap_or_else(|| adaptive_t_burst_ms(items));
    let mut out: Vec<Vec<usize>> = vec![vec![0]];
    let mut first = 0usize;
    for i in 1..items.len() {
        let prev = i - 1;
        let s = similarity(&items[prev], &items[i], p);
        let dt = match (items[prev].t, items[i].t) {
            (Some(a), Some(b)) => Some((b - a).abs()),
            _ => None,
        };
        let close = match dt {
            Some(dt) => {
                (dt <= t_burst && s >= p.theta_burst) || (dt <= p.t_slow_ms && s >= p.theta_strict)
            }
            None => s >= p.theta_strict,
        };
        let drifted = similarity(&items[first], &items[i], p) < p.theta_drift;
        if close && !drifted {
            out.last_mut().unwrap().push(i);
        } else {
            out.push(vec![i]);
            first = i;
        }
    }
    out
}

/// Summary of a burst used for scene merging.
#[derive(Debug, Clone)]
pub struct SceneUnit {
    pub start: i64,
    pub end: i64,
    pub emb: Option<Vec<f32>>,
    pub phash: Option<u64>,
}

/// Merges time-adjacent bursts into scenes: gap <= 10 min and similar content.
pub fn segment_scenes(units: &[SceneUnit], p: &GroupParams) -> Vec<Vec<usize>> {
    if units.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<Vec<usize>> = vec![vec![0]];
    for i in 1..units.len() {
        let (a, b) = (&units[i - 1], &units[i]);
        let gap = b.start - a.end;
        let same = if gap > p.scene_gap_ms {
            false
        } else {
            match (&a.emb, &b.emb) {
                (Some(x), Some(y)) if x.len() == y.len() => cosine(x, y) >= p.scene_cos,
                _ => match (a.phash, b.phash) {
                    (Some(x), Some(y)) => 1.0 - hamming(x, y) as f32 / 64.0 >= 0.75,
                    _ => gap <= 60_000,
                },
            }
        };
        if same {
            out.last_mut().unwrap().push(i);
        } else {
            out.push(vec![i]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_emb(angle_deg: f32) -> Vec<f32> {
        let a = angle_deg.to_radians();
        vec![a.cos(), a.sin(), 0.0, 0.0]
    }

    fn item(id: i64, t_s: i64, angle: f32, hash: u64) -> GroupItem {
        GroupItem {
            id,
            t: Some(t_s * 1000),
            phash: Some(hash),
            emb: Some(unit_emb(angle)),
        }
    }

    #[test]
    fn similar_and_close_photos_form_a_burst() {
        let items = vec![
            item(1, 0, 0.0, 0xFFFF),
            item(2, 1, 2.0, 0xFFFF),
            item(3, 2, 3.0, 0xFFFE),
            item(4, 60, 3.0, 0xFFFE), // too late for either rule
            item(5, 61, 4.0, 0xFFFE),
        ];
        let g = segment_bursts(&items, &GroupParams::default());
        assert_eq!(g, vec![vec![0, 1, 2], vec![3, 4]]);
    }

    #[test]
    fn dissimilar_photos_split_even_when_simultaneous() {
        let items = vec![item(1, 0, 0.0, 0), item(2, 0, 90.0, u64::MAX)];
        let g = segment_bursts(&items, &GroupParams::default());
        assert_eq!(g.len(), 2);
    }

    #[test]
    fn slow_burst_needs_strict_similarity() {
        // 10 s apart: only the strict rule applies
        let a = item(1, 0, 0.0, 0xFF00);
        let near = item(2, 10, 1.0, 0xFF00); // sim ~1.0
        let mid = item(3, 10, 25.0, 0xFF00); // cos 0.906 -> 0.6*0.906+0.4 = 0.94? keep below via hash
        let mut mid2 = mid.clone();
        mid2.phash = Some(0xFF00 ^ 0x3FF); // 10 bits differ
        let p = GroupParams::default();
        assert_eq!(segment_bursts(&[a.clone(), near], &p).len(), 1);
        assert_eq!(segment_bursts(&[a, mid2], &p).len(), 2);
    }

    #[test]
    fn long_group_drift_is_cut() {
        // each step is similar to the previous one, but the chain walks away from the first photo
        let items: Vec<GroupItem> = (0..8)
            .map(|i| item(i, i, i as f32 * 12.0, 0xAAAA_AAAA))
            .collect();
        let p = GroupParams::default();
        // consecutive cos(12deg) = 0.978 -> same group by the pairwise rule...
        assert!(similarity(&items[0], &items[1], &p) > 0.95);
        let g = segment_bursts(&items, &p);
        // ...but cos(first, i) drops below 0.75 once the angle exceeds ~54 deg (0.6*cos + 0.4 < 0.75)
        assert!(g.len() >= 2, "{g:?}");
        assert!(g.iter().all(|b| !b.is_empty()));
        assert_eq!(g.iter().map(Vec::len).sum::<usize>(), 8);
    }

    #[test]
    fn missing_time_requires_strict_similarity() {
        let mut a = item(1, 0, 0.0, 1);
        let mut b = item(2, 0, 0.5, 1);
        a.t = None;
        b.t = None;
        assert_eq!(segment_bursts(&[a, b], &GroupParams::default()).len(), 1);
    }

    #[test]
    fn phash_only_when_embeddings_missing() {
        let mk = |id, h| GroupItem {
            id,
            t: Some(0),
            phash: Some(h),
            emb: None,
        };
        let p = GroupParams::default();
        assert_eq!(segment_bursts(&[mk(1, 0), mk(2, 0b111)], &p).len(), 1); // 3 bits
        assert_eq!(segment_bursts(&[mk(1, 0), mk(2, 0xFFFF)], &p).len(), 2); // 16 bits
    }

    #[test]
    fn adaptive_threshold_finds_the_valley() {
        // 40 gaps of ~0.4 s (burst) and 40 of ~8 s (separate shots)
        let mut t = 0i64;
        let mut items = Vec::new();
        for i in 0..80 {
            items.push(GroupItem {
                id: i,
                t: Some(t),
                phash: None,
                emb: None,
            });
            t += if i % 2 == 0 {
                400 + (i % 5) * 20
            } else {
                8_000
            };
        }
        let tb = adaptive_t_burst_ms(&items);
        assert!((1_000..=5_000).contains(&tb), "{tb}");
        // too little data -> default
        assert_eq!(adaptive_t_burst_ms(&items[..5]), 3_000);
    }

    #[test]
    fn scenes_merge_close_similar_bursts() {
        let u = |s: i64, e: i64, a: f32| SceneUnit {
            start: s * 1000,
            end: e * 1000,
            emb: Some(unit_emb(a)),
            phash: None,
        };
        let units = vec![
            u(0, 5, 0.0),
            u(60, 70, 20.0),           // 1 min later, cos 0.94 -> same scene
            u(100, 110, 80.0),         // dissimilar -> new scene
            u(100 + 3600, 4000, 80.0), // an hour later -> new scene
        ];
        let s = segment_scenes(&units, &GroupParams::default());
        assert_eq!(s, vec![vec![0, 1], vec![2], vec![3]]);
    }
}
