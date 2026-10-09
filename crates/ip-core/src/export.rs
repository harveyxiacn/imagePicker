//! Export task: copy originals or write resized JPEGs, never overwriting existing files.

use std::collections::{HashMap, HashSet};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use ip_render::{EditStack, RgbImage};
use ip_worker_client::{AiWorker, EnhanceRequest, MaskPhoto};
use tokio::runtime::Handle;

use crate::catalog::{self, now_ms, PhotoRef};
use crate::edit::service::{encode_jpeg, MaskMode};
use crate::edit::{store, RenderService};
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::imaging::Imaging;
use crate::jpegmeta;
use crate::model::ExportRequest;
use crate::Core;

pub struct ExportOptions {
    pub long_edge: Option<u32>,
    pub quality: u8,
    pub name_template: String,
    /// Leave the GPS position out of re-encoded outputs (EXIF GPS IFD, XMP `GPS*` properties).
    pub strip_gps: bool,
    pub upscale: Option<UpscaleExport>,
    /// Crop-fit every output to this aspect ratio and, when larger, scale it down to exactly
    /// `(width, height)` (social media presets). Never upscales.
    pub fit: Option<(u32, u32)>,
}

/// Center-crops `img` to the aspect ratio of `(w, h)`, then scales it down to `w x h` when it
/// is larger.
pub fn fit_crop(img: &RgbImage, w: u32, h: u32) -> RgbImage {
    let (iw, ih) = (img.width as u64, img.height as u64);
    if iw == 0 || ih == 0 || w == 0 || h == 0 {
        return img.clone();
    }
    // largest centered region with aspect w:h
    let (cw, ch) = if iw * h as u64 > ih * w as u64 {
        ((ih * w as u64 / h as u64).max(1), ih)
    } else {
        (iw, (iw * h as u64 / w as u64).max(1))
    };
    let (x0, y0) = ((iw - cw) / 2, (ih - ch) / 2);
    let mut crop = Vec::with_capacity((cw * ch * 3) as usize);
    for y in 0..ch {
        let start = (((y0 + y) * iw + x0) * 3) as usize;
        crop.extend_from_slice(&img.data[start..start + (cw * 3) as usize]);
    }
    let (ow, oh) = if cw > w as u64 {
        (w, h)
    } else {
        (cw as u32, ch as u32)
    };
    if (ow as u64, oh as u64) == (cw, ch) {
        return RgbImage {
            width: cw as u32,
            height: ch as u32,
            data: crop,
        };
    }
    let buf = image::RgbImage::from_raw(cw as u32, ch as u32, crop).expect("size matches");
    let out = image::imageops::resize(&buf, ow, oh, image::imageops::FilterType::Triangle);
    RgbImage {
        width: ow,
        height: oh,
        data: out.into_raw(),
    }
}

/// Super-resolution of exports: render -> temp PNG -> `enhance.run upscale` -> JPEG.
pub struct UpscaleExport {
    pub svc: Arc<RenderService>,
    pub worker: Arc<dyn AiWorker>,
    pub handle: Handle,
    pub scale: u32,
    /// Scratch directory for the intermediate PNGs and the worker's output.
    pub tmp: PathBuf,
}

/// Saved edits to render into the output (M3): photos listed in `stacks` are rendered at full
/// resolution and written as JPEG; the rest follow the original M1 behaviour.
pub struct EditedExport {
    pub svc: Arc<RenderService>,
    pub handle: Handle,
    pub stacks: HashMap<i64, EditStack>,
}

#[derive(Debug, Default)]
pub struct ExportReport {
    pub written: Vec<PathBuf>,
    pub errors: Vec<(i64, String)>,
}

/// Days since 1970-01-01 -> (year, month, day), proleptic Gregorian.
pub(crate) fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn date_string(ms: i64) -> String {
    let (y, m, d) = civil_from_days(ms.div_euclid(86_400_000));
    format!("{y:04}{m:02}{d:02}")
}

fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| {
            if c.is_control() || "<>:\"/\\|?*".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    cleaned
        .trim_matches(|c: char| c == '.' || c == ' ')
        .to_string()
}

fn render_name(template: &str, r: &PhotoRef, seq: usize) -> String {
    let stem = Path::new(&r.file_name)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| format!("photo{}", r.id));
    let date = r
        .taken_at
        .or(r.mtime_ms)
        .map(date_string)
        .unwrap_or_else(|| "nodate".into());
    let out = template
        .replace("{name}", &stem)
        .replace("{seq}", &format!("{seq:04}"))
        .replace("{date}", &date);
    let out = sanitize(&out);
    if out.is_empty() {
        sanitize(&stem)
    } else {
        out
    }
}

/// Decide the output file name of every photo up front (collision-free, deterministic).
pub fn plan_names(refs: &[PhotoRef], template: &str, dest: &Path, resized: bool) -> Vec<PathBuf> {
    plan_names_with(refs, template, dest, &|_| resized)
}

/// Like [`plan_names`] with a per-photo decision whether the output is a re-encoded JPEG.
pub fn plan_names_with(
    refs: &[PhotoRef],
    template: &str,
    dest: &Path,
    jpeg: &dyn Fn(&PhotoRef) -> bool,
) -> Vec<PathBuf> {
    let mut taken: HashSet<String> = HashSet::new();
    let mut out = Vec::with_capacity(refs.len());
    for (i, r) in refs.iter().enumerate() {
        let base = render_name(template, r, i + 1);
        let ext = if jpeg(r) {
            "jpg".to_string()
        } else {
            Path::new(&r.file_name)
                .extension()
                .map(|e| e.to_string_lossy().into_owned())
                .unwrap_or_else(|| "jpg".into())
        };
        let mut n = 0u32;
        let path = loop {
            let name = if n == 0 {
                format!("{base}.{ext}")
            } else {
                format!("{base}_{n}.{ext}")
            };
            let p = dest.join(&name);
            if !taken.contains(&name.to_lowercase()) && !p.exists() {
                taken.insert(name.to_lowercase());
                break p;
            }
            n += 1;
        };
        out.push(path);
    }
    out
}

fn write_new(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(bytes)
}

fn copy_new(src: &Path, dst: &Path) -> std::io::Result<()> {
    let mut input = std::fs::File::open(src)?;
    let mut f = OpenOptions::new().write(true).create_new(true).open(dst)?;
    std::io::copy(&mut input, &mut f)?;
    Ok(())
}

fn jpeg_dims(img: &RgbImage) -> (u32, u32) {
    (img.width, img.height)
}

/// The XMP sidecar of a source (`<stem>.xmp` / `<file>.xmp`), unless unreasonably large.
fn sidecar_xmp(path: &Path) -> Option<String> {
    let p = crate::xmp::find_sidecar(path)?;
    if std::fs::metadata(&p).ok()?.len() > 4 << 20 {
        return None;
    }
    std::fs::read_to_string(p).ok()
}

/// What the source contributes to the metadata of a re-encoded export: a JPEG its own EXIF,
/// XMP (else the sidecar's) and IPTC; other formats their EXIF (copied or rebuilt by the
/// imaging layer, else synthesised from the catalog metadata) and the sidecar XMP.
fn source_meta(imaging: &dyn Imaging, r: &PhotoRef) -> jpegmeta::SourceMeta {
    if r.format == ip_imaging::ImageFormat::Jpeg {
        let mut m = jpegmeta::SourceMeta::of_jpeg_file(&r.path);
        if m.xmp.is_none() {
            m.xmp = sidecar_xmp(&r.path);
        }
        return m;
    }
    jpegmeta::SourceMeta {
        exif: imaging.read_exif(&r.path, r.format).ok().flatten(),
        xmp: sidecar_xmp(&r.path),
        irb: None,
        metadata: imaging.read_metadata(&r.path, r.format).ok(),
    }
}

/// Encodes a rendered (sRGB) picture as JPEG with the source's metadata (EXIF minus
/// orientation, XMP, IPTC; GPS unless stripped) and an sRGB ICC profile.
fn encode_with_meta(
    imaging: &dyn Imaging,
    r: &PhotoRef,
    img: &RgbImage,
    opts: &ExportOptions,
) -> std::result::Result<Vec<u8>, String> {
    let bytes = encode_jpeg(img, opts.quality).map_err(|e| format!("{e:#}"))?;
    Ok(jpegmeta::finalize_export(
        bytes,
        &source_meta(imaging, r),
        jpeg_dims(img),
        opts.strip_gps,
    ))
}

/// Upscaled export of one photo (see [`UpscaleExport`]).
fn export_upscaled(
    imaging: &dyn Imaging,
    r: &PhotoRef,
    stack: Option<&EditStack>,
    up: &UpscaleExport,
    opts: &ExportOptions,
) -> std::result::Result<Vec<u8>, String> {
    let edge = opts
        .long_edge
        .map(|e| (e as f64 / up.scale as f64).ceil().max(16.0) as u32);
    let default_stack = EditStack::default();
    let rendered = up
        .svc
        .render_gated(
            r,
            stack.unwrap_or(&default_stack),
            edge,
            MaskMode::Strict,
            &up.handle,
        )
        .map_err(|e| e.to_string())?;
    let dir = up.tmp.join(r.id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let input = dir.join("render.png");
    image::RgbImage::from_raw(
        rendered.image.width,
        rendered.image.height,
        rendered.image.data.clone(),
    )
    .ok_or_else(|| "bad render buffer".to_string())?
    .save_with_format(&input, image::ImageFormat::Png)
    .map_err(|e| e.to_string())?;
    drop(rendered);
    let resp = up
        .handle
        .block_on(up.worker.enhance_run(&EnhanceRequest {
            photo: MaskPhoto {
                photo_id: r.id,
                path: input.to_string_lossy().into_owned(),
                orientation: 1,
            },
            op: "upscale".into(),
            strength: 1.0,
            scale: Some(up.scale),
            faces: None,
            out_dir: dir.join("out").to_string_lossy().into_owned(),
            allow_download: false,
        }))
        .map_err(|e| crate::analysis::map_worker_err(e).to_string());
    let result = resp.and_then(|resp| {
        let path = resp
            .image
            .ok_or_else(|| "enhance.run returned no image".to_string())?;
        let big = image::open(&path)
            .map_err(|e| format!("unreadable upscaled image: {e}"))?
            .to_rgb8();
        let img = RgbImage {
            width: big.width(),
            height: big.height(),
            data: big.into_raw(),
        };
        encode_with_meta(imaging, r, &img, opts)
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Runs the export synchronously (parallel over photos). `progress(done, total)` is called after each photo.
pub fn export_photos(
    imaging: &dyn Imaging,
    refs: &[PhotoRef],
    dest: &Path,
    opts: &ExportOptions,
    edits: Option<&EditedExport>,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> ExportReport {
    let edited = |r: &PhotoRef| edits.and_then(|e| e.stacks.get(&r.id));
    let names = plan_names_with(refs, &opts.name_template, dest, &|r| {
        opts.long_edge.is_some()
            || edited(r).is_some()
            || opts.upscale.is_some()
            || opts.fit.is_some()
    });
    let done = AtomicUsize::new(0);
    let total = refs.len();
    let results: Vec<std::result::Result<PathBuf, (i64, String)>> = refs
        .par_iter()
        .zip(names.par_iter())
        .map(|(r, out)| {
            let stack = edited(r);
            let res = match (stack, &opts.upscale, opts.long_edge) {
                (stack, None, _) if opts.fit.is_some() => {
                    let (fw, fh) = opts.fit.expect("checked");
                    // render a little larger than the target so the crop keeps its resolution
                    let edge = fw.max(fh).saturating_mul(2);
                    let rendered = match (stack, edits) {
                        (Some(stack), Some(e)) => e
                            .svc
                            .render_gated(r, stack, Some(edge), MaskMode::Strict, &e.handle)
                            .map(|o| o.image)
                            .map_err(|e| e.to_string()),
                        _ => imaging
                            .decode_rgb8(&r.path, r.format, r.orientation, edge)
                            .map(|(width, height, data)| RgbImage {
                                width,
                                height,
                                data,
                            })
                            .map_err(|e| format!("{e:#}")),
                    };
                    rendered
                        .map(|img| fit_crop(&img, fw, fh))
                        .and_then(|img| encode_with_meta(imaging, r, &img, opts))
                        .and_then(|bytes| write_new(out, &bytes).map_err(|e| e.to_string()))
                }
                (stack, Some(up), _) => export_upscaled(imaging, r, stack, up, opts)
                    .and_then(|bytes| write_new(out, &bytes).map_err(|e| e.to_string())),
                (Some(stack), None, _) => {
                    let e = edits.expect("edited stacks come with a render context");
                    e.svc
                        .render_gated(r, stack, opts.long_edge, MaskMode::Strict, &e.handle)
                        .map_err(|e| e.to_string())
                        .and_then(|o| encode_with_meta(imaging, r, &o.image, opts))
                        .and_then(|bytes| write_new(out, &bytes).map_err(|e| e.to_string()))
                }
                (None, None, None) => copy_new(&r.path, out).map_err(|e| e.to_string()),
                (None, None, Some(edge)) => imaging
                    .generate_thumbnail(&r.path, r.format, r.orientation, edge, opts.quality)
                    .map_err(|e| format!("{e:#}"))
                    .map(|img| {
                        jpegmeta::finalize_export(
                            img.bytes,
                            &source_meta(imaging, r),
                            (img.width, img.height),
                            opts.strip_gps,
                        )
                    })
                    .and_then(|bytes| write_new(out, &bytes).map_err(|e| e.to_string())),
            };
            let d = done.fetch_add(1, Ordering::SeqCst) + 1;
            progress(d, total);
            match res {
                Ok(()) => Ok(out.clone()),
                Err(e) => {
                    let _ = std::fs::remove_file(out); // do not leave partial files we created
                    Err((r.id, format!("{}: {e}", r.file_name)))
                }
            }
        })
        .collect();
    let mut report = ExportReport::default();
    for r in results {
        match r {
            Ok(p) => report.written.push(p),
            Err(e) => report.errors.push(e),
        }
    }
    report
}

impl Core {
    /// Validates and starts an export task; progress arrives as `task.progress` events.
    pub async fn export(self: &Arc<Self>, req: ExportRequest) -> Result<String> {
        self.export_fit(req, None).await
    }

    /// [`Core::export`] with crop-fit to `fit` (width, height); see [`ExportOptions::fit`].
    pub async fn export_fit(
        self: &Arc<Self>,
        req: ExportRequest,
        fit: Option<(u32, u32)>,
    ) -> Result<String> {
        let folders = req.folders.clone().filter(|f| !f.is_empty());
        match (&folders, req.ids.is_empty()) {
            (Some(_), false) => {
                return Err(CoreError::bad_request(
                    "give either ids or folders, not both",
                ))
            }
            (None, true) => return Err(CoreError::bad_request("ids must not be empty")),
            _ => {}
        }
        if req.dest.trim().is_empty() {
            return Err(CoreError::bad_request("dest must not be empty"));
        }
        if req.long_edge == Some(0) {
            return Err(CoreError::bad_request("long_edge must be positive or null"));
        }
        if !(1..=100).contains(&req.quality) {
            return Err(CoreError::bad_request("quality must be 1..100"));
        }
        if req.upscale.is_some_and(|s| s != 2 && s != 4) {
            return Err(CoreError::bad_request("upscale must be 2 or 4"));
        }
        if let Some(scale) = req.upscale {
            let scale = scale.to_string();
            self.preflight_models("enhance", &["upscale", scale.as_str()])
                .await?;
        }
        let dest = PathBuf::from(&req.dest);
        std::fs::create_dir_all(&dest)
            .map_err(|e| CoreError::bad_request(format!("cannot create destination: {e}")))?;
        // (output directory, photos); one group unless `folders` was given
        let mut groups: Vec<(PathBuf, Vec<PhotoRef>)> = Vec::new();
        match folders {
            None => {
                let ids = req.ids.clone();
                let refs = self.db.call(move |c| catalog::photo_refs(c, &ids)).await?;
                groups.push((dest.clone(), refs));
            }
            Some(folders) => {
                for (name, ids) in folders {
                    let clean = sanitize(&name);
                    if clean.is_empty() {
                        return Err(CoreError::bad_request(format!(
                            "folder name {name:?} is not usable"
                        )));
                    }
                    let dir = dest.join(&clean);
                    let refs = self.db.call(move |c| catalog::photo_refs(c, &ids)).await?;
                    if refs.is_empty() {
                        continue;
                    }
                    match groups.iter_mut().find(|(d, _)| *d == dir) {
                        Some((_, existing)) => existing.extend(refs),
                        None => groups.push((dir, refs)),
                    }
                }
            }
        }
        let refs: Vec<PhotoRef> = groups.iter().flat_map(|(_, r)| r.iter().cloned()).collect();
        if refs.is_empty() {
            return Err(CoreError::not_found("none of the given photos exist"));
        }
        for (dir, _) in &groups {
            std::fs::create_dir_all(dir).map_err(|e| {
                CoreError::bad_request(format!("cannot create {}: {e}", dir.display()))
            })?;
        }
        let task_id = format!("export-{}", self.next_task_seq());
        let total = refs.len();
        {
            let (tid, params) = (
                task_id.clone(),
                serde_json::json!({"dest": req.dest, "long_edge": req.long_edge, "count": total})
                    .to_string(),
            );
            self.db
                .call(move |c| {
                    c.execute(
                        "INSERT INTO task(id, kind, status, priority, params, progress, created_at, updated_at)
                         VALUES(?1,'export','running',0,?2,0,?3,?3)",
                        rusqlite::params![tid, params, now_ms()],
                    )?;
                    Ok(())
                })
                .await?;
        }
        let core = self.clone();
        let tid = task_id.clone();
        let opts = ExportOptions {
            long_edge: req.long_edge,
            quality: req.quality,
            name_template: req.name_template.clone(),
            strip_gps: req.strip_gps,
            fit,
            upscale: req.upscale.map(|scale| UpscaleExport {
                svc: self.render.clone(),
                worker: self.worker.clone(),
                handle: Handle::current(),
                scale,
                tmp: self.dirs.gen.join(&task_id),
            }),
        };
        let edits = if req.apply_edits {
            let with_edits: Vec<i64> = refs
                .iter()
                .filter(|r| r.edit_hash.is_some())
                .map(|r| r.id)
                .collect();
            let stacks = self
                .db
                .call(move |c| {
                    let mut m = HashMap::new();
                    for id in with_edits {
                        if let Some(cur) = store::current(c, id)? {
                            if cur.has_edits {
                                let st: EditStack =
                                    serde_json::from_value(cur.stack).map_err(|e| {
                                        CoreError::Internal(anyhow::anyhow!(
                                            "stored edit stack of photo {id} is invalid: {e}"
                                        ))
                                    })?;
                                m.insert(id, st);
                            }
                        }
                    }
                    Ok(m)
                })
                .await?;
            Some(EditedExport {
                svc: self.render.clone(),
                handle: Handle::current(),
                stacks,
            })
        } else {
            None
        };
        tokio::spawn(async move {
            let ev = |done: usize, state: &str, error: Option<String>| Event::TaskProgress {
                task_id: tid.clone(),
                kind: "export".into(),
                done: done as i64,
                total: total as i64,
                state: state.into(),
                error,
            };
            core.events.emit(ev(0, "running", None));
            let (imaging, events, tid2) = (core.imaging.clone(), core.events.clone(), tid.clone());
            let res = tokio::task::spawn_blocking(move || {
                let mut report = ExportReport::default();
                let mut offset = 0usize;
                for (dir, refs) in &groups {
                    let rep =
                        export_photos(&*imaging, refs, dir, &opts, edits.as_ref(), &|done, _| {
                            events.emit(Event::TaskProgress {
                                task_id: tid2.clone(),
                                kind: "export".into(),
                                done: (offset + done) as i64,
                                total: total as i64,
                                state: "running".into(),
                                error: None,
                            });
                        });
                    offset += refs.len();
                    report.written.extend(rep.written);
                    report.errors.extend(rep.errors);
                }
                report
            })
            .await;
            let (state, error, done) = match res {
                Ok(rep) if rep.errors.is_empty() => ("done", None, rep.written.len()),
                Ok(rep) if rep.written.is_empty() => (
                    "failed",
                    Some(rep.errors.first().map(|e| e.1.clone()).unwrap_or_default()),
                    0,
                ),
                Ok(rep) => (
                    "done",
                    Some(format!(
                        "{} of {} failed; first: {}",
                        rep.errors.len(),
                        total,
                        rep.errors[0].1
                    )),
                    rep.written.len(),
                ),
                Err(e) => ("failed", Some(format!("export task crashed: {e}")), 0),
            };
            let (tid3, st, er) = (tid.clone(), state.to_string(), error.clone());
            let _ = core
                .db
                .call(move |c| {
                    c.execute(
                        "UPDATE task SET status=?2, error=?3, progress=1, updated_at=?4 WHERE id=?1",
                        rusqlite::params![tid3, st, er, now_ms()],
                    )?;
                    Ok(())
                })
                .await;
            core.events.emit(ev(done, state, error));
        });
        Ok(task_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ip_imaging::ImageFormat;

    fn r(id: i64, name: &str, taken: Option<i64>) -> PhotoRef {
        PhotoRef {
            id,
            path: PathBuf::from(name),
            file_name: name.to_string(),
            format: ImageFormat::Jpeg,
            orientation: 1,
            fast_key: "0".repeat(32),
            taken_at: taken,
            mtime_ms: None,
            edit_hash: None,
            width: None,
            height: None,
        }
    }

    #[test]
    fn civil_dates() {
        assert_eq!(date_string(0), "19700101");
        assert_eq!(date_string(1_709_164_800_000), "20240229");
    }

    #[test]
    fn collisions_get_suffixes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), b"x").unwrap();
        let refs = vec![
            r(1, "a.jpg", None),
            r(2, "a.jpg", None),
            r(3, "b.jpg", None),
        ];
        let names = plan_names(&refs, "{name}", dir.path(), false);
        let n: Vec<String> = names
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(n, vec!["a_1.jpg", "a_2.jpg", "b.jpg"]);
    }

    #[test]
    fn template_placeholders() {
        let dir = tempfile::tempdir().unwrap();
        let refs = vec![r(1, "IMG_1.CR2", Some(1_709_164_800_000))];
        let names = plan_names(&refs, "{date}_{seq}_{name}", dir.path(), true);
        assert_eq!(
            names[0].file_name().unwrap().to_string_lossy(),
            "20240229_0001_IMG_1.jpg"
        );
        let names = plan_names(&refs, "{date}/{name}", dir.path(), false);
        assert_eq!(
            names[0].file_name().unwrap().to_string_lossy(),
            "20240229_IMG_1.CR2"
        );
    }
}
