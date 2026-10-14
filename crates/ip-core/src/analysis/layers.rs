//! SQL for the event layer (`event -> scene -> burst`, docs/09 P1-1) and the duplicate groups
//! (P1-3). The rules themselves are the pure functions of `timeline.rs` and `duplicates.rs`.

use std::collections::{BTreeMap, HashMap};

use rusqlite::{params, Connection, TransactionBehavior};

use super::duplicates::{self, DupAssign, DupItem, DupParams};
use super::store;
use super::timeline::{self, EventParams, EventScene, LatLon};
use super::types::*;
use crate::error::{CoreError, Result};

struct ScenePhoto {
    t: Option<i64>,
    gps: Option<LatLon>,
    offset: Option<i32>,
    id: i64,
}

struct SceneRow {
    id: i64,
    /// Stored bounds of the scene, used when none of its photos has a time.
    start: i64,
    end: i64,
    photos: Vec<ScenePhoto>,
}

impl SceneRow {
    /// Photos with a time, oldest first.
    fn timed(&self) -> Vec<&ScenePhoto> {
        let mut v: Vec<&ScenePhoto> = self.photos.iter().filter(|p| p.t.is_some()).collect();
        v.sort_by_key(|p| (p.t, p.id));
        v
    }

    fn summary(&self) -> EventScene {
        let timed = self.timed();
        let (start, end) = match (timed.first(), timed.last()) {
            (Some(a), Some(b)) => (a.t.unwrap_or(0), b.t.unwrap_or(0)),
            _ => (self.start, self.end),
        };
        EventScene {
            start,
            end,
            first_gps: timed.iter().find_map(|p| p.gps),
            last_gps: timed.iter().rev().find_map(|p| p.gps),
        }
    }
}

/// Rebuilds the events of a session from its scenes: splits them by the rules of
/// [`timeline`] and rewrites the `event` rows and `scene.event_id`. Event ids change on every
/// call. Returns the number of events.
pub(crate) fn rebuild_events(conn: &Connection, session_id: i64) -> Result<usize> {
    let mut scenes: BTreeMap<i64, SceneRow> = BTreeMap::new();
    {
        let mut st = conn.prepare(&format!(
            "SELECT s.id, COALESCE(s.start_at,0), COALESCE(s.end_at,0),
                    p.id, {}, p.gps_lat, p.gps_lon, p.taken_at_offset_min
             FROM scene s JOIN burst b ON b.scene_id=s.id JOIN photo p ON p.burst_id=b.id
             WHERE s.session_id=?1",
            super::time_align::corrected_time_sql("p", "s.session_id")
        ))?;
        let rows = st.query_map([session_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, i64>(2)?,
                ScenePhoto {
                    id: r.get(3)?,
                    t: r.get(4)?,
                    gps: timeline::valid_gps(r.get(5)?, r.get(6)?),
                    offset: r.get(7)?,
                },
            ))
        })?;
        for row in rows {
            let (id, start, end, photo) = row?;
            scenes
                .entry(id)
                .or_insert_with(|| SceneRow {
                    id,
                    start,
                    end,
                    photos: Vec::new(),
                })
                .photos
                .push(photo);
        }
    }
    conn.execute(
        "UPDATE scene SET event_id=NULL WHERE session_id=?1",
        [session_id],
    )?;
    conn.execute("DELETE FROM event WHERE session_id=?1", [session_id])?;

    let mut rows: Vec<(EventScene, SceneRow)> =
        scenes.into_values().map(|s| (s.summary(), s)).collect();
    rows.sort_by_key(|(sum, s)| (sum.start, s.id));
    let summaries: Vec<EventScene> = rows.iter().map(|(s, _)| s.clone()).collect();
    let events = timeline::split_events(&summaries, &EventParams::default());
    for members in &events {
        let start = members
            .iter()
            .map(|&i| summaries[i].start)
            .min()
            .unwrap_or(0);
        let end = members.iter().map(|&i| summaries[i].end).max().unwrap_or(0);
        let mut photos: Vec<&ScenePhoto> =
            members.iter().flat_map(|&i| rows[i].1.timed()).collect();
        photos.sort_by_key(|p| (p.t, p.id));
        let count: usize = members.iter().map(|&i| rows[i].1.photos.len()).sum();
        let offset = photos.iter().find_map(|p| p.offset);
        let fixes: Vec<LatLon> = photos.iter().filter_map(|p| p.gps).collect();
        let centre = (!fixes.is_empty()).then(|| {
            let n = fixes.len() as f64;
            (
                fixes.iter().map(|g| g.0).sum::<f64>() / n,
                fixes.iter().map(|g| g.1).sum::<f64>() / n,
            )
        });
        conn.execute(
            "INSERT INTO event(session_id, start_at, end_at, photo_count, offset_min, gps_lat, gps_lon)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                session_id,
                start,
                end,
                count as i64,
                offset,
                centre.map(|c| c.0),
                centre.map(|c| c.1)
            ],
        )?;
        let event_id = conn.last_insert_rowid();
        for &i in members {
            conn.execute(
                "UPDATE scene SET event_id=?2 WHERE id=?1",
                params![rows[i].1.id, event_id],
            )?;
        }
    }
    Ok(events.len())
}

/// [`rebuild_events`] in its own transaction.
pub fn regroup_events(conn: &mut Connection, session_id: i64) -> Result<usize> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let n = rebuild_events(&tx, session_id)?;
    tx.commit()?;
    Ok(n)
}

/// Sessions that have scenes without an event (a catalog from before the event layer, or scenes
/// created by an older build) get their events built.
pub fn ensure_events(conn: &mut Connection, session_id: i64) -> Result<()> {
    let missing: i64 = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM scene WHERE session_id=?1 AND event_id IS NULL)",
        [session_id],
        |r| r.get(0),
    )?;
    if missing != 0 {
        regroup_events(conn, session_id)?;
    }
    Ok(())
}

/// `GET /api/sessions/{id}/timeline`: events with their scenes and bursts, in time order.
pub fn timeline_of_session(conn: &mut Connection, session_id: i64) -> Result<TimelineOut> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM session WHERE id=?1",
        [session_id],
        |r| r.get(0),
    )?;
    if exists == 0 {
        return Err(CoreError::not_found(format!(
            "session {session_id} not found"
        )));
    }
    ensure_events(conn, session_id)?;
    let groups = store::groups_of_session(conn, session_id)?;
    let mut event_of: HashMap<i64, i64> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT id, event_id FROM scene WHERE session_id=?1 AND event_id IS NOT NULL",
        )?;
        for r in st.query_map([session_id], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
        })? {
            let (s, e) = r?;
            event_of.insert(s, e);
        }
    }
    struct Ev {
        title: Option<String>,
        place: Option<String>,
        start: i64,
        end: i64,
        offset: Option<i32>,
        lat: Option<f64>,
        lon: Option<f64>,
    }
    let mut info: HashMap<i64, Ev> = HashMap::new();
    {
        let mut st = conn.prepare(
            "SELECT id, title, place, COALESCE(start_at,0), COALESCE(end_at,0), offset_min, gps_lat, gps_lon
             FROM event WHERE session_id=?1",
        )?;
        for r in st.query_map([session_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                Ev {
                    title: r.get(1)?,
                    place: r.get(2)?,
                    start: r.get(3)?,
                    end: r.get(4)?,
                    offset: r.get(5)?,
                    lat: r.get(6)?,
                    lon: r.get(7)?,
                },
            ))
        })? {
            let (id, ev) = r?;
            info.insert(id, ev);
        }
    }
    let mut by_event: BTreeMap<i64, Vec<SceneOut>> = BTreeMap::new();
    for mut sc in groups.scenes {
        // scenes the grouping never stored (a burst outside any scene) belong to no event
        let Some(&eid) = event_of.get(&sc.id) else {
            continue;
        };
        sc.event_id = Some(eid);
        by_event.entry(eid).or_default().push(sc);
    }
    let mut events: Vec<EventOut> = by_event
        .into_iter()
        .filter_map(|(id, scenes)| {
            let e = info.get(&id)?;
            let photo_count = scenes
                .iter()
                .flat_map(|s| &s.bursts)
                .map(|b| b.photo_ids.len() as i64)
                .sum();
            let default_name = timeline::event_name(e.start, e.end, e.offset);
            Some(EventOut {
                id,
                name: e
                    .title
                    .clone()
                    .filter(|t| !t.is_empty())
                    .unwrap_or(default_name),
                title: e.title.clone(),
                place: e.place.clone(),
                start_at: e.start,
                end_at: e.end,
                offset_min: e.offset,
                gps_lat: e.lat,
                gps_lon: e.lon,
                photo_count,
                scenes,
            })
        })
        .collect();
    events.sort_by_key(|e| (e.start_at, e.id));
    Ok(TimelineOut { events })
}

// ====================================================================== duplicates

/// What `refresh_duplicates` changed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DupChange {
    /// Photos now in a duplicate group.
    pub grouped: usize,
    /// Photos whose group or kind changed (including photos that left their group).
    pub changed: Vec<i64>,
    /// Sessions of the changed photos.
    pub sessions: Vec<i64>,
}

/// Recomputes the duplicate groups of the whole catalog (duplicates cross sessions and roots) and
/// writes `photo.dup_group_id` / `photo.dup_kind`, touching only the rows that changed.
///
/// Photos whose metadata is not read yet, files that are gone and RAW companions are left out.
pub fn refresh_duplicates(conn: &mut Connection) -> Result<DupChange> {
    let items: Vec<DupItem> = {
        let mut st = conn.prepare(
            "SELECT p.id, p.content_key, p.file_size, a.phash, p.width, p.height, p.taken_at IS NOT NULL
             FROM photo p LEFT JOIN analysis a ON a.photo_id=p.id
             WHERE p.meta_done=1 AND COALESCE(p.missing,0)=0 AND p.pair_id IS NULL",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(DupItem {
                    id: r.get(0)?,
                    content_key: r.get(1)?,
                    file_size: r.get(2)?,
                    phash: store::blob_phash(r.get(3)?),
                    width: r.get(4)?,
                    height: r.get(5)?,
                    has_capture_time: r.get::<_, i64>(6)? != 0,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        v
    };
    let new: HashMap<i64, DupAssign> = duplicates::find_duplicates(&items, &DupParams::default())
        .into_iter()
        .map(|a| (a.photo_id, a))
        .collect();
    let old: HashMap<i64, (i64, Option<String>)> = {
        let mut st = conn.prepare(
            "SELECT id, dup_group_id, dup_kind FROM photo WHERE dup_group_id IS NOT NULL OR dup_kind IS NOT NULL",
        )?;
        let v = st
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    (r.get::<_, Option<i64>>(1)?.unwrap_or(0), r.get(2)?),
                ))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        v
    };
    let mut changed: Vec<i64> = Vec::new();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (id, a) in &new {
        let same = old
            .get(id)
            .is_some_and(|(g, k)| *g == a.group_id && k.as_deref() == Some(a.kind.as_str()));
        if !same {
            tx.execute(
                "UPDATE photo SET dup_group_id=?2, dup_kind=?3 WHERE id=?1",
                params![id, a.group_id, a.kind.as_str()],
            )?;
            changed.push(*id);
        }
    }
    for id in old.keys().filter(|id| !new.contains_key(*id)) {
        tx.execute(
            "UPDATE photo SET dup_group_id=NULL, dup_kind=NULL WHERE id=?1",
            [id],
        )?;
        changed.push(*id);
    }
    tx.commit()?;
    changed.sort_unstable();
    let sessions = store::sessions_of_photos(conn, &changed)?;
    Ok(DupChange {
        grouped: new.len(),
        changed,
        sessions,
    })
}

/// `GET /api/photos/{id}/duplicates`: the other members of the photo's duplicate group.
pub fn duplicates_of(conn: &Connection, photo_id: i64) -> Result<DuplicatesOut> {
    let group: Option<i64> = conn
        .query_row(
            "SELECT dup_group_id FROM photo WHERE id=?1",
            [photo_id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => {
                CoreError::not_found(format!("photo {photo_id} not found"))
            }
            e => e.into(),
        })?;
    let Some(group) = group else {
        return Ok(DuplicatesOut {
            group_id: None,
            photo_ids: Vec::new(),
            exact_ids: Vec::new(),
        });
    };
    let mut st =
        conn.prepare("SELECT id, dup_kind FROM photo WHERE dup_group_id=?1 ORDER BY id")?;
    let rows = st
        .query_map([group], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(DuplicatesOut {
        group_id: Some(group),
        exact_ids: rows
            .iter()
            .filter(|(_, k)| k.as_deref() == Some("exact"))
            .map(|(i, _)| *i)
            .collect(),
        photo_ids: rows.into_iter().map(|(i, _)| i).collect(),
    })
}

/// Fills the event layer and the duplicate groups of a catalog that was created before they
/// existed (run once, when the schema migrates to the version that adds them).
pub fn backfill(conn: &mut Connection) -> Result<()> {
    let sessions: Vec<i64> = {
        let mut st =
            conn.prepare("SELECT DISTINCT session_id FROM scene WHERE session_id IS NOT NULL")?;
        let v = st
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;
        v
    };
    for s in sessions {
        regroup_events(conn, s)?;
    }
    refresh_duplicates(conn)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// 2024-05-03T00:00:00Z
    const BASE: i64 = 1_714_694_400_000;
    const MIN: i64 = 60_000;
    const H: i64 = 3_600_000;
    const A: (f64, f64) = (35.6762, 139.6503);
    const FAR: (f64, f64) = (35.6762, 139.7700); // 11 km east of A

    fn conn() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        db::migrate(&mut c).unwrap();
        seed(&c);
        c
    }

    fn seed(c: &Connection) {
        c.execute(
            "INSERT INTO root_folder(id, path, added_at) VALUES(1,'/r',0)",
            [],
        )
        .unwrap();
        for id in [1, 2] {
            c.execute(
                "INSERT INTO session(id, title, root_id, created_at) VALUES(?1, ?2, 1, 0)",
                params![id, format!("s{id}")],
            )
            .unwrap();
        }
    }

    /// A photo of `session` taken at `t` (UTC ms); also in `session_photo`.
    fn photo(c: &Connection, session: i64, name: &str, t: i64) -> i64 {
        c.execute(
            "INSERT INTO photo(root_id, rel_path, file_name, fast_key, file_size, mtime, taken_at,
                               width, height, meta_done)
             VALUES(1, ?1, ?1, ?1, 1000, ?2, ?2, 600, 400, 1)",
            params![format!("{session}/{name}"), t],
        )
        .unwrap();
        let id = c.last_insert_rowid();
        c.execute(
            "INSERT INTO session_photo(session_id, photo_id) VALUES(?1, ?2)",
            [session, id],
        )
        .unwrap();
        id
    }

    fn set(c: &Connection, id: i64, assignments: &str) {
        c.execute(&format!("UPDATE photo SET {assignments} WHERE id=?1"), [id])
            .unwrap();
    }

    fn gps(c: &Connection, id: i64, g: (f64, f64)) {
        set(c, id, &format!("gps_lat={}, gps_lon={}", g.0, g.1));
    }

    fn phash(c: &Connection, id: i64, h: u64) {
        c.execute(
            "INSERT INTO analysis(photo_id, phash) VALUES(?1, ?2)",
            params![id, h.to_be_bytes().to_vec()],
        )
        .unwrap();
    }

    /// One burst in one scene holding `photos`.
    fn scene(c: &Connection, session: i64, photos: &[i64]) -> i64 {
        let ts: Vec<i64> = photos
            .iter()
            .map(|p| {
                c.query_row("SELECT taken_at FROM photo WHERE id=?1", [p], |r| r.get(0))
                    .unwrap()
            })
            .collect();
        let (s, e) = (*ts.iter().min().unwrap(), *ts.iter().max().unwrap());
        c.execute(
            "INSERT INTO scene(session_id, start_at, end_at) VALUES(?1,?2,?3)",
            params![session, s, e],
        )
        .unwrap();
        let sid = c.last_insert_rowid();
        c.execute(
            "INSERT INTO burst(session_id, scene_id, size, manual, start_at, end_at) VALUES(?1,?2,?3,0,?4,?5)",
            params![session, sid, photos.len() as i64, s, e],
        )
        .unwrap();
        let bid = c.last_insert_rowid();
        for p in photos {
            c.execute("UPDATE photo SET burst_id=?2 WHERE id=?1", [*p, bid])
                .unwrap();
        }
        sid
    }

    fn events_of(c: &Connection, session: i64) -> Vec<Vec<i64>> {
        let mut st = c
            .prepare("SELECT id FROM event WHERE session_id=?1 ORDER BY start_at, id")
            .unwrap();
        let ids: Vec<i64> = st
            .query_map([session], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        ids.into_iter()
            .map(|e| {
                let mut st = c
                    .prepare("SELECT id FROM scene WHERE event_id=?1 ORDER BY start_at, id")
                    .unwrap();
                let v: Vec<i64> = st
                    .query_map([e], |r| r.get(0))
                    .unwrap()
                    .collect::<rusqlite::Result<_>>()
                    .unwrap();
                v
            })
            .collect()
    }

    #[test]
    fn events_follow_pauses_and_moves() {
        let mut c = conn();
        let mut scenes = Vec::new();
        for (i, (dt, g)) in [
            (0, Some(A)),
            (90 * MIN, Some(A)),
            (4 * H, Some(A)),
            (4 * H + 10 * MIN, Some(FAR)),
            (4 * H + 20 * MIN, None),
        ]
        .into_iter()
        .enumerate()
        {
            let p1 = photo(&c, 1, &format!("p{i}a.jpg"), BASE + dt);
            let p2 = photo(&c, 1, &format!("p{i}b.jpg"), BASE + dt + MIN);
            if let Some(g) = g {
                gps(&c, p1, g);
            }
            scenes.push(scene(&c, 1, &[p1, p2]));
        }
        assert_eq!(regroup_events(&mut c, 1).unwrap(), 3);
        assert_eq!(
            events_of(&c, 1),
            vec![
                vec![scenes[0], scenes[1]],
                vec![scenes[2]],
                vec![scenes[3], scenes[4]]
            ]
        );
        // bounds, counts and position of the first event
        let (s, e, n, lat, lon): (i64, i64, i64, f64, f64) = c
            .query_row(
                "SELECT start_at, end_at, photo_count, gps_lat, gps_lon FROM event WHERE session_id=1 ORDER BY start_at LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((s, e, n), (BASE, BASE + 91 * MIN, 4));
        assert!((lat - A.0).abs() < 1e-9 && (lon - A.1).abs() < 1e-9);
        // recomputing gives the same structure and leaves no stale rows
        assert_eq!(regroup_events(&mut c, 1).unwrap(), 3);
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
        // the other session is untouched by a rebuild of this one
        assert_eq!(regroup_events(&mut c, 2).unwrap(), 0);
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn without_gps_only_time_splits() {
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE);
        let b = photo(&c, 1, "b.jpg", BASE + 119 * MIN);
        let s1 = scene(&c, 1, &[a]);
        let s2 = scene(&c, 1, &[b]);
        regroup_events(&mut c, 1).unwrap();
        assert_eq!(events_of(&c, 1), vec![vec![s1, s2]]);
    }

    #[test]
    fn time_zones_do_not_fake_a_pause_or_hide_one() {
        let mut c = conn();
        // 10:00 in Tokyo (UTC+9) and 11:00 in New York (UTC-4) the same day are 14 h apart
        let tokyo = photo(&c, 1, "tokyo.jpg", BASE + H); // 01:00Z = 10:00 local
        let ny = photo(&c, 1, "ny.jpg", BASE + 15 * H); // 15:00Z = 11:00 local
        set(&c, tokyo, "taken_at_offset_min=540");
        set(&c, ny, "taken_at_offset_min=-240");
        let (s1, s2) = (scene(&c, 1, &[tokyo]), scene(&c, 1, &[ny]));
        regroup_events(&mut c, 1).unwrap();
        assert_eq!(events_of(&c, 1), vec![vec![s1], vec![s2]]);
        let offsets: Vec<Option<i32>> = {
            let mut st = c
                .prepare("SELECT offset_min FROM event ORDER BY start_at")
                .unwrap();
            let v = st
                .query_map([], |r| r.get(0))
                .unwrap()
                .collect::<rusqlite::Result<_>>()
                .unwrap();
            v
        };
        assert_eq!(offsets, vec![Some(540), Some(-240)]);

        // a Tokyo photo at 23:30 local and a New York photo 30 min later in real time (the wall
        // clocks differ by 13 h 30 min) belong together
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE + 14 * H + 30 * MIN); // 23:30 local in Tokyo
        let b = photo(&c, 1, "b.jpg", BASE + 15 * H); // 11:00 local in New York
        set(&c, a, "taken_at_offset_min=540");
        set(&c, b, "taken_at_offset_min=-240");
        let (s1, s2) = (scene(&c, 1, &[a]), scene(&c, 1, &[b]));
        regroup_events(&mut c, 1).unwrap();
        assert_eq!(events_of(&c, 1), vec![vec![s1, s2]]);
    }

    #[test]
    fn timeline_has_the_event_scene_burst_shape() {
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE + 10 * H); // 2024-05-03 10:00Z
        let b = photo(&c, 1, "b.jpg", BASE + 10 * H + 30 * MIN);
        let d = photo(&c, 1, "d.jpg", BASE + 20 * H);
        set(&c, a, "taken_at_offset_min=120");
        let s1 = scene(&c, 1, &[a, b]);
        let s2 = scene(&c, 1, &[d]);
        // built on demand: the scenes have no event yet
        let t = timeline_of_session(&mut c, 1).unwrap();
        assert_eq!(t.events.len(), 2);
        let (e1, e2) = (&t.events[0], &t.events[1]);
        assert_eq!(e1.scenes.len(), 1);
        assert_eq!(e1.scenes[0].id, s1);
        assert_eq!(e1.scenes[0].event_id, Some(e1.id));
        assert_eq!(e1.scenes[0].bursts[0].photo_ids.len(), 2);
        assert_eq!((e1.photo_count, e2.photo_count), (2, 1));
        assert_eq!(e2.scenes[0].id, s2);
        assert_eq!(
            (e1.start_at, e1.end_at),
            (BASE + 10 * H, BASE + 10 * H + 30 * MIN)
        );
        assert_eq!(e1.offset_min, Some(120));
        assert_eq!(e1.name, "2024-05-03 12:00-12:30");
        assert_eq!(
            e2.name, "2024-05-03 20:00",
            "unknown offset: stored value is the wall clock"
        );
        assert!(e1.title.is_none() && e1.place.is_none() && e1.gps_lat.is_none());
        // a title wins over the generated name
        c.execute("UPDATE event SET title='Trip' WHERE id=?1", [e1.id])
            .unwrap();
        let t = timeline_of_session(&mut c, 1).unwrap();
        assert_eq!(t.events[0].name, "Trip");
        assert_eq!(t.events[0].title.as_deref(), Some("Trip"));
        // wire shape
        let v = serde_json::to_value(&t).unwrap();
        assert!(v["events"][0]["scenes"][0]["bursts"][0]["photo_ids"].is_array());
        // unknown sessions
        assert!(matches!(
            timeline_of_session(&mut c, 99),
            Err(CoreError::NotFound(_))
        ));
        // empty sessions have no events
        assert!(timeline_of_session(&mut c, 2).unwrap().events.is_empty());
    }

    #[test]
    fn deleting_the_groups_of_a_session_removes_its_events() {
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE);
        scene(&c, 1, &[a]);
        regroup_events(&mut c, 1).unwrap();
        store::purge_session_groups(&c, 1).unwrap();
        let n: i64 = c
            .query_row("SELECT COUNT(*) FROM event", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    fn dup_of(c: &Connection, id: i64) -> (Option<i64>, Option<String>) {
        c.query_row(
            "SELECT dup_group_id, dup_kind FROM photo WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn exact_duplicates_are_found_across_sessions() {
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE);
        let b = photo(&c, 2, "copy-of-a.jpg", BASE + 5 * H);
        let other = photo(&c, 2, "other.jpg", BASE + 6 * H);
        let same_key_other_size = photo(&c, 2, "x.jpg", BASE + 7 * H);
        for id in [a, b, other, same_key_other_size] {
            set(&c, id, "content_key='k0'");
        }
        set(&c, other, "content_key='k1'");
        set(&c, same_key_other_size, "file_size=999");
        let ch = refresh_duplicates(&mut c).unwrap();
        assert_eq!(ch.grouped, 2);
        assert_eq!(ch.changed, vec![a, b]);
        assert_eq!(ch.sessions, vec![1, 2]);
        assert_eq!(dup_of(&c, a), (Some(a), Some("exact".into())));
        assert_eq!(dup_of(&c, b), (Some(a), Some("exact".into())));
        assert_eq!(dup_of(&c, other), (None, None));
        assert_eq!(dup_of(&c, same_key_other_size), (None, None));
        // nothing changed: nothing is written, nobody is told
        let again = refresh_duplicates(&mut c).unwrap();
        assert_eq!(
            (again.grouped, again.changed, again.sessions),
            (2, vec![], vec![])
        );
        let d = duplicates_of(&c, b).unwrap();
        assert_eq!(d.group_id, Some(a));
        assert_eq!(d.photo_ids, vec![a, b]);
        assert_eq!(d.exact_ids, vec![a, b]);
        let d = duplicates_of(&c, other).unwrap();
        assert_eq!((d.group_id, d.photo_ids), (None, vec![]));
        assert!(matches!(
            duplicates_of(&c, 999),
            Err(CoreError::NotFound(_))
        ));
    }

    #[test]
    fn near_duplicates_need_the_same_picture_and_missing_metadata() {
        let mut c = conn();
        const H0: u64 = 0x0F0F_3C3C_A5A5_5A5A;
        let original = photo(&c, 1, "a.jpg", BASE);
        let stripped = photo(&c, 1, "a-web.jpg", BASE + H);
        let burst_mate = photo(&c, 1, "a2.jpg", BASE + 2 * H);
        let too_far = photo(&c, 1, "b-web.jpg", BASE + 3 * H);
        let crop = photo(&c, 1, "a-crop.jpg", BASE + 4 * H);
        set(&c, stripped, "taken_at=NULL");
        set(&c, too_far, "taken_at=NULL");
        set(&c, crop, "taken_at=NULL, width=400, height=400");
        phash(&c, original, H0);
        phash(&c, stripped, H0 ^ 0b11); // 2 bits
        phash(&c, burst_mate, H0 ^ (0b11111 << 40)); // has EXIF: not a copy
        phash(&c, too_far, H0 ^ (0b11111 << 8)); // 5 bits
        phash(&c, crop, H0);
        refresh_duplicates(&mut c).unwrap();
        assert_eq!(dup_of(&c, original), (Some(original), Some("near".into())));
        assert_eq!(dup_of(&c, stripped), (Some(original), Some("near".into())));
        for id in [burst_mate, too_far, crop] {
            assert_eq!(dup_of(&c, id), (None, None), "photo {id}");
        }
    }

    #[test]
    fn groups_follow_the_catalog() {
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE);
        let b = photo(&c, 1, "b.jpg", BASE + H);
        let d = photo(&c, 1, "d.jpg", BASE + 2 * H);
        for id in [a, b, d] {
            set(&c, id, "content_key='k'");
        }
        refresh_duplicates(&mut c).unwrap();
        assert_eq!(duplicates_of(&c, d).unwrap().photo_ids, vec![a, b, d]);
        // a copy goes away: the others stay a pair
        set(&c, a, "missing=1");
        let ch = refresh_duplicates(&mut c).unwrap();
        assert_eq!(
            ch.changed,
            vec![a, b, d],
            "the group id is the smallest member id"
        );
        assert_eq!(dup_of(&c, b), (Some(b), Some("exact".into())));
        assert_eq!(dup_of(&c, a), (None, None));
        // only one is left: no group
        set(&c, b, "missing=1");
        refresh_duplicates(&mut c).unwrap();
        assert_eq!(dup_of(&c, d), (None, None));
    }

    #[test]
    fn unread_and_companion_files_are_not_compared() {
        let mut c = conn();
        let a = photo(&c, 1, "a.jpg", BASE);
        let unread = photo(&c, 1, "b.jpg", BASE + H);
        let raw = photo(&c, 1, "a.dng", BASE + 2 * H);
        for id in [a, unread, raw] {
            set(&c, id, "content_key='k'");
        }
        set(&c, unread, "meta_done=0");
        set(&c, raw, &format!("pair_id={a}"));
        let ch = refresh_duplicates(&mut c).unwrap();
        assert_eq!(ch.grouped, 0);
        // metadata read later: the pair shows up
        set(&c, unread, "meta_done=1");
        assert_eq!(refresh_duplicates(&mut c).unwrap().grouped, 2);
    }

    #[test]
    fn catalogs_from_before_the_layers_are_filled_in_on_migration() {
        let mut c = Connection::open_in_memory().unwrap();
        c.execute_batch("PRAGMA foreign_keys=ON").unwrap();
        assert_eq!(db::migrate_to(&mut c, 12).unwrap(), 12);
        seed(&c);
        // v12 schema: no dup columns yet; photo() only uses v12 columns
        let a = photo(&c, 1, "a.jpg", BASE);
        let b = photo(&c, 2, "b.jpg", BASE + 5 * H);
        c.execute("UPDATE photo SET content_key='k'", []).unwrap();
        let s1 = scene(&c, 1, &[a]);
        assert_eq!(db::migrate(&mut c).unwrap(), db::MIGRATIONS.len());
        backfill(&mut c).unwrap();
        assert_eq!(events_of(&c, 1), vec![vec![s1]]);
        assert_eq!(dup_of(&c, a), (Some(a), Some("exact".into())));
        assert_eq!(dup_of(&c, b), (Some(a), Some("exact".into())));
    }
}
