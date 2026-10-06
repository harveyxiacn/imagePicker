//! `imagepicker` command line: `serve` the API/WebUI, or run a headless `import`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ip_core::{
    AnalysisRunRequest, AnalysisStatus, BestTakePlan, BestTakeResult, Core, CoreConfig, Event,
    ImportRequest, ImportState, InpaintBody, PhotoQuery, Profile, RunState,
};
use ip_server::ServerConfig;

mod bench;

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
        /// LAN mode: bind 0.0.0.0 and require a password from remote clients (needs a stored
        /// password, or --password / $IMAGEPICKER_PASSWORD).
        #[arg(long)]
        lan: bool,
        /// Owner password (min. 8 characters; stored as an Argon2id hash). Prefer the
        /// IMAGEPICKER_PASSWORD environment variable: command lines are visible to other users.
        #[arg(long)]
        password: Option<String>,
        /// Optional read-only guest password (also enables guest login).
        #[arg(long)]
        guest_password: Option<String>,
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
    /// Render a photo (catalog id or image path) through an edit stack to a JPEG (debugging).
    Render {
        /// Photo id of the catalog, or a path to an image file.
        target: String,
        /// Edit stack JSON file (`{"version":1,"ops":[...]}`); default: the saved edits of a
        /// catalog photo, or no edits for a path.
        #[arg(long)]
        stack: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        /// Resize the result so its long edge is at most this many pixels.
        #[arg(long)]
        long_edge: Option<u32>,
        /// Force the CPU renderer (same as IMAGEPICKER_RENDER=cpu).
        #[arg(long)]
        cpu: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// JPEG quality of the output.
        #[arg(long, default_value_t = 92)]
        quality: u8,
    },
    /// Print the best-take plan of a burst (who could get a better expression from which
    /// frame); with `--auto` also compose the best takes into the base photo's edit stack.
    Besttake {
        burst_id: i64,
        /// Run the automatic choice (needs the AI worker and its best-take models).
        #[arg(long)]
        auto: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Remove bystanders (non-subject faces) or given faces from a photo with the inpainting
    /// model; the result is saved as a patch in the photo's edit stack (debugging).
    Inpaint {
        photo_id: i64,
        /// Remove every non-subject face.
        #[arg(long)]
        bystanders: bool,
        /// Remove these faces (comma separated face ids).
        #[arg(long, value_delimiter = ',')]
        face_ids: Vec<i64>,
        /// `lama` (default) or `sdxl`.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Ask the assistant: print the plan for a natural-language request (zh-CN or English);
    /// with `--execute` run it too.
    Ask {
        message: String,
        /// Library session id.
        #[arg(long)]
        session: i64,
        /// Run the plan after printing it (destructive steps included).
        #[arg(long)]
        execute: bool,
        /// `auto` (default from the settings), `rules` or `llm`.
        #[arg(long)]
        engine: Option<String>,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
    /// Synthetic libraries for the performance benchmarks (see bench/README.md).
    Bench {
        #[command(subcommand)]
        cmd: bench::BenchCmd,
    },
    /// Show what the personalised scoring has learned (labels, accuracy, fusion weight).
    Taste {
        /// Train now instead of waiting for the next 50 labels.
        #[arg(long)]
        retrain: bool,
        #[arg(long)]
        data_dir: Option<PathBuf>,
    },
}

/// How long a headless generative task may take before the CLI gives up waiting.
const GEN_WAIT: Duration = Duration::from_secs(30 * 60);

fn print_plan(plan: &BestTakePlan) {
    println!("base photo:    {}", plan.base_photo_id);
    if plan.people.is_empty() {
        println!("people:        none (no face of the base photo appears in other frames)");
    }
    for p in &plan.people {
        println!(
            "person:        track {} / person {:?} {} (base face {}) -> best photo {}",
            p.track_id,
            p.person_id,
            p.person_name.as_deref().unwrap_or(""),
            p.base_face_id,
            p.best_photo_id
        );
        for c in &p.candidates {
            println!(
                "  candidate:   photo {} face {} expression {} {}",
                c.photo_id,
                c.face_id,
                c.expression_score
                    .map(|s| format!("{s:.3}"))
                    .unwrap_or_else(|| "-".into()),
                match (c.composable, &c.reason) {
                    (true, _) => "composable".to_string(),
                    (false, r) => format!("not composable ({})", r.as_deref().unwrap_or("?")),
                }
            );
        }
    }
}

/// The plan of a burst and, with `auto`, the outcome of composing it (`besttake.done`).
async fn besttake_headless(
    core: &Arc<Core>,
    burst_id: i64,
    auto: bool,
) -> Result<(BestTakePlan, Option<Vec<BestTakeResult>>)> {
    let plan = core.besttake_plan(burst_id).await?;
    if !auto {
        return Ok((plan, None));
    }
    let mut rx = core.events.subscribe();
    core.besttake_auto(burst_id).await?;
    let wait = async {
        loop {
            match rx.recv().await {
                Ok(Event::BestTakeDone { photo_id, results }) if photo_id == plan.base_photo_id => {
                    return Ok(results)
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => anyhow::bail!("event stream closed: {e}"),
            }
        }
    };
    let results = tokio::time::timeout(GEN_WAIT, wait)
        .await
        .context("timed out waiting for besttake.done")??;
    Ok((plan, Some(results)))
}

/// Starts an inpainting task and waits for `inpaint.done`.
async fn inpaint_headless(core: &Arc<Core>, photo_id: i64, body: InpaintBody) -> Result<()> {
    let mut rx = core.events.subscribe();
    core.inpaint_start(photo_id, body).await?;
    let wait = async {
        loop {
            match rx.recv().await {
                Ok(Event::InpaintDone {
                    photo_id: p,
                    ok,
                    reason,
                }) if p == photo_id => {
                    return if ok {
                        Ok(())
                    } else {
                        Err(anyhow::anyhow!(
                            "inpainting failed: {}",
                            reason.unwrap_or_default()
                        ))
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => anyhow::bail!("event stream closed: {e}"),
            }
        }
    };
    tokio::time::timeout(GEN_WAIT, wait)
        .await
        .context("timed out waiting for inpaint.done")?
}

fn print_plan_out(p: &ip_core::assistant::PlanOut) {
    println!("engine:        {}", p.engine);
    println!("reply:         {}", p.reply);
    if let Some(u) = &p.unsupported {
        println!("unsupported:   {u}");
    }
    for (i, s) in p.steps.iter().enumerate() {
        println!(
            "step {}:        {} {}{}",
            i + 1,
            s.tool,
            s.args,
            if s.destructive { "  [destructive]" } else { "" }
        );
        println!("               {} ({} photos)", s.summary, s.affects);
    }
    if p.needs_confirmation {
        println!("confirmation:  needed (pass --execute to run it)");
    }
}

/// Plans a request and, with `execute`, runs the plan and waits for `assistant.done`.
async fn ask_headless(
    core: &Arc<Core>,
    session: i64,
    message: &str,
    execute: bool,
    engine: Option<String>,
) -> Result<(ip_core::assistant::PlanOut, Option<Event>)> {
    let plan = core
        .assistant_plan(ip_core::assistant::PlanRequest {
            session_id: session,
            message: message.to_string(),
            context: Default::default(),
            engine,
        })
        .await?;
    if !execute || plan.unsupported.is_some() || plan.steps.is_empty() {
        return Ok((plan, None));
    }
    let mut rx = core.events.subscribe();
    core.assistant_execute(&plan.plan_id).await?;
    let wait = async {
        loop {
            match rx.recv().await {
                Ok(ev @ Event::AssistantDone { .. }) => {
                    if matches!(&ev, Event::AssistantDone { plan_id, .. } if *plan_id == plan.plan_id)
                    {
                        return Ok(ev);
                    }
                }
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(e) => anyhow::bail!("event stream closed: {e}"),
            }
        }
    };
    let done = tokio::time::timeout(GEN_WAIT, wait)
        .await
        .context("timed out waiting for assistant.done")??;
    Ok((plan, Some(done)))
}

fn print_taste(t: &ip_core::taste::TasteOut) {
    let pct = |v: Option<f64>| {
        v.map(|v| format!("{:.1}%", v * 100.0))
            .unwrap_or_else(|| "-".into())
    };
    println!("labels:            {}", t.labels);
    println!("training pairs:    {}", t.pairs);
    println!("model active:      {}", if t.active { "yes" } else { "no" });
    println!("holdout accuracy:  {}", pct(t.holdout_accuracy));
    println!("base accuracy:     {}", pct(t.base_accuracy));
    println!("fusion alpha:      {:.2}", t.alpha);
    for tr in &t.traits {
        println!("trait:             {tr}");
    }
}

#[derive(Debug)]
struct RenderReport {
    backend: ip_core::ip_render::Backend,
    width: u32,
    height: u32,
    render_ms: u64,
    total_ms: u128,
}

async fn render_headless(
    core: &Arc<Core>,
    target: &str,
    stack_file: Option<&std::path::Path>,
    out: &std::path::Path,
    long_edge: Option<u32>,
    quality: u8,
) -> Result<RenderReport> {
    let started = Instant::now();
    let given = match stack_file {
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .with_context(|| format!("read stack file {}", p.display()))?;
            let v: serde_json::Value =
                serde_json::from_str(&text).context("stack file is not JSON")?;
            Some(
                ip_core::edit::validate_stack(v)
                    .context("invalid edit stack")?
                    .stack,
            )
        }
        None => None,
    };
    if let Some(e) = long_edge {
        anyhow::ensure!(e > 0, "--long-edge must be positive");
    }
    let rendered = match target.parse::<i64>() {
        Ok(id) => {
            let r = core
                .db
                .call(move |c| ip_core::catalog::photo_ref(c, id))
                .await
                .context("photo")?;
            let stack = match given {
                Some(s) => s,
                None => serde_json::from_value(core.get_edit(id).await?.stack)
                    .context("saved edit stack")?,
            };
            core.render
                .render_async(
                    r,
                    stack,
                    long_edge,
                    ip_core::edit::service::MaskMode::Strict,
                )
                .await?
        }
        Err(_) => {
            let (core2, path) = (core.clone(), PathBuf::from(target));
            let stack = given.unwrap_or_default();
            tokio::task::spawn_blocking(move || {
                core2.render_path_blocking(&path, &stack, long_edge)
            })
            .await??
        }
    };
    let (w, h) = (rendered.image.width, rendered.image.height);
    let jpeg = ip_core::edit::service::encode_jpeg(&rendered.image, quality)?;
    std::fs::write(out, jpeg).with_context(|| format!("write {}", out.display()))?;
    Ok(RenderReport {
        backend: rendered.backend,
        width: w,
        height: h,
        render_ms: rendered.ms,
        total_ms: started.elapsed().as_millis(),
    })
}

fn print_render(r: &RenderReport, out: &std::path::Path) {
    println!("output:        {}", out.display());
    println!("size:          {}x{}", r.width, r.height);
    println!("backend:       {:?}", r.backend);
    println!("render:        {} ms (decode + render)", r.render_ms);
    println!("total:         {} ms", r.total_ms);
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
            lan,
            password,
            guest_password,
        } => {
            init_tracing("info,tower_http=info");
            let password = password.or_else(|| {
                std::env::var("IMAGEPICKER_PASSWORD")
                    .ok()
                    .filter(|p| !p.is_empty())
            });
            let web_dir = web_dir.or_else(|| {
                let p = PathBuf::from("web/dist");
                p.join("index.html").is_file().then_some(p)
            });
            ip_server::run(ServerConfig {
                host,
                port,
                data_dir,
                web_dir,
                lan,
                password,
                guest_password,
                ..ServerConfig::default()
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
        Command::Besttake {
            burst_id,
            auto,
            data_dir,
        } => {
            init_tracing("warn");
            let core = Core::open(CoreConfig::new(data_dir)).context("open catalog")?;
            let res = besttake_headless(&core, burst_id, auto).await;
            core.worker.shutdown().await;
            let (plan, results) = res?;
            print_plan(&plan);
            if let Some(results) = results {
                for r in &results {
                    println!(
                        "composed:      base face {} -> {}{}",
                        r.base_face_id,
                        if r.ok { "ok" } else { "failed" },
                        r.reason
                            .as_deref()
                            .map(|s| format!(" ({s})"))
                            .unwrap_or_default()
                    );
                }
                if results.iter().any(|r| !r.ok) {
                    std::process::exit(1);
                }
            }
            Ok(())
        }
        Command::Inpaint {
            photo_id,
            bystanders,
            face_ids,
            model,
            data_dir,
        } => {
            init_tracing("warn");
            anyhow::ensure!(
                bystanders || !face_ids.is_empty(),
                "give --bystanders or --face-ids"
            );
            let core = Core::open(CoreConfig::new(data_dir)).context("open catalog")?;
            let body = InpaintBody {
                bystanders,
                face_ids: (!face_ids.is_empty()).then_some(face_ids),
                strokes: None,
                model,
            };
            let res = inpaint_headless(&core, photo_id, body).await;
            core.worker.shutdown().await;
            res?;
            println!("inpainted photo {photo_id}; the patch is part of its edit stack");
            Ok(())
        }
        Command::Ask {
            message,
            session,
            execute,
            engine,
            data_dir,
        } => {
            init_tracing("warn");
            let core = Core::open(CoreConfig::new(data_dir)).context("open catalog")?;
            let (plan, done) = ask_headless(&core, session, &message, execute, engine).await?;
            print_plan_out(&plan);
            if let Some(Event::AssistantDone { ok, results, .. }) = done {
                println!("executed:      {}", if ok { "ok" } else { "with errors" });
                for r in results {
                    println!("  {r}");
                }
                if !ok {
                    anyhow::bail!("the plan did not run completely");
                }
            }
            Ok(())
        }
        Command::Bench { cmd } => tokio::task::block_in_place(|| bench::run(cmd)),
        Command::Taste { retrain, data_dir } => {
            init_tracing("warn");
            let core = Core::open(CoreConfig::new(data_dir)).context("open catalog")?;
            let t = if retrain {
                core.retrain_taste().await?
            } else {
                core.taste().await?
            };
            print_taste(&t);
            Ok(())
        }
        Command::Render {
            target,
            stack,
            out,
            long_edge,
            cpu,
            data_dir,
            quality,
        } => {
            init_tracing("warn");
            let core = Core::open(CoreConfig {
                force_cpu: cpu,
                ..CoreConfig::new(data_dir)
            })
            .context("open catalog")?;
            let res =
                render_headless(&core, &target, stack.as_deref(), &out, long_edge, quality).await;
            core.worker.shutdown().await;
            print_render(&res?, &out);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ip_core::testutil::{make_photos, FakeImaging};

    #[test]
    fn parses_ask() {
        let cli = Cli::try_parse_from([
            "imagepicker",
            "ask",
            "只看小明的4星以上照片",
            "--session",
            "3",
            "--execute",
        ])
        .unwrap();
        match cli.command {
            Command::Ask {
                message,
                session,
                execute,
                engine,
                ..
            } => {
                assert_eq!(message, "只看小明的4星以上照片");
                assert_eq!(session, 3);
                assert!(execute && engine.is_none());
            }
            _ => panic!("wrong command"),
        }
        assert!(
            Cli::try_parse_from(["imagepicker", "ask", "x"]).is_err(),
            "--session is required"
        );
    }

    #[tokio::test]
    async fn ask_plans_and_executes() {
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        make_photos(src.path(), 3);
        let core = Core::open(CoreConfig {
            imaging: std::sync::Arc::new(FakeImaging::new()),
            worker: Some(std::sync::Arc::new(ip_core::testutil::FakeWorker::new())),
            renderer: Some(std::sync::Arc::new(ip_core::testutil::FakeRenderer::new(
                ip_core::ip_render::Backend::Cpu,
            ))),
            ..CoreConfig::new(Some(data.path().to_path_buf()))
        })
        .unwrap();
        let st = import_headless(&core, src.path().to_path_buf(), true, None)
            .await
            .unwrap();
        // plan only
        let (plan, done) = ask_headless(&core, st.session_id, "给所有照片打4星", false, None)
            .await
            .unwrap();
        assert_eq!(plan.steps[0].tool, "set_rating");
        assert!(done.is_none());
        assert!(plan.needs_confirmation);
        let page = core
            .photos(PhotoQuery {
                session_id: st.session_id,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(
            page.photos.iter().all(|p| p.user_rating.is_none()),
            "planning changes nothing"
        );
        // execute
        let (_, done) = ask_headless(&core, st.session_id, "give all photos 4 stars", true, None)
            .await
            .unwrap();
        match done {
            Some(Event::AssistantDone { ok, results, .. }) => {
                assert!(ok, "{results:?}");
                assert_eq!(results[0]["affected"], 3);
            }
            other => panic!("{other:?}"),
        }
        let page = core
            .photos(PhotoQuery {
                session_id: st.session_id,
                ..Default::default()
            })
            .await
            .unwrap();
        assert!(page.photos.iter().all(|p| p.user_rating == Some(4)));
        // unsupported: nothing runs
        let (plan, done) = ask_headless(&core, st.session_id, "sing", true, None)
            .await
            .unwrap();
        assert!(plan.unsupported.is_some() && done.is_none());
    }

    #[test]
    fn parses_serve_defaults() {
        let cli = Cli::try_parse_from(["imagepicker", "serve"]).unwrap();
        match cli.command {
            Command::Serve {
                port,
                host,
                data_dir,
                web_dir,
                lan,
                password,
                guest_password,
            } => {
                assert_eq!(port, 7878);
                assert_eq!(host, "127.0.0.1");
                assert!(data_dir.is_none() && web_dir.is_none());
                assert!(!lan && password.is_none() && guest_password.is_none());
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
        let cli = Cli::try_parse_from([
            "imagepicker",
            "serve",
            "--lan",
            "--password",
            "hunter22",
            "--guest-password",
            "visitor1",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Serve {
                lan: true,
                password: Some(_),
                guest_password: Some(_),
                ..
            }
        ));
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
            renderer: None,
            force_cpu: false,
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
        assert!(
            analyze_headless(&core, "99999", Profile::Fast, false, false)
                .await
                .is_err()
        );
    }

    #[test]
    fn parses_besttake_and_inpaint() {
        let cli = Cli::try_parse_from(["imagepicker", "besttake", "7", "--auto"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Besttake {
                burst_id: 7,
                auto: true,
                ..
            }
        ));
        let cli = Cli::try_parse_from([
            "imagepicker",
            "inpaint",
            "9",
            "--bystanders",
            "--face-ids",
            "1,2",
        ])
        .unwrap();
        match cli.command {
            Command::Inpaint {
                photo_id,
                bystanders,
                face_ids,
                ..
            } => assert_eq!((photo_id, bystanders, face_ids), (9, true, vec![1, 2])),
            _ => panic!("wrong command"),
        }
        assert!(Cli::try_parse_from(["imagepicker", "besttake"]).is_err());
    }

    #[tokio::test]
    async fn headless_inpaint_and_besttake_plan_with_a_fake_worker() {
        use ip_core::testutil::{person, FakeFace, FakeSpec, FakeWorker};
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        make_photos(src.path(), 2);
        let worker = Arc::new(FakeWorker::new());
        worker.set(
            "img_000.jpg",
            FakeSpec::at(0.0).with_faces(vec![
                FakeFace::new([0.3, 0.2, 0.3, 0.4], person(1, 0.0)),
                FakeFace::new([0.8, 0.7, 0.06, 0.08], person(2, 0.0)),
            ]),
        );
        worker.set("img_001.jpg", FakeSpec::at(90.0));
        let core = Core::open(CoreConfig {
            data_dir: Some(data.path().to_path_buf()),
            imaging: Arc::new(FakeImaging::new()),
            thumb_workers: Some(2),
            worker: Some(worker.clone()),
            renderer: None,
            force_cpu: true,
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
        let page = core
            .photos(PhotoQuery {
                session_id: r.session_id,
                limit: Some(10),
                ..Default::default()
            })
            .await
            .unwrap();
        let first = page
            .photos
            .iter()
            .find(|p| p.file_name == "img_000.jpg")
            .unwrap();
        inpaint_headless(
            &core,
            first.id,
            InpaintBody {
                bystanders: true,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let stack = core.get_edit(first.id).await.unwrap().stack;
        assert_eq!(stack["ops"][0]["kind"], "inpaint");
        assert_eq!(
            worker
                .inpaint_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
        // without a model the failure is reported
        worker.set_missing(&[ip_core::testutil::INPAINT_MODEL]);
        assert!(inpaint_headless(
            &core,
            first.id,
            InpaintBody {
                bystanders: true,
                ..Default::default()
            }
        )
        .await
        .is_err());
        let burst = first.burst_id;
        if let Some(b) = burst {
            let (plan, res) = besttake_headless(&core, b, false).await.unwrap();
            assert!(res.is_none());
            print_plan(&plan);
        }
    }

    #[test]
    fn parses_render_args() {
        let cli = Cli::try_parse_from([
            "imagepicker",
            "render",
            "12",
            "--stack",
            "s.json",
            "--out",
            "o.jpg",
            "--long-edge",
            "800",
            "--cpu",
        ])
        .unwrap();
        match cli.command {
            Command::Render {
                target,
                stack,
                long_edge,
                cpu,
                quality,
                ..
            } => {
                assert_eq!(target, "12");
                assert_eq!(stack.unwrap(), PathBuf::from("s.json"));
                assert_eq!((long_edge, cpu, quality), (Some(800), true, 92));
            }
            _ => panic!("wrong command"),
        }
        assert!(Cli::try_parse_from(["imagepicker", "render", "1"]).is_err()); // --out required
    }

    #[tokio::test]
    async fn headless_render_by_path_and_by_id() {
        use ip_core::testutil::FakeRenderer;
        let data = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        make_photos(src.path(), 1);
        let renderer = Arc::new(FakeRenderer::new(ip_core::ip_render::Backend::Cpu));
        let core = Core::open(CoreConfig {
            data_dir: Some(data.path().to_path_buf()),
            imaging: Arc::new(FakeImaging::new()),
            thumb_workers: Some(1),
            worker: None,
            renderer: Some(renderer),
            force_cpu: true,
        })
        .unwrap();
        let stack = src.path().join("stack.json");
        std::fs::write(
            &stack,
            r#"{"version":1,"ops":[{"type":"global","exposure":1.0}]}"#,
        )
        .unwrap();
        let photo = src.path().join("img_000.jpg");
        let out = std::env::temp_dir().join(format!("ip-cli-render-{}.jpg", std::process::id()));
        let r = render_headless(
            &core,
            &photo.to_string_lossy(),
            Some(&stack),
            &out,
            Some(100),
            90,
        )
        .await
        .unwrap();
        assert_eq!(r.width.max(r.height), 100);
        let img = image::open(&out).unwrap();
        assert_eq!(img.width(), r.width);
        print_render(&r, &out);

        // by catalog id, using the saved edits
        let st = import_headless(&core, src.path().to_path_buf(), true, None)
            .await
            .unwrap();
        let page = core
            .photos(PhotoQuery {
                session_id: st.session_id,
                ..Default::default()
            })
            .await
            .unwrap();
        let id = page.photos[0].id;
        let r2 = render_headless(&core, &id.to_string(), None, &out, None, 90)
            .await
            .unwrap();
        assert_eq!(r2.width, 640);
        // a bad stack file is reported
        std::fs::write(
            &stack,
            r#"{"version":1,"ops":[{"type":"global","exposure":99}]}"#,
        )
        .unwrap();
        assert!(
            render_headless(&core, &id.to_string(), Some(&stack), &out, None, 90)
                .await
                .is_err()
        );
        let _ = std::fs::remove_file(&out);
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
            renderer: None,
            force_cpu: false,
        })
        .unwrap();
        let stats = import_headless(&core, src.path().to_path_buf(), true, None)
            .await
            .unwrap();
        assert_eq!((stats.photos, stats.thumbs_ready), (9, 9));
    }
}
