//! `imagepicker` command line: `serve` the API/WebUI, or run a headless `import`.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use ip_core::{Core, CoreConfig, ImportRequest, ImportState, PhotoQuery};
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
