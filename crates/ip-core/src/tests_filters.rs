//! Filters complete (docs/09 P1-5): lens, capture time range, keywords, sharpness state and
//! similar faces on `GET /api/photos`, plus the session filter options and the adjustable face
//! search floor. Photos come from `FakeImaging`; the metadata the filters read is seeded in SQL.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use crate::analysis::vecs::encode_f16;
use crate::testutil::{make_photos, FakeImaging};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);
/// `taken_at` (ms) of `img_000.jpg`; photo `i` is `i` minutes later (`make_photos`).
const BASE: i64 = 1_700_000_000_000;
const MIN: i64 = 60_000;
const ZOOM: &str = "RF 24-70mm F2.8, L";

struct Env {
    core: Arc<Core>,
    sid: i64,
    /// photo ids, `ids[i]` = `img_00{i}.jpg`
    ids: Vec<i64>,
    /// face ids by name
    faces: Vec<(&'static str, i64)>,
    _data: tempfile::TempDir,
    _src: tempfile::TempDir,
}

impl Env {
    fn face(&self, name: &str) -> i64 {
        self.faces.iter().find(|(n, _)| *n == name).unwrap().1
    }
    fn q(&self) -> PhotoQuery {
        PhotoQuery {
            session_id: self.sid,
            sort: SortKey::Name,
            ..Default::default()
        }
    }
    /// Indexes (into `ids`) of the photos a query returns, in order, following all pages.
    async fn idx(&self, q: PhotoQuery) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let page = self
                .core
                .photos(PhotoQuery {
                    cursor: cursor.clone(),
                    ..q.clone()
                })
                .await
                .unwrap();
            out.extend(
                page.photos
                    .iter()
                    .map(|p| self.ids.iter().position(|i| *i == p.id).unwrap()),
            );
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => {
                    assert_eq!(page.total as usize, out.len(), "total matches the pages");
                    return out;
                }
            }
        }
    }
}

fn unit(v: [f32; 4]) -> Vec<u8> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    encode_f16(&v.iter().map(|x| x / n).collect::<Vec<_>>())
}

/// 10 photos (`img_000`..`img_009`, one minute apart). Seeded:
/// - lens: 0-2 `ZOOM`, 3-4 `85mm`, the rest `50mm` (the fake imaging's lens)
/// - keywords: 0 {travel, kyoto}, 1 {travel}, 2 {kyoto, food}, 7 {travel, kyoto}
/// - sharpness: 0 .9, 1 .2, 2 .35 (the edge counts as sharp), 3 .34, 4 no measure, 5 .8 but with
///   the `blurry` issue (relative to its burst), 6-9 not analysed; 9 also has no capture time
/// - faces (embedding cosine to `a`): `a` photo 0; `b` photo 1 (.9); `c` photo 2 (.5);
///   `d` photo 3 (0); `e` photo 4 (no embedding); `a2` photo 5 (1.0) and `x` photo 5 (0)
async fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: None,
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    make_photos(src.path(), 10);
    let s = core
        .import(ImportRequest {
            path: src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    let s = core.wait_session_ready(s.id, WAIT).await.unwrap();
    let page = core
        .photos(PhotoQuery {
            session_id: s.id,
            sort: SortKey::Name,
            ..Default::default()
        })
        .await
        .unwrap();
    let ids: Vec<i64> = page.photos.iter().map(|p| p.id).collect();
    assert_eq!(ids.len(), 10);
    assert_eq!(page.photos[3].taken_at, Some(BASE + 3 * MIN));

    let ids2 = ids.clone();
    let faces = core
        .db
        .call(move |c| {
            let id = |i: usize| ids2[i];
            for i in 0..3 {
                c.execute(
                    "UPDATE photo SET lens=?1 WHERE id=?2",
                    rusqlite::params![ZOOM, id(i)],
                )?;
            }
            for i in 3..5 {
                c.execute("UPDATE photo SET lens='85mm' WHERE id=?1", [id(i)])?;
            }
            c.execute("UPDATE photo SET taken_at=NULL WHERE id=?1", [id(9)])?;
            for (i, tags) in [
                (0, vec!["travel", "kyoto"]),
                (1, vec!["travel"]),
                (2, vec!["kyoto", "food"]),
                (7, vec!["travel", "kyoto"]),
            ] {
                for t in tags {
                    c.execute(
                        "INSERT INTO photo_tag(photo_id, tag) VALUES(?1, ?2)",
                        rusqlite::params![id(i), t],
                    )?;
                }
            }
            for (i, sharp) in [
                (0, Some(0.9)),
                (1, Some(0.2)),
                (2, Some(0.35)),
                (3, Some(0.34)),
                (4, None),
                (5, Some(0.8)),
            ] {
                c.execute(
                    "INSERT INTO analysis(photo_id, sharpness) VALUES(?1, ?2)",
                    rusqlite::params![id(i), sharp],
                )?;
            }
            c.execute(
                "UPDATE photo SET issues=?1 WHERE id=?2",
                rusqlite::params![Issue::Blurry.bit(), id(5)],
            )?;
            let mut faces = Vec::new();
            for (name, photo, emb) in [
                ("a", 0, Some(unit([1.0, 0.0, 0.0, 0.0]))),
                ("b", 1, Some(unit([0.9, 0.436, 0.0, 0.0]))),
                ("c", 2, Some(unit([0.5, 0.866, 0.0, 0.0]))),
                ("d", 3, Some(unit([0.0, 1.0, 0.0, 0.0]))),
                ("e", 4, None),
                ("a2", 5, Some(unit([1.0, 0.0, 0.0, 0.0]))),
                ("x", 5, Some(unit([0.0, 0.0, 1.0, 0.0]))),
            ] {
                c.execute(
                    "INSERT INTO face(photo_id, bbox_x, bbox_y, bbox_w, bbox_h, embedding)
                     VALUES(?1, 0.1, 0.1, 0.3, 0.3, ?2)",
                    rusqlite::params![id(photo), emb],
                )?;
                faces.push((name, c.last_insert_rowid()));
            }
            Ok(faces)
        })
        .await
        .unwrap();
    Env {
        core,
        sid: s.id,
        ids,
        faces,
        _data: data,
        _src: src,
    }
}

#[tokio::test]
async fn lens_filter() {
    let e = env().await;
    let q = |lenses: &[&str]| PhotoQuery {
        lenses: lenses.iter().map(|s| s.to_string()).collect(),
        ..e.q()
    };
    assert_eq!(
        e.idx(q(&[ZOOM])).await,
        [0, 1, 2],
        "a name with a comma is one lens"
    );
    assert_eq!(e.idx(q(&["85mm"])).await, [3, 4]);
    assert_eq!(
        e.idx(q(&["85mm", ZOOM])).await,
        [0, 1, 2, 3, 4],
        "lenses are OR-ed"
    );
    assert_eq!(e.idx(q(&["50mm"])).await, [5, 6, 7, 8, 9]);
    assert!(e.idx(q(&["unknown lens"])).await.is_empty());
    assert_eq!(e.idx(e.q()).await.len(), 10, "empty list = no filter");
    // combined with an existing filter and with small pages (sort stays stable across pages)
    e.core
        .patch_photos(PatchRequest {
            ids: vec![e.ids[1], e.ids[4]],
            flag: Some(1),
            ..Default::default()
        })
        .await
        .unwrap();
    let both = PhotoQuery {
        flag: FlagFilter::Picked,
        ..q(&["85mm", ZOOM])
    };
    assert_eq!(e.idx(both).await, [1, 4]);
    let paged = PhotoQuery {
        limit: Some(2),
        sort: SortKey::TakenAtDesc,
        ..q(&["85mm", ZOOM])
    };
    assert_eq!(e.idx(paged).await, [4, 3, 2, 1, 0]);
}

#[tokio::test]
async fn taken_range_filter() {
    let e = env().await;
    let range = |from: Option<i64>, to: Option<i64>| PhotoQuery {
        taken_from: from,
        taken_to: to,
        ..e.q()
    };
    // both ends are inclusive
    assert_eq!(
        e.idx(range(Some(BASE + 2 * MIN), Some(BASE + 4 * MIN)))
            .await,
        [2, 3, 4]
    );
    assert_eq!(
        e.idx(range(Some(BASE + 7 * MIN), None)).await,
        [7, 8],
        "no capture time = out"
    );
    assert_eq!(e.idx(range(None, Some(BASE + MIN))).await, [0, 1]);
    assert!(e.idx(range(Some(BASE + 99 * MIN), None)).await.is_empty());
    assert_eq!(e.idx(range(None, None)).await.len(), 10);
    // inverted range is a request error, not an empty page
    assert!(matches!(
        e.core.photos(range(Some(BASE + 5 * MIN), Some(BASE))).await,
        Err(CoreError::BadRequest(_))
    ));
    // combined with a lens and a keyword, paged in capture order and in reverse
    let combo = PhotoQuery {
        lenses: vec![ZOOM.into(), "85mm".into()],
        limit: Some(1),
        ..range(Some(BASE + MIN), Some(BASE + 3 * MIN))
    };
    assert_eq!(e.idx(combo.clone()).await, [1, 2, 3]);
    let desc = PhotoQuery {
        sort: SortKey::TakenAtDesc,
        ..combo
    };
    assert_eq!(e.idx(desc).await, [3, 2, 1]);
    let tagged = PhotoQuery {
        tags: vec!["kyoto".into()],
        ..range(Some(BASE + MIN), Some(BASE + 7 * MIN))
    };
    assert_eq!(e.idx(tagged).await, [2, 7]);
}

#[tokio::test]
async fn tags_filter() {
    let e = env().await;
    let q = |tags: &[&str], all: bool| PhotoQuery {
        tags: tags.iter().map(|s| s.to_string()).collect(),
        tags_all: all,
        ..e.q()
    };
    assert_eq!(e.idx(q(&["travel"], false)).await, [0, 1, 7]);
    assert_eq!(
        e.idx(q(&["food", "travel"], false)).await,
        [0, 1, 2, 7],
        "any of them"
    );
    assert_eq!(
        e.idx(q(&["travel", "kyoto"], true)).await,
        [0, 7],
        "all of them"
    );
    assert_eq!(
        e.idx(q(&["travel", "food"], true)).await,
        Vec::<usize>::new()
    );
    assert_eq!(
        e.idx(q(&["  travel ", "travel"], true)).await,
        [0, 1, 7],
        "trimmed, de-duplicated"
    );
    assert!(e.idx(q(&["nothing"], false)).await.is_empty());
    assert_eq!(
        e.idx(q(&[], true)).await.len(),
        10,
        "empty list = no filter"
    );
    // existing filter + small pages
    e.core
        .patch_photos(PatchRequest {
            ids: vec![e.ids[7], e.ids[2]],
            user_rating: Some(Some(4)),
            ..Default::default()
        })
        .await
        .unwrap();
    let rated = PhotoQuery {
        rating_gte: Some(4),
        limit: Some(1),
        ..q(&["kyoto"], false)
    };
    assert_eq!(e.idx(rated).await, [2, 7]);
    let paged = PhotoQuery {
        limit: Some(3),
        sort: SortKey::Rating,
        ..q(&["travel", "kyoto", "food"], false)
    };
    let got = e.idx(paged).await;
    assert_eq!(
        got.iter().copied().collect::<HashSet<_>>(),
        HashSet::from([0, 1, 2, 7])
    );
    assert_eq!(got.len(), 4, "no photo twice across pages");
}

#[tokio::test]
async fn sharp_state_filter() {
    let e = env().await;
    let q = |s| PhotoQuery {
        sharp: Some(s),
        ..e.q()
    };
    assert_eq!(e.idx(q(SharpState::Sharp)).await, [0, 2]);
    assert_eq!(
        e.idx(q(SharpState::Blurry)).await,
        [1, 3, 5],
        "below the blurry threshold, or flagged blurry"
    );
    assert_eq!(e.idx(q(SharpState::Unknown)).await, [4, 6, 7, 8, 9]);
    // the three states partition the session
    let mut all: Vec<usize> = Vec::new();
    for s in [SharpState::Sharp, SharpState::Blurry, SharpState::Unknown] {
        all.extend(e.idx(q(s)).await);
    }
    all.sort_unstable();
    assert_eq!(all, (0..10).collect::<Vec<_>>());
    // combined with lens + keyword, paged
    let combo = PhotoQuery {
        lenses: vec![ZOOM.into()],
        limit: Some(1),
        ..q(SharpState::Blurry)
    };
    assert_eq!(e.idx(combo).await, [1]);
    let tagged = PhotoQuery {
        tags: vec!["travel".into()],
        limit: Some(1),
        ..q(SharpState::Unknown)
    };
    assert_eq!(e.idx(tagged).await, [7]);
    // the existing `issues_any=blurry` filter and the new one agree on the flagged photo
    let issue = PhotoQuery {
        issues_any: vec![Issue::Blurry],
        ..q(SharpState::Blurry)
    };
    assert_eq!(e.idx(issue).await, [5]);
}

#[tokio::test]
async fn similar_faces_filter() {
    let e = env().await;
    let q = |faces: &[&str], sim: Option<f64>| PhotoQuery {
        face_ids: faces.iter().map(|f| e.face(f)).collect(),
        face_similarity: sim,
        ..e.q()
    };
    // the given face's own photo, and the other photo with an identical face (unclustered faces)
    assert_eq!(e.idx(q(&["a"], Some(0.95))).await, [0, 5]);
    assert_eq!(e.idx(q(&["a"], Some(0.85))).await, [0, 1, 5]);
    assert_eq!(e.idx(q(&["a"], Some(0.4))).await, [0, 1, 2, 5]);
    assert_eq!(
        e.idx(q(&["a"], None)).await,
        [0, 1, 2, 5],
        "default = the clustering threshold ({})",
        DEFAULT_FACE_SIMILARITY
    );
    assert_eq!(
        e.idx(q(&["a"], Some(0.0))).await,
        [0, 1, 2, 3, 5],
        "faces without embedding never match"
    );
    assert_eq!(e.idx(q(&["a"], Some(1.0))).await, [0, 5]);
    // several faces: any of them
    assert_eq!(e.idx(q(&["d", "x"], Some(0.95))).await, [3, 5]);
    // combined with another filter, paged
    let combo = PhotoQuery {
        limit: Some(1),
        sharp: Some(SharpState::Blurry),
        ..q(&["a"], Some(0.4))
    };
    assert_eq!(e.idx(combo).await, [1, 5]);
    let tagged = PhotoQuery {
        tags: vec!["kyoto".into()],
        limit: Some(1),
        sort: SortKey::TakenAtDesc,
        ..q(&["a"], Some(0.4))
    };
    assert_eq!(e.idx(tagged).await, [2, 0]);
    // errors
    let missing = PhotoQuery {
        face_ids: vec![9_999_999],
        ..e.q()
    };
    assert!(matches!(
        e.core.photos(missing).await,
        Err(CoreError::BadRequest(_))
    ));
    assert!(matches!(
        e.core.photos(q(&["e"], None)).await,
        Err(CoreError::Conflict(_))
    ));
    for bad in [-0.1, 1.5] {
        assert!(matches!(
            e.core.photos(q(&["a"], Some(bad))).await,
            Err(CoreError::BadRequest(_))
        ));
    }
}

#[tokio::test]
async fn similar_faces_stay_inside_the_session() {
    let e = env().await;
    let other = tempfile::tempdir().unwrap();
    crate::testutil::write_jpeg(&other.path().join("solo.jpg"), 64, 48, 5);
    let s2 = e
        .core
        .import(ImportRequest {
            path: other.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    let s2 = e.core.wait_session_ready(s2.id, WAIT).await.unwrap();
    let p2 = e
        .core
        .photos(PhotoQuery {
            session_id: s2.id,
            ..Default::default()
        })
        .await
        .unwrap()
        .photos[0]
        .id;
    e.core
        .db
        .call(move |c| {
            c.execute(
                "INSERT INTO face(photo_id, embedding) VALUES(?1, ?2)",
                rusqlite::params![p2, unit([1.0, 0.0, 0.0, 0.0])],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let in_s2 = e
        .core
        .photos(PhotoQuery {
            session_id: s2.id,
            face_ids: vec![e.face("a")],
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(in_s2.total, 1, "a face of another session can be the query");
    assert_eq!(in_s2.photos[0].id, p2);
    let q = PhotoQuery {
        face_ids: vec![e.face("a")],
        face_similarity: Some(0.95),
        ..e.q()
    };
    assert_eq!(
        e.idx(q).await,
        [0, 5],
        "the other session's photo is not listed"
    );
}

#[tokio::test]
async fn filter_options_list_session_values() {
    let e = env().await;
    let o = e.core.filter_options(e.sid).await.unwrap();
    let lenses: Vec<(&str, i64)> = o
        .lenses
        .iter()
        .map(|l| (l.lens.as_str(), l.photo_count))
        .collect();
    assert_eq!(
        lenses,
        [("50mm", 5), (ZOOM, 3), ("85mm", 2)],
        "most photos first"
    );
    let tags: Vec<(&str, i64)> = o
        .tags
        .iter()
        .map(|t| (t.tag.as_str(), t.photo_count))
        .collect();
    assert_eq!(tags, [("kyoto", 3), ("travel", 3), ("food", 1)]);
    assert_eq!(o.taken_min, Some(BASE));
    assert_eq!(
        o.taken_max,
        Some(BASE + 8 * MIN),
        "the photo without a capture time is ignored"
    );
    assert!(matches!(
        e.core.filter_options(9_999).await,
        Err(CoreError::NotFound(_))
    ));
}

#[tokio::test]
async fn face_search_floor_is_adjustable() {
    let e = env().await;
    let a = e.face("a");
    let hits = |out: &crate::analysis::faces::FaceSearchOut| {
        out.similar_faces
            .iter()
            .map(|f| f.photo_id)
            .collect::<Vec<_>>()
    };
    let p = |i: usize| e.ids[i];
    // default floor 0.25: b (.9), c (.5) and the identical face on photo 5
    let out = e.core.faces_search_face(a, None).await.unwrap();
    assert_eq!(hits(&out), [p(5), p(1), p(2)]);
    let strict = e
        .core
        .faces_search_face_min(a, None, Some(0.8))
        .await
        .unwrap();
    assert_eq!(hits(&strict), [p(5), p(1)]);
    let loose = e
        .core
        .faces_search_face_min(a, None, Some(0.0))
        .await
        .unwrap();
    assert_eq!(
        hits(&loose).len(),
        5,
        "everything with an embedding but the query face"
    );
    let exact = e
        .core
        .faces_search_face_min(a, None, Some(1.0))
        .await
        .unwrap();
    assert_eq!(hits(&exact), [p(5)]);
    for bad in [-0.5, 1.01] {
        assert!(matches!(
            e.core.faces_search_face_min(a, None, Some(bad)).await,
            Err(CoreError::BadRequest(_))
        ));
    }
}
