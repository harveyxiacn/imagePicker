//! `imagepicker` command line: `serve` the API/WebUI, or run a headless `import`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ip_core::{
    AnalysisRunRequest, AnalysisStatus, Core, CoreConfig, ImportRequest, ImportState, PhotoQuery,
    Profile, RunState,
};
use ip_server::ServerConfig;

#[derive(Parser, Debug)]
#[command(name = "imagepicker", version, about = "Local-first AI photo culling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Run the HTTP/WebSocket API and (optionally) serve the web UI.
    Serve {
        #[arg(long, default_value_t = 7878)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        /// Data directory (default: platform app-data dir or $IMAGEPICKER_DATA_DIR).
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Directory with the built web UI (default: ./web/dist if present).
        #[arg(long)]
        web_dir: Option<PathBuf>,
    },
    /// Import a folder without a UI, wait for thumbnails and print statistics.
    Import {
        dir: PathBuf,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Do not descend into sub-folders.
        #[arg(long)]
        no_recursive: bool,
        #[arg(long)]
        title: Option<String>,
    },
    /// Analyse a session (id) or a folder (imported first): grouping, scoring, people.
    Analyze {
        /// Session id, or a folder to import and analyse.
        target: String,
        /// `fast` or `standard`.
        #[arg(long, default_value = "standard")]
        profile: String,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Directory with the worker's models (sets IMAGEPICKER_MODELS_DIR).
        #[arg(long)]
        models_dir: Option<PathBuf>,
        /// Let the worker download missing models (they can be large).
        #[arg(long)]
        allow_download: bool,
        /// Re-analyse photos that already have a result.
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug)]
struct ImportStats {
    session_id: i64,
    photos: i64,
    thumbs_ready: i64,
    elapsed: Duration,
}

async fn import_headless(
    core: &Arc<Core>,
    dir: PathBuf,
    recursive: bool,
    title: Option<String>,
) -> Result<ImportStats> {
    let started = Instant::now();
    let session = core
        .import(ImportRequest {
            path: dir.to_string_lossy().into_owned(),
            recursive,
            title,
        })
        .await
        .context("import")?;
    let mut last_print = Instant::now() - Duration::from_secs(1);
    loop {
        let s = core.session(session.id).await?;
        if last_print.elapsed() >= Duration::from_millis(500) {
            eprintln!("[{}] {} photos", s.import_state.as_str(), s.photo_count);
            last_print = Instant::now();
        }
        if s.import_state == ImportState::Ready {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let elapsed = started.elapsed();
    let mut photos = 0;
    let mut thumbs_ready = 0;
    let mut cursor = None;
    loop {
        let page = core
            .photos(PhotoQuery {
                session_id: session.id,
                limit: Some(5000),
                cursor: cursor.take(),
                ..Default::default()
            })
            .await?;
        photos += page.photos.len() as i64;
        thumbs_ready += page.photos.iter().filter(|p| p.thumb_ready).count() as i64;
        match page.next_cursor {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    Ok(ImportStats {
        session_id: session.id,
        photos,
        thumbs_ready,
        elapsed,
    })
}

fn print_stats(s: &ImportStats) {
    let secs = s.elapsed.as_secs_f64().max(0.001);
    println!("session:       {}", s.session_id);
    println!("photos:        {}", s.photos);
    println!(
        "thumbnails:    {} ready, {} failed",
        s.thumbs_ready,
        s.photos - s.thumbs_ready
    );
    println!(
        "elapsed:       {secs:.2}s ({:.0} photos/s)",
        s.photos as f64 / secs
    );
}

/// What `analyze` prints.
#[derive(Debug)]
struct AnalyzeReport {
    session_id: i64,
    status: AnalysisStatus,
    /// (stage, seconds) in the order the stages were seen.
    stages: Vec<(String, f64)>,
    total_secs: f64,
    photos: usize,
    analyzed: usize,
    scenes: usize,
    bursts: usize,
    stacked_bursts: usize,
    largest_burst: i64,
    stars: std::collections::BTreeMap<String, usize>,
    issues: std::collections::BTreeMap<String, usize>,
    people: usize,
    faces: i64,
    sample_groups: Vec<String>,
}

async fn analyze_headless(
    core: &Arc<Core>,
    target: &str,
    profile: Profile,
    force: bool,
    allow_download: bool,
) -> Result<AnalyzeReport> {
    let session_id = match target.parse::<i64>() {
        Ok(id) => {
            core.session(id).await.context("session")?;
            id
        }
        Err(_) => {
            let t0 = Instant::now();
            let st = import_headless(core, PathBuf::from(target), true, None).await?;
            eprintln!(
                "imported {} photos in {:.2}s",
                st.photos,
                t0.elapsed().as_secs_f64()
            );
            st.session_id
        }
    };
    let started = Instant::now();
    let mut rx = core.events.subscribe();
    core.analysis_run(AnalysisRunRequest {
        session_id,
        profile,
        photo_ids: None,
        force,
        allow_download,
    })
    .await
    .context("start analysis")?;
    // Stage timings come from the progress events (exact), not from polling.
    let mut stages: Vec<(String, f64)> = Vec::new();
    let mut cur: Option<(String, Instant)> = None;
    let mut last_print = Instant::now() - Duration::from_secs(1);
    loop {
        let ev = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
        let (state, stage, done, total) = match ev {
            Ok(Ok(ip_core::Event::AnalysisProgress {
                session_id: s,
                state,
                stage,
                done,
                total,
            })) if s == session_id => (state, stage, done, total),
            Ok(_) => continue,
            Err(_) => {
                // quiet period: make sure we did not miss the end
                let st = core.analysis_status(session_id);
                if st.state == RunState::Running {
                    continue;
                }
                (st.state, st.stage, st.done, st.total)
            }
        };
        let changed = match &cur {
            Some(c) => Some(&c.0) != stage.as_ref(),
            None => stage.is_some(),
        };
        if changed {
            if let Some((name, t)) = cur.take() {
                stages.push((name, t.elapsed().as_secs_f64()));
            }
            if let Some(s) = stage.clone() {
                eprintln!("[{s}]");
                cur = Some((s, Instant::now()));
            }
        }
        if state != RunState::Running {
            if let Some((name, t)) = cur.take() {
                stages.push((name, t.elapsed().as_secs_f64()));
            }
            break;
        }
        if last_print.elapsed() >= Duration::from_millis(500) {
            eprintln!("  {done}/{total}");
            last_print = Instant::now();
        }
    }
    let status = core.analysis_status(session_id);
    let total_secs = started.elapsed().as_secs_f64();

    let page = core
        .photos(PhotoQuery {
            session_id,
            limit: Some(5000),
            ..Default::default()
        })
        .await?;
    let groups = core.groups(session_id).await?;
    let people = core.people(Some(session_id)).await?;
    let mut stars = std::collections::BTreeMap::new();
    let mut issues = std::collections::BTreeMap::new();
    let mut faces = 0;
    for p in &page.photos {
        if let Some(r) = p.ai_rating {
            *stars.entry(format!("{r:.1}")).or_insert(0) += 1;
        }
        for i in &p.issues {
            *issues.entry(format!("{i:?}")).or_insert(0) += 1;
        }
        faces += p.face_count.unwrap_or(0);
    }
    let bursts: Vec<&ip_core::BurstOut> = groups.scenes.iter().flat_map(|s| &s.bursts).collect();
    let name_of: std::collections::HashMap<i64, &str> = page
        .photos
        .iter()
        .map(|p| (p.id, p.file_name.as_str()))
        .collect();
    let mut sample_groups: Vec<String> = bursts
        .iter()
        .filter(|b| b.size >= 2)
        .take(8)
        .map(|b| {
            let best = b
                .best_photo_id
                .and_then(|i| name_of.get(&i))
                .copied()
                .unwrap_or("?");
            let names: Vec<&str> = b
                .photo_ids
                .iter()
                .filter_map(|i| name_of.get(i).copied())
                .collect();
            format!("burst {} (best {best}): {}", b.id, names.join(", "))
        })
        .collect();
    if sample_groups.is_empty() {
        sample_groups.push("(no multi-photo bursts)".into());
    }
    Ok(AnalyzeReport {
        session_id,
        status,
        stages,
        total_secs,
        photos: page.photos.len(),
        analyzed: page.photos.iter().filter(|p| p.analyzed).count(),
        scenes: groups.scenes.len(),
        bursts: bursts.len(),
        stacked_bursts: bursts.iter().filter(|b| b.size >= 2).count(),
        largest_burst: bursts.iter().map(|b| b.size).max().unwrap_or(0),
        stars,
        issues,
        people: people.len(),
        faces,
        sample_groups,
    })
}

fn print_report(r: &AnalyzeReport) {
    println!("session:       {}", r.session_id);
    println!(
        "state:         {:?}{}",
        r.status.state,
        r.status
            .error
            .as_ref()
            .map(|e| format!(" ({e})"))
            .unwrap_or_default()
    );
    if !r.status.skipped_steps.is_empty() {
        println!("skipped steps: {}", r.status.skipped_steps.join(", "));
    }
    println!(
        "photos:        {} ({} analysed, {} faces)",
        r.photos, r.analyzed, r.faces
    );
    let secs = r.total_secs.max(0.001);
    println!(
        "elapsed:       {secs:.2}s ({:.1} photos/s)",
        r.analyzed as f64 / secs
    );
    for (s, t) in &r.stages {
        println!("  {s:<11} {t:.2}s");
    }
    println!(
        "groups:        {} scenes, {} bursts ({} with 2+ photos, largest {})",
        r.scenes, r.bursts, r.stacked_bursts, r.largest_burst
    );
    for g in &r.sample_groups {
        println!("  {g}");
    }
    let stars: Vec<String> = r.stars.iter().map(|(k, v)| format!("{k}*: {v}")).collect();
    println!("ai stars:      {}", stars.join("  "));
    let issues: Vec<String> = r.issues.iter().map(|(k, v)| format!("{k}: {v}")).collect();
    println!(
        "issues:        {}",
        if issues.is_empty() {
            "none".into()
        } else {
            issues.join("  ")
        }
    );
    println!("people:        {}", r.people);
}

fn init_tracing(default: &str) {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Serve {
            port,
            host,
            data_dir,
            web_dir,
        } => {
            init_tracing("info,tower_http=info");
            let web_dir = web_dir.or_else(|| {
                let p = PathBuf::from("web/dist");
                p.join("index.html").is_file().then_some(p)
            });
            ip_server::run(ServerConfig {
                host,
                port,
                data_dir,
                web_dir,
            })
            .await
        }
        Command::Import {
            dir,
            data_dir,
            no_recursive,
            title,
        } => {
            init_tracing("warn");
            let core = Core::open(CoreConfig::new(data_dir)).context("open catalog")?;
            let stats = import_headless(&core, dir, !no_recursive, title).await?;
            print_stats(&stats);
            Ok(())
        }
        Command::Analyze {
            target,
            profile,
            data_dir,
            models_dir,
            allow_download,
            force,
        } => {
            init_tracing("warn");
            let profile = Profile::parse(&profile)
                .ok_or_else(|| anyhow::anyhow!("--profile must be fast or standard"))?;
            if let Some(d) = models_dir {
                std::env::set_var("IMAGEPICKER_MODELS_DIR", d);
            }
            let core = Core::open(CoreConfig::new(data_dir)).context("open catalog")?;
            let report = analyze_headless(&core, &target, profile, force, allow_download).await;
            core.worker.shutdown().await;
            let report = report?;
            print_report(&report);
            if report.status.state == RunState::Failed {
                std::process::exit(1);
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ip_core::testutil::{make_photos, FakeImaging};

    #[test]
    fn parses_serve_defaults() {
        let cli = Cli::try_parse_from(["imagepicker", "serve"]).unwrap();
        match cli.command {
            Command::Serve {
                port,
                host,
                data_dir,
                web_dir,
            } => {
                assert_eq!(port, 7878);
                assert_eq!(host, "127.0.0.1");
                assert!(data_dir.is_none() && web_dir.is_none());
            }
            _ => panic!("wrong command"),
        }
        let cli = Cli::try_parse_from([
            "imagepicker",
            "serve",
            "--port",
            "9000",
            "--host",
            "0.0.0.0",
            "--data-dir",
            "d",
            "--web-dir",
            "w",
        ])
        .unwrap();
        assert!(matches!(cli.command, Command::Serve { port: 9000, .. }));
    }

    #[test]
    fn parses_import() {
        let cli =
            Cli::try_parse_from(["imagepicker", "import", "some/dir", "--no-recursive"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Import {
                no_recursive: true,
                ..
            }
        ));
        assert!(Cli::try_parse_from(["imagepicker", "import"]).is_err());
    }

    #[test]
    fn parses_analyze() {
        let cli =
            Cli::try_parse_from(["imagepicker", "analyze", "photos", "--allow-download"]).unwrap();
        match cli.command {
            Command::Analyze {
                target,
                profile,
                allow_download,
                force,
                ..
            } => {
                assert_eq!((target.as_str(), profile.as_str()), ("photos", "standard"));
                assert!(allow_download && !force);
            }
            _ => panic!("wrong command"),
        }
        assert!(Cli::try_parse_from(["imagepicker", "analyze"]).is_err());
    }

    #[tokio::test]
    async fn headless_analyze_with_a_fake_worker() {
        use ip_core::testutil::{FakeSpec, FakeWorker};
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        make_photos(src.path(), 6); // one minute apart
        let worker = Arc::new(FakeWorker::new());
        for i in 0..3 {
            worker.set(&format!("img_{i:03}.jpg"), FakeSpec::at(i as f32));
        }
        let core = Core::open(CoreConfig {
            data_dir: Some(data.path().to_path_buf()),
            imaging: Arc::new(FakeImaging::new()),
            thumb_workers: Some(2),
            worker: Some(worker),
        })
        .unwrap();
        let r = analyze_headless(
            &core,
            &src.path().to_string_lossy(),
            Profile::Standard,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(r.status.state, RunState::Done, "{:?}", r.status);
        assert_eq!((r.photos, r.analyzed), (6, 6));
        assert!(r.bursts >= 1 && r.scenes >= 1);
        assert!(r.stages.iter().any(|(s, _)| s == "grouping"));
        // by session id the second call has nothing left to analyse
        let second = analyze_headless(
            &core,
            &r.session_id.to_string(),
            Profile::Standard,
            false,
            false,
        )
        .await
        .unwrap();
        assert_eq!(second.status.total, 0);
        print_report(&r);
        assert!(analyze_headless(&core, "99999", Profile::Fast, false, false)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn headless_import_waits_for_thumbnails() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        make_photos(src.path(), 9);
        let core = Core::open(CoreConfig {
            data_dir: Some(data.path().to_path_buf()),
            imaging: Arc::new(FakeImaging::new()),
            thumb_workers: Some(2),
            worker: None,
        })
        .unwrap();
        let stats = import_headless(&core, src.path().to_path_buf(), true, None)
            .await
            .unwrap();
        assert_eq!((stats.photos, stats.thumbs_ready), (9, 9));
    }
}
