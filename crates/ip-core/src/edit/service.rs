//! The shared rendering service: one [`Renderer`], decoded-proxy LRU, mask and LUT providers,
//! bounded render concurrency and the cache of edited thumbnails/previews.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use ip_imaging::ThumbCache;
use ip_render::{
    Backend, EditStack, Mask, MaskProvider, MaskTarget, Op, RenderRequest, Renderer, RgbImage,
    RgbaImage,
};
use ip_worker_client::{AiWorker, MaskPhoto, MaskRequest};
use tokio::runtime::Handle;
use tokio::sync::{OnceCell, Semaphore};

use super::luts::LutLibrary;
use super::patch::PatchStore;
use super::store;
use crate::analysis::map_worker_err;
use crate::catalog::PhotoRef;
use crate::db::Db;
use crate::error::{CoreError, Result};
use crate::imaging::Imaging;
use crate::thumbs::{PREVIEW_SIZES, THUMB_SIZES};

/// Concurrent renders (each may use the GPU or all CPU cores; decoding is the other cost).
pub const RENDER_CONCURRENCY: usize = 2;
/// Memory budget of the decoded-proxy cache.
const PROXY_BUDGET_BYTES: usize = 512 * 1024 * 1024;
/// Decode sizes (long edge); requests round up to the next bucket so slider dragging at
/// slightly different sizes shares one decoded proxy.
const BUCKETS: [u32; 9] = [512, 800, 1024, 1600, 2048, 3072, 4096, 6144, 8192];
/// Long edge of the AI masks requested from the worker.
pub const MASK_SIZE: u32 = 1024;
const MASK_LRU: usize = 24;

pub const THUMB_QUALITY: u8 = 80;
pub const PREVIEW_QUALITY: u8 = 88;

/// How AI masks are obtained while rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskMode {
    /// Load from the cache, otherwise ask the worker; failures (409/503) abort the render.
    Strict,
    /// Only use masks that are already cached; missing ones skip their local op.
    CachedOnly,
}

pub struct RenderOut {
    pub image: RgbImage,
    pub backend: Backend,
    /// Milliseconds spent decoding (cache miss), rendering and resizing; excludes queueing.
    pub ms: u64,
}

// ------------------------------------------------------------------ proxy cache

type ProxyKey = (i64, String, u32);

#[derive(Default)]
struct ProxyCache {
    map: HashMap<ProxyKey, (Arc<RgbImage>, u64)>,
    bytes: usize,
    tick: u64,
}

impl ProxyCache {
    fn get(&mut self, key: &ProxyKey) -> Option<Arc<RgbImage>> {
        self.tick += 1;
        let t = self.tick;
        self.map.get_mut(key).map(|e| {
            e.1 = t;
            e.0.clone()
        })
    }

    fn put(&mut self, key: ProxyKey, img: Arc<RgbImage>) {
        let size = img.data.len();
        if size > PROXY_BUDGET_BYTES / 2 {
            return;
        }
        self.tick += 1;
        if let Some(old) = self.map.insert(key, (img, self.tick)) {
            self.bytes -= old.0.data.len();
        }
        self.bytes += size;
        while self.bytes > PROXY_BUDGET_BYTES {
            let Some(oldest) = self
                .map
                .iter()
                .min_by_key(|(_, (_, t))| *t)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            if let Some(e) = self.map.remove(&oldest) {
                self.bytes -= e.0.data.len();
            }
        }
    }
}

// ------------------------------------------------------------------ masks

pub fn target_name(t: MaskTarget) -> &'static str {
    match t {
        MaskTarget::Subject => "subject",
        MaskTarget::Background => "background",
        MaskTarget::Sky => "sky",
        MaskTarget::Person => "person",
        MaskTarget::Skin => "skin",
        MaskTarget::Hair => "hair",
        MaskTarget::Clothes => "clothes",
    }
}

pub fn parse_target(s: &str) -> Option<MaskTarget> {
    Some(match s {
        "subject" => MaskTarget::Subject,
        "background" => MaskTarget::Background,
        "sky" => MaskTarget::Sky,
        "person" => MaskTarget::Person,
        "skin" => MaskTarget::Skin,
        "hair" => MaskTarget::Hair,
        "clothes" => MaskTarget::Clothes,
        _ => return None,
    })
}

/// Disk cache of AI masks (`<cache>/masks/<fast_key>/<photo>_<target>.png`) fed by the worker.
pub struct MaskStore {
    db: Db,
    worker: Arc<dyn AiWorker>,
    root: PathBuf,
    lru: Mutex<Vec<(PathBuf, Arc<Mask>)>>,
    /// Serialises generation so concurrent requests for one mask hit the worker once.
    gen_lock: Mutex<()>,
}

impl MaskStore {
    pub fn new(db: Db, worker: Arc<dyn AiWorker>, root: PathBuf) -> Self {
        Self {
            db,
            worker,
            root,
            lru: Mutex::new(Vec::new()),
            gen_lock: Mutex::new(()),
        }
    }

    fn cache_path(
        &self,
        r: &PhotoRef,
        target: MaskTarget,
        person_id: Option<i64>,
    ) -> Result<PathBuf> {
        let name = match (target, person_id) {
            (MaskTarget::Person, Some(p)) => format!("{}_person_{p}.png", r.id),
            (MaskTarget::Person, None) => {
                return Err(CoreError::bad_request(
                    "person_id is required for target=person",
                ))
            }
            (t, _) => format!("{}_{}.png", r.id, target_name(t)),
        };
        Ok(self.root.join(&r.fast_key).join(name))
    }

    /// Path of the mask PNG. With `generate`, a cache miss asks the worker (blocking, via
    /// `handle`); without it a miss yields `Ok(None)`.
    pub fn mask_file(
        &self,
        r: &PhotoRef,
        target: MaskTarget,
        person_id: Option<i64>,
        handle: &Handle,
        generate: bool,
    ) -> Result<Option<PathBuf>> {
        let path = self.cache_path(r, target, person_id)?;
        if path.is_file() {
            return Ok(Some(path));
        }
        if !generate {
            return Ok(None);
        }
        let _g = self.gen_lock.lock().unwrap();
        if path.is_file() {
            return Ok(Some(path));
        }
        let dir = path
            .parent()
            .expect("cache path has a parent")
            .to_path_buf();
        std::fs::create_dir_all(&dir)?;
        if target == MaskTarget::Background {
            // background = inverted subject (computed by the core, docs/api-contract-m3.md D)
            drop(_g);
            let subject = self
                .mask_file(r, MaskTarget::Subject, None, handle, true)?
                .ok_or_else(|| CoreError::Internal(anyhow::anyhow!("subject mask missing")))?;
            let _g = self.gen_lock.lock().unwrap();
            let mut img = image::open(&subject)
                .map_err(|e| CoreError::Internal(anyhow::anyhow!("bad mask {subject:?}: {e}")))?
                .to_luma8();
            for p in img.pixels_mut() {
                p.0[0] = 255 - p.0[0];
            }
            write_png_atomic(&img, &path)?;
            return Ok(Some(path));
        }
        let name = target_name(target);
        let person_bbox = if target == MaskTarget::Person {
            let pid = person_id.expect("checked in cache_path");
            let rid = r.id;
            let faces = self
                .db
                .with(|c| crate::analysis::store::faces_of_photo(c, rid))?;
            let face = faces
                .iter()
                .find(|f| f.person_id == Some(pid))
                .ok_or_else(|| {
                    CoreError::not_found(format!("person {pid} has no face in photo {rid}"))
                })?;
            Some(face.bbox)
        } else {
            None
        };
        let req = MaskRequest {
            photo: MaskPhoto {
                photo_id: r.id,
                path: r.path.to_string_lossy().into_owned(),
                orientation: r.orientation,
            },
            targets: vec![name.to_string()],
            person_bbox,
            size: MASK_SIZE,
            out_dir: dir.to_string_lossy().into_owned(),
            allow_download: false,
        };
        let resp = handle
            .block_on(self.worker.mask_generate(&req))
            .map_err(map_worker_err)?;
        if let Some(reason) = resp.skipped.get(name) {
            return Err(if reason == "model_unavailable" {
                CoreError::ModelsMissing(vec![format!("mask.{name}")])
            } else {
                CoreError::Conflict(format!("mask {name} is not available: {reason}"))
            });
        }
        let produced = resp.masks.get(name).ok_or_else(|| {
            CoreError::Internal(anyhow::anyhow!("worker returned no {name} mask"))
        })?;
        let produced = PathBuf::from(produced);
        if produced != path {
            std::fs::copy(&produced, &path)?;
            let _ = std::fs::remove_file(&produced);
        }
        Ok(Some(path))
    }

    /// Decoded mask (8-bit grey) with a small in-memory LRU.
    pub fn load(&self, path: &Path) -> Result<Arc<Mask>> {
        {
            let mut l = self.lru.lock().unwrap();
            if let Some(i) = l.iter().position(|(p, _)| p == path) {
                let e = l.remove(i);
                let m = e.1.clone();
                l.push(e);
                return Ok(m);
            }
        }
        let img = image::open(path)
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("bad mask {path:?}: {e}")))?
            .to_luma8();
        let m = Arc::new(Mask {
            width: img.width(),
            height: img.height(),
            data: img.into_raw(),
        });
        let mut l = self.lru.lock().unwrap();
        if l.len() >= MASK_LRU {
            l.remove(0);
        }
        l.push((path.to_path_buf(), m.clone()));
        Ok(m)
    }
}

fn write_png_atomic(img: &image::GrayImage, path: &Path) -> Result<()> {
    let tmp = path.with_extension("tmp");
    img.save_with_format(&tmp, image::ImageFormat::Png)
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("write mask: {e}")))?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

struct PhotoMasks {
    store: Arc<MaskStore>,
    patches: Arc<PatchStore>,
    beauty: Arc<super::beauty::BeautyStore>,
    photo: PhotoRef,
    handle: Handle,
    mode: MaskMode,
}

impl MaskProvider for PhotoMasks {
    fn mask(&self, target: MaskTarget, person_id: Option<i64>) -> ip_render::Result<Option<Mask>> {
        let generate = self.mode == MaskMode::Strict;
        // A local op that cannot be resolved without a person is skipped, not fatal.
        if target == MaskTarget::Person && person_id.is_none() {
            return Ok(None);
        }
        match self
            .store
            .mask_file(&self.photo, target, person_id, &self.handle, generate)
        {
            Ok(Some(p)) => Ok(Some((*self.store.load(&p)?).clone())),
            Ok(None) => Ok(None),
            // preserved through anyhow so the HTTP layer can answer 409/503
            Err(e) => Err(anyhow::Error::new(e)),
        }
    }

    fn patch(&self, asset: &str) -> ip_render::Result<Option<RgbaImage>> {
        Ok(self
            .patches
            .load(self.photo.id, asset)
            .map_err(anyhow::Error::new)?
            .map(|a| (*a).clone()))
    }

    fn people(&self) -> ip_render::Result<Vec<ip_render::PersonGeometry>> {
        let generate = self.mode == MaskMode::Strict;
        self.beauty
            .people(&self.photo, &self.handle, generate)
            .map_err(anyhow::Error::new)
    }
}

fn map_render_err(e: anyhow::Error) -> CoreError {
    match e.downcast::<CoreError>() {
        Ok(c) => c,
        Err(e) => CoreError::Internal(e),
    }
}

// ------------------------------------------------------------------ service

type Cell = Arc<OnceCell<std::result::Result<Option<PathBuf>, String>>>;

pub struct RenderService {
    pub renderer: Arc<dyn Renderer>,
    pub luts: Arc<LutLibrary>,
    pub masks: Arc<MaskStore>,
    pub beauty: Arc<super::beauty::BeautyStore>,
    pub patches: Arc<PatchStore>,
    imaging: Arc<dyn Imaging>,
    db: Db,
    gate: Arc<Semaphore>,
    proxies: Mutex<ProxyCache>,
    thumbs: ThumbCache,
    previews: ThumbCache,
    inflight: Mutex<HashMap<(i64, String, u32), Cell>>,
    tmp_seq: AtomicU64,
    /// Number of renderer invocations (tests, diagnostics).
    pub renders: AtomicUsize,
    /// Number of source decodes (proxy cache misses).
    pub decodes: AtomicUsize,
}

pub struct ServiceParts {
    pub renderer: Arc<dyn Renderer>,
    pub imaging: Arc<dyn Imaging>,
    pub db: Db,
    pub worker: Arc<dyn AiWorker>,
    pub luts_dir: PathBuf,
    pub masks_dir: PathBuf,
    pub beauty_dir: PathBuf,
    pub edited_thumbs_dir: PathBuf,
    pub edited_previews_dir: PathBuf,
    pub edits_dir: PathBuf,
}

impl RenderService {
    pub fn new(p: ServiceParts) -> Self {
        let masks = Arc::new(MaskStore::new(p.db.clone(), p.worker.clone(), p.masks_dir));
        Self {
            renderer: p.renderer,
            luts: Arc::new(LutLibrary::new(p.luts_dir)),
            beauty: Arc::new(super::beauty::BeautyStore::new(
                p.db.clone(),
                p.worker,
                p.beauty_dir,
                masks.clone(),
            )),
            masks,
            patches: Arc::new(PatchStore::new(p.edits_dir)),
            imaging: p.imaging,
            db: p.db,
            gate: Arc::new(Semaphore::new(RENDER_CONCURRENCY)),
            proxies: Mutex::new(ProxyCache::default()),
            thumbs: ThumbCache::new(p.edited_thumbs_dir),
            previews: ThumbCache::new(p.edited_previews_dir),
            inflight: Mutex::new(HashMap::new()),
            tmp_seq: AtomicU64::new(0),
            renders: AtomicUsize::new(0),
            decodes: AtomicUsize::new(0),
        }
    }

    pub fn backend(&self) -> Backend {
        self.renderer.backend()
    }

    fn cache_for(&self, size: u32) -> &ThumbCache {
        if size <= 512 {
            &self.thumbs
        } else {
            &self.previews
        }
    }

    /// Cache file of an edited image (`key` from [`super::edited_key`]).
    pub fn edited_path(&self, key: &str, size: u32) -> PathBuf {
        self.cache_for(size).path_for(key, size)
    }

    /// Removes the cached edited images of one `(fast_key, hash)` (best effort).
    pub fn purge_edited(&self, key: &str) {
        for s in THUMB_SIZES.iter().chain(PREVIEW_SIZES.iter()) {
            let _ = std::fs::remove_file(self.edited_path(key, *s));
        }
    }

    // -------------------------------------------------------------- decoding

    /// Decoded source for rendering at `edge` (long edge bucket; `None` = full resolution).
    fn source(&self, r: &PhotoRef, edge: Option<u32>) -> Result<Arc<RgbImage>> {
        let key = edge.map(|e| (r.id, r.fast_key.clone(), e));
        if let Some(k) = &key {
            if let Some(img) = self.proxies.lock().unwrap().get(k) {
                return Ok(img);
            }
        }
        let img = Arc::new(self.decode(r, edge.unwrap_or(100_000))?);
        if let Some(k) = key {
            self.proxies.lock().unwrap().put(k, img.clone());
        }
        Ok(img)
    }

    fn decode(&self, r: &PhotoRef, long_edge: u32) -> Result<RgbImage> {
        self.decodes.fetch_add(1, Ordering::SeqCst);
        let (width, height, data) = self
            .imaging
            .decode_rgb8(&r.path, r.format, r.orientation, long_edge)
            .map_err(|e| CoreError::Internal(e.context(format!("decode {}", r.path.display()))))?;
        Ok(RgbImage {
            width,
            height,
            data,
        })
    }

    // -------------------------------------------------------------- rendering

    /// Renders `stack` for `r` (blocking; call from a blocking thread). `long_edge` limits the
    /// output after cropping; `None` renders at the source's native resolution.
    pub fn render(
        &self,
        r: &PhotoRef,
        stack: &EditStack,
        long_edge: Option<u32>,
        mode: MaskMode,
        handle: &Handle,
    ) -> Result<RenderOut> {
        let t0 = Instant::now();
        if stack.is_identity() {
            let image = self.decode(r, long_edge.unwrap_or(100_000))?;
            return Ok(RenderOut {
                image,
                backend: self.backend(),
                ms: t0.elapsed().as_millis() as u64,
            });
        }
        let src = self.source(r, source_edge(r, stack, long_edge))?;
        let masks = PhotoMasks {
            store: self.masks.clone(),
            patches: self.patches.clone(),
            beauty: self.beauty.clone(),
            photo: r.clone(),
            handle: handle.clone(),
            mode,
        };
        self.renders.fetch_add(1, Ordering::SeqCst);
        let image = self
            .renderer
            .render(&RenderRequest {
                source: &src,
                stack,
                masks: &masks,
                luts: &*self.luts,
                max_long_edge: long_edge,
            })
            .map_err(map_render_err)?;
        Ok(RenderOut {
            image,
            backend: self.backend(),
            ms: t0.elapsed().as_millis() as u64,
        })
    }

    /// [`RenderService::render`] under the concurrency gate, from a blocking thread.
    pub fn render_gated(
        &self,
        r: &PhotoRef,
        stack: &EditStack,
        long_edge: Option<u32>,
        mode: MaskMode,
        handle: &Handle,
    ) -> Result<RenderOut> {
        let _permit = handle
            .block_on(self.gate.acquire())
            .map_err(|_| CoreError::Internal(anyhow::anyhow!("render gate closed")))?;
        self.render(r, stack, long_edge, mode, handle)
    }

    /// Async entry point: waits for a render slot (dropping the future while queued cancels the
    /// request, which is how superseded slider-drag previews disappear), then renders on the
    /// blocking pool.
    pub async fn render_async(
        self: &Arc<Self>,
        r: PhotoRef,
        stack: EditStack,
        long_edge: Option<u32>,
        mode: MaskMode,
    ) -> Result<RenderOut> {
        let permit = self
            .gate
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| CoreError::Internal(anyhow::anyhow!("render gate closed")))?;
        let svc = self.clone();
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            svc.render(&r, &stack, long_edge, mode, &handle)
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("render task failed: {e}")))?
    }

    /// Runs `f` under a render slot on the blocking pool (analysis helpers such as `auto`).
    pub async fn run_gated<T, F>(self: &Arc<Self>, f: F) -> Result<T>
    where
        T: Send + 'static,
        F: FnOnce(&RenderService, &Handle) -> Result<T> + Send + 'static,
    {
        let permit = self
            .gate
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| CoreError::Internal(anyhow::anyhow!("render gate closed")))?;
        let svc = self.clone();
        let handle = Handle::current();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f(&svc, &handle)
        })
        .await
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("render task failed: {e}")))?
    }

    /// Decoded proxy (cached) at roughly `long_edge`, uncropped; for analysis such as `auto`.
    pub fn proxy(&self, r: &PhotoRef, long_edge: u32) -> Result<Arc<RgbImage>> {
        self.source(r, bucket_for(long_edge))
    }

    // -------------------------------------------------------------- edited thumbnails

    /// The cached rendered thumbnail/preview of an edited photo, rendering it on a miss.
    /// `Ok(None)` = the photo has no (visible) edits or rendering failed: serve the original.
    pub async fn edited_image(self: &Arc<Self>, r: PhotoRef, size: u32) -> Result<Option<PathBuf>> {
        let id = r.id;
        let Some(cur) = self.db.call(move |c| store::current(c, id)).await? else {
            return Ok(None);
        };
        if !cur.has_edits {
            return Ok(None);
        }
        let key = super::edited_key(&r.fast_key, &cur.hash);
        let path = self.edited_path(&key, size);
        if tokio::fs::try_exists(&path).await.unwrap_or(false) {
            return Ok(Some(path));
        }
        let ck = (id, cur.hash.clone(), size);
        let cell: Cell = self
            .inflight
            .lock()
            .unwrap()
            .entry(ck.clone())
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let res = cell
            .get_or_init(|| {
                let svc = self.clone();
                let path = path.clone();
                async move {
                    let stack: EditStack = serde_json::from_value(cur.stack.clone())
                        .map_err(|e| format!("stored edit stack is invalid: {e}"))?;
                    let out = svc
                        .render_async(r, stack, Some(size), MaskMode::CachedOnly)
                        .await
                        .map_err(|e| e.to_string())?;
                    let quality = if size <= 512 {
                        THUMB_QUALITY
                    } else {
                        PREVIEW_QUALITY
                    };
                    let seq = svc.tmp_seq.fetch_add(1, Ordering::Relaxed);
                    let done_path = path.clone();
                    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                        let bytes = encode_jpeg(&out.image, quality)?;
                        if let Some(parent) = path.parent() {
                            std::fs::create_dir_all(parent)?;
                        }
                        let tmp = path.with_extension(format!("tmp{seq}"));
                        std::fs::write(&tmp, &bytes)?;
                        if let Err(e) = std::fs::rename(&tmp, &path) {
                            let _ = std::fs::remove_file(&tmp);
                            if !path.exists() {
                                return Err(e.into());
                            }
                        }
                        Ok(())
                    })
                    .await
                    .map_err(|e| e.to_string())?
                    .map_err(|e| format!("{e:#}"))?;
                    Ok(Some(done_path))
                }
            })
            .await
            .clone();
        {
            let mut m = self.inflight.lock().unwrap();
            if m.get(&ck).is_some_and(|c| Arc::ptr_eq(c, &cell)) {
                m.remove(&ck);
            }
        }
        match res {
            Ok(p) => Ok(p),
            Err(msg) => {
                tracing::warn!(photo = id, error = %msg, "edited image render failed; serving the original");
                Ok(None)
            }
        }
    }
}

/// Decode size (a bucket) that keeps the cropped output sharp at `out_long`; `None` = full.
pub fn source_edge(r: &PhotoRef, stack: &EditStack, out_long: Option<u32>) -> Option<u32> {
    let l = out_long?;
    let (rw, rh) = stack
        .ops
        .iter()
        .find_map(|o| match o {
            Op::Crop(c) => Some((
                (c.rect[2] as f64).clamp(0.01, 1.0),
                (c.rect[3] as f64).clamp(0.01, 1.0),
            )),
            _ => None,
        })
        .unwrap_or((1.0, 1.0));
    let (w, h) = match (r.width, r.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w as f64, h as f64),
        _ => (1.0, 1.0),
    };
    let crop_long = (rw * w).max(rh * h);
    let need = (l as f64 * w.max(h) / crop_long * 1.05).ceil();
    let need = need.clamp(l as f64, u32::MAX as f64) as u32;
    bucket_for(need)
}

fn bucket_for(need: u32) -> Option<u32> {
    BUCKETS.iter().copied().find(|b| *b >= need)
}

/// Baseline JPEG encode of an 8-bit RGB image.
pub fn encode_jpeg(img: &RgbImage, quality: u8) -> anyhow::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(img.data.len() / 8 + 1024);
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100));
    image::ImageEncoder::write_image(
        enc,
        &img.data,
        img.width,
        img.height,
        image::ExtendedColorType::Rgb8,
    )?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ip_imaging::ImageFormat;

    fn pref(w: u32, h: u32) -> PhotoRef {
        PhotoRef {
            id: 1,
            path: PathBuf::from("x.jpg"),
            file_name: "x.jpg".into(),
            format: ImageFormat::Jpeg,
            orientation: 1,
            fast_key: "0".repeat(32),
            taken_at: None,
            mtime_ms: None,
            edit_hash: None,
            width: Some(w),
            height: Some(h),
        }
    }

    fn crop_stack(rect: [f32; 4]) -> EditStack {
        EditStack {
            version: 1,
            ops: vec![Op::Crop(ip_render::Crop {
                rect,
                angle: 0.0,
                aspect: None,
            })],
        }
    }

    #[test]
    fn decode_size_accounts_for_crop() {
        let r = pref(6000, 4000);
        let full = EditStack::default();
        assert_eq!(source_edge(&r, &full, Some(800)), Some(1024));
        assert_eq!(source_edge(&r, &full, None), None);
        // a quarter-width crop needs ~4x the proxy
        let c = crop_stack([0.0, 0.0, 0.25, 0.25]);
        assert_eq!(source_edge(&r, &c, Some(800)), Some(4096));
        // tiny crops fall through to a full decode
        let t = crop_stack([0.0, 0.0, 0.05, 0.05]);
        assert_eq!(source_edge(&r, &t, Some(1600)), None);
    }

    #[test]
    fn proxy_cache_evicts_least_recently_used() {
        let mut c = ProxyCache::default();
        let big = |n: usize| {
            Arc::new(RgbImage {
                width: 1,
                height: 1,
                data: vec![0; n],
            })
        };
        let third = PROXY_BUDGET_BYTES / 3;
        c.put((1, "a".into(), 1), big(third));
        c.put((2, "a".into(), 1), big(third));
        assert!(c.get(&(1, "a".into(), 1)).is_some()); // touch 1
        c.put((3, "a".into(), 1), big(third));
        c.put((4, "a".into(), 1), big(third));
        assert!(c.get(&(2, "a".into(), 1)).is_none());
        assert!(c.get(&(1, "a".into(), 1)).is_some() || c.bytes <= PROXY_BUDGET_BYTES);
        assert!(c.bytes <= PROXY_BUDGET_BYTES);
    }
}
