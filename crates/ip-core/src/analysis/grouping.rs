//! Burst and scene segmentation (docs/03 §2.2). Pure functions over time-sorted items.

use super::vecs::cosine;

#[derive(Debug, Clone)]
pub struct GroupItem {
    pub id: i64,
    /// Capture time (ms); `None` when neither EXIF nor file time is known.
    pub t: Option<i64>,
    /// Camera that took the photo (`photo.device_id`); `None` = unknown (no EXIF make/model).
    pub device: Option<i64>,
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
    /// A photo is compared with up to this many preceding photos, not only the previous one.
    pub lookback: usize,
    /// At most this many photos in a row that interrupt a burst are absorbed into it.
    pub max_fill: usize,
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
            lookback: 8,
            max_fill: 2,
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

/// docs/03 §2.2 pair rule: `dt <= T_burst && sim >= θ_burst`, or `dt <= 30 s && sim >= θ_strict`;
/// without a capture time only the strict similarity counts.
fn linked(a: &GroupItem, b: &GroupItem, s: f32, t_burst: i64, p: &GroupParams) -> bool {
    match (a.t, b.t) {
        (Some(x), Some(y)) => {
            let dt = (y - x).abs();
            (dt <= t_burst && s >= p.theta_burst) || (dt <= p.t_slow_ms && s >= p.theta_strict)
        }
        _ => s >= p.theta_strict,
    }
}

/// Splits time-sorted `items` into bursts; returns index lists (each non-empty, in order), ordered
/// by their first photo.
///
/// Two refinements of the purely adjacent rule of docs/03 §2.2 (same thresholds, same drift cut):
/// - Look-back: a photo joins the burst of the most similar of the previous `lookback` photos it is
///   linked to, not only the burst of its predecessor. With the adjacent rule alone one odd frame
///   (a blink, a turned head, motion blur) cut a burst in two, because the next, normal frame was
///   compared with the odd one: on the synthetic library 53 of 284 consecutive burst frames are
///   more than 9 pHash bits apart (θ_burst 0.85 without embeddings), although most of them are
///   within 0-4 bits of the frame before the odd one. Look-back also keeps two cameras shooting at
///   the same time apart instead of chaining their interleaved frames.
/// - Gap filling: see `fill_gaps`.
pub fn segment_bursts(items: &[GroupItem], p: &GroupParams) -> Vec<Vec<usize>> {
    if items.is_empty() {
        return Vec::new();
    }
    let t_burst = p.t_burst_ms.unwrap_or_else(|| adaptive_t_burst_ms(items));
    let mut gid: Vec<usize> = Vec::with_capacity(items.len());
    let mut first: Vec<usize> = Vec::new(); // first photo of every burst (drift reference)
    for i in 0..items.len() {
        let mut best: Option<(f32, usize)> = None; // (similarity, burst)
        for j in (i.saturating_sub(p.lookback.max(1))..i).rev() {
            let s = similarity(&items[j], &items[i], p);
            if !linked(&items[j], &items[i], s, t_burst, p) {
                continue;
            }
            let g = gid[j];
            // the drift cut of docs/03: a photo too far from the burst's first one starts a new one
            if similarity(&items[first[g]], &items[i], p) < p.theta_drift {
                continue;
            }
            if best.map(|(b, _)| s > b).unwrap_or(true) {
                best = Some((s, g)); // ties go to the nearer photo
            }
        }
        match best {
            Some((_, g)) => gid.push(g),
            None => {
                gid.push(first.len());
                first.push(i);
            }
        }
    }
    fill_gaps(items, &mut gid, first.len(), t_burst, p);
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); first.len()];
    for (i, g) in gid.into_iter().enumerate() {
        out[g].push(i);
    }
    out.retain(|g| !g.is_empty());
    out.sort_by_key(|g| g[0]);
    out
}

/// Gap filling: up to `max_fill` consecutive photos that sit inside a burst, taken by the same
/// camera as their neighbours of that burst with every step at burst cadence (`<= T_burst`), join
/// it even when they do not look similar enough. A camera cannot take an unrelated photo in the
/// middle of a continuous burst; such frames are the subject jumping, a blink, heavy motion blur or
/// someone walking through the frame, and the burst is where the user wants to reject them. The
/// run must be whole bursts of its own (never splits another burst) and needs a known camera, so
/// interleaved photos of a second camera or of files without EXIF stay separate.
fn fill_gaps(
    items: &[GroupItem],
    gid: &mut [usize],
    n_groups: usize,
    t_burst: i64,
    p: &GroupParams,
) {
    let n = items.len();
    let mut size = vec![0usize; n_groups];
    for &g in gid.iter() {
        size[g] += 1;
    }
    let mut i = 1;
    while i + 1 < n {
        let h = gid[i - 1];
        if gid[i] == h {
            i += 1;
            continue;
        }
        // the run i..j of photos outside burst h, closed by the next photo of h
        let mut j = i;
        while j < n && j - i < p.max_fill && gid[j] != h {
            j += 1;
        }
        if j == n || gid[j] != h {
            i += 1;
            continue;
        }
        let whole = (i..j).all(|x| (i..j).filter(|&y| gid[y] == gid[x]).count() == size[gid[x]]);
        let cadence = (i - 1..j).all(
            |a| matches!((items[a].t, items[a + 1].t), (Some(x), Some(y)) if y - x <= t_burst),
        );
        let camera = items[i - 1].device.is_some()
            && (i..=j).all(|x| items[x].device == items[i - 1].device);
        if whole && cadence && camera {
            for g in &mut gid[i..j] {
                size[*g] -= 1;
                *g = h;
                size[h] += 1;
            }
        }
        i = j;
    }
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
            device: Some(1),
            phash: Some(hash),
            emb: Some(unit_emb(angle)),
        }
    }

    /// pHash-only photo (lite/fast profiles) of camera `dev`, `t_ms` after the start.
    fn shot(id: i64, t_ms: i64, dev: Option<i64>, hash: u64) -> GroupItem {
        GroupItem {
            id,
            t: Some(t_ms),
            device: dev,
            phash: Some(hash),
            emb: None,
        }
    }

    /// `base` with the lowest `bits` bits flipped.
    fn flip(base: u64, bits: u32) -> u64 {
        base ^ ((1u64 << bits) - 1)
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
            device: None,
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
                device: None,
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

    const B: u64 = 0x0F0F_3C3C_A5A5_5A5A;

    #[test]
    fn one_odd_frame_does_not_cut_the_burst() {
        // 0.4 s burst; photo 3 is 12 bits off (blink / motion blur), photo 4 is back to normal
        let p = GroupParams {
            t_burst_ms: Some(1_000),
            ..GroupParams::default()
        };
        let items = vec![
            shot(1, 0, None, B),
            shot(2, 400, None, flip(B, 2)),
            shot(3, 800, None, flip(B, 12)),
            shot(4, 1_200, None, flip(B, 1)),
            shot(5, 1_600, None, B),
        ];
        // look-back: photo 4 links to photo 2 (1 bit apart), not to the odd photo 3 (11 bits)
        let g = segment_bursts(&items, &p);
        assert_eq!(g, vec![vec![0, 1, 3, 4], vec![2]]);
        // with a known camera the odd frame inside the burst is absorbed as well
        let items: Vec<GroupItem> = items
            .into_iter()
            .map(|it| GroupItem {
                device: Some(7),
                ..it
            })
            .collect();
        assert_eq!(segment_bursts(&items, &p), vec![vec![0, 1, 2, 3, 4]]);
    }

    #[test]
    fn gap_filling_needs_same_camera_and_burst_cadence() {
        let p = GroupParams {
            t_burst_ms: Some(1_000),
            ..GroupParams::default()
        };
        let other = !B; // 64 bits off: a different picture

        // another camera fires in the middle of the burst -> stays separate
        let g = segment_bursts(
            &[
                shot(1, 0, Some(1), B),
                shot(2, 300, Some(2), other),
                shot(3, 600, Some(1), B),
            ],
            &p,
        );
        assert_eq!(g, vec![vec![0, 2], vec![1]]);
        // same camera, but a 1.5 s pause before the odd photo: not a continuous burst (photo 3
        // still joins photo 1 by the slow rule)
        let g = segment_bursts(
            &[
                shot(1, 0, Some(1), B),
                shot(2, 1_500, Some(1), other),
                shot(3, 1_600, Some(1), B),
            ],
            &p,
        );
        assert_eq!(g, vec![vec![0, 2], vec![1]]);
        // three odd frames in a row exceed max_fill (2)
        let g = segment_bursts(
            &[
                shot(1, 0, Some(1), B),
                shot(2, 200, Some(1), other),
                shot(3, 400, Some(1), other ^ 1),
                shot(4, 600, Some(1), other ^ 3),
                shot(5, 800, Some(1), B),
            ],
            &p,
        );
        assert_eq!(g, vec![vec![0, 4], vec![1, 2, 3]]);
        // ...two are absorbed
        let g = segment_bursts(
            &[
                shot(1, 0, Some(1), B),
                shot(2, 200, Some(1), other),
                shot(3, 400, Some(1), other ^ 1),
                shot(4, 600, Some(1), B),
            ],
            &p,
        );
        assert_eq!(g, vec![vec![0, 1, 2, 3]]);
    }

    #[test]
    fn interleaved_cameras_keep_their_own_bursts() {
        // two cameras shooting different subjects at the same moment, frames interleaved in time
        let p = GroupParams {
            t_burst_ms: Some(1_000),
            ..GroupParams::default()
        };
        let other = !B;
        let items: Vec<GroupItem> = (0..8)
            .map(|k| {
                let (dev, h) = if k % 2 == 0 { (1, B) } else { (2, other) };
                shot(k, k * 150, Some(dev), h)
            })
            .collect();
        // the adjacent rule alone made eight single photos of them
        let g = segment_bursts(&items, &p);
        assert_eq!(g, vec![vec![0, 2, 4, 6], vec![1, 3, 5, 7]]);
    }

    #[test]
    fn look_back_is_bounded() {
        let p = GroupParams {
            t_burst_ms: Some(1_000),
            lookback: 2,
            ..GroupParams::default()
        };
        // frame 4 matches only frame 1, three photos back
        let items = vec![
            shot(1, 0, None, B),
            shot(2, 100, None, !B),
            shot(3, 200, None, !B ^ 1),
            shot(4, 300, None, B),
        ];
        assert_eq!(segment_bursts(&items, &p).len(), 3);
        let p = GroupParams { lookback: 3, ..p };
        assert_eq!(segment_bursts(&items, &p), vec![vec![0, 3], vec![1, 2]]);
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
