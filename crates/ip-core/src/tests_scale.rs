//! Large sessions (docs/05 section 1.3): the per-session counters and the sort keys kept next to
//! the session membership are derived data maintained by triggers. These tests compare them with
//! a fresh computation after every kind of write, and check that cursor paging follows the
//! documented order (ties on `taken_at`, NULL times, ratings, scores and names).

use ip_imaging::ImageFormat;
use rusqlite::{params, Connection};

use crate::catalog::{self, NewPhoto};
use crate::db::Db;
use crate::*;

/// xorshift64*: deterministic, no dependency.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    }
}

fn open() -> (Db, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("catalog.db")).unwrap();
    (db, dir)
}

fn new_photo(rel: &str, companion: bool) -> NewPhoto {
    NewPhoto {
        rel_path: rel.to_string(),
        file_name: rel.rsplit('/').next().unwrap().to_string(),
        size: 1,
        mtime_ms: 1,
        fast_key: format!("fk-{rel}"),
        format: if companion {
            ImageFormat::Raw
        } else {
            ImageFormat::Jpeg
        },
        companion,
    }
}

/// A root, a session and `n` photos (`n` grid items; `raws` extra RAW companions).
fn add_session(c: &mut Connection, name: &str, n: usize, raws: usize) -> (i64, Vec<i64>) {
    let root = catalog::upsert_root(c, &format!("/lib/{name}")).unwrap();
    let sid = catalog::create_session(c, root, name).unwrap();
    let mut batch: Vec<NewPhoto> = (0..n)
        .map(|i| new_photo(&format!("{name}/IMG_{i:05}.jpg"), false))
        .collect();
    batch.extend((0..raws).map(|i| new_photo(&format!("{name}/IMG_{i:05}.cr3"), true)));
    let ins = catalog::insert_batch(c, root, sid, &batch).unwrap();
    (sid, ins.iter().take(n).map(|i| i.id).collect())
}

/// Every session's counters and every membership row's sort keys equal a fresh computation.
fn assert_derived(c: &Connection, what: &str) {
    let mut st = c
        .prepare(
            "SELECT s.id,
                (SELECT COUNT(*) FROM session_photo sp WHERE sp.session_id=s.id),
                (SELECT COUNT(*) FROM session_photo sp JOIN photo p ON p.id=sp.photo_id
                   WHERE sp.session_id=s.id AND p.flag=1),
                (SELECT COUNT(*) FROM session_photo sp JOIN photo p ON p.id=sp.photo_id
                   WHERE sp.session_id=s.id AND p.flag=-1),
                (SELECT COUNT(*) FROM session_photo sp JOIN photo p ON p.id=sp.photo_id
                   WHERE sp.session_id=s.id AND p.user_rating IS NOT NULL),
                t.photo_count, t.picked_count, t.rejected_count, t.rated_count
             FROM session s LEFT JOIN session_stat t ON t.session_id=s.id",
        )
        .unwrap();
    let rows: Vec<Vec<Option<i64>>> = st
        .query_map([], |r| (0..9).map(|i| r.get(i)).collect())
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    for r in rows {
        assert_eq!(r[1..5], r[5..9], "{what}: counters of session {:?}", r[0]);
    }
    let stale: i64 = c
        .query_row(
            "SELECT COUNT(*) FROM session_photo sp JOIN photo p ON p.id=sp.photo_id
             WHERE sp.sk_taken IS NOT p.taken_at OR sp.sk_name IS NOT p.file_name
                OR sp.sk_rating IS NOT p.user_rating OR sp.sk_ai IS NOT p.ai_score",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0, "{what}: stale sort keys");
    let orphans: i64 = c
        .query_row(
            "SELECT (SELECT COUNT(*) FROM session_stat WHERE session_id NOT IN (SELECT id FROM session))
                  + (SELECT COUNT(*) FROM session WHERE id NOT IN (SELECT session_id FROM session_stat))",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(orphans, 0, "{what}: session_stat rows match sessions");
}

fn patch(c: &mut Connection, ids: &[i64], rating: Option<Option<i64>>, flag: Option<i64>) {
    catalog::patch_photos(
        c,
        &PatchRequest {
            ids: ids.to_vec(),
            user_rating: rating,
            flag,
            color_label: None,
        },
    )
    .unwrap();
}

#[test]
fn session_counters_follow_every_write() {
    let (db, _dir) = open();
    db.with(|c| {
        // an empty session has a zero row from the start
        let root = catalog::upsert_root(c, "/lib/empty")?;
        let empty = catalog::create_session(c, root, "empty")?;
        assert_eq!(catalog::get_session(c, empty)?.photo_count, 0);

        // import: 40 grid items, 15 RAW companions that are not counted
        let (a, ids_a) = add_session(c, "a", 40, 15);
        assert_eq!(catalog::get_session(c, a)?.photo_count, 40);
        assert_derived(c, "import");

        // re-importing the same files links nothing twice
        let root_a = catalog::upsert_root(c, "/lib/a")?;
        let again: Vec<NewPhoto> = (0..40)
            .map(|i| new_photo(&format!("a/IMG_{i:05}.jpg"), false))
            .collect();
        catalog::insert_batch(c, root_a, a, &again)?;
        assert_eq!(catalog::get_session(c, a)?.photo_count, 40);
        assert_derived(c, "re-import");

        // ratings and flags (also through a companion that follows its JPEG)
        patch(c, &ids_a[0..10], Some(Some(4)), Some(1));
        patch(c, &ids_a[5..20], None, Some(-1));
        patch(c, &ids_a[8..12], Some(None), None);
        patch(c, &ids_a[0..3], Some(Some(2)), Some(0));
        let s = catalog::get_session(c, a)?;
        assert_eq!(
            (s.picked_count, s.rejected_count, s.rated_count),
            (2, 15, 8)
        );
        assert_derived(c, "patch");
        // the same value written again changes nothing
        patch(c, &ids_a[0..3], Some(Some(2)), Some(0));
        assert_derived(c, "patch again");

        // metadata and analysis writes move the sort keys, not the counters
        c.execute(
            "UPDATE photo SET taken_at=1000 + id % 7 WHERE id <= ?1",
            [ids_a[30]],
        )?;
        c.execute(
            "UPDATE photo SET ai_score=id / 100.0 WHERE id <= ?1",
            [ids_a[20]],
        )?;
        c.execute(
            "UPDATE photo SET file_name='Renamed_' || id WHERE id=?1",
            [ids_a[1]],
        )?;
        assert_derived(c, "metadata");

        // a photo shared by two sessions counts in both
        let (b, _) = add_session(c, "b", 5, 0);
        let root_b = catalog::upsert_root(c, "/lib/a")?;
        catalog::insert_batch(c, root_b, b, &again[..8])?;
        assert_eq!(catalog::get_session(c, b)?.photo_count, 5 + 8);
        patch(c, &ids_a[0..2], Some(Some(5)), Some(1));
        assert_derived(c, "shared photos");

        // pairing: a JPEG that becomes the companion of another leaves the grid
        let raw = ids_a[39];
        c.execute(
            "UPDATE photo SET pair_id=?2 WHERE id=?1",
            params![raw, ids_a[38]],
        )?;
        c.execute("DELETE FROM session_photo WHERE photo_id=?1", [raw])?;
        assert_eq!(catalog::get_session(c, a)?.photo_count, 39);
        assert_derived(c, "pairing");

        // deleting photos (also removes their links in every session)
        c.execute(
            "DELETE FROM photo WHERE id IN (?1, ?2)",
            params![ids_a[0], ids_a[15]],
        )?;
        assert_eq!(catalog::get_session(c, a)?.photo_count, 37);
        assert_derived(c, "photo delete");

        // deleting a session drops its row and leaves the other sessions alone
        catalog::delete_session(c, b)?;
        assert!(catalog::get_session(c, b).is_err());
        assert_derived(c, "session delete");
        assert_eq!(catalog::list_sessions(c)?.len(), 2);
        Ok(())
    })
    .unwrap();
}

#[test]
fn derived_data_is_rebuilt_when_the_epoch_differs() {
    let (db, dir) = open();
    let sid = db
        .with(|c| {
            let (sid, ids) = add_session(c, "r", 30, 3);
            patch(c, &ids[0..6], Some(Some(3)), Some(1));
            c.execute("UPDATE photo SET taken_at=id WHERE id <= ?1", [ids[9]])?;
            // simulate data written while the triggers were missing or buggy
            c.execute(
                "UPDATE session_stat SET photo_count=999, picked_count=0",
                [],
            )?;
            c.execute("UPDATE session_photo SET sk_taken=NULL, sk_name='x'", [])?;
            c.execute(
                "DELETE FROM catalog_meta WHERE key='session_derived_epoch'",
                [],
            )?;
            Ok(sid)
        })
        .unwrap();
    drop(db);
    let db = Db::open(&dir.path().join("catalog.db")).unwrap();
    db.with(|c| {
        let s = catalog::get_session(c, sid)?;
        assert_eq!((s.photo_count, s.picked_count, s.rated_count), (30, 6, 6));
        assert_derived(c, "repair");
        // an up-to-date marker is left alone
        c.execute("UPDATE session_stat SET photo_count=7", [])?;
        catalog::ensure_session_derived(c)?;
        assert_eq!(catalog::get_session(c, sid)?.photo_count, 7);
        catalog::rebuild_session_derived(c)?;
        assert_eq!(catalog::get_session(c, sid)?.photo_count, 30);
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_v13_migration_backfills_an_existing_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let mut c = Connection::open(dir.path().join("old.db")).unwrap();
    c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    crate::db::migrate_to(&mut c, 12).unwrap();
    let root = catalog::upsert_root(&c, "/lib/old").unwrap();
    let sid = catalog::create_session(&c, root, "old").unwrap();
    let batch: Vec<NewPhoto> = (0..25)
        .map(|i| new_photo(&format!("IMG_{i:03}.jpg"), false))
        .collect();
    catalog::insert_batch(&mut c, root, sid, &batch).unwrap();
    c.execute(
        "UPDATE photo SET flag=1, user_rating=3, taken_at=id WHERE id % 2 = 0",
        [],
    )
    .unwrap();
    crate::db::migrate(&mut c).unwrap();
    let s = catalog::get_session(&c, sid).unwrap();
    assert_eq!((s.photo_count, s.picked_count, s.rated_count), (25, 12, 12));
    assert_derived(&c, "migration");
}

#[derive(Clone, Debug)]
struct Row {
    id: i64,
    taken: Option<i64>,
    name: String,
    rating: Option<i64>,
    ai: Option<f64>,
}

/// The documented order, computed in Rust from the photo table (not from the sort keys).
fn reference(rows: &[Row], sort: SortKey) -> Vec<i64> {
    let mut v = rows.to_vec();
    match sort {
        SortKey::TakenAt => v.sort_by_key(|r| (r.taken.unwrap_or(i64::MAX), r.id)),
        SortKey::TakenAtDesc => {
            v.sort_by_key(|r| (std::cmp::Reverse(r.taken.unwrap_or(-i64::MAX)), r.id))
        }
        SortKey::Name => v.sort_by_key(|r| (r.name.to_ascii_lowercase(), r.id)),
        SortKey::Rating => v.sort_by_key(|r| (std::cmp::Reverse(r.rating.unwrap_or(-1)), r.id)),
        SortKey::Ai => v.sort_by_key(|r| {
            (
                std::cmp::Reverse((r.ai.unwrap_or(-0.001) * 1_000_000.0) as i64),
                r.id,
            )
        }),
    }
    v.into_iter().map(|r| r.id).collect()
}

fn read_rows(c: &Connection, sid: i64) -> Vec<Row> {
    let mut st = c
        .prepare(
            "SELECT p.id, p.taken_at, p.file_name, p.user_rating, p.ai_score
             FROM session_photo sp JOIN photo p ON p.id=sp.photo_id WHERE sp.session_id=?1",
        )
        .unwrap();
    st.query_map([sid], |r| {
        Ok(Row {
            id: r.get(0)?,
            taken: r.get(1)?,
            name: r.get(2)?,
            rating: r.get(3)?,
            ai: r.get(4)?,
        })
    })
    .unwrap()
    .collect::<rusqlite::Result<_>>()
    .unwrap()
}

/// All pages of a query, following the cursor; checks `total` on every page.
fn pages(c: &Connection, q: &PhotoQuery) -> Vec<i64> {
    let mut out = Vec::new();
    let mut q = q.clone();
    loop {
        let page = catalog::query_photos(c, &q).unwrap();
        out.extend(page.photos.iter().map(|p| p.id));
        match page.next_cursor {
            Some(cur) => q.cursor = Some(cur),
            None => {
                assert_eq!(page.total as usize, out.len(), "total on the last page");
                return out;
            }
        }
    }
}

#[test]
fn cursor_paging_follows_the_sort_order_with_ties_and_nulls() {
    let (db, _dir) = open();
    db.with(|c| {
        let n = 1500;
        let (sid, ids) = add_session(c, "p", n, 20);
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        for &id in &ids {
            // few distinct times (many ties), ~12% without a time; ratings, scores, names in
            // mixed case so every key has ties and NULLs
            let taken: Option<i64> = (rng.below(8) != 0).then(|| 1_700_000_000_000 + rng.below(40) as i64 * 1000);
            let rating: Option<i64> = (rng.below(3) != 0).then(|| rng.below(6) as i64);
            let ai: Option<f64> = (rng.below(4) != 0).then(|| rng.below(25) as f64 / 25.0);
            let name = format!(
                "{}{}_{:03}.jpg",
                ["img", "IMG", "Img", "photo", "Zed"][rng.below(5) as usize],
                ["", "a", "B"][rng.below(3) as usize],
                rng.below(60)
            );
            c.execute(
                "UPDATE photo SET taken_at=?2, user_rating=?3, ai_score=?4, file_name=?5 WHERE id=?1",
                params![id, taken, rating, ai, name],
            )?;
        }
        // later writes keep the keys in step as well
        c.execute("UPDATE photo SET taken_at=NULL WHERE id IN (?1, ?2)", params![ids[3], ids[4]])?;
        c.execute("UPDATE photo SET user_rating=5 WHERE id=?1", [ids[7]])?;
        assert_derived(c, "paging fixture");

        let rows = read_rows(c, sid);
        assert_eq!(rows.len(), n, "companions are not grid items");
        for sort in [
            SortKey::TakenAt,
            SortKey::TakenAtDesc,
            SortKey::Name,
            SortKey::Rating,
            SortKey::Ai,
        ] {
            let want = reference(&rows, sort);
            for limit in [1, 7, 97, 500, 5000] {
                if limit == 1 && !matches!(sort, SortKey::TakenAtDesc) {
                    continue; // 1500 round trips: once is enough
                }
                let got = pages(
                    c,
                    &PhotoQuery {
                        session_id: sid,
                        sort,
                        limit: Some(limit),
                        ..Default::default()
                    },
                );
                assert_eq!(got, want, "{sort:?} limit {limit}");
            }
            // a filter keeps the order of the survivors
            let kept: std::collections::HashSet<i64> = rows
                .iter()
                .filter(|r| r.rating.is_some_and(|x| x >= 3))
                .map(|r| r.id)
                .collect();
            let want_f: Vec<i64> = want.iter().copied().filter(|i| kept.contains(i)).collect();
            let got_f = pages(
                c,
                &PhotoQuery {
                    session_id: sid,
                    sort,
                    rating_gte: Some(3),
                    limit: Some(64),
                    ..Default::default()
                },
            );
            assert_eq!(got_f, want_f, "{sort:?} rating_gte=3");
        }
        Ok(())
    })
    .unwrap();
}
