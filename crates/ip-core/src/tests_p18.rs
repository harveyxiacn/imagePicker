//! P1-8 end to end (FakeImaging + FakeWorker): the bystander / bad-expression tags, statues and
//! posters, the stored composition score, and people that are human with a visible cover.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::analysis::scoring::Issue;
use crate::analysis::*;
use crate::fake_worker::{person, FakeFace, FakeSpec, FakeWorker};
use crate::testutil::{write_jpeg, FakeImaging};
use crate::*;

const BASE: u64 = 1_700_000_000;

struct Env {
    core: Arc<Core>,
    worker: Arc<FakeWorker>,
    _data: tempfile::TempDir,
    src: tempfile::TempDir,
}

fn env() -> Env {
    let data = tempfile::tempdir().unwrap();
    let src = tempfile::tempdir().unwrap();
    let worker = Arc::new(FakeWorker::new());
    let core = Core::open(CoreConfig {
        data_dir: Some(data.path().to_path_buf()),
        imaging: Arc::new(FakeImaging::new()),
        thumb_workers: Some(2),
        worker: Some(worker.clone()),
        renderer: None,
        force_cpu: false,
    })
    .unwrap();
    crate::testutil::grant_face_consent(&core);
    Env {
        core,
        worker,
        _data: data,
        src,
    }
}

/// One photo per hour so that every file is its own burst.
fn photo(dir: &Path, name: &str, i: u64) {
    let p = dir.join(name);
    write_jpeg(&p, 320, 240, (i * 7 % 200) as u8);
    let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
    f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(BASE + 3600 * i))
        .unwrap();
}

async fn analysed(e: &Env) -> (i64, HashMap<String, Photo>) {
    let s = e
        .core
        .import(ImportRequest {
            path: e.src.path().to_string_lossy().into_owned(),
            recursive: true,
            title: None,
        })
        .await
        .unwrap();
    e.core
        .wait_session_ready(s.id, Duration::from_secs(30))
        .await
        .unwrap();
    e.core
        .analysis_run(AnalysisRunRequest {
            session_id: s.id,
            profile: Profile::Standard,
            photo_ids: None,
            force: false,
            allow_download: false,
            fallback_lite: false,
        })
        .await
        .unwrap();
    let t = std::time::Instant::now();
    while e.core.analysis_status(s.id).state == RunState::Running {
        assert!(t.elapsed() < Duration::from_secs(120), "analysis hung");
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    let photos = e
        .core
        .photos(PhotoQuery {
            session_id: s.id,
            limit: Some(100),
            ..Default::default()
        })
        .await
        .unwrap()
        .photos
        .into_iter()
        .map(|p| (p.file_name.clone(), p))
        .collect();
    (s.id, photos)
}

/// The main subject sits on the left third line, eyes on the upper third.
const MAIN: [f64; 4] = [0.26, 0.25, 0.14, 0.2];
/// A sharp, sizeable face far from the subject.
const STRANGER: [f64; 4] = [0.8, 0.6, 0.07, 0.1];

fn main_face() -> FakeFace {
    FakeFace::new(MAIN, person(0, 0.0))
}

#[tokio::test]
async fn bystander_tag_skips_statues_and_posters_but_not_strangers() {
    let e = env();
    let d = e.src.path();
    for (i, n) in ["real.jpg", "statue.jpg", "alone.jpg"].iter().enumerate() {
        photo(d, n, i as u64);
    }
    e.worker.set(
        "real.jpg",
        FakeSpec::at(10.0).with_faces(vec![
            main_face(),
            FakeFace::new(STRANGER, person(1, 0.0)).artificial(0.1),
        ]),
    );
    e.worker.set(
        "statue.jpg",
        FakeSpec::at(80.0).with_faces(vec![
            main_face(),
            FakeFace::new(STRANGER, person(2, 0.0)).artificial(0.92),
        ]),
    );
    e.worker.set(
        "alone.jpg",
        FakeSpec::at(150.0).with_faces(vec![main_face()]),
    );
    let (sid, ph) = analysed(&e).await;

    assert!(ph["real.jpg"].issues.contains(&Issue::Bystander));
    assert!(!ph["statue.jpg"].issues.contains(&Issue::Bystander));
    assert!(!ph["alone.jpg"].issues.contains(&Issue::Bystander));
    // the tag does not cost stars: same score as the photo without the stranger
    assert_eq!(ph["real.jpg"].ai_score, ph["alone.jpg"].ai_score);

    // the filter finds exactly the real one
    let page = e
        .core
        .photos(PhotoQuery {
            session_id: sid,
            issues_any: vec![Issue::Bystander],
            ..Default::default()
        })
        .await
        .unwrap();
    let found: Vec<&str> = page.photos.iter().map(|p| p.file_name.as_str()).collect();
    assert_eq!(found, vec!["real.jpg"]);

    // "remove bystanders" candidates: the stranger yes, the statue no (removal by hand stays possible)
    assert_eq!(e.core.bystanders(ph["real.jpg"].id).await.unwrap().len(), 1);
    assert!(e
        .core
        .bystanders(ph["statue.jpg"].id)
        .await
        .unwrap()
        .is_empty());
    let detail = e.core.photo_analysis(ph["statue.jpg"].id).await.unwrap();
    let statue = detail.faces.iter().find(|f| !f.is_subject).unwrap();
    assert!(statue.likely_artificial);
    assert_eq!(statue.artificial, Some(0.92));
    let real = e.core.photo_analysis(ph["real.jpg"].id).await.unwrap();
    assert!(real.faces.iter().all(|f| !f.likely_artificial));
}

#[tokio::test]
async fn bad_expression_tag_for_a_subject_mid_word() {
    let e = env();
    let d = e.src.path();
    for (i, n) in ["talk.jpg", "laugh.jpg", "calm.jpg"].iter().enumerate() {
        photo(d, n, i as u64);
    }
    let face = |smile: f64, mouth: f64| {
        FakeSpec::at(30.0 * (smile as f32 + mouth as f32 + 1.0))
            .with_faces(vec![main_face().smile(smile).mouth(mouth)])
    };
    e.worker.set("talk.jpg", face(0.05, 0.45));
    e.worker.set("laugh.jpg", face(0.9, 0.6));
    e.worker.set("calm.jpg", face(0.05, 0.02));
    let (_sid, ph) = analysed(&e).await;
    assert!(ph["talk.jpg"].issues.contains(&Issue::BadExpression));
    assert!(!ph["laugh.jpg"].issues.contains(&Issue::BadExpression));
    assert!(!ph["calm.jpg"].issues.contains(&Issue::BadExpression));
    let d = e.core.photo_analysis(ph["talk.jpg"].id).await.unwrap();
    let r = d
        .reasons
        .iter()
        .find(|r| r.key == "bad_expression")
        .unwrap();
    assert_eq!(r.params["kind"], "talking");
}

#[tokio::test]
async fn composition_is_stored_and_ranks_the_better_framed_photo() {
    let e = env();
    let d = e.src.path();
    for (i, n) in ["framed.jpg", "corner.jpg", "none.jpg"].iter().enumerate() {
        photo(d, n, i as u64);
    }
    e.worker.set(
        "framed.jpg",
        FakeSpec::at(20.0).with_faces(vec![main_face()]),
    );
    // same face quality, jammed into the top-right corner
    e.worker.set(
        "corner.jpg",
        FakeSpec::at(110.0)
            .with_faces(vec![FakeFace::new([0.84, 0.0, 0.16, 0.22], person(0, 0.0))]),
    );
    e.worker.set("none.jpg", FakeSpec::at(200.0));
    let (_sid, ph) = analysed(&e).await;
    let comp = |n: &str| {
        let id = ph[n].id;
        let core = e.core.clone();
        async move {
            core.photo_analysis(id)
                .await
                .unwrap()
                .scores
                .unwrap()
                .composition
        }
    };
    let (framed, corner, none) = (
        comp("framed.jpg").await,
        comp("corner.jpg").await,
        comp("none.jpg").await,
    );
    assert!(framed.unwrap() > 0.9, "{framed:?}");
    assert!(corner.unwrap() < 0.45, "{corner:?}");
    assert!(
        none.is_none(),
        "no face and no saliency box: no composition"
    );
    // the composition is the only thing that differs between these two photos
    assert!(ph["framed.jpg"].ai_score.unwrap() > ph["corner.jpg"].ai_score.unwrap());
    // and a photo without one is scored as before (components renormalised)
    let d = e.core.photo_analysis(ph["none.jpg"].id).await.unwrap();
    assert!(d.contributions.iter().all(|c| c.key != "composition"));
}

/// The dog walked next to the subject: about as big and central as she is.
const DOG: [f64; 4] = [0.45, 0.5, 0.16, 0.22];

#[tokio::test]
async fn a_recurring_dog_is_no_person_no_subject_and_no_bystander() {
    let e = env();
    let d = e.src.path();
    let names = [
        "walk1.jpg",
        "walk2.jpg",
        "walk3.jpg",
        "dog.jpg",
        "parrot.jpg",
    ];
    for (i, n) in names.iter().enumerate() {
        photo(d, n, i as u64);
    }
    // YuNet found the dog, the landmarker did not (one of the 4 dog faces got a mesh anyway, as
    // on the library: the cluster is still mostly not human)
    let dog = |mesh: bool| {
        let f = FakeFace::new(DOG, person(5, 0.0));
        if mesh {
            f
        } else {
            f.not_human()
        }
    };
    for (i, n) in names[..3].iter().enumerate() {
        e.worker.set(
            n,
            FakeSpec::at(20.0 * i as f32).with_faces(vec![main_face(), dog(i == 0)]),
        );
    }
    e.worker
        .set("dog.jpg", FakeSpec::at(90.0).with_faces(vec![dog(false)]));
    // a single unrecognised face: no singleton person, but not marked either
    e.worker.set(
        "parrot.jpg",
        FakeSpec::at(150.0).with_faces(vec![FakeFace::new(MAIN, person(6, 0.0)).not_human()]),
    );
    let (sid, ph) = analysed(&e).await;

    let people = e.core.people(Some(sid)).await.unwrap();
    assert_eq!(people.len(), 1, "{people:?}");
    assert_eq!(people[0].photo_count, 3);
    for n in &names[..3] {
        let a = e.core.photo_analysis(ph[*n].id).await.unwrap();
        let (her, dog): (Vec<_>, Vec<_>) = a.faces.iter().partition(|f| !f.nonhuman);
        assert_eq!((her.len(), dog.len()), (1, 1), "{n}");
        assert!(her[0].is_subject && her[0].person_id == Some(people[0].id));
        assert!(!dog[0].is_subject && dog[0].person_id.is_none());
        assert!(!ph[*n].issues.contains(&Issue::Bystander), "{n}");
        assert!(e.core.bystanders(ph[*n].id).await.unwrap().is_empty());
    }
    let parrot = e.core.photo_analysis(ph["parrot.jpg"].id).await.unwrap();
    assert!(parrot.faces[0].person_id.is_none() && !parrot.faces[0].nonhuman);
    let all = e.core.people_with(Some(sid), true).await.unwrap();
    assert_eq!(
        all.len(),
        1,
        "no singleton for the unrecognised face: {all:?}"
    );
}

#[tokio::test]
async fn cover_is_the_visible_face_not_the_biggest_backlit_one() {
    let e = env();
    let d = e.src.path();
    let names = ["backlit.jpg", "lit1.jpg", "lit2.jpg"];
    for (i, n) in names.iter().enumerate() {
        photo(d, n, i as u64);
    }
    let big = [0.3, 0.2, 0.3, 0.4];
    e.worker.set(
        "backlit.jpg",
        FakeSpec::at(0.0).with_faces(vec![FakeFace::new(big, person(0, 0.0)).tone(0.05, 0.015)]),
    );
    for (i, n) in names[1..].iter().enumerate() {
        e.worker.set(
            n,
            FakeSpec::at(40.0 * (i + 1) as f32)
                .with_faces(vec![main_face().tone(0.5 + 0.05 * i as f64, 0.11)]),
        );
    }
    let (sid, ph) = analysed(&e).await;
    let people = e.core.people(Some(sid)).await.unwrap();
    assert_eq!(people.len(), 1);
    let backlit = e.core.photo_analysis(ph["backlit.jpg"].id).await.unwrap();
    assert_eq!(backlit.faces[0].person_id, Some(people[0].id));
    assert_ne!(people[0].cover_face_id, Some(backlit.faces[0].id));
}
