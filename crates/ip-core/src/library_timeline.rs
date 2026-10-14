//! Library-wide timeline (docs/09 §5.3 P2, PRD F1.9): photos of every session (or one) bucketed
//! by local capture year / month / day, with a few representative photos per bucket.
//!
//! The bucket of a photo is its local calendar date: `taken_at` is an absolute UTC instant when
//! the photo carries a UTC offset (`taken_at_offset_min`), so the local time is
//! `taken_at + offset`. A photo without an offset keeps the wall-clock time it was stored with
//! (offset 0); a photo without a capture time goes to the "undated" bucket. Offsets are plain
//! fixed minutes (no DST tables): the offset recorded with the photo already is the DST-aware one.
//!
//! The bucketing is a pure fold over `(id, taken_at, offset, rank)` rows ([`Accumulator`]); SQL
//! only supplies the rows, with the very filters of the photo list ([`catalog::photo_filters`]).

use std::collections::BTreeMap;

use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};
use serde::Serialize;

use crate::catalog;
use crate::error::{CoreError, Result};
use crate::export::civil_from_days;
use crate::model::PhotoQuery;

/// Representative photos kept per bucket.
pub const REPRESENTATIVES: usize = 4;
const DAY_MS: i64 = 86_400_000;
/// Largest UTC offset magnitude (+14:00 / -12:00) plus an hour of slack, in ms. A local date
/// window widens by this much to become a `taken_at` range the index can serve.
const OFFSET_SLACK_MS: i64 = 15 * 3_600_000;
/// Events listed with a bounded window.
const MAX_EVENTS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Granularity {
    Year,
    Month,
    Day,
}

impl Granularity {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "day" => Some(Self::Day),
            _ => None,
        }
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Hinnant's `days_from_civil`).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `YYYY-MM-DD` -> days since the epoch; `None` for anything that is not a real calendar date.
pub fn parse_ymd(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| -> Option<i64> {
        let t = &s[r];
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let days = days_from_civil(y, m, d);
    let (cy, cm, cd) = civil_from_days(days);
    (cy == y && cm as i64 == m && cd as i64 == d).then_some(days)
}

/// Days since the epoch -> `YYYY-MM-DD`.
pub fn format_ymd(days: i64) -> String {
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Local calendar day (days since the epoch) of a capture time and its UTC offset.
pub fn local_day(taken_at: i64, offset_min: Option<i32>) -> i64 {
    (taken_at + offset_min.unwrap_or(0) as i64 * 60_000).div_euclid(DAY_MS)
}

/// The bucket a day belongs to: its key (`2024`, `2024-05`, `2024-05-03`) and first / last day.
pub fn bucket_of(day: i64, g: Granularity) -> (String, i64, i64) {
    let (y, m, _) = civil_from_days(day);
    match g {
        Granularity::Day => (format_ymd(day), day, day),
        Granularity::Month => {
            let first = days_from_civil(y, m as i64, 1);
            let (ny, nm) = if m == 12 {
                (y + 1, 1)
            } else {
                (y, m as i64 + 1)
            };
            (
                format!("{y:04}-{m:02}"),
                first,
                days_from_civil(ny, nm, 1) - 1,
            )
        }
        Granularity::Year => (
            format!("{y:04}"),
            days_from_civil(y, 1, 1),
            days_from_civil(y + 1, 1, 1) - 1,
        ),
    }
}

/// First day of the bucket of `day` (the map key of the accumulator; no allocation per photo).
fn bucket_first(day: i64, g: Granularity) -> i64 {
    match g {
        Granularity::Day => day,
        Granularity::Month => {
            let (y, m, _) = civil_from_days(day);
            days_from_civil(y, m as i64, 1)
        }
        Granularity::Year => days_from_civil(civil_from_days(day).0, 1, 1),
    }
}

/// One photo as the timeline sees it.
#[derive(Debug, Clone, Copy)]
pub struct Row {
    pub id: i64,
    pub taken_at: Option<i64>,
    pub offset_min: Option<i32>,
    /// Representative order, larger is better: not rejected, user rating, AI score.
    pub rank: (i64, i64, i64),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineBucket {
    pub key: String,
    /// First / last local day of the bucket, `YYYY-MM-DD`: usable as `taken_from` / `taken_to`.
    pub from: String,
    pub to: String,
    pub count: i64,
    /// Best photos first (not rejected, rating, AI score, then the oldest id).
    pub photo_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct UndatedBucket {
    pub count: i64,
    pub photo_ids: Vec<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimelineEventRef {
    pub id: i64,
    pub session_id: i64,
    pub name: String,
    pub title: Option<String>,
    pub start_at: i64,
    pub end_at: i64,
    pub offset_min: Option<i32>,
    pub photo_count: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LibraryTimeline {
    pub granularity: Granularity,
    /// The requested local date window, if any.
    pub from: Option<String>,
    pub to: Option<String>,
    /// Photos in the buckets plus the undated ones.
    pub total: i64,
    pub buckets: Vec<TimelineBucket>,
    pub undated: UndatedBucket,
    /// Events whose local dates touch the window; only for a bounded month / day window.
    pub events: Vec<TimelineEventRef>,
}

#[derive(Debug, Clone)]
pub struct TimelineQuery {
    pub granularity: Granularity,
    /// Only the photos of this session; `None` = the whole library.
    pub session_id: Option<i64>,
    /// Local date window, days since the epoch, both ends inclusive.
    pub from_day: Option<i64>,
    pub to_day: Option<i64>,
    /// The photo-list filters (`session_id`, paging and sort of it are ignored).
    pub filters: PhotoQuery,
}

fn insert_rep(reps: &mut Vec<Row>, row: Row) {
    let key = |r: &Row| (r.rank, std::cmp::Reverse(r.id));
    let at = reps
        .iter()
        .position(|r| key(&row) > key(r))
        .unwrap_or(reps.len());
    if at < REPRESENTATIVES {
        reps.insert(at, row);
        reps.truncate(REPRESENTATIVES);
    }
}

/// Folds photo rows into buckets.
pub struct Accumulator {
    granularity: Granularity,
    from_day: Option<i64>,
    to_day: Option<i64>,
    buckets: BTreeMap<i64, (i64, Vec<Row>)>,
    undated: (i64, Vec<Row>),
}

impl Accumulator {
    pub fn new(granularity: Granularity, from_day: Option<i64>, to_day: Option<i64>) -> Self {
        Self {
            granularity,
            from_day,
            to_day,
            buckets: BTreeMap::new(),
            undated: (0, Vec::new()),
        }
    }

    pub fn add(&mut self, row: Row) {
        let Some(t) = row.taken_at else {
            // a bounded window is about dates: photos without one are not in it
            if self.from_day.is_none() && self.to_day.is_none() {
                self.undated.0 += 1;
                insert_rep(&mut self.undated.1, row);
            }
            return;
        };
        let day = local_day(t, row.offset_min);
        if self.from_day.is_some_and(|f| day < f) || self.to_day.is_some_and(|e| day > e) {
            return;
        }
        let first = bucket_first(day, self.granularity);
        let e = self.buckets.entry(first).or_insert((0, Vec::new()));
        e.0 += 1;
        insert_rep(&mut e.1, row);
    }

    pub fn finish(self) -> (Vec<TimelineBucket>, UndatedBucket) {
        let g = self.granularity;
        let buckets = self
            .buckets
            .into_iter()
            .map(|(first, (count, reps))| {
                let (key, from, to) = bucket_of(first, g);
                TimelineBucket {
                    key,
                    from: format_ymd(from),
                    to: format_ymd(to),
                    count,
                    photo_ids: reps.into_iter().map(|r| r.id).collect(),
                }
            })
            .collect();
        let undated = UndatedBucket {
            count: self.undated.0,
            photo_ids: self.undated.1.into_iter().map(|r| r.id).collect(),
        };
        (buckets, undated)
    }
}

/// `GET /api/timeline`: see the module docs.
pub fn library_timeline(conn: &Connection, q: &TimelineQuery) -> Result<LibraryTimeline> {
    if let (Some(a), Some(b)) = (q.from_day, q.to_day) {
        if a > b {
            return Err(CoreError::bad_request("from is after to"));
        }
    }
    if let Some(s) = q.session_id {
        catalog::get_session_exists(conn, s)?;
    }
    let (mut wheres, filter_args) = catalog::photo_filters(conn, &q.filters)?;
    let mut args: Vec<Value> = Vec::new();
    // The companions of a RAW+JPEG pair are not photos of their own (they left every session).
    let source = match q.session_id {
        Some(s) => {
            args.push(Value::Integer(s));
            "session_photo sp CROSS JOIN photo p ON p.id=sp.photo_id
             WHERE sp.session_id=? AND p.pair_id IS NULL"
        }
        None => "photo p WHERE p.pair_id IS NULL",
    };
    args.extend(filter_args);
    // The local date window as a `taken_at` range (widened by the largest UTC offset) that
    // `idx_photo_taken` can serve; the exact local date is checked per row.
    if let Some(f) = q.from_day {
        wheres.push("p.taken_at >= ?".into());
        args.push(Value::Integer(f * DAY_MS - OFFSET_SLACK_MS));
    }
    if let Some(e) = q.to_day {
        wheres.push("p.taken_at < ?".into());
        args.push(Value::Integer((e + 1) * DAY_MS + OFFSET_SLACK_MS));
    }
    let extra = if wheres.is_empty() {
        String::new()
    } else {
        format!(" AND {}", wheres.join(" AND "))
    };
    let sql = format!(
        "SELECT p.id, p.taken_at, p.taken_at_offset_min,
                CASE WHEN COALESCE(p.flag,0) < 0 THEN 0 ELSE 1 END,
                COALESCE(p.user_rating,0),
                CAST(COALESCE(p.ai_score,-0.001) * 1000000 AS INTEGER)
         FROM {source}{extra}"
    );
    let mut acc = Accumulator::new(q.granularity, q.from_day, q.to_day);
    let mut st = conn.prepare_cached(&sql)?;
    let mut rows = st.query(params_from_iter(args.iter()))?;
    while let Some(r) = rows.next()? {
        acc.add(Row {
            id: r.get(0)?,
            taken_at: r.get(1)?,
            offset_min: r.get(2)?,
            rank: (r.get(3)?, r.get(4)?, r.get(5)?),
        });
    }
    let (buckets, undated) = acc.finish();
    let total = buckets.iter().map(|b| b.count).sum::<i64>() + undated.count;
    let bounded = q.from_day.is_some() && q.to_day.is_some();
    let events = if bounded && q.granularity != Granularity::Year {
        window_events(conn, q.session_id, q.from_day.unwrap(), q.to_day.unwrap())?
    } else {
        Vec::new()
    };
    Ok(LibraryTimeline {
        granularity: q.granularity,
        from: q.from_day.map(format_ymd),
        to: q.to_day.map(format_ymd),
        total,
        buckets,
        undated,
        events,
    })
}

/// Events (docs/09 P1-1) of the library or one session that touch the local dates `from..=to`.
fn window_events(
    conn: &Connection,
    session_id: Option<i64>,
    from_day: i64,
    to_day: i64,
) -> Result<Vec<TimelineEventRef>> {
    let mut st = conn.prepare_cached(
        "SELECT id, session_id, title, start_at, end_at, offset_min, photo_count
         FROM event
         WHERE session_id IS NOT NULL AND (?1 IS NULL OR session_id=?1)
           AND start_at < ?3 AND end_at >= ?2
         ORDER BY start_at, id LIMIT 2000",
    )?;
    let lo = from_day * DAY_MS - OFFSET_SLACK_MS;
    let hi = (to_day + 1) * DAY_MS + OFFSET_SLACK_MS;
    let rows = st
        .query_map(rusqlite::params![session_id, lo, hi], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i32>>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, _, _, s, e, off, _)| {
            local_day(*s, *off) <= to_day && local_day((*e).max(*s), *off) >= from_day
        })
        .take(MAX_EVENTS)
        .map(
            |(id, session_id, title, start_at, end_at, offset_min, photo_count)| {
                let name = match title.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
                    Some(t) => t.to_string(),
                    None => crate::analysis::timeline::event_name(start_at, end_at, offset_min),
                };
                TimelineEventRef {
                    id,
                    session_id,
                    name,
                    title,
                    start_at,
                    end_at,
                    offset_min,
                    photo_count,
                }
            },
        )
        .collect())
}

impl crate::Core {
    /// `GET /api/timeline`.
    pub async fn library_timeline(&self, q: TimelineQuery) -> Result<LibraryTimeline> {
        self.db.call(move |c| library_timeline(c, &q)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const H: i64 = 3_600_000;

    fn ms(y: i64, m: i64, d: i64, hh: i64, mm: i64) -> i64 {
        days_from_civil(y, m, d) * DAY_MS + hh * H + mm * 60_000
    }

    #[test]
    fn ymd_round_trips_and_rejects_non_dates() {
        for s in [
            "1970-01-01",
            "2024-02-29",
            "1969-12-31",
            "2000-02-29",
            "0999-01-01",
        ] {
            assert_eq!(format_ymd(parse_ymd(s).unwrap()), s);
        }
        assert_eq!(parse_ymd("1970-01-02"), Some(1));
        for s in [
            "2023-02-29",
            "1900-02-29",
            "2024-13-01",
            "2024-00-10",
            "2024-1-1",
            "2024/01/01",
            "20240101",
            "",
            "2024-01-1x",
            "+024-01-01",
        ] {
            assert_eq!(parse_ymd(s), None, "{s}");
        }
    }

    #[test]
    fn local_day_applies_the_utc_offset() {
        let t = ms(2024, 5, 3, 23, 30);
        assert_eq!(format_ymd(local_day(t, None)), "2024-05-03");
        assert_eq!(format_ymd(local_day(t, Some(0))), "2024-05-03");
        // +09:00: already the next morning
        assert_eq!(format_ymd(local_day(t, Some(540))), "2024-05-04");
        // +05:45 (Nepal) and +14:00 cross midnight, +05:15 does not
        assert_eq!(format_ymd(local_day(t, Some(345))), "2024-05-04");
        assert_eq!(format_ymd(local_day(t, Some(840))), "2024-05-04");
        assert_eq!(format_ymd(local_day(t, Some(15))), "2024-05-03");
        // -05:00 and -12:00 go back
        let early = ms(2024, 5, 3, 2, 0);
        assert_eq!(format_ymd(local_day(early, Some(-300))), "2024-05-02");
        assert_eq!(format_ymd(local_day(early, Some(-720))), "2024-05-02");
        // the exact boundaries
        assert_eq!(
            format_ymd(local_day(ms(2024, 5, 3, 0, 0), Some(0))),
            "2024-05-03"
        );
        assert_eq!(
            format_ymd(local_day(ms(2024, 5, 3, 0, 0) - 1, Some(0))),
            "2024-05-02"
        );
        // before 1970
        assert_eq!(format_ymd(local_day(-1, None)), "1969-12-31");
    }

    #[test]
    fn buckets_cover_whole_calendar_units() {
        let d = parse_ymd("2024-02-10").unwrap();
        let (k, a, b) = bucket_of(d, Granularity::Month);
        assert_eq!(
            (k.as_str(), format_ymd(a), format_ymd(b)),
            ("2024-02", "2024-02-01".into(), "2024-02-29".into())
        );
        let (k, a, b) = bucket_of(parse_ymd("2023-12-31").unwrap(), Granularity::Month);
        assert_eq!(
            (k.as_str(), format_ymd(a), format_ymd(b)),
            ("2023-12", "2023-12-01".into(), "2023-12-31".into())
        );
        let (k, a, b) = bucket_of(d, Granularity::Year);
        assert_eq!(
            (k.as_str(), format_ymd(a), format_ymd(b)),
            ("2024", "2024-01-01".into(), "2024-12-31".into())
        );
        let (k, a, b) = bucket_of(d, Granularity::Day);
        assert_eq!((k.as_str(), a, b), ("2024-02-10", d, d));
        // consecutive months tile the year without a gap
        let mut next = parse_ymd("2024-01-01").unwrap();
        for _ in 0..12 {
            let (_, a, b) = bucket_of(next, Granularity::Month);
            assert_eq!(a, next);
            next = b + 1;
        }
        assert_eq!(format_ymd(next), "2025-01-01");
    }

    #[test]
    fn bucket_first_matches_bucket_of() {
        for day in (-800..800).chain(19_000..19_800) {
            for g in [Granularity::Year, Granularity::Month, Granularity::Day] {
                assert_eq!(bucket_first(day, g), bucket_of(day, g).1, "{day} {g:?}");
            }
        }
    }

    fn row(id: i64, t: Option<i64>, off: Option<i32>, rank: (i64, i64, i64)) -> Row {
        Row {
            id,
            taken_at: t,
            offset_min: off,
            rank,
        }
    }

    #[test]
    fn representatives_are_best_first_and_rejects_last() {
        let t = ms(2024, 5, 3, 12, 0);
        let mut acc = Accumulator::new(Granularity::Month, None, None);
        acc.add(row(1, Some(t), None, (1, 0, 500))); // unrated, AI 0.0005
        acc.add(row(2, Some(t), None, (1, 3, 100)));
        acc.add(row(3, Some(t), None, (0, 5, 900))); // rejected: after everything else
        acc.add(row(4, Some(t), None, (1, 3, 900)));
        acc.add(row(5, Some(t), None, (1, 3, 900))); // ties keep the older id first
        acc.add(row(6, Some(t), None, (1, 0, 0)));
        let (b, u) = acc.finish();
        assert_eq!(u, UndatedBucket::default());
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].count, 6);
        assert_eq!(b[0].photo_ids, vec![4, 5, 2, 1]);
    }

    #[test]
    fn undated_photos_have_their_own_bucket_and_leave_a_bounded_window() {
        let t = ms(2024, 5, 3, 12, 0);
        let mut acc = Accumulator::new(Granularity::Year, None, None);
        acc.add(row(1, None, None, (1, 0, 0)));
        acc.add(row(2, None, Some(60), (1, 4, 0)));
        acc.add(row(3, Some(t), None, (1, 0, 0)));
        let (b, u) = acc.finish();
        assert_eq!((b.len(), b[0].key.as_str(), b[0].count), (1, "2024", 1));
        assert_eq!((u.count, u.photo_ids.clone()), (2, vec![2, 1]));

        let day = parse_ymd("2024-05-03");
        let mut acc = Accumulator::new(Granularity::Day, day, day);
        acc.add(row(1, None, None, (1, 0, 0)));
        acc.add(row(3, Some(t), None, (1, 0, 0)));
        acc.add(row(4, Some(t + DAY_MS), None, (1, 0, 0)));
        let (b, u) = acc.finish();
        assert_eq!((b.len(), u.count), (1, 0));
    }

    #[test]
    fn local_dates_decide_the_bucket_not_utc() {
        // 2024-05-31 23:30 UTC is already June in Tokyo and still May in New York
        let t = ms(2024, 5, 31, 23, 30);
        let mut acc = Accumulator::new(Granularity::Month, None, None);
        acc.add(row(1, Some(t), Some(540), (1, 0, 0)));
        acc.add(row(2, Some(t), Some(-240), (1, 0, 0)));
        acc.add(row(3, Some(t), None, (1, 0, 0)));
        let (b, _) = acc.finish();
        let keys: Vec<_> = b.iter().map(|x| (x.key.as_str(), x.count)).collect();
        assert_eq!(keys, vec![("2024-05", 2), ("2024-06", 1)]);
        assert_eq!(
            (b[1].from.as_str(), b[1].to.as_str()),
            ("2024-06-01", "2024-06-30")
        );
    }
}
