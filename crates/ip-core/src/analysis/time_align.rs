//! Multi-device capture-time alignment (docs/09 P1-2, PRD F2.11, docs/03 §2.5).
//!
//! Photos of several cameras and phones in one session carry clocks that differ by seconds to
//! hours, which breaks burst grouping and events. The model is one constant clock offset per
//! device and session: `corrected = taken_at + offset`. The EXIF capture time itself is never
//! changed (and no file is written); the offset is applied when a session is grouped.
//!
//! - [`estimate`] is pure: it measures each device against a reference from
//!   (1) the GPS fix time (UTC truth) against the EXIF time, where the file has both, and
//!   (2) visual co-occurrence: mutual best matches between two devices by image embedding (or
//!   perceptual hash), whose time differences pile up at the offset while chance matches of
//!   similar-looking scenes scatter over the whole +-24 h range. A weak peak is rejected.
//!   Support is counted in independent *moments* (matches within a few minutes are one moment):
//!   a burst of one device matched frame by frame to a burst of another is one coincidence, not
//!   five. A rival peak at another offset (a place shot again twelve hours or a day later)
//!   lowers the confidence, because the matches cannot tell the two apart.
//! - the rest of the module stores estimates and applied offsets (`session_device_offset`) and
//!   the `Core` API behind `GET/PUT /api/sessions/{id}/devices/offsets`.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};

use super::grouping::hamming;
use super::store;
use super::vecs::{cosine, decode_f16};
use crate::catalog::{self, now_ms};
use crate::device::DeviceKind;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

// ============================================================================ estimator

/// One photo as the estimator sees it.
#[derive(Debug, Clone)]
pub struct AlignPhoto {
    pub device: i64,
    /// `photo.taken_at` (ms).
    pub t: i64,
    /// GPS fix time minus `t` (ms), only where `t` is real UTC (the file has an UTC offset tag).
    pub gps_delta: Option<i64>,
    pub phash: Option<u64>,
    pub emb: Option<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct AlignParams {
    /// Largest offset looked for (+-).
    pub max_offset_ms: i64,
    /// Embedding cosine at or above which two photos of different devices show the same scene.
    pub cos_min: f32,
    /// pHash distance (bits) at or below which they do, when an embedding is missing.
    pub hamming_max: u32,
    /// Half width of the window that finds the densest cluster of pair differences.
    pub coarse_ms: i64,
    /// Half width of the cluster that is counted as support of an offset.
    pub fine_ms: i64,
    /// Fewer matching pairs than this at the peak: no estimate.
    pub min_support: usize,
    /// Matches whose capture times (on the first device) fall in one window of this width are one
    /// independent moment: a burst matched against a burst counts once.
    pub moment_ms: i64,
    /// Fewer independent moments than this at the peak: no estimate.
    pub min_moments: usize,
    /// Photos per device used for matching (an even sample above this).
    pub max_per_device: usize,
    /// Photos with GPS time needed to anchor a device on it.
    pub gps_min: usize,
    /// GPS differences must agree within this (ms) to anchor.
    pub gps_spread_ms: i64,
}

impl Default for AlignParams {
    fn default() -> Self {
        Self {
            max_offset_ms: 24 * 3_600_000,
            cos_min: 0.90,
            hamming_max: 8,
            coarse_ms: 60_000,
            fine_ms: 10_000,
            min_support: 3,
            moment_ms: 5 * 60_000,
            min_moments: 2,
            max_per_device: 1_500,
            gps_min: 3,
            gps_spread_ms: 5_000,
        }
    }
}

/// How an offset was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EstSource {
    /// Against the GPS fix time (UTC).
    Gps,
    /// Visual co-occurrence with an already aligned device.
    Visual,
    /// The device the others are aligned to (offset 0).
    Reference,
}

impl EstSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gps => "gps",
            Self::Visual => "visual",
            Self::Reference => "reference",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "gps" => Self::Gps,
            "visual" => Self::Visual,
            "reference" => Self::Reference,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceEstimate {
    pub device_id: i64,
    /// Add to `taken_at` to get the common time.
    pub offset_ms: i64,
    /// 0..1.
    pub confidence: f32,
    /// Matching pairs (visual) or photos with a GPS time (gps) behind the offset.
    pub evidence: u32,
    pub source: EstSource,
}

/// Confidence from which "accept all" takes an estimate.
pub const ACCEPT_MIN_CONFIDENCE: f32 = 0.5;

fn median(v: &mut [i64]) -> i64 {
    v.sort_unstable();
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        // not (a + b) / 2: cannot overflow, and rounds towards the lower half
        v[n / 2 - 1] + (v[n / 2] - v[n / 2 - 1]) / 2
    }
}

/// An even sample of at most `max` photos, in time order.
fn thin(mut v: Vec<&AlignPhoto>, max: usize) -> Vec<&AlignPhoto> {
    v.sort_by_key(|p| p.t);
    if v.len() <= max || max == 0 {
        return v;
    }
    let n = v.len();
    (0..max).map(|i| v[i * n / max]).collect()
}

fn similarity(x: &AlignPhoto, y: &AlignPhoto, p: &AlignParams) -> Option<f32> {
    if let (Some(a), Some(b)) = (&x.emb, &y.emb) {
        if a.len() == b.len() && !a.is_empty() {
            let c = cosine(a, b);
            return (c >= p.cos_min).then_some(c);
        }
    }
    let (a, b) = (x.phash?, y.phash?);
    let h = hamming(a, b);
    (h <= p.hamming_max).then(|| 1.0 - h as f32 / 64.0)
}

/// Offset of device `b` relative to `a` found by matching, with its support and confidence.
#[derive(Debug, Clone, Copy)]
struct PairEst {
    /// `o_b = o_a + delta`.
    delta: i64,
    support: usize,
    confidence: f32,
}

/// One mutual best match: the capture time on device `a` and the difference `a.t - b.t`.
#[derive(Debug, Clone, Copy)]
struct Match {
    t: i64,
    delta: i64,
}

/// The mutual best visual matches between two devices.
fn pair_deltas(a: &[&AlignPhoto], b: &[&AlignPhoto], p: &AlignParams) -> Vec<Match> {
    let mut best_a: Vec<Option<(f32, usize)>> = vec![None; a.len()];
    let mut best_b: Vec<Option<(f32, usize)>> = vec![None; b.len()];
    for (i, x) in a.iter().enumerate() {
        for (j, y) in b.iter().enumerate() {
            if (x.t - y.t).abs() > p.max_offset_ms {
                continue;
            }
            let Some(s) = similarity(x, y, p) else {
                continue;
            };
            if best_a[i].is_none_or(|(bs, _)| s > bs) {
                best_a[i] = Some((s, j));
            }
            if best_b[j].is_none_or(|(bs, _)| s > bs) {
                best_b[j] = Some((s, i));
            }
        }
    }
    best_a
        .iter()
        .enumerate()
        .filter_map(|(i, m)| {
            let (_, j) = (*m)?;
            (best_b[j]?.1 == i).then(|| Match {
                t: a[i].t,
                delta: a[i].t - b[j].t,
            })
        })
        .collect()
}

/// Independent moments among capture times: greedy windows of `width` ms over the sorted times.
fn moments(mut times: Vec<i64>, width: i64) -> usize {
    times.sort_unstable();
    let mut count = 0;
    let mut open: Option<i64> = None;
    for t in times {
        if open.is_none_or(|start| t - start > width) {
            open = Some(t);
            count += 1;
        }
    }
    count
}

/// The most independent moments in one cluster of `fine_ms` half width (centred on a match).
fn densest_moments(matches: &[Match], p: &AlignParams) -> usize {
    matches
        .iter()
        .map(|m| {
            moments(
                matches
                    .iter()
                    .filter(|x| (x.delta - m.delta).abs() <= p.fine_ms)
                    .map(|x| x.t)
                    .collect(),
                p.moment_ms,
            )
        })
        .max()
        .unwrap_or(0)
}

/// The densest cluster of match differences and the offset in its centre; `None` when it is too
/// weak to tell from chance matches.
///
/// Confidence = significance x size x share x uniqueness, all counted in independent moments:
/// - significance: how many of the matched moments would fall in the cluster by chance (chance
///   matches spread evenly over +-max_offset), against the `k` moments it holds;
/// - size `k / (k + 3)`: three agreeing moments are half the way;
/// - share: the cluster's part of all matched moments, as `0.4 + 0.6 x share`;
/// - uniqueness `1 - k2 / k`: `k2` moments at the strongest other offset (outside the coarse
///   window), e.g. a place shot again twelve hours or a day later. A tie gives 0.
fn find_peak(mut matches: Vec<Match>, p: &AlignParams) -> Option<PairEst> {
    let n = matches.len();
    if n < p.min_support {
        return None;
    }
    matches.sort_unstable_by_key(|m| m.delta);
    let deltas: Vec<i64> = matches.iter().map(|m| m.delta).collect();
    // coarse: the window of 2 * coarse_ms with the most differences
    let (mut best, mut best_lo, mut hi) = (0usize, 0usize, 0usize);
    for lo in 0..n {
        while hi < n && deltas[hi] - deltas[lo] <= 2 * p.coarse_ms {
            hi += 1;
        }
        if hi - lo > best {
            best = hi - lo;
            best_lo = lo;
        }
    }
    let mut window: Vec<i64> = deltas[best_lo..best_lo + best].to_vec();
    let mut centre = median(&mut window);
    // fine: re-centre on the differences close to the centre
    for _ in 0..4 {
        let mut near: Vec<i64> = deltas
            .iter()
            .copied()
            .filter(|d| (d - centre).abs() <= p.fine_ms)
            .collect();
        if near.is_empty() {
            return None;
        }
        let c = median(&mut near);
        if c == centre {
            break;
        }
        centre = c;
    }
    let inside: Vec<i64> = matches
        .iter()
        .filter(|m| (m.delta - centre).abs() <= p.fine_ms)
        .map(|m| m.t)
        .collect();
    let support = inside.len();
    let k = moments(inside, p.moment_ms);
    if support < p.min_support || k < p.min_moments {
        return None;
    }
    let total = moments(matches.iter().map(|m| m.t).collect(), p.moment_ms);
    let chance = total as f64 * (2 * p.fine_ms) as f64 / (2 * p.max_offset_ms) as f64;
    let significance = (1.0 - chance / k as f64).clamp(0.0, 1.0);
    if significance < 0.5 {
        return None;
    }
    let rivals: Vec<Match> = matches
        .iter()
        .copied()
        .filter(|m| (m.delta - centre).abs() > 2 * p.coarse_ms)
        .collect();
    let k2 = densest_moments(&rivals, p);
    let uniqueness = (1.0 - k2 as f64 / k as f64).clamp(0.0, 1.0);
    let share = (k as f64 / total as f64).min(1.0);
    let size = k as f64 / (k as f64 + 3.0);
    let confidence = significance * size * (0.4 + 0.6 * share) * uniqueness;
    Some(PairEst {
        delta: centre,
        support,
        confidence: confidence as f32,
    })
}

/// Anchors a device on the GPS fix time: median of `gps - exif` over its photos.
fn gps_anchor(photos: &[&AlignPhoto], p: &AlignParams) -> Option<(i64, f32, u32)> {
    let mut deltas: Vec<i64> = photos.iter().filter_map(|x| x.gps_delta).collect();
    if deltas.len() < p.gps_min {
        return None;
    }
    let m = median(&mut deltas);
    let inliers = deltas
        .iter()
        .filter(|d| (**d - m).abs() <= p.gps_spread_ms)
        .count();
    // a clock offset is constant: most photos must agree
    if inliers < p.gps_min || inliers * 4 < deltas.len() * 3 {
        return None;
    }
    let mut near: Vec<i64> = deltas
        .iter()
        .copied()
        .filter(|d| (*d - m).abs() <= p.gps_spread_ms)
        .collect();
    let offset = median(&mut near);
    let frac = inliers as f32 / deltas.len() as f32;
    let conf = frac * (inliers as f32 / (inliers as f32 + 2.0));
    Some((offset, conf, inliers as u32))
}

/// One clock offset per device of a session. Devices anchored on GPS time are absolute (their
/// corrected time is UTC); the others are aligned to them by visual co-occurrence, chained
/// through devices that overlap (A-B, B-C gives C relative to A). Without any GPS anchor the
/// device with most photos is the reference (offset 0). Devices that cannot be tied to the rest
/// are left out.
pub fn estimate(photos: &[AlignPhoto], p: &AlignParams) -> Vec<DeviceEstimate> {
    let mut by_dev: BTreeMap<i64, Vec<&AlignPhoto>> = BTreeMap::new();
    for ph in photos {
        by_dev.entry(ph.device).or_default().push(ph);
    }
    let mut est: BTreeMap<i64, DeviceEstimate> = BTreeMap::new();
    for (dev, ph) in &by_dev {
        if let Some((offset_ms, confidence, evidence)) = gps_anchor(ph, p) {
            est.insert(
                *dev,
                DeviceEstimate {
                    device_id: *dev,
                    offset_ms,
                    confidence,
                    evidence,
                    source: EstSource::Gps,
                },
            );
        }
    }
    if est.is_empty() {
        // most photos, then the lowest id
        let Some((&dev, _)) = by_dev
            .iter()
            .max_by_key(|(id, v)| (v.len(), std::cmp::Reverse(**id)))
        else {
            return Vec::new();
        };
        est.insert(
            dev,
            DeviceEstimate {
                device_id: dev,
                offset_ms: 0,
                confidence: 1.0,
                evidence: 0,
                source: EstSource::Reference,
            },
        );
    }
    let thinned: BTreeMap<i64, Vec<&AlignPhoto>> = by_dev
        .iter()
        .map(|(d, v)| (*d, thin(v.clone(), p.max_per_device)))
        .collect();
    // pair estimates, computed once per unordered pair (low id first)
    let mut pairs: HashMap<(i64, i64), Option<PairEst>> = HashMap::new();
    let mut pair = |x: i64, y: i64| -> Option<PairEst> {
        let (lo, hi) = (x.min(y), x.max(y));
        let e = (*pairs
            .entry((lo, hi))
            .or_insert_with(|| find_peak(pair_deltas(&thinned[&lo], &thinned[&hi], p), p)))?;
        // `delta` runs from lo to hi
        Some(if x == lo {
            e
        } else {
            PairEst {
                delta: -e.delta,
                ..e
            }
        })
    };
    loop {
        let mut best: Option<(f32, i64, i64, PairEst)> = None; // (confidence, known, new, edge)
        for &y in by_dev.keys().filter(|d| !est.contains_key(d)) {
            for (&x, ex) in &est {
                let Some(e) = pair(x, y) else { continue };
                let conf = e.confidence.min(ex.confidence);
                if best.as_ref().is_none_or(|b| conf > b.0) {
                    best = Some((conf, x, y, e));
                }
            }
        }
        let Some((confidence, x, y, e)) = best else {
            break;
        };
        let base = est[&x].offset_ms;
        est.insert(
            y,
            DeviceEstimate {
                device_id: y,
                offset_ms: base + e.delta,
                confidence,
                evidence: e.support as u32,
                source: EstSource::Visual,
            },
        );
    }
    est.into_values().collect()
}

// ============================================================================ storage

/// SQL for the capture time of photo `p` used for grouping, events and burst order: `taken_at`
/// shifted by the clock offset of its device in session `sess` (an SQL expression); a photo
/// without EXIF time keeps its file time (the file system clock is not the camera's).
pub fn corrected_time_sql(p: &str, sess: &str) -> String {
    format!(
        "COALESCE({p}.taken_at + COALESCE((SELECT o.offset_ms FROM session_device_offset o \
         WHERE o.session_id={sess} AND o.device_id={p}.device_id), 0), {p}.mtime)"
    )
}

/// What the estimator reads for a session.
pub fn load_align_photos(conn: &Connection, session_id: i64) -> Result<Vec<AlignPhoto>> {
    let mut st = conn.prepare(
        "SELECT p.device_id, p.taken_at,
                CASE WHEN p.gps_time_ms IS NOT NULL AND p.taken_at_offset_min IS NOT NULL
                     THEN p.gps_time_ms - p.taken_at END,
                a.phash, a.embedding
         FROM session_photo sp
         JOIN photo p ON p.id=sp.photo_id AND COALESCE(p.missing,0)=0
         LEFT JOIN analysis a ON a.photo_id=p.id
         WHERE sp.session_id=?1 AND p.device_id IS NOT NULL AND p.taken_at IS NOT NULL",
    )?;
    let rows = st
        .query_map([session_id], |r| {
            Ok(AlignPhoto {
                device: r.get(0)?,
                t: r.get(1)?,
                gps_delta: r.get(2)?,
                phash: store::blob_phash(r.get(3)?),
                emb: r
                    .get::<_, Option<Vec<u8>>>(4)?
                    .map(|b| decode_f16(&b))
                    .filter(|e| !e.is_empty()),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Whether an offset is applied, and by whom.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffsetSource {
    None,
    /// An accepted estimate.
    Auto,
    /// Typed in by the user.
    Manual,
}

impl OffsetSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Auto => "auto",
            Self::Manual => "manual",
        }
    }

    fn parse(s: &str) -> Self {
        match s {
            "auto" => Self::Auto,
            "manual" => Self::Manual,
            _ => Self::None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OffsetEstimate {
    pub offset_ms: i64,
    pub confidence: f32,
    pub evidence: u32,
    pub source: EstSource,
}

/// One device of `GET /api/sessions/{id}/devices/offsets`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceOffset {
    pub device_id: i64,
    pub name: String,
    pub kind: DeviceKind,
    pub photo_count: i64,
    /// Applied correction (ms added to the capture time); 0 = none.
    pub offset_ms: i64,
    pub source: OffsetSource,
    /// Latest estimate; `None` when the device could not be tied to the others.
    pub estimate: Option<OffsetEstimate>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceOffsetsOut {
    pub devices: Vec<DeviceOffset>,
    /// When the estimate was made (ms); `None` = never.
    pub estimated_at: Option<i64>,
    /// The device the estimates are relative to (no GPS anchor in the session), if any.
    pub reference_device_id: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OffsetAction {
    /// Apply the estimate of `device_ids` / `device_id`; without either every estimate with
    /// confidence >= [`ACCEPT_MIN_CONFIDENCE`].
    Accept,
    /// Apply `offset_ms` to `device_id`.
    Set,
    /// Remove the correction of `device_id` (every device when omitted).
    Reset,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OffsetsRequest {
    pub action: OffsetAction,
    #[serde(default)]
    pub device_id: Option<i64>,
    #[serde(default)]
    pub device_ids: Option<Vec<i64>>,
    #[serde(default)]
    pub offset_ms: Option<i64>,
}

/// Largest correction that can be typed in.
pub const MAX_MANUAL_OFFSET_MS: i64 = 7 * 24 * 3_600_000;

struct StoredRow {
    offset_ms: i64,
    source: OffsetSource,
    estimate: Option<OffsetEstimate>,
    est_at: Option<i64>,
}

fn stored_rows(conn: &Connection, session_id: i64) -> Result<HashMap<i64, StoredRow>> {
    let mut st = conn.prepare(
        "SELECT device_id, offset_ms, source, est_offset_ms, est_confidence, est_evidence,
                est_source, est_at
         FROM session_device_offset WHERE session_id=?1",
    )?;
    let rows = st
        .query_map([session_id], |r| {
            let est_offset: Option<i64> = r.get(3)?;
            let est_source: Option<String> = r.get(6)?;
            let estimate = match (est_offset, est_source.as_deref().and_then(EstSource::parse)) {
                (Some(offset_ms), Some(source)) => Some(OffsetEstimate {
                    offset_ms,
                    confidence: r.get::<_, Option<f64>>(4)?.unwrap_or(0.0) as f32,
                    evidence: r.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0) as u32,
                    source,
                }),
                _ => None,
            };
            Ok((
                r.get::<_, i64>(0)?,
                StoredRow {
                    offset_ms: r.get(1)?,
                    source: OffsetSource::parse(&r.get::<_, String>(2)?),
                    estimate,
                    est_at: r.get(7)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    Ok(rows)
}

/// Devices of a session with their applied offset and latest estimate (404 for an unknown
/// session).
pub fn read_offsets(conn: &Connection, session_id: i64) -> Result<DeviceOffsetsOut> {
    let devices = catalog::session_devices(conn, session_id)?;
    let mut rows = stored_rows(conn, session_id)?;
    let estimated_at = rows.values().filter_map(|r| r.est_at).max();
    let mut reference = None;
    let devices = devices
        .into_iter()
        .map(|d| {
            let row = rows.remove(&d.id);
            let (offset_ms, source, estimate) = match row {
                Some(r) => (r.offset_ms, r.source, r.estimate),
                None => (0, OffsetSource::None, None),
            };
            if estimate
                .as_ref()
                .is_some_and(|e| e.source == EstSource::Reference)
            {
                reference = Some(d.id);
            }
            DeviceOffset {
                device_id: d.id,
                name: d.name,
                kind: d.kind,
                photo_count: d.photo_count,
                offset_ms,
                source,
                estimate,
            }
        })
        .collect();
    Ok(DeviceOffsetsOut {
        devices,
        estimated_at,
        reference_device_id: reference,
    })
}

/// Replaces the stored estimates of a session (applied offsets stay). Every device of the session
/// gets a row so that "estimated, nothing found" can be told from "never estimated".
pub fn store_estimates(
    conn: &mut Connection,
    session_id: i64,
    ests: &[DeviceEstimate],
) -> Result<()> {
    let devices = catalog::session_devices(conn, session_id)?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_ms();
    tx.execute(
        "UPDATE session_device_offset SET est_offset_ms=NULL, est_confidence=NULL,
                est_evidence=NULL, est_source=NULL, est_at=NULL WHERE session_id=?1",
        [session_id],
    )?;
    for d in &devices {
        let e = ests.iter().find(|e| e.device_id == d.id);
        tx.execute(
            "INSERT INTO session_device_offset(session_id, device_id, est_offset_ms,
                    est_confidence, est_evidence, est_source, est_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(session_id, device_id) DO UPDATE SET
                est_offset_ms=excluded.est_offset_ms, est_confidence=excluded.est_confidence,
                est_evidence=excluded.est_evidence, est_source=excluded.est_source,
                est_at=excluded.est_at",
            params![
                session_id,
                d.id,
                e.map(|e| e.offset_ms),
                e.map(|e| f64::from(e.confidence)),
                e.map(|e| i64::from(e.evidence)),
                e.map(|e| e.source.as_str()),
                now
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

/// Drops the stored estimates (the photos or their analysis changed). Applied offsets stay.
pub fn clear_estimates(conn: &Connection, session_id: i64) -> Result<()> {
    conn.execute(
        "UPDATE session_device_offset SET est_offset_ms=NULL, est_confidence=NULL,
                est_evidence=NULL, est_source=NULL, est_at=NULL WHERE session_id=?1",
        [session_id],
    )?;
    Ok(())
}

/// Applies `(device, offset, source)` changes; returns whether any applied offset differs from
/// before.
fn apply_changes(
    conn: &mut Connection,
    session_id: i64,
    changes: &[(i64, i64, OffsetSource)],
) -> Result<bool> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let now = now_ms();
    let mut changed = false;
    for &(device_id, offset_ms, source) in changes {
        let before: i64 = tx
            .query_row(
                "SELECT offset_ms FROM session_device_offset WHERE session_id=?1 AND device_id=?2",
                params![session_id, device_id],
                |r| r.get(0),
            )
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(0),
                e => Err(e),
            })?;
        changed |= before != offset_ms;
        tx.execute(
            "INSERT INTO session_device_offset(session_id, device_id, offset_ms, source, updated_at)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(session_id, device_id) DO UPDATE SET
                offset_ms=excluded.offset_ms, source=excluded.source,
                updated_at=excluded.updated_at",
            params![session_id, device_id, offset_ms, source.as_str(), now],
        )?;
    }
    tx.commit()?;
    Ok(changed)
}

/// The changes a request asks for, validated against the current state.
fn plan_changes(
    cur: &DeviceOffsetsOut,
    req: &OffsetsRequest,
) -> Result<Vec<(i64, i64, OffsetSource)>> {
    let known = |id: i64| -> Result<&DeviceOffset> {
        cur.devices
            .iter()
            .find(|d| d.device_id == id)
            .ok_or_else(|| {
                CoreError::not_found(format!("device {id} has no photos in the session"))
            })
    };
    let mut ids: Vec<i64> = req.device_ids.clone().unwrap_or_default();
    ids.extend(req.device_id);
    ids.sort_unstable();
    ids.dedup();
    match req.action {
        OffsetAction::Set => {
            let (Some(id), Some(ms)) = (req.device_id, req.offset_ms) else {
                return Err(CoreError::bad_request("set needs device_id and offset_ms"));
            };
            known(id)?;
            if ms.abs() > MAX_MANUAL_OFFSET_MS {
                return Err(CoreError::bad_request("offset_ms must be within +-7 days"));
            }
            let source = if ms == 0 {
                OffsetSource::None
            } else {
                OffsetSource::Manual
            };
            Ok(vec![(id, ms, source)])
        }
        OffsetAction::Reset => {
            for id in &ids {
                known(*id)?;
            }
            if ids.is_empty() {
                ids = cur.devices.iter().map(|d| d.device_id).collect();
            }
            Ok(ids
                .into_iter()
                .map(|id| (id, 0, OffsetSource::None))
                .collect())
        }
        OffsetAction::Accept => {
            let explicit = !ids.is_empty();
            let mut out = Vec::new();
            if explicit {
                for id in &ids {
                    let d = known(*id)?;
                    let e = d.estimate.as_ref().ok_or_else(|| {
                        CoreError::bad_request(format!("device {id} has no estimate to accept"))
                    })?;
                    out.push((*id, e.offset_ms, OffsetSource::Auto));
                }
            } else {
                for d in &cur.devices {
                    let Some(e) = &d.estimate else { continue };
                    if e.confidence >= ACCEPT_MIN_CONFIDENCE && e.source != EstSource::Reference {
                        out.push((d.device_id, e.offset_ms, OffsetSource::Auto));
                    }
                }
                if out.is_empty() {
                    return Err(CoreError::bad_request(
                        "no estimate with enough confidence to accept",
                    ));
                }
            }
            Ok(out)
        }
    }
}

// ============================================================================ Core API

impl Core {
    /// `GET /api/sessions/{id}/devices/offsets`: applied offsets and the estimate per device.
    /// The estimate is made on the first call after an analysis (or with `refresh`) and kept.
    pub async fn device_offsets(&self, session_id: i64, refresh: bool) -> Result<DeviceOffsetsOut> {
        let cur = self.db.call(move |c| read_offsets(c, session_id)).await?;
        if !refresh && cur.estimated_at.is_some() {
            return Ok(cur);
        }
        let photos = self
            .db
            .call(move |c| load_align_photos(c, session_id))
            .await?;
        let ests = tokio::task::spawn_blocking(move || estimate(&photos, &AlignParams::default()))
            .await
            .map_err(|e| CoreError::Internal(e.into()))?;
        self.db
            .call(move |c| {
                store_estimates(c, session_id, &ests)?;
                read_offsets(c, session_id)
            })
            .await
    }

    /// `PUT /api/sessions/{id}/devices/offsets`: accept an estimate, type an offset or reset.
    /// When an applied offset changes the session is grouped again (manual bursts are kept).
    pub async fn update_device_offsets(
        &self,
        session_id: i64,
        req: OffsetsRequest,
    ) -> Result<DeviceOffsetsOut> {
        let cur = if req.action == OffsetAction::Accept {
            self.device_offsets(session_id, false).await?
        } else {
            self.db.call(move |c| read_offsets(c, session_id)).await?
        };
        let changes = plan_changes(&cur, &req)?;
        let changed = self
            .db
            .call(move |c| apply_changes(c, session_id, &changes))
            .await?;
        if changed {
            self.regroup_for_offsets(session_id).await?;
        }
        self.db.call(move |c| read_offsets(c, session_id)).await
    }

    /// Bursts, scenes, events, face tracks and scores of the session from the corrected times.
    async fn regroup_for_offsets(&self, session_id: i64) -> Result<()> {
        let params = self.settings().group_params();
        self.db
            .call(move |c| {
                let burst_ids = store::regroup_session(c, session_id, &params)?;
                store::cluster_session(c, session_id)?;
                store::rescore_bursts(c, &burst_ids)?;
                Ok(())
            })
            .await?;
        self.events.emit(Event::GroupsUpdated { session_id });
        self.events.emit(Event::PeopleUpdated { session_id });
        Ok(())
    }
}

// ============================================================================ tests

#[cfg(test)]
mod tests {
    use super::*;

    /// xorshift64*: deterministic noise without a dependency.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
        }

        /// Uniform in `0..n`.
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }

        /// Uniform in `-r..=r`.
        fn jitter(&mut self, r: i64) -> i64 {
            self.below(2 * r as u64 + 1) as i64 - r
        }

        fn unit(&mut self) -> f32 {
            (self.next() >> 40) as f32 / (1u64 << 24) as f32
        }
    }

    const DIM: usize = 24;
    const S: i64 = 1_000;
    const MIN: i64 = 60 * S;
    const H: i64 = 60 * MIN;
    const T0: i64 = 1_700_000_000_000;

    fn scene_vec(rng: &mut Rng) -> Vec<f32> {
        let v: Vec<f32> = (0..DIM).map(|_| rng.unit() - 0.5).collect();
        norm(v)
    }

    fn norm(mut v: Vec<f32>) -> Vec<f32> {
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= n);
        v
    }

    /// The same scene seen by another camera: a little noise.
    fn view(rng: &mut Rng, scene: &[f32]) -> Vec<f32> {
        norm(
            scene
                .iter()
                .map(|x| x + (rng.unit() - 0.5) * 0.04)
                .collect(),
        )
    }

    fn photo(device: i64, t: i64, emb: Vec<f32>) -> AlignPhoto {
        AlignPhoto {
            device,
            t,
            gps_delta: None,
            phash: None,
            emb: Some(emb),
        }
    }

    /// A day of shooting: `n` scenes 2-12 minutes apart. Each device in `devs` is
    /// `(id, clock error ms, share of the scenes it shot)`; a device sees a scene at the true time
    /// plus its clock error plus up to 2 s of jitter. Every device also has solo shots that
    /// nobody else took, and one in five of its shots is a "landmark" photo that looks like a
    /// scene of another time (chance matches).
    fn session(devs: &[(i64, i64, f64)], n: usize, seed: u64) -> Vec<AlignPhoto> {
        let mut rng = Rng(seed);
        let mut out = Vec::new();
        let mut t = T0;
        let scenes: Vec<(i64, Vec<f32>)> = (0..n)
            .map(|_| {
                t += (2 + rng.below(10) as i64) * MIN;
                (t, scene_vec(&mut rng))
            })
            .collect();
        for &(id, err, share) in devs {
            for (st, sv) in &scenes {
                if (rng.below(1000) as f64) < share * 1000.0 {
                    out.push(photo(id, st + err + rng.jitter(2 * S), view(&mut rng, sv)));
                }
                if rng.below(5) == 0 {
                    // a landmark that looks like another scene, shot at a random time
                    let (_, other) = &scenes[rng.below(n as u64) as usize];
                    let at = T0 + rng.below(10 * H as u64) as i64;
                    out.push(photo(id, at, view(&mut rng, other)));
                }
                if rng.below(3) == 0 {
                    let at = st + rng.jitter(3 * MIN);
                    out.push(photo(id, at + err, scene_vec(&mut rng)));
                }
            }
        }
        out
    }

    fn off(ests: &[DeviceEstimate], id: i64) -> Option<&DeviceEstimate> {
        ests.iter().find(|e| e.device_id == id)
    }

    /// Every device of `errs` (`(id, clock error)`) is aligned to the reference the estimator
    /// chose: `corrected = recorded + offset` must agree with the reference clock within 5 s.
    fn check_all(ests: &[DeviceEstimate], errs: &[(i64, i64)], what: &str) {
        let r = ests
            .iter()
            .find(|e| e.source == EstSource::Reference)
            .unwrap_or_else(|| panic!("{what}: no reference in {ests:?}"));
        let ref_err = errs.iter().find(|(id, _)| *id == r.device_id).unwrap().1;
        for (id, err) in errs.iter().filter(|(id, _)| *id != r.device_id) {
            let e = off(ests, *id).unwrap_or_else(|| panic!("{what}: no estimate for {id}"));
            assert_close(e, ref_err - err, &format!("{what}: device {id}"));
        }
    }

    fn assert_close(e: &DeviceEstimate, want_ms: i64, what: &str) {
        let err = (e.offset_ms - want_ms).abs();
        assert!(
            err <= 5 * S,
            "{what}: offset {} want {want_ms} err {err} ms",
            e.offset_ms
        );
    }

    #[test]
    fn two_devices_offset_of_an_hour() {
        // device 2's clock runs 1 h 2 min 5 s fast: corrected = taken - that
        let err = H + 2 * MIN + 5 * S;
        let ph = session(&[(1, 0, 1.0), (2, err, 0.7)], 80, 7);
        let ests = estimate(&ph, &AlignParams::default());
        assert_eq!(ests.len(), 2);
        let r = off(&ests, 1).unwrap();
        assert_eq!((r.offset_ms, r.source), (0, EstSource::Reference));
        let e = off(&ests, 2).unwrap();
        assert_close(e, -err, "device 2");
        assert_eq!(e.source, EstSource::Visual);
        assert!(e.confidence >= ACCEPT_MIN_CONFIDENCE, "{e:?}");
        assert!(e.evidence >= 20, "{e:?}");
    }

    #[test]
    fn offsets_from_seconds_to_hours_both_signs() {
        for (i, err) in [
            42 * S,
            -90 * S,
            7 * MIN + 13 * S,
            -(3 * H + 11 * MIN),
            11 * H + 59 * MIN,
            -(23 * H) + 17 * S,
        ]
        .into_iter()
        .enumerate()
        {
            let ph = session(&[(5, 0, 0.9), (9, err, 0.7)], 70, 100 + i as u64);
            let ests = estimate(&ph, &AlignParams::default());
            check_all(&ests, &[(5, 0), (9, err)], &format!("err {err}"));
        }
    }

    #[test]
    fn three_devices_chain_through_the_middle_one() {
        // B overlaps both; A and C hardly overlap (C shoots only the scenes B shot)
        let (ea, eb, ec) = (0, -2 * H + 13 * S, 95 * S);
        let mut rng = Rng(3);
        let mut ph = Vec::new();
        let mut t = T0;
        for i in 0..120 {
            t += (2 + rng.below(8) as i64) * MIN;
            let v = scene_vec(&mut rng);
            // first third: A+B, last third: B+C, middle: B alone
            if i < 50 {
                ph.push(photo(1, t + ea + rng.jitter(2 * S), view(&mut rng, &v)));
                ph.push(photo(1, t + ea + 20 * S, scene_vec(&mut rng)));
            }
            ph.push(photo(2, t + eb + rng.jitter(2 * S), view(&mut rng, &v)));
            ph.push(photo(2, t + eb + 25 * S, scene_vec(&mut rng)));
            if i >= 70 {
                ph.push(photo(3, t + ec + rng.jitter(2 * S), view(&mut rng, &v)));
            }
        }
        // B has the most photos so it is the reference; A and C are measured against it
        let ests = estimate(&ph, &AlignParams::default());
        assert_eq!(off(&ests, 2).unwrap().source, EstSource::Reference);
        check_all(&ests, &[(1, ea), (2, eb), (3, ec)], "three devices");
    }

    #[test]
    fn chained_device_without_direct_overlap() {
        // A (most photos, reference) overlaps B only in the first half, C overlaps B only in the
        // second half: C has no direct match with A
        let (eb, ec) = (4 * MIN + 30 * S, -3 * H);
        let mut rng = Rng(11);
        let mut ph = Vec::new();
        let mut t = T0;
        for i in 0..100 {
            t += (3 + rng.below(6) as i64) * MIN;
            let v = scene_vec(&mut rng);
            ph.push(photo(1, t + rng.jitter(S), view(&mut rng, &v)));
            ph.push(photo(1, t + 30 * S, scene_vec(&mut rng)));
            if i < 50 {
                ph.push(photo(2, t + eb + rng.jitter(S), view(&mut rng, &v)));
            } else {
                let w = scene_vec(&mut rng);
                ph.push(photo(2, t + eb + rng.jitter(S), view(&mut rng, &w)));
                ph.push(photo(3, t + ec + rng.jitter(S), view(&mut rng, &w)));
            }
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert_close(off(&ests, 2).unwrap(), -eb, "B");
        // C is only tied to B: its offset is B's plus the B-C difference
        assert_close(off(&ests, 3).unwrap(), -ec, "C via B");
    }

    #[test]
    fn device_without_overlap_gets_no_estimate() {
        let mut ph = session(&[(1, 0, 0.9), (2, 5 * MIN, 0.8)], 60, 21);
        let mut rng = Rng(99);
        // device 3: its own scenes only
        for i in 0..60 {
            ph.push(photo(3, T0 + i * 4 * MIN + 17 * MIN, scene_vec(&mut rng)));
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert!(off(&ests, 3).is_none(), "{ests:?}");
        assert!(off(&ests, 2).is_some());
    }

    #[test]
    fn chance_matches_alone_are_rejected() {
        // the same scenes seen by both devices, but at unrelated times
        let mut rng = Rng(5);
        let scenes: Vec<Vec<f32>> = (0..30).map(|_| scene_vec(&mut rng)).collect();
        let mut ph = Vec::new();
        for s in &scenes {
            ph.push(photo(
                1,
                T0 + rng.below(20 * H as u64) as i64,
                view(&mut rng, s),
            ));
            ph.push(photo(
                2,
                T0 + rng.below(20 * H as u64) as i64,
                view(&mut rng, s),
            ));
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert_eq!(ests.len(), 1, "only the reference: {ests:?}");
    }

    #[test]
    fn two_matching_pairs_are_too_weak() {
        let mut rng = Rng(8);
        let mut ph = Vec::new();
        for i in 0..2 {
            let v = scene_vec(&mut rng);
            ph.push(photo(1, T0 + i * 10 * MIN, view(&mut rng, &v)));
            ph.push(photo(2, T0 + i * 10 * MIN + 600 * S, view(&mut rng, &v)));
        }
        for i in 0..20 {
            ph.push(photo(1, T0 + i * MIN, scene_vec(&mut rng)));
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert_eq!(ests.len(), 1);
    }

    #[test]
    fn gps_time_anchors_a_device_and_the_others_follow() {
        // phone (1): clock 20 min slow, GPS says so (UTC truth); camera (2): 3 min fast, no GPS
        let (e1, e2) = (-20 * MIN, 3 * MIN + 8 * S);
        let mut ph = session(&[(1, e1, 0.7), (2, e2, 0.9)], 70, 33);
        // device 2 has more photos, but device 1 is anchored on GPS
        let mut rng = Rng(4);
        for p in ph.iter_mut().filter(|p| p.device == 1) {
            p.gps_delta = Some(-e1 + rng.jitter(S));
        }
        let ests = estimate(&ph, &AlignParams::default());
        let a = off(&ests, 1).unwrap();
        assert_eq!(a.source, EstSource::Gps);
        assert_close(a, -e1, "gps device");
        // the camera is aligned to UTC through the phone: corrected = UTC
        assert_close(off(&ests, 2).unwrap(), -e2, "camera vs gps device");
        assert!(ests.iter().all(|e| e.source != EstSource::Reference));
    }

    #[test]
    fn gps_alone_is_enough_for_a_device_without_overlap() {
        let mut ph = Vec::new();
        let mut rng = Rng(77);
        for i in 0..10 {
            let mut p = photo(4, T0 + i * MIN, scene_vec(&mut rng));
            p.gps_delta = Some(95 * MIN);
            ph.push(p);
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert_eq!(ests.len(), 1);
        assert_eq!(ests[0].source, EstSource::Gps);
        assert_eq!(ests[0].offset_ms, 95 * MIN);
        assert!(ests[0].confidence >= ACCEPT_MIN_CONFIDENCE);
    }

    #[test]
    fn inconsistent_gps_deltas_do_not_anchor() {
        let mut rng = Rng(78);
        let mut ph = Vec::new();
        for i in 0..10 {
            let mut p = photo(4, T0 + i * MIN, scene_vec(&mut rng));
            // a fix that is hours old in half of the photos
            p.gps_delta = Some(if i % 2 == 0 { 2 * S } else { 3 * H + i * MIN });
            ph.push(p);
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert_eq!(ests.len(), 1);
        assert_eq!(ests[0].source, EstSource::Reference);
    }

    #[test]
    fn phash_path_without_embeddings() {
        let err = 17 * MIN + 4 * S;
        let mut rng = Rng(55);
        let mut ph = Vec::new();
        let mut t = T0;
        for _ in 0..60 {
            t += (2 + rng.below(8) as i64) * MIN;
            let h = rng.next();
            let noisy = |rng: &mut Rng| h ^ (1u64 << rng.below(64)) ^ (1u64 << rng.below(64));
            let (a, b) = (noisy(&mut rng), noisy(&mut rng));
            ph.push(AlignPhoto {
                device: 1,
                t: t + rng.jitter(2 * S),
                gps_delta: None,
                phash: Some(a),
                emb: None,
            });
            ph.push(AlignPhoto {
                device: 2,
                t: t + err + rng.jitter(2 * S),
                gps_delta: None,
                phash: Some(b),
                emb: None,
            });
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert_close(off(&ests, 2).unwrap(), -err, "phash");
    }

    #[test]
    fn same_clock_gives_about_zero() {
        let ph = session(&[(1, 0, 0.9), (2, 0, 0.9)], 60, 9);
        let ests = estimate(&ph, &AlignParams::default());
        check_all(&ests, &[(1, 0), (2, 0)], "same clock");
    }

    #[test]
    fn accuracy_over_many_random_sessions() {
        // 40 sessions with two or three devices and random offsets: every estimate within 5 s
        let mut rng = Rng(2024);
        let mut errors: Vec<i64> = Vec::new();
        for k in 0..40u64 {
            let e2 = rng.jitter(10 * H);
            let e3 = rng.jitter(10 * H);
            let three = k % 2 == 0;
            let mut devs = vec![(1, 0, 0.8), (2, e2, 0.8)];
            if three {
                devs.push((3, e3, 0.8));
            }
            let ph = session(&devs, 60, 1000 + k);
            let ests = estimate(&ph, &AlignParams::default());
            let errs: Vec<(i64, i64)> = devs.iter().map(|d| (d.0, d.1)).collect();
            let r = ests
                .iter()
                .find(|e| e.source == EstSource::Reference)
                .unwrap();
            let ref_err = errs.iter().find(|(id, _)| *id == r.device_id).unwrap().1;
            for (id, err) in errs.iter().filter(|(id, _)| *id != r.device_id) {
                let e = off(&ests, *id).unwrap_or_else(|| panic!("session {k}: no estimate"));
                errors.push((e.offset_ms - (ref_err - err)).abs());
            }
        }
        errors.sort_unstable();
        let worst = *errors.last().unwrap();
        eprintln!(
            "time_align accuracy: {} device offsets in 40 sessions, error median {} ms, p95 {} ms, max {} ms",
            errors.len(),
            errors[errors.len() / 2],
            errors[errors.len() * 95 / 100],
            worst
        );
        assert!(worst <= 5 * S, "worst error {worst} ms");
    }

    #[test]
    fn a_burst_matched_to_a_burst_is_one_coincidence() {
        // what the 1000-photo library showed: two devices never shot the same scene, but each has a
        // burst of the same person (six frames 25 s apart, the pose changing frame by frame) at
        // unrelated times. The frames pair up one to one with identical differences: six matches
        // at one offset, all from a single moment. That is one coincidence, not an offset.
        let mut rng = Rng(61);
        let mut ph = Vec::new();
        for i in 0..40 {
            ph.push(photo(1, T0 + i * 3 * MIN, scene_vec(&mut rng)));
            ph.push(photo(2, T0 + i * 4 * MIN + 20 * S, scene_vec(&mut rng)));
        }
        let (a, b) = (T0 + 2 * H, T0 + 11 * H + 34 * MIN);
        for i in 0..6 {
            let pose = scene_vec(&mut rng);
            ph.push(photo(1, a + i * 25 * S, view(&mut rng, &pose)));
            ph.push(photo(2, b + i * 25 * S, view(&mut rng, &pose)));
        }
        let ests = estimate(&ph, &AlignParams::default());
        assert!(off(&ests, 2).is_none(), "{ests:?}");
        // two such bursts at the same offset by chance would be an estimate, but a weak one
        let pose = scene_vec(&mut rng);
        ph.push(photo(1, a + 3 * H, view(&mut rng, &pose)));
        ph.push(photo(2, b + 3 * H, view(&mut rng, &pose)));
        let pose = scene_vec(&mut rng);
        ph.push(photo(1, a + 3 * H + 30 * S, view(&mut rng, &pose)));
        ph.push(photo(2, b + 3 * H + 30 * S, view(&mut rng, &pose)));
        let ests = estimate(&ph, &AlignParams::default());
        let e = off(&ests, 2).expect("two moments give an estimate");
        assert!(e.confidence < ACCEPT_MIN_CONFIDENCE, "{e:?}");
    }

    #[test]
    fn a_place_shot_again_twelve_hours_later_is_ambiguous() {
        // device 1 shoots 30 places in the morning and the same places again in the evening;
        // device 2 (clock 40 min fast) only in the evening. Each of its photos matches a morning and
        // an evening photo equally well: the offset and the offset +-12 h are both supported, so
        // the estimate must not be taken by "accept all".
        let err = 40 * MIN;
        let mut rng = Rng(62);
        let mut ph = Vec::new();
        let mut t = T0;
        for _ in 0..30 {
            t += (2 + rng.below(5) as i64) * MIN;
            let v = scene_vec(&mut rng);
            ph.push(photo(1, t + rng.jitter(2 * S), view(&mut rng, &v)));
            ph.push(photo(1, t + 12 * H + rng.jitter(2 * S), view(&mut rng, &v)));
            ph.push(photo(
                2,
                t + 12 * H + err + rng.jitter(2 * S),
                view(&mut rng, &v),
            ));
        }
        let ests = estimate(&ph, &AlignParams::default());
        if let Some(e) = off(&ests, 2) {
            assert!(e.confidence < ACCEPT_MIN_CONFIDENCE, "{e:?}");
        }
        // shot only once, the same places give a confident estimate
        let once: Vec<AlignPhoto> = ph
            .iter()
            .filter(|p| p.device == 2 || p.t >= T0 + 12 * H - H)
            .cloned()
            .collect();
        let ests = estimate(&once, &AlignParams::default());
        let e = off(&ests, 2).expect("estimate");
        assert_close(e, -err, "evening only");
        assert!(e.confidence >= ACCEPT_MIN_CONFIDENCE, "{e:?}");
    }

    #[test]
    fn moments_count_windows_not_frames() {
        assert_eq!(moments(vec![], 5 * MIN), 0);
        // a burst over two minutes is one moment, and so is anything within the window
        assert_eq!(moments(vec![0, 25 * S, 50 * S, 125 * S], 5 * MIN), 1);
        assert_eq!(moments(vec![10 * MIN, 0, 4 * MIN, 6 * MIN], 5 * MIN), 2);
        assert_eq!(moments((0..10).map(|i| i * 6 * MIN).collect(), 5 * MIN), 10);
    }

    #[test]
    fn single_device_or_nothing() {
        assert!(estimate(&[], &AlignParams::default()).is_empty());
        let mut rng = Rng(1);
        let ph: Vec<_> = (0..5)
            .map(|i| photo(7, T0 + i, scene_vec(&mut rng)))
            .collect();
        let e = estimate(&ph, &AlignParams::default());
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].source, EstSource::Reference);
    }

    #[test]
    fn median_and_thin() {
        assert_eq!(median(&mut [5, 1, 3]), 3);
        assert_eq!(median(&mut [1, 3]), 2);
        let ph: Vec<AlignPhoto> = (0..100).map(|i| photo(1, i, vec![1.0, 0.0])).collect();
        let refs: Vec<&AlignPhoto> = ph.iter().collect();
        let t = thin(refs, 10);
        assert_eq!(t.len(), 10);
        assert!(t.windows(2).all(|w| w[0].t < w[1].t));
    }
}
