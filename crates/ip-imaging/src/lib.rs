//! Scanning, metadata, hashing and thumbnail generation.
//!
//! CONTRACT: the public signatures in this file are consumed by `ip-core`.
//! Implementations live in `imp` and may change freely; signatures change only in coordination.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub type Result<T> = anyhow::Result<T>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Heif,
    Avif,
    Tiff,
    /// Camera RAW (cr2, cr3, nef, arw, raf, orf, rw2, dng, ...).
    Raw,
}

impl ImageFormat {
    /// Detect from file extension (case-insensitive). `None` if unsupported.
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        Some(match ext.as_str() {
            "jpg" | "jpeg" | "jpe" => Self::Jpeg,
            "png" => Self::Png,
            "webp" => Self::Webp,
            "heic" | "heif" | "hif" => Self::Heif,
            "avif" => Self::Avif,
            "tif" | "tiff" => Self::Tiff,
            "cr2" | "cr3" | "nef" | "nrw" | "arw" | "srf" | "sr2" | "raf" | "orf" | "rw2"
            | "dng" | "pef" | "srw" | "3fr" | "iiq" | "x3f" => Self::Raw,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Heif => "heif",
            Self::Avif => "avif",
            Self::Tiff => "tiff",
            Self::Raw => "raw",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub size: u64,
    /// Modification time, ms since Unix epoch.
    pub mtime_ms: i64,
    pub format: ImageFormat,
}

/// List supported image files under `root`, sorted by path.
/// `exclude` entries are case-insensitive path substrings; hidden entries and
/// `.imagepicker` dirs are always skipped. Must be parallel and fast.
pub fn scan_dir(root: &Path, recursive: bool, exclude: &[String]) -> Result<Vec<ScannedFile>> {
    imp::scan_dir(root, recursive, exclude)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Metadata {
    /// Pixel dimensions as stored (before orientation is applied).
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// EXIF orientation 1..=8 (1 = normal).
    pub orientation: u8,
    /// Capture time in ms since Unix epoch, including SubSecTimeOriginal.
    /// If OffsetTimeOriginal is missing, the naive local time is interpreted as UTC.
    pub taken_at_ms: Option<i64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub camera_serial: Option<String>,
    pub lens: Option<String>,
    pub focal_mm: Option<f32>,
    pub aperture: Option<f32>,
    /// Exposure time in seconds.
    pub shutter_s: Option<f32>,
    pub iso: Option<u32>,
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
}

/// Read EXIF/container metadata without decoding pixels.
pub fn read_metadata(path: &Path, format: ImageFormat) -> Result<Metadata> {
    imp::read_metadata(path, format)
}

/// Cache key from path + size + mtime (cheap, no IO). 32 hex chars.
pub fn fast_key(path: &Path, size: u64, mtime_ms: i64) -> String {
    let mut h = blake3::Hasher::new();
    h.update(path.to_string_lossy().as_bytes());
    h.update(&size.to_le_bytes());
    h.update(&mtime_ms.to_le_bytes());
    h.finalize().to_hex()[..32].to_string()
}

/// Move-tolerant content key: blake3(first 64 KiB + last 64 KiB + size). 32 hex chars.
pub fn content_key(path: &Path) -> Result<String> {
    imp::content_key(path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThumbSource {
    /// Embedded EXIF thumbnail or RAW embedded preview.
    Embedded,
    /// JPEG decoded with DCT-domain downscaling (1/2, 1/4, 1/8).
    DctScaled,
    /// Fully decoded then resized.
    FullDecode,
}

#[derive(Debug, Clone)]
pub struct EncodedImage {
    /// JPEG bytes with orientation already applied (upright pixels).
    pub bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub source: ThumbSource,
}

/// Produce an upright JPEG whose long edge is <= `long_edge` (never upscales).
/// Fastest path first: embedded preview if large enough -> DCT-scaled decode -> full decode.
pub fn generate_thumbnail(
    path: &Path,
    format: ImageFormat,
    orientation: u8,
    long_edge: u32,
    quality: u8,
) -> Result<EncodedImage> {
    imp::generate_thumbnail(path, format, orientation, long_edge, quality)
}

/// Decode to upright RGB8 with long edge <= `max_long_edge` (analysis / render proxies).
pub fn decode_rgb8(
    path: &Path,
    format: ImageFormat,
    orientation: u8,
    max_long_edge: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    imp::decode_rgb8(path, format, orientation, max_long_edge)
}

/// On-disk thumbnail cache layout: `<root>/<k[0..2]>/<k[2..4]>/<key>_<size>.jpg`.
#[derive(Debug, Clone)]
pub struct ThumbCache {
    pub root: PathBuf,
}

impl ThumbCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn path_for(&self, key: &str, long_edge: u32) -> PathBuf {
        self.root
            .join(&key[0..2])
            .join(&key[2..4])
            .join(format!("{key}_{long_edge}.jpg"))
    }
}

mod imp;
