//! Duplicate detection (docs/09 P1-3, PRD F2.12). Pure functions over the whole catalog; the SQL
//! side is `store::refresh_duplicates`.
//!
//! - **Exact** duplicates are different files with the same content key (`photo.content_key`:
//!   BLAKE3 over the first and last 64 KiB plus the length, the same key the thumbnail cache and
//!   the edit store use) and the same file size, wherever they live: another folder, another
//!   session, another root.
//! - **Near** duplicates are the same picture saved again without its metadata (a messenger
//!   download, a "save for web" export, a screenshot of the file): the perceptual hash differs in
//!   at most [`DUP_PHASH_MAX_BITS`] of 64 bits, the aspect ratio is the same, and one of the two
//!   has no capture time (no EXIF). The last condition is what keeps the frames of a burst, which
//!   are as close as 0-4 bits, out of the duplicates.
//!
//! Both relations are joined transitively; a group is identified by its smallest photo id.

use std::collections::HashMap;

/// Largest perceptual-hash distance (of 64 bits) of two near-duplicates. Unrelated photos are
/// about 32 bits apart; saving the same pixels again moves the hash by 0-2 bits when the codec and
/// quality are the same (an EXIF-stripping tool, or the synthetic library's no-EXIF copies: 0),
/// and by at most 4 bits for a JPEG re-encode at quality 90-95 of every one of 24 synthetic
/// pictures (`jpeg_reencodes_stay_within_the_threshold`; lower qualities and a half-size resize
/// still stay within 4 bits for about 85-95 % of them). The metadata rule above, not this
/// threshold, keeps the frames of a burst out.
pub const DUP_PHASH_MAX_BITS: u32 = 4;
/// Allowed relative difference of the aspect ratios of two near-duplicates.
pub const DUP_ASPECT_TOLERANCE: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DupParams {
    pub max_bits: u32,
}

impl Default for DupParams {
    fn default() -> Self {
        Self {
            max_bits: DUP_PHASH_MAX_BITS,
        }
    }
}

#[derive(Debug, Clone)]
pub struct DupItem {
    pub id: i64,
    pub content_key: Option<String>,
    pub file_size: Option<i64>,
    pub phash: Option<u64>,
    /// Display size (orientation applied).
    pub width: Option<i64>,
    pub height: Option<i64>,
    /// The file carries a capture time (EXIF); `false` = nothing but file times.
    pub has_capture_time: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DupKind {
    /// The photo has a byte-identical twin.
    Exact,
    /// Its group holds it only through the perceptual hash.
    Near,
}

impl DupKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Near => "near",
        }
    }
}

/// Group membership of one photo. Photos that are not duplicates of anything are not listed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DupAssign {
    pub photo_id: i64,
    /// Smallest photo id of the group.
    pub group_id: i64,
    pub kind: DupKind,
}

struct Dsu(Vec<usize>);

impl Dsu {
    fn find(&mut self, mut x: usize) -> usize {
        while self.0[x] != x {
            self.0[x] = self.0[self.0[x]];
            x = self.0[x];
        }
        x
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0[a.max(b)] = a.min(b);
        }
    }
}

fn same_aspect(a: &DupItem, b: &DupItem) -> bool {
    match (a.width, a.height, b.width, b.height) {
        (Some(aw), Some(ah), Some(bw), Some(bh)) if aw > 0 && ah > 0 && bw > 0 && bh > 0 => {
            let (x, y) = ((aw * bh) as f64, (ah * bw) as f64);
            (x - y).abs() <= DUP_ASPECT_TOLERANCE * x.max(y)
        }
        _ => false,
    }
}

/// Is `(a, b)` a near-duplicate pair (see the module docs)?
pub fn near_pair(a: &DupItem, b: &DupItem, p: &DupParams) -> bool {
    let (Some(x), Some(y)) = (a.phash, b.phash) else {
        return false;
    };
    (!a.has_capture_time || !b.has_capture_time)
        && (x ^ y).count_ones() <= p.max_bits
        && same_aspect(a, b)
}

/// Finds every duplicate group among `items`.
pub fn find_duplicates(items: &[DupItem], p: &DupParams) -> Vec<DupAssign> {
    let n = items.len();
    let mut dsu = Dsu((0..n).collect());
    let mut exact = vec![false; n];

    // exact: same content key and size
    let mut by_key: HashMap<(&str, i64), usize> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        let (Some(k), Some(sz)) = (it.content_key.as_deref(), it.file_size) else {
            continue;
        };
        match by_key.get(&(k, sz)) {
            Some(&j) => {
                dsu.union(i, j);
                exact[i] = true;
                exact[j] = true;
            }
            None => {
                by_key.insert((k, sz), i);
            }
        }
    }

    // near: pigeonhole index. Two hashes within `max_bits` differing bits agree exactly on at
    // least one of `max_bits + 1` chunks, so only photos sharing a chunk value are compared, and
    // only against the photos without a capture time (the probes).
    let chunks = (p.max_bits as usize + 1).min(64);
    let bounds: Vec<(u32, u32)> = (0..chunks)
        .map(|c| ((c * 64 / chunks) as u32, ((c + 1) * 64 / chunks) as u32))
        .collect();
    let chunk_of = |h: u64, c: usize| -> u64 {
        let (lo, hi) = bounds[c];
        let width = hi - lo;
        let v = h >> lo;
        if width >= 64 {
            v
        } else {
            v & ((1u64 << width) - 1)
        }
    };
    let mut index: HashMap<(usize, u64), Vec<usize>> = HashMap::new();
    for (i, it) in items.iter().enumerate() {
        if let Some(h) = it.phash {
            for c in 0..chunks {
                index.entry((c, chunk_of(h, c))).or_default().push(i);
            }
        }
    }
    for (i, it) in items.iter().enumerate() {
        let Some(h) = it.phash else { continue };
        if it.has_capture_time {
            continue;
        }
        for c in 0..chunks {
            for &j in index.get(&(c, chunk_of(h, c))).into_iter().flatten() {
                if j != i && near_pair(it, &items[j], p) {
                    dsu.union(i, j);
                }
            }
        }
    }

    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = dsu.find(i);
        members.entry(r).or_default().push(i);
    }
    let mut out = Vec::new();
    for idxs in members.into_values().filter(|m| m.len() > 1) {
        let gid = idxs.iter().map(|&i| items[i].id).min().unwrap();
        for i in idxs {
            out.push(DupAssign {
                photo_id: items[i].id,
                group_id: gid,
                kind: if exact[i] {
                    DupKind::Exact
                } else {
                    DupKind::Near
                },
            });
        }
    }
    out.sort_by_key(|a| a.photo_id);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: u64 = 0x0F0F_3C3C_A5A5_5A5A;

    fn item(id: i64) -> DupItem {
        DupItem {
            id,
            content_key: None,
            file_size: None,
            phash: None,
            width: Some(6000),
            height: Some(4000),
            has_capture_time: true,
        }
    }

    fn keyed(id: i64, key: &str, size: i64) -> DupItem {
        DupItem {
            content_key: Some(key.into()),
            file_size: Some(size),
            ..item(id)
        }
    }

    /// A copy without metadata: hash `h`, no capture time.
    fn stripped(id: i64, h: u64) -> DupItem {
        DupItem {
            phash: Some(h),
            has_capture_time: false,
            ..item(id)
        }
    }

    fn shot(id: i64, h: u64) -> DupItem {
        DupItem {
            phash: Some(h),
            ..item(id)
        }
    }

    fn flip(h: u64, bits: u32) -> u64 {
        // flip `bits` bits spread over the word
        (0..bits).fold(h, |acc, b| acc ^ (1u64 << (b * 21 % 64)))
    }

    fn p() -> DupParams {
        DupParams::default()
    }

    fn groups(a: &[DupAssign]) -> Vec<(i64, i64, &'static str)> {
        a.iter()
            .map(|x| (x.photo_id, x.group_id, x.kind.as_str()))
            .collect()
    }

    #[test]
    fn exact_duplicates_share_content_key_and_size() {
        let items = vec![
            keyed(5, "aa", 100),
            keyed(2, "aa", 100),
            keyed(9, "aa", 100),
            keyed(3, "bb", 100),
            keyed(4, "aa", 101), // same key, other size: not the same file
            item(7),             // unknown key
        ];
        let out = find_duplicates(&items, &p());
        assert_eq!(
            groups(&out),
            vec![(2, 2, "exact"), (5, 2, "exact"), (9, 2, "exact")]
        );
    }

    #[test]
    fn several_groups_get_their_smallest_id() {
        let items = vec![
            keyed(1, "a", 1),
            keyed(2, "b", 1),
            keyed(3, "a", 1),
            keyed(4, "b", 1),
        ];
        assert_eq!(
            groups(&find_duplicates(&items, &p())),
            vec![
                (1, 1, "exact"),
                (2, 2, "exact"),
                (3, 1, "exact"),
                (4, 2, "exact")
            ]
        );
    }

    #[test]
    fn a_stripped_copy_joins_its_original() {
        let items = vec![shot(1, H), stripped(2, flip(H, 1)), shot(3, !H)];
        assert_eq!(
            groups(&find_duplicates(&items, &p())),
            vec![(1, 1, "near"), (2, 1, "near")]
        );
    }

    #[test]
    fn near_threshold_is_inclusive_and_tight() {
        for (bits, dup) in [(0, true), (1, true), (2, true), (4, true), (5, false), (8, false)] {
            let items = vec![shot(1, H), stripped(2, flip(H, bits))];
            assert_eq!(
                !find_duplicates(&items, &p()).is_empty(),
                dup,
                "{bits} bits apart"
            );
        }
        let narrow = DupParams { max_bits: 2 };
        let items = vec![shot(1, H), stripped(2, flip(H, 3))];
        assert!(find_duplicates(&items, &narrow).is_empty());
        let wide = DupParams { max_bits: 6 };
        let items = vec![shot(1, H), stripped(2, flip(H, 6))];
        assert_eq!(find_duplicates(&items, &wide).len(), 2);
    }

    #[test]
    fn burst_frames_with_metadata_are_not_duplicates() {
        // identical hashes, both carry a capture time: two frames of a burst
        let items = vec![shot(1, H), shot(2, H)];
        assert!(find_duplicates(&items, &p()).is_empty());
    }

    #[test]
    fn different_aspect_is_not_a_duplicate() {
        let mut crop = stripped(2, H);
        crop.width = Some(4000);
        crop.height = Some(4000);
        assert!(find_duplicates(&[shot(1, H), crop], &p()).is_empty());
        // a resize keeps the aspect (within 1 %)
        let mut small = stripped(2, H);
        small.width = Some(1500);
        small.height = Some(1000);
        assert_eq!(find_duplicates(&[shot(1, H), small.clone()], &p()).len(), 2);
        small.height = Some(1020);
        assert!(find_duplicates(&[shot(1, H), small], &p()).is_empty());
        // unknown size cannot be compared
        let mut unknown = stripped(2, H);
        unknown.width = None;
        assert!(find_duplicates(&[shot(1, H), unknown], &p()).is_empty());
    }

    #[test]
    fn exact_and_near_join_into_one_group() {
        // 1 and 3 are byte-identical; 2 is a stripped re-encode of them
        let mut a = keyed(1, "k", 10);
        a.phash = Some(H);
        let mut c = keyed(3, "k", 10);
        c.phash = Some(H);
        let items = vec![a, stripped(2, flip(H, 2)), c];
        assert_eq!(
            groups(&find_duplicates(&items, &p())),
            vec![(1, 1, "exact"), (2, 1, "near"), (3, 1, "exact")]
        );
    }

    #[test]
    fn near_matching_finds_every_chunk_layout() {
        // the pigeonhole index (5 chunks for 4 bits) loses no pair: try a few hundred spread-out
        // flips of exactly `max_bits` bits (found) and of one more (never found)
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        let mut next = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        for round in 0..300 {
            for (bits, dup) in [(4usize, true), (5, false)] {
                let mut mask = 0u64;
                while (mask.count_ones() as usize) < bits {
                    mask |= 1u64 << (next() % 64);
                }
                let items = vec![shot(1, H), stripped(2, H ^ mask)];
                assert_eq!(
                    !find_duplicates(&items, &p()).is_empty(),
                    dup,
                    "round {round}: mask {mask:#018x}"
                );
            }
        }
    }

    #[test]
    fn unrelated_photos_without_metadata_stay_apart() {
        let items = vec![
            stripped(1, H),
            stripped(2, !H),
            stripped(3, H.rotate_left(17)),
        ];
        assert!(find_duplicates(&items, &p()).is_empty());
    }

    /// Smooth synthetic picture with edges, no personal photo.
    fn synthetic(w: u32, h: u32, seed: u32) -> image::RgbImage {
        image::RgbImage::from_fn(w, h, |x, y| {
            let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
            let s = seed as f32;
            let l = 128.0
                + 70.0 * ((fx * (4.0 + s)).sin() * (fy * (3.0 + s * 0.5) + 0.3).cos())
                + if (fx - 0.5).powi(2) + (fy - 0.45).powi(2) < 0.05 {
                    40.0
                } else {
                    0.0
                };
            let l = l.clamp(0.0, 255.0) as u8;
            image::Rgb([l, (l as u16 * 3 / 4) as u8 + 20, 255 - l])
        })
    }

    fn phash_of(img: &image::RgbImage) -> u64 {
        ip_lite::phash_u64(img.as_raw(), img.width() as usize, img.height() as usize)
    }

    fn reencode(img: &image::RgbImage, quality: u8) -> image::RgbImage {
        let mut buf = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality);
        img.write_with_encoder(enc).unwrap();
        image::load_from_memory_with_format(&buf, image::ImageFormat::Jpeg)
            .unwrap()
            .to_rgb8()
    }

    #[test]
    fn jpeg_reencodes_stay_within_the_threshold() {
        for seed in 1..=24 {
            let img = synthetic(480, 320, seed);
            let h0 = phash_of(&img);
            for q in [95u8, 90] {
                let d = (h0 ^ phash_of(&reencode(&img, q))).count_ones();
                assert!(d <= DUP_PHASH_MAX_BITS, "seed {seed} q{q}: {d} bits");
            }
        }
    }

    #[test]
    fn a_different_picture_is_far_away() {
        let checker = image::RgbImage::from_fn(480, 320, |x, y| {
            let v = if (x / 40 + y / 40) % 2 == 0 { 220 } else { 30 };
            image::Rgb([v, v, v])
        });
        let hc = phash_of(&checker);
        for seed in 1..=24 {
            let d = (hc ^ phash_of(&synthetic(480, 320, seed))).count_ones();
            assert!(d > 2 * DUP_PHASH_MAX_BITS, "seed {seed}: {d} bits");
        }
    }
}
