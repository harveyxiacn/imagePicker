//! Library-wide timeline (`GET /api/timeline`): bucketing over several sessions, UTC offsets,
//! undated photos, and the photo-list filters combined with the buckets.

use std::time::Instant;

use ip_imaging::ImageFormat;
use rusqlite::{params, Connection};

use crate::catalog::{self, NewPhoto};
use crate::db::Db;
use crate::library_timeline::{
    days_from_civil, library_timeline, parse_ymd, Granularity, LibraryTimeline, TimelineQuery,
};
use crate::*;

const H: i64 = 3_600_000;

fn ms(y: i64, m: i64, d: i64, hh: i64, mm: i64) -> i64 {
    days_from_civil(y, m, d) * 86_400_000 + hh * H + mm * 60_000
}

fn open() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("catalog.db")).unwrap();
    (db, dir)
}

fn new_photo(rel: &str) -> NewPhoto {
    NewPhoto {
        rel_path: rel.to_string(),
        file_name: rel.rsplit('/').next().unwrap().to_string(),
        size: 1,
        mtime_ms: 1,
        fast_key: format!("fk-{rel}"),
        format: ImageFormat::Jpeg,
        companion: false,
    }
}

/// A session with one photo per `(taken_at, offset)`; `None` = no capture time.
fn add_session(
    c: &mut Connection,
    name: &str,
    times: &[(Option<i64>, Option<i32>)],
) -> (i64, Vec<i64>) {
    let root = catalog::upsert_root(c, &format!("/lib/{name}")).unwrap();
    let sid = catalog::create_session(c, root, name).unwrap();
    let batch: Vec<NewPhoto> = (0..times.len())
        .map(|i| new_photo(&format!("{name}/IMG_{i:05}.jpg")))
        .collect();
    let ins = catalog::insert_batch(c, root, sid, &batch).unwrap();
    let ids: Vec<i64> = ins.iter().map(|i| i.id).collect();
    for (id, (t, off)) in ids.iter().zip(times) {
        c.execute(
            "UPDATE photo SET taken_at=?2, taken_at_offset_min=?3 WHERE id=?1",
            params![id, t, off],
        )
        .unwrap();
    }
    (sid, ids)
}

fn query(g: Granularity) -> TimelineQuery {
    TimelineQuery {
        granularity: g,
        session_id: None,
        from_day: None,
        to_day: None,
        filters: PhotoQuery::default(),
    }
}

fn keys(t: &LibraryTimeline) -> Vec<(String, i64)> {
    t.buckets.iter().map(|b| (b.key.clone(), b.count)).collect()
}

fn k(s: &str, n: i64) -> (String, i64) {
    (s.to_string(), n)
}

#[test]
fn buckets_span_sessions_and_use_local_dates() {
    let (db, _dir) = open();
    db.with(|c| {
        let (_a, ia) = add_session(
            c,
            "a",
            &[
                (Some(ms(2023, 12, 31, 20, 0)), Some(0)),   // 2023-12-31
                (Some(ms(2023, 12, 31, 20, 0)), Some(300)), // +05:00: 2024-01-01 01:00
                (Some(ms(2024, 5, 3, 10, 0)), Some(-420)),  // -07:00: still 05-03
                (None, None),
            ],
        );
        let (_b, ib) = add_session(
            c,
            "b",
            &[
                (Some(ms(2024, 5, 3, 23, 30)), Some(540)), // +09:00: 05-04
                (Some(ms(2024, 5, 3, 23, 30)), None),      // no offset: wall clock as stored
                (None, Some(60)),
            ],
        );

        let year = library_timeline(c, &query(Granularity::Year))?;
        assert_eq!(keys(&year), vec![k("2023", 1), k("2024", 4)]);
        assert_eq!((year.total, year.undated.count), (7, 2));
        assert_eq!(year.undated.photo_ids.len(), 2);
        assert_eq!(
            (year.buckets[0].from.as_str(), year.buckets[0].to.as_str()),
            ("2023-01-01", "2023-12-31")
        );

        let month = library_timeline(c, &query(Granularity::Month))?;
        assert_eq!(
            keys(&month),
            vec![k("2023-12", 1), k("2024-01", 1), k("2024-05", 3)]
        );

        let day = library_timeline(c, &query(Granularity::Day))?;
        assert_eq!(
            keys(&day),
            vec![
                k("2023-12-31", 1),
                k("2024-01-01", 1),
                k("2024-05-03", 2),
                k("2024-05-04", 1)
            ]
        );
        // the 05-03 bucket holds photos of both sessions
        let may3 = day.buckets.iter().find(|b| b.key == "2024-05-03").unwrap();
        let mut got = may3.photo_ids.clone();
        got.sort();
        assert_eq!(got, vec![ia[2], ib[1]]);

        // one session only
        let mut q = query(Granularity::Month);
        q.session_id = Some(_b);
        assert_eq!(keys(&library_timeline(c, &q)?), vec![k("2024-05", 2)]);
        q.session_id = Some(9999);
        assert!(library_timeline(c, &q).is_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_photo_in_two_sessions_counts_once_and_companions_not_at_all() {
    let (db, _dir) = open();
    db.with(|c| {
        let (_s1, ids) = add_session(
            c,
            "a",
            &[
                (Some(ms(2024, 1, 1, 12, 0)), None),
                (Some(ms(2024, 1, 1, 13, 0)), None),
            ],
        );
        let root = catalog::upsert_root(c, "/lib/other")?;
        let s2 = catalog::create_session(c, root, "other")?;
        c.execute(
            "INSERT INTO session_photo(session_id, photo_id) VALUES(?1, ?2)",
            params![s2, ids[0]],
        )
        .unwrap();
        // the second photo is the RAW companion of the first
        c.execute(
            "UPDATE photo SET pair_id=?2 WHERE id=?1",
            params![ids[1], ids[0]],
        )
        .unwrap();
        let t = library_timeline(c, &query(Granularity::Day))?;
        assert_eq!(keys(&t), vec![k("2024-01-01", 1)]);
        assert_eq!(t.total, 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn photo_filters_combine_with_the_buckets() {
    let (db, _dir) = open();
    db.with(|c| {
        let (_s, ids) = add_session(
            c,
            "a",
            &[
                (Some(ms(2024, 3, 1, 9, 0)), None),
                (Some(ms(2024, 3, 2, 9, 0)), None),
                (Some(ms(2024, 4, 1, 9, 0)), None),
                (Some(ms(2024, 4, 2, 9, 0)), None),
                (None, None),
            ],
        );
        catalog::patch_photos(
            c,
            &PatchRequest {
                ids: vec![ids[0], ids[2], ids[4]],
                user_rating: Some(Some(4)),
                flag: None,
                color_label: None,
            },
        )?;
        catalog::patch_photos(
            c,
            &PatchRequest {
                ids: vec![ids[3]],
                user_rating: None,
                flag: Some(-1),
                color_label: None,
            },
        )?;

        let mut q = query(Granularity::Month);
        q.filters.rating_gte = Some(4);
        let t = library_timeline(c, &q)?;
        assert_eq!(keys(&t), vec![k("2024-03", 1), k("2024-04", 1)]);
        assert_eq!(t.undated.count, 1);

        let mut q = query(Granularity::Month);
        q.filters.flag = FlagFilter::NotRejected;
        assert_eq!(
            keys(&library_timeline(c, &q)?),
            vec![k("2024-03", 2), k("2024-04", 1)]
        );

        // a capture range filter and rating together
        let mut q = query(Granularity::Day);
        q.filters.rating_gte = Some(4);
        q.filters.taken_from = Some(ms(2024, 4, 1, 0, 0));
        let t = library_timeline(c, &q)?;
        assert_eq!(keys(&t), vec![k("2024-04-01", 1)]);
        assert_eq!(
            t.undated.count, 0,
            "a capture range leaves out undated photos"
        );

        // the representative of a bucket is the best photo: rated 4 beats unrated, rejects last
        let t = library_timeline(c, &query(Granularity::Month))?;
        assert_eq!(t.buckets[1].photo_ids, vec![ids[2], ids[3]]);
        Ok(())
    })
    .unwrap();
}

#[test]
fn local_date_window_and_events() {
    let (db, _dir) = open();
    db.with(|c| {
        let t_a = ms(2024, 5, 3, 23, 30); // +09:00 -> 05-04 08:30
        let (sid, _) = add_session(
            c,
            "a",
            &[
                (Some(t_a), Some(540)),
                (Some(ms(2024, 5, 3, 12, 0)), Some(540)),
                (Some(ms(2024, 5, 5, 12, 0)), Some(0)),
                (None, None),
            ],
        );
        c.execute(
            "INSERT INTO event(session_id, title, start_at, end_at, offset_min, photo_count) VALUES(?1, NULL, ?2, ?3, 540, 2)",
            params![sid, ms(2024, 5, 3, 22, 0), t_a],
        )
        .unwrap();
        c.execute(
            "INSERT INTO event(session_id, title, start_at, end_at, offset_min, photo_count) VALUES(?1, 'Trip', ?2, ?3, 0, 1)",
            params![sid, ms(2024, 5, 5, 12, 0), ms(2024, 5, 5, 12, 0)],
        )
        .unwrap();

        let mut q = query(Granularity::Day);
        q.from_day = parse_ymd("2024-05-04");
        q.to_day = parse_ymd("2024-05-04");
        let t = library_timeline(c, &q)?;
        // local 05-04: the first photo only; the 05-03 12:00Z one is 21:00 local on 05-03
        assert_eq!(keys(&t), vec![k("2024-05-04", 1)]);
        assert_eq!((t.undated.count, t.total), (0, 1));
        assert_eq!(t.events.len(), 1);
        // 22:00Z-23:30Z at +09:00 is 05-04 07:00-08:30 local
        assert_eq!(t.events[0].name, "2024-05-04 07:00-08:30");
        assert_eq!(t.events[0].offset_min, Some(540));
        assert_eq!((t.from.as_deref(), t.to.as_deref()), (Some("2024-05-04"), Some("2024-05-04")));

        // a month window sees both events, the year view none
        q.from_day = parse_ymd("2024-05-01");
        q.to_day = parse_ymd("2024-05-31");
        q.granularity = Granularity::Month;
        let t = library_timeline(c, &q)?;
        assert_eq!(keys(&t), vec![k("2024-05", 3)]);
        assert_eq!(t.events.len(), 2);
        assert_eq!(t.events[1].name, "Trip");
        q.granularity = Granularity::Year;
        assert!(library_timeline(c, &q)?.events.is_empty());

        // inverted window
        q.from_day = parse_ymd("2024-06-01");
        assert!(library_timeline(c, &q).is_err());
        Ok(())
    })
    .unwrap();
}

/// 100k photos over eight years in a handful of sessions: the timeline of the whole library
/// must stay interactive. `cargo test --release -p ip-core timeline_100k -- --ignored --nocapture`
#[test]
#[ignore = "benchmark: run in release mode"]
fn timeline_100k_catalog_timings() {
    let (db, _dir) = open();
    let n = 100_000usize;
    db.with(|c| {
        let root = catalog::upsert_root(c, "/lib/big")?;
        for s in 0..10 {
            let sid = catalog::create_session(c, root, &format!("s{s}"))?;
            let batch: Vec<NewPhoto> = (0..n / 10).map(|i| new_photo(&format!("s{s}/IMG_{i:06}.jpg"))).collect();
            catalog::insert_batch(c, root, sid, &batch)?;
        }
        let t0 = ms(2016, 1, 1, 0, 0);
        c.execute(
            "UPDATE photo SET taken_at = CASE WHEN id % 50 = 0 THEN NULL ELSE ?1 + (id * 2654435761 % 252288000) * 1000 END,
                              taken_at_offset_min = CASE id % 3 WHEN 0 THEN 480 WHEN 1 THEN -300 ELSE NULL END,
                              user_rating = id % 6, ai_score = (id % 100) / 100.0",
            [t0],
        )
        .unwrap();
        c.execute_batch("ANALYZE").unwrap();
        for g in [Granularity::Year, Granularity::Month, Granularity::Day] {
            let t = Instant::now();
            let r = library_timeline(c, &query(g))?;
            println!("timeline {g:?} 100k photos, all: {:?} ({} buckets, total {})", t.elapsed(), r.buckets.len(), r.total);
        }
        let mut q = query(Granularity::Day);
        q.from_day = parse_ymd("2019-03-01");
        q.to_day = parse_ymd("2019-03-31");
        let t = Instant::now();
        let r = library_timeline(c, &q)?;
        println!("timeline Day, one month window: {:?} ({} buckets, total {})", t.elapsed(), r.buckets.len(), r.total);
        let mut q = query(Granularity::Month);
        q.filters.rating_gte = Some(4);
        let t = Instant::now();
        let r = library_timeline(c, &q)?;
        println!("timeline Month, rating >= 4: {:?} ({} buckets, total {})", t.elapsed(), r.buckets.len(), r.total);
        Ok(())
    })
    .unwrap();
}
