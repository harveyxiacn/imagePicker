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
//! libheif applies the container's crop (`clap`) and `irot`/`imir` itself; the other decoders
//! return stored pixels, cropped here, and the caller applies the orientation `read_metadata`
//! derived from `irot`/`imir`. Colours: libheif and WIC return the file's own colour space (the
//! primary item's `colr`, e.g. Display P3 on iPhones), ImageIO returns sRGB; [`load`] reports
//! which, and `thumb.rs` converts with `color.rs` after resizing.
use std::path::{Path, PathBuf};

use anyhow::anyhow;

use super::color::{self, SourceSpace};
use super::container::{self, Colr, HeifInfo};
use super::io::open_data;
use super::thumb::{decode_jpeg, Loaded, PixelSpace, Rgb};
use crate::{Result, ThumbSource};

/// See `crate::set_heif_library_dirs`.
pub fn set_library_dirs(dirs: Vec<PathBuf>, private_dir: Option<PathBuf>) {
    #[cfg(not(target_os = "android"))]
    super::libheif::set_library_dirs(dirs, private_dir);
    #[cfg(target_os = "android")]
    let _ = (dirs, private_dir);
}

/// What produced the pixels, for colour management.
#[derive(Clone, Copy)]
pub(super) enum Via<'a> {
    /// An embedded JPEG stream.
    Jpeg(&'a [u8]),
    /// The OS codec.
    Os,
    #[cfg_attr(target_os = "android", allow(dead_code))]
    Libheif,
}

/// The colour space of decoded pixels (`None`: sRGB already). `file` is the primary item's
/// `colr`; an embedded JPEG's own profile wins over it.
pub(super) fn pixel_space(file: Option<Colr<'_>>, via: Via<'_>) -> Option<SourceSpace> {
    let file = || {
        file.map(|c| match c {
            Colr::Icc(icc) => SourceSpace::Icc(icc.to_vec()),
            Colr::Nclx {
                primaries,
                transfer,
            } => SourceSpace::Nclx {
                primaries,
                transfer,
            },
        })
    };
    match via {
        Via::Jpeg(j) => color::jpeg_space(j).or_else(file),
        // ImageIO colour-matches to sRGB itself
        Via::Os if cfg!(target_os = "macos") => None,
        Via::Os | Via::Libheif => file(),
    }
}

/// Pixels with long edge >= min(`long_edge`, original) where cheap, whether the decoder already
/// applied the orientation, and their colour space.
pub fn load(path: &Path, long_edge: u32, embedded: bool) -> Result<Loaded> {
    let data = open_data(path)?;
    let info = container::parse_heif(&data);
    let loaded = |rgb, source, oriented, via| Loaded {
        rgb,
        source,
        oriented,
        space: PixelSpace::Known(pixel_space(info.colr, via)),
    };
    if let Some(j) = info.primary_jpeg {
        let (rgb, scaled) = decode_jpeg(j, long_edge)?;
        let rgb = crop(&info, rgb);
        let src = if scaled {
            ThumbSource::DctScaled
        } else {
            ThumbSource::FullDecode
        };
        return Ok(loaded(rgb, src, false, Via::Jpeg(j)));
    }
    let previews = if embedded {
        previews(&info)
    } else {
        Vec::new()
    };
    let big_enough = previews
        .iter()
        .filter(|(_, long)| *long >= long_edge)
        .find_map(|(j, _)| Some((decode_jpeg(j, long_edge).ok()?.0, *j)));
    if let Some((rgb, j)) = big_enough {
        return Ok(loaded(rgb, ThumbSource::Embedded, false, Via::Jpeg(j)));
    }
    let mut why = Vec::new();
    if info.missing_codec_config {
        why.push("the image has no decoder configuration (damaged file)".to_string());
    } else {
        match os_decode(path, long_edge) {
            Some(Ok(rgb)) => {
                let rgb = crop(&info, rgb);
                let rotated = rotated(&info, &rgb);
                return Ok(loaded(rgb, ThumbSource::FullDecode, rotated, Via::Os));
            }
            Some(Err(e)) => why.push(e),
            None => {}
        }
        #[cfg(not(target_os = "android"))]
        match super::libheif::decode(&data, long_edge, embedded, display_size(&info)) {
            Ok((rgb, thumb)) => {
                let src = if thumb {
                    ThumbSource::Embedded
                } else {
                    ThumbSource::FullDecode
                };
                return Ok(loaded(rgb, src, true, Via::Libheif));
            }
            Err(e) => why.push(e),
        }
    }
    if let Some((rgb, j)) = previews
        .iter()
        .rev()
        .find_map(|(j, _)| Some((decode_jpeg(j, long_edge).ok()?.0, *j)))
    {
        return Ok(loaded(rgb, ThumbSource::Embedded, false, Via::Jpeg(j)));
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

/// The primary image's upright size, `(0, 0)` when unknown.
fn display_size(info: &HeifInfo) -> (u32, u32) {
    match info.orientation {
        Some(5..=8) => (info.height, info.width),
        Some(_) => (info.width, info.height),
        None => (0, 0),
    }
}

/// The clean aperture of pixels a decoder returned at the coded size (`clap` ignored).
fn crop(info: &HeifInfo, rgb: Rgb) -> Rgb {
    let Some(c) = info.crop.filter(|c| (rgb.w, rgb.h) == c.coded) else {
        return rgb;
    };
    let mut data = Vec::with_capacity(c.w as usize * c.h as usize * 3);
    for y in c.y..c.y + c.h {
        let start = (y as usize * rgb.w as usize + c.x as usize) * 3;
        data.extend_from_slice(&rgb.data[start..start + c.w as usize * 3]);
    }
    Rgb {
        w: c.w,
        h: c.h,
        data,
    }
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
