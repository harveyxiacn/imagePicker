//! Test helpers: a pure-Rust fake of the imaging layer.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use ip_imaging::{EncodedImage, ImageFormat, Metadata, ScannedFile, ThumbSource};

pub use crate::fake_renderer::FakeRenderer;
pub use crate::fake_worker::{person, unit, FakeFace, FakeSpec, FakeWorker, BEAUTY_MODEL};
use crate::imaging::Imaging;

/// Decodes real image files with the `image` crate and produces tiny JPEG thumbnails.
/// `taken_at` is the file mtime, so tests can control ordering through file times.
#[derive(Default)]
pub struct FakeImaging {
    pub thumbnails_generated: AtomicUsize,
}

impl FakeImaging {
    pub fn new() -> Self {
        Self::default()
    }
}

fn walk(dir: &Path, recursive: bool, out: &mut Vec<ScannedFile>) -> std::io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let e = e?;
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let ft = e.file_type()?;
        if ft.is_dir() {
            if recursive {
                walk(&path, recursive, out)?;
            }
        } else if let Some(format) = ImageFormat::from_path(&path) {
            let md = e.metadata()?;
            let mtime_ms = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            out.push(ScannedFile {
                path,
                size: md.len(),
                mtime_ms,
                format,
            });
        }
    }
    Ok(())
}

impl Imaging for FakeImaging {
    fn scan_dir(
        &self,
        root: &Path,
        recursive: bool,
        _exclude: &[String],
    ) -> Result<Vec<ScannedFile>> {
        let mut v = Vec::new();
        walk(root, recursive, &mut v)?;
        v.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(v)
    }

    fn read_metadata(&self, path: &Path, _format: ImageFormat) -> Result<Metadata> {
        let (w, h) = image::image_dimensions(path)
            .with_context(|| format!("dimensions of {}", path.display()))?;
        let mtime = std::fs::metadata(path)?
            .modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as i64;
        Ok(Metadata {
            width: Some(w),
            height: Some(h),
            orientation: 1,
            taken_at_ms: Some(mtime),
            camera_make: Some("FAKE".into()),
            camera_model: Some("FAKE Cam 1".into()),
            lens: Some("50mm".into()),
            focal_mm: Some(50.0),
            aperture: Some(1.8),
            shutter_s: Some(0.01),
            iso: Some(100),
            ..Default::default()
        })
    }

    fn content_key(&self, path: &Path) -> Result<String> {
        let bytes = std::fs::read(path)?;
        Ok(blake3::hash(&bytes).to_hex()[..32].to_string())
    }

    fn generate_thumbnail(
        &self,
        path: &Path,
        _format: ImageFormat,
        _orientation: u8,
        long_edge: u32,
        quality: u8,
    ) -> Result<EncodedImage> {
        let img = image::open(path).with_context(|| format!("decode {}", path.display()))?;
        let img = if img.width().max(img.height()) > long_edge {
            img.thumbnail(long_edge, long_edge)
        } else {
            img
        };
        let rgb = img.to_rgb8();
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(&rgb)?;
        self.thumbnails_generated.fetch_add(1, Ordering::SeqCst);
        Ok(EncodedImage {
            bytes,
            width: rgb.width(),
            height: rgb.height(),
            source: ThumbSource::FullDecode,
        })
    }
}

/// Writes a solid-colour JPEG of the given size.
pub fn write_jpeg(path: &Path, w: u32, h: u32, shade: u8) {
    let img = image::RgbImage::from_pixel(w, h, image::Rgb([shade, 255 - shade, 128]));
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    img.save_with_format(path, image::ImageFormat::Jpeg)
        .unwrap();
}

/// Creates `n` JPEGs (`img_000.jpg` ...) in `dir` and returns their paths. Sizes cycle through
/// landscape/portrait shapes; mtimes increase with the index unless `reverse_time`.
pub fn make_photos(dir: &Path, n: usize) -> Vec<PathBuf> {
    (0..n)
        .map(|i| {
            let p = dir.join(format!("img_{i:03}.jpg"));
            let (w, h) = if i % 2 == 0 { (640, 480) } else { (480, 640) };
            write_jpeg(&p, w, h, (i * 7 % 255) as u8);
            let t = std::time::UNIX_EPOCH
                + std::time::Duration::from_secs(1_700_000_000 + i as u64 * 60);
            let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            f.set_modified(t).unwrap();
            p
        })
        .collect()
}
