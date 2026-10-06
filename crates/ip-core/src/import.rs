//! Import service: create session -> scan -> batch insert -> metadata (rayon) -> thumbnails.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ip_imaging::ImageFormat;
use rayon::prelude::*;

use crate::catalog::{self, MetaUpdate, NewPhoto};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::model::{ImportRequest, ImportState, Session};
use crate::thumbs::Tracker;
use crate::Core;

pub const INSERT_BATCH: usize = 1000;
pub const META_BATCH: usize = 256;

struct MetaJob {
    id: i64,
    path: PathBuf,
    format: ImageFormat,
}

/// Absolute, lexically normalised path (no `\\?\` prefix, no trailing separator).
fn normalize(p: &Path) -> std::io::Result<PathBuf> {
    let abs = std::path::absolute(p)?;
    Ok(abs.components().collect())
}

impl Core {
    /// Creates the session immediately and continues scanning in the background.
    pub async fn import(self: &Arc<Self>, req: ImportRequest) -> Result<Session> {
        if req.path.trim().is_empty() {
            return Err(CoreError::bad_request("path must not be empty"));
        }
        let root = normalize(Path::new(&req.path))
            .map_err(|e| CoreError::bad_request(format!("invalid path: {e}")))?;
        if !root.is_dir() {
            return Err(CoreError::bad_request(format!(
                "not a directory: {}",
                root.display()
            )));
        }
        let root_str = root.to_string_lossy().into_owned();
        let title = req
            .title
            .filter(|t| !t.trim().is_empty())
            .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| root_str.clone());
        let rs = root_str.clone();
        let (root_id, session_id) = self
            .db
            .call(move |c| {
                let root_id = catalog::upsert_root(c, &rs)?;
                let sid = catalog::create_session(c, root_id, &title)?;
                Ok((root_id, sid))
            })
            .await?;
        let session = self.session(session_id).await?;
        self.events.emit(Event::SessionUpdated {
            session: session.clone(),
        });
        let core = self.clone();
        let recursive = req.recursive;
        tokio::spawn(async move {
            if let Err(e) = core.run_import(root, root_id, session_id, recursive).await {
                tracing::error!(session = session_id, error = %format!("{e:#}"), "import failed");
            }
            // Whatever happened, the session must not stay in a transient state.
            let _ = core
                .db
                .call(move |c| catalog::set_import_state(c, session_id, ImportState::Ready))
                .await;
            core.emit_session(session_id).await;
        });
        Ok(session)
    }

    pub(crate) async fn emit_session(&self, id: i64) {
        if let Ok(session) = self.session(id).await {
            self.events.emit(Event::SessionUpdated { session });
        }
    }

    async fn run_import(
        self: &Arc<Self>,
        root: PathBuf,
        root_id: i64,
        session_id: i64,
        recursive: bool,
    ) -> anyhow::Result<()> {
        // 1. scan (imaging scans in parallel itself)
        let imaging = self.imaging.clone();
        let scan_root = root.clone();
        let files =
            tokio::task::spawn_blocking(move || imaging.scan_dir(&scan_root, recursive, &[]))
                .await??;
        tracing::info!(session = session_id, files = files.len(), "scan finished");

        // 2. batch insert
        let tracker = Tracker::new();
        let mut meta_jobs: Vec<MetaJob> = Vec::new();
        let mut thumb_only: Vec<i64> = Vec::new();
        for chunk in files.chunks(INSERT_BATCH) {
            let batch: Vec<NewPhoto> = chunk
                .iter()
                .filter_map(|f| {
                    let rel = catalog::rel_path(&root, &f.path)?;
                    Some(NewPhoto {
                        file_name: f.path.file_name()?.to_string_lossy().into_owned(),
                        rel_path: rel,
                        size: f.size as i64,
                        mtime_ms: f.mtime_ms,
                        fast_key: self.imaging.fast_key(&f.path, f.size, f.mtime_ms),
                        format: f.format,
                    })
                })
                .collect();
            let paths: Vec<(PathBuf, ImageFormat)> = chunk
                .iter()
                .filter(|f| {
                    catalog::rel_path(&root, &f.path).is_some() && f.path.file_name().is_some()
                })
                .map(|f| (f.path.clone(), f.format))
                .collect();
            let inserted = self
                .db
                .call(move |c| catalog::insert_batch(c, root_id, session_id, &batch))
                .await?;
            for (ins, (path, format)) in inserted.iter().zip(paths) {
                if !ins.meta_done {
                    meta_jobs.push(MetaJob {
                        id: ins.id,
                        path,
                        format,
                    });
                } else if ins.thumb_state < 2 {
                    thumb_only.push(ins.id);
                }
            }
            self.events.emit(Event::PhotosAdded {
                session_id,
                count: inserted.len() as i64,
            });
            self.emit_session(session_id).await;
        }

        // 3. scanning is over; metadata + thumbnails
        self.db
            .call(move |c| catalog::set_import_state(c, session_id, ImportState::Thumbnailing))
            .await?;
        self.emit_session(session_id).await;
        if !thumb_only.is_empty() {
            self.thumbs.enqueue(&thumb_only, Some(&tracker), false);
        }

        let total_meta = meta_jobs.len();
        for chunk in meta_jobs.chunks(META_BATCH) {
            let jobs: Vec<(i64, PathBuf, ImageFormat)> = chunk
                .iter()
                .map(|j| (j.id, j.path.clone(), j.format))
                .collect();
            let imaging = self.imaging.clone();
            let updates: Vec<MetaUpdate> = tokio::task::spawn_blocking(move || {
                jobs.par_iter()
                    .map(|(id, path, format)| {
                        let meta = match imaging.read_metadata(path, *format) {
                            Ok(m) => Some(m),
                            Err(e) => {
                                tracing::warn!(path = %path.display(), error = %format!("{e:#}"), "metadata failed");
                                None
                            }
                        };
                        MetaUpdate {
                            id: *id,
                            meta,
                            content_key: imaging.content_key(path).ok(),
                        }
                    })
                    .collect()
            })
            .await?;
            let ids: Vec<i64> = updates.iter().map(|u| u.id).collect();
            self.db
                .call(move |c| catalog::apply_metadata(c, &updates))
                .await?;
            self.xmp_import(ids.clone()).await;
            self.thumbs.enqueue(&ids, Some(&tracker), false);
        }
        if total_meta > 0 {
            // Sort order / dimensions changed; tell clients to refetch.
            self.events.emit(Event::PhotosAdded {
                session_id,
                count: total_meta as i64,
            });
        }

        // 4. wait for the grid thumbnails of this import
        tracker.wait_idle().await;
        // 5. settings: analyse right away
        let st = self.settings();
        if st.analysis.auto_analyze_on_import && total_meta > 0 {
            if let Some(profile) = crate::Profile::parse(&st.analysis.default_profile) {
                let req = crate::AnalysisRunRequest {
                    session_id,
                    profile,
                    photo_ids: None,
                    force: false,
                    allow_download: false,
                };
                if let Err(e) = self.analysis_run(req).await {
                    tracing::info!(session = session_id, error = %e, "auto analysis not started");
                }
            }
        }
        Ok(())
    }
}
