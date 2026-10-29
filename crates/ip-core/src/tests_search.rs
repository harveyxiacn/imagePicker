//! Natural-language search (docs/09 P1-4): ranking, threshold, filters, scope and error mapping
//! with the fake worker's `text.embed`. Embeddings are seeded in SQL (4 dimensions).

use std::sync::Arc;
use std::time::Duration;

use crate::analysis::vecs::encode_f16;
use crate::fake_worker::FakeWorker;
use crate::search::{score_of, SearchRequest, CONFIDENT_SIMILARITY};
use crate::testutil::{make_photos, FakeImaging};
use crate::*;

const WAIT: Duration = Duration::from_secs(30);

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    sid: i64,
    /// photo ids, `ids[i]` = `img_00{i}.jpg`
    ids: Vec<i64>,
    _data: tempfile::TempDir,
    _src: tempfile::TempDir,
}

fn unit(v: [f32; 4]) -> Vec<u8> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    encode_f16(&v.iter().map(|x| x / n).collect::<Vec<_>>())
}

/// 8 photos. Query "sunset" = e0 and "dog" = e1; cosine of each photo to "sunset":
/// 0: 1.0, 1: .9, 2: .5, 3: 0 (it is "dog"), 4: no analysis row, 5: .08 (below the confident
/// similarity, the default threshold of a scope this small), 6: .15 (above it), 7: analysed
/// without an embedding.
async fn env_with(worker: Arc<dyn ip_worker_client::AiWorker>, fake: Arc<FakeWorker>) -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker),
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    make_photos(src.path(), 8);
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
    assert_eq!(ids.len(), 8);
    let ids2 = ids.clone();
    core.db
        .call(move |c| {
            let sin = |cos: f32| (1.0 - cos * cos).sqrt();
            for (i, emb) in [
                (0, Some(unit([1.0, 0.0, 0.0, 0.0]))),
                (1, Some(unit([0.9, sin(0.9), 0.0, 0.0]))),
                (2, Some(unit([0.5, sin(0.5), 0.0, 0.0]))),
                (3, Some(unit([0.0, 1.0, 0.0, 0.0]))),
                (5, Some(unit([0.08, sin(0.08), 0.0, 0.0]))),
                (6, Some(unit([0.15, sin(0.15), 0.0, 0.0]))),
                (7, None),
            ] {
                c.execute(
                    "INSERT INTO analysis(photo_id, embedding) VALUES(?1, ?2)",
                    rusqlite::params![ids2[i], emb],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();
    fake.set_text_vector("sunset", vec![1.0, 0.0, 0.0, 0.0]);
    fake.set_text_vector("dog", vec![0.0, 1.0, 0.0, 0.0]);
    Env {
        core,
        worker: fake,
        sid: s.id,
        ids,
        _data: data,
        _src: src,
    }
}

async fn env() -> Env {
    let fake = Arc::new(FakeWorker::new());
    env_with(fake.clone(), fake).await
}

impl Env {
    fn req(&self, q: &str) -> SearchRequest {
        SearchRequest {
            q: q.into(),
            filter: Some(PhotoQuery {
                session_id: self.sid,
                ..Default::default()
            }),
            ..Default::default()
        }
    }
    /// Indexes into `ids` of the hits, best first.
    fn idx(&self, out: &crate::search::SearchOut) -> Vec<usize> {
        out.hits
            .iter()
            .map(|h| self.ids.iter().position(|i| *i == h.photo_id).unwrap())
            .collect()
    }
}

#[tokio::test]
async fn ranks_by_similarity_and_applies_the_default_threshold() {
    let e = env().await;
    let out = e.core.search_photos(e.req("sunset")).await.unwrap();
    assert_eq!(
        e.idx(&out),
        [0, 1, 2, 6],
        "best first; photo 5 is below the threshold"
    );
    assert_eq!(out.threshold, "adaptive");
    assert_eq!(out.min_similarity, Some(0.095));
    assert!((out.min_score - score_of(CONFIDENT_SIMILARITY)).abs() < 1e-6);
    assert!(!out.low_confidence);
    assert_eq!((out.total_matched, out.searched, out.unindexed), (4, 6, 2));
    assert_eq!(out.query, "sunset");
    assert_eq!(out.model, "fake-siglip-text");
    let s: Vec<f64> = out.hits.iter().map(|h| h.score).collect();
    assert!(s.windows(2).all(|w| w[0] >= w[1]), "scores descend: {s:?}");
    assert!((out.hits[0].similarity - 1.0).abs() < 1e-3);
    assert!((out.hits[1].similarity - 0.9).abs() < 2e-3);
    assert!(out.hits[0].score > 0.99 && out.hits[3].score < 0.9 && out.hits[3].score > 0.5);
    // strong matches all saturate to a score of 1.0: the cosine still orders them
    assert_eq!(out.hits[1].score, 1.0);
    assert!(out.hits[0].similarity > out.hits[1].similarity);
    // another query ranks the other photo first
    let dog = e.core.search_photos(e.req("dog")).await.unwrap();
    assert_eq!(e.idx(&dog)[0], 3);
}

#[tokio::test]
async fn min_score_and_limit() {
    let e = env().await;
    let all = e
        .core
        .search_photos(SearchRequest {
            min_score: Some(0.0),
            ..e.req("sunset")
        })
        .await
        .unwrap();
    assert_eq!(all.hits.len(), 6, "every photo with an embedding, ranked");
    assert_eq!(e.idx(&all), [0, 1, 2, 6, 5, 3]);
    let strict = e
        .core
        .search_photos(SearchRequest {
            min_score: Some(0.9),
            ..e.req("sunset")
        })
        .await
        .unwrap();
    assert_eq!(e.idx(&strict), [0, 1, 2], "photo 6 (score .6) falls out");
    assert_eq!((strict.threshold, strict.min_score), ("min_score", 0.9));
    assert_eq!(strict.min_similarity, None);
    let top2 = e
        .core
        .search_photos(SearchRequest {
            limit: Some(2),
            ..e.req("sunset")
        })
        .await
        .unwrap();
    assert_eq!(e.idx(&top2), [0, 1]);
    assert_eq!(top2.total_matched, 4, "the count ignores the limit");
}

#[tokio::test]
async fn a_query_nothing_matches_is_low_confidence() {
    let e = env().await;
    // only photo 5 (cosine .08) and photo 3 (0) are in scope: both below the confident similarity
    let ids = e.ids.clone();
    e.core
        .db
        .call(move |c| {
            for i in [3, 5] {
                c.execute("UPDATE photo SET flag=1 WHERE id=?1", [ids[i]])?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let mut req = e.req("sunset");
    req.filter.as_mut().unwrap().flag = FlagFilter::Picked;
    let out = e.core.search_photos(req.clone()).await.unwrap();
    assert!(out.hits.is_empty() && out.low_confidence);
    assert_eq!(out.searched, 2);
    // an explicit threshold returns the nearest photos, still flagged
    req.min_score = Some(0.0);
    let out = e.core.search_photos(req).await.unwrap();
    assert_eq!(e.idx(&out), [5, 3]);
    assert!(out.low_confidence);
    // the whole session has strong matches
    assert!(
        !e.core
            .search_photos(e.req("sunset"))
            .await
            .unwrap()
            .low_confidence
    );
}

#[tokio::test]
async fn photo_filters_narrow_the_scope() {
    let e = env().await;
    let ids = e.ids.clone();
    e.core
        .db
        .call(move |c| {
            for i in [1, 2, 3] {
                c.execute("UPDATE photo SET flag=1 WHERE id=?1", [ids[i]])?;
            }
            Ok(())
        })
        .await
        .unwrap();
    let mut req = e.req("sunset");
    req.filter.as_mut().unwrap().flag = FlagFilter::Picked;
    let out = e.core.search_photos(req).await.unwrap();
    assert_eq!(
        e.idx(&out),
        [1, 2],
        "photo 0 is not picked; photo 3 does not match"
    );
    assert_eq!(out.searched, 3, "only the filtered photos are compared");
    assert_eq!(out.unindexed, 0);
}

#[tokio::test]
async fn library_scope_skips_missing_photos() {
    let e = env().await;
    let mut req = e.req("sunset");
    req.filter = None;
    let out = e.core.search_photos(req.clone()).await.unwrap();
    assert_eq!(e.idx(&out), [0, 1, 2, 6]);
    let id = e.ids[1];
    e.core
        .db
        .call(move |c| {
            c.execute("UPDATE photo SET missing=1 WHERE id=?1", [id])?;
            Ok(())
        })
        .await
        .unwrap();
    let out = e.core.search_photos(req).await.unwrap();
    assert_eq!(e.idx(&out), [0, 2, 6]);
}

#[tokio::test]
async fn query_is_cleaned_before_it_reaches_the_worker() {
    let e = env().await;
    // not in the vector table: the fake answers a deterministic pseudo vector
    e.core.search_photos(e.req("  海边   日落 ")).await.unwrap();
    let sent = e.worker.last_text_request.lock().unwrap().clone().unwrap();
    assert_eq!(sent.texts, ["海边 日落"]);
    assert!(!sent.allow_download, "searching never downloads models");
}

#[tokio::test]
async fn validation_errors() {
    let e = env().await;
    for q in ["", "   \n"] {
        assert!(matches!(
            e.core.search_photos(e.req(q)).await,
            Err(CoreError::BadRequest(_))
        ));
    }
    let long = "x".repeat(300);
    assert!(matches!(
        e.core.search_photos(e.req(&long)).await,
        Err(CoreError::BadRequest(_))
    ));
    for min_score in [-0.1, 1.5] {
        let r = SearchRequest {
            min_score: Some(min_score),
            ..e.req("sunset")
        };
        assert!(matches!(
            e.core.search_photos(r).await,
            Err(CoreError::BadRequest(_))
        ));
    }
    for limit in [0, 5001] {
        let r = SearchRequest {
            limit: Some(limit),
            ..e.req("sunset")
        };
        assert!(matches!(
            e.core.search_photos(r).await,
            Err(CoreError::BadRequest(_))
        ));
    }
    let mut r = e.req("sunset");
    r.filter.as_mut().unwrap().session_id = 9999;
    assert!(matches!(
        e.core.search_photos(r).await,
        Err(CoreError::NotFound(_))
    ));
    assert_eq!(
        e.worker
            .text_embed_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "bad requests never reach the worker"
    );
}

#[tokio::test]
async fn missing_text_model_is_models_missing() {
    let e = env().await;
    e.worker.set_text_missing(&["siglip2-base-text"]);
    match e.core.search_photos(e.req("sunset")).await {
        Err(CoreError::ModelsMissing(m)) => assert_eq!(m, ["siglip2-base-text"]),
        other => panic!("expected models_missing, got {other:?}"),
    }
    e.worker.set_text_missing(&[]);
    assert!(e.core.search_photos(e.req("sunset")).await.is_ok());
}

#[tokio::test]
async fn worker_without_text_embed_is_unavailable() {
    let fake = Arc::new(FakeWorker::new());
    let e = env_with(
        Arc::new(ip_worker_client::UnavailableWorker::new("no ai here")),
        fake,
    )
    .await;
    match e.core.search_photos(e.req("sunset")).await {
        Err(CoreError::WorkerUnavailable(m)) => assert!(m.contains("no ai here")),
        other => panic!("expected worker_unavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn only_ids_restricts_the_photo_list() {
    let e = env().await;
    let list = |ids: Option<Vec<i64>>| {
        let q = PhotoQuery {
            session_id: e.sid,
            sort: SortKey::Name,
            only_ids: ids,
            ..Default::default()
        };
        let core = e.core.clone();
        async move { core.photos(q).await.unwrap() }
    };
    let page = list(Some(vec![e.ids[2], e.ids[0], 424242])).await;
    assert_eq!(page.total, 2, "unknown ids are ignored");
    assert_eq!(
        page.photos.iter().map(|p| p.id).collect::<Vec<_>>(),
        [e.ids[0], e.ids[2]]
    );
    assert_eq!(
        list(Some(vec![])).await.total,
        0,
        "an empty list matches nothing"
    );
    assert_eq!(list(None).await.total, 8);
}
