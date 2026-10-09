//! HEIF/HEIC and AVIF pixels (docs/02 "HEIF/HEIC 解码"). There is no pure-Rust HEVC decoder and
//! bundling one would ship a patented codec, so pixels come from, in order:
//!
//! 1. a JPEG inside the file: a JPEG-coded primary image, or (thumbnails only) a JPEG thumbnail
//!    item / the EXIF thumbnail when it is large enough;
//! 2. the OS codec: Windows WIC (Store "HEIF Image Extensions" + "HEVC Video Extensions"),
//!    macOS ImageIO;
//! 3. libheif loaded at runtime (see `libheif.rs`), never on Android;
//! 4. (thumbnails only) a smaller embedded JPEG: a soft thumbnail beats a broken tile.
//!
//! Decoders return stored pixels; the caller applies the orientation `read_metadata` derived
//! from `irot`/`imir`.
use std::path::{Path, PathBuf};

use anyhow::anyhow;

use super::container::{self, HeifInfo};
use super::io::open_data;
use super::thumb::{decode_jpeg, Rgb};
use crate::{Result, ThumbSource};

/// See `crate::set_heif_library_dirs`.
pub fn set_library_dirs(dirs: Vec<PathBuf>, private_dir: Option<PathBuf>) {
    #[cfg(not(target_os = "android"))]
    super::libheif::set_library_dirs(dirs, private_dir);
    #[cfg(target_os = "android")]
    let _ = (dirs, private_dir);
}

/// Pixels with long edge >= min(`long_edge`, original) where cheap, plus whether the decoder
/// already applied the orientation.
pub fn load(path: &Path, long_edge: u32, embedded: bool) -> Result<(Rgb, ThumbSource, bool)> {
    let data = open_data(path)?;
    let info = container::parse_heif(&data);
    if let Some(j) = info.primary_jpeg {
        let (rgb, scaled) = decode_jpeg(j, long_edge)?;
        let src = if scaled {
            ThumbSource::DctScaled
        } else {
            ThumbSource::FullDecode
        };
        return Ok((rgb, src, false));
    }
    let previews = if embedded {
        previews(&info)
    } else {
        Vec::new()
    };
    let big_enough = previews
        .iter()
        .filter(|(_, long)| *long >= long_edge)
        .find_map(|(j, _)| decode_jpeg(j, long_edge).ok());
    if let Some((rgb, _)) = big_enough {
        return Ok((rgb, ThumbSource::Embedded, false));
    }
    let mut why = Vec::new();
    match os_decode(path, long_edge) {
        Some(Ok(rgb)) => {
            let rotated = rotated(&info, &rgb);
            return Ok((rgb, ThumbSource::FullDecode, rotated));
        }
        Some(Err(e)) => why.push(e),
        None => {}
    }
    #[cfg(not(target_os = "android"))]
    match super::libheif::decode(&data, long_edge, embedded, (info.width, info.height)) {
        Ok((rgb, thumb)) => {
            let src = if thumb {
                ThumbSource::Embedded
            } else {
                ThumbSource::FullDecode
            };
            return Ok((rgb, src, false));
        }
        Err(e) => why.push(e),
    }
    if let Some((rgb, _)) = previews
        .iter()
        .rev()
        .find_map(|(j, _)| decode_jpeg(j, long_edge).ok())
    {
        return Ok((rgb, ThumbSource::Embedded, false));
    }
    if why.is_empty() {
        why.push("no HEIF decoder on this platform".into());
    }
    Err(anyhow!(
        "cannot decode HEIF/AVIF {}: {}",
        path.display(),
        why.join("; ")
    ))
}

/// Embedded JPEGs with the primary image's aspect ratio (2%), smallest first, with long edge.
fn previews<'a>(info: &HeifInfo<'a>) -> Vec<(&'a [u8], u32)> {
    let mut v: Vec<(&[u8], (u32, u32))> = info
        .jpeg_previews
        .iter()
        .filter_map(|j| Some((*j, container::usable_jpeg_dims(j)?)))
        .filter(|(_, (w, h))| {
            info.width == 0
                || info.height == 0
                || ((*w as f64 / *h as f64) - (info.width as f64 / info.height as f64)).abs()
                    < 0.02 * (info.width as f64 / info.height as f64)
        })
        .collect();
    v.sort_by_key(|(_, (w, h))| *w as u64 * *h as u64);
    v.into_iter().map(|(j, (w, h))| (j, w.max(h))).collect()
}

/// An OS codec whose output has its long side on the other axis than the stored image applied
/// the container rotation itself (a 180° turn or a mirror would go unnoticed).
fn rotated(info: &HeifInfo, rgb: &Rgb) -> bool {
    info.width != info.height && rgb.w != rgb.h && (info.width > info.height) != (rgb.w > rgb.h)
}

/// The OS codec; `None` when the platform has none.
#[cfg(windows)]
pub(super) fn os_decode(path: &Path, _long_edge: u32) -> Option<std::result::Result<Rgb, String>> {
    Some(super::wic::decode(path))
}

#[cfg(target_os = "macos")]
pub(super) fn os_decode(path: &Path, long_edge: u32) -> Option<std::result::Result<Rgb, String>> {
    Some(super::imageio::decode(path, long_edge))
}

#[cfg(not(any(windows, target_os = "macos")))]
pub(super) fn os_decode(_path: &Path, _long_edge: u32) -> Option<std::result::Result<Rgb, String>> {
    None
}
