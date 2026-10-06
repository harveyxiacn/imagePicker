//! Abstraction over `ip-imaging` so the pipeline is testable with a fake.

use std::path::Path;

use ip_imaging::{EncodedImage, ImageFormat, Metadata, ScannedFile};

pub type Result<T> = anyhow::Result<T>;

pub trait Imaging: Send + Sync {
    fn scan_dir(
        &self,
        root: &Path,
        recursive: bool,
        exclude: &[String],
    ) -> Result<Vec<ScannedFile>>;
    fn read_metadata(&self, path: &Path, format: ImageFormat) -> Result<Metadata>;
    fn content_key(&self, path: &Path) -> Result<String>;
    fn fast_key(&self, path: &Path, size: u64, mtime_ms: i64) -> String {
        ip_imaging::fast_key(path, size, mtime_ms)
    }
    fn generate_thumbnail(
        &self,
        path: &Path,
        format: ImageFormat,
        orientation: u8,
        long_edge: u32,
        quality: u8,
    ) -> Result<EncodedImage>;
    /// Upright RGB8 with long edge <= `max_long_edge` (render sources).
    fn decode_rgb8(
        &self,
        path: &Path,
        format: ImageFormat,
        orientation: u8,
        max_long_edge: u32,
    ) -> Result<(u32, u32, Vec<u8>)> {
        ip_imaging::decode_rgb8(path, format, orientation, max_long_edge)
    }
}

/// Delegates to `ip_imaging::*`.
#[derive(Debug, Default, Clone, Copy)]
pub struct RealImaging;

impl Imaging for RealImaging {
    fn scan_dir(
        &self,
        root: &Path,
        recursive: bool,
        exclude: &[String],
    ) -> Result<Vec<ScannedFile>> {
        ip_imaging::scan_dir(root, recursive, exclude)
    }
    fn read_metadata(&self, path: &Path, format: ImageFormat) -> Result<Metadata> {
        ip_imaging::read_metadata(path, format)
    }
    fn content_key(&self, path: &Path) -> Result<String> {
        ip_imaging::content_key(path)
    }
    fn generate_thumbnail(
        &self,
        path: &Path,
        format: ImageFormat,
        orientation: u8,
        long_edge: u32,
        quality: u8,
    ) -> Result<EncodedImage> {
        ip_imaging::generate_thumbnail(path, format, orientation, long_edge, quality)
    }
}
