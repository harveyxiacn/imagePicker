//! Thumbnail / proxy pipeline: embedded preview -> DCT-scaled decode -> full decode, SIMD resize.
use std::path::Path;

use anyhow::{anyhow, bail, Context};
use fast_image_resize as fir;

use super::container::{self, jpeg_info};
use super::io::{open_data, read_all};
use super::orient;
use crate::{EncodedImage, ImageFormat, Result, ThumbSource};

pub struct Rgb {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

fn fit_dims(w: u32, h: u32, long_edge: u32) -> (u32, u32) {
    let long = w.max(h);
    if long <= long_edge {
        return (w, h);
    }
    let s = long_edge as f64 / long as f64;
    (
        ((w as f64 * s).round() as u32).max(1),
        ((h as f64 * s).round() as u32).max(1),
    )
}

pub fn resize_rgb(src: Rgb, nw: u32, nh: u32) -> Result<Rgb> {
    if src.w == nw && src.h == nh {
        return Ok(src);
    }
    let src_img = fir::images::Image::from_vec_u8(src.w, src.h, src.data, fir::PixelType::U8x3)?;
    let mut dst = fir::images::Image::new(nw, nh, fir::PixelType::U8x3);
    let opts =
        fir::ResizeOptions::new().resize_alg(fir::ResizeAlg::Convolution(fir::FilterType::Hamming));
    fir::Resizer::new().resize(&src_img, &mut dst, &opts)?;
    Ok(Rgb {
        w: nw,
        h: nh,
        data: dst.into_vec(),
    })
}

/// Decode a JPEG, DCT-scaling by the smallest factor that keeps long edge >= `target_long`.
/// Returns the pixels and whether DCT scaling was actually applied.
pub fn decode_jpeg(data: &[u8], target_long: u32) -> Result<(Rgb, bool)> {
    match decode_jpeg_fast(data, target_long) {
        Ok(r) => Ok(r),
        Err(e) => {
            // Fallback (arithmetic coding, odd markers, ...): the `image` crate.
            let img = image::load_from_memory_with_format(data, image::ImageFormat::Jpeg)
                .map_err(|e2| anyhow!("JPEG decode failed: {e}; fallback: {e2}"))?
                .to_rgb8();
            let (w, h) = img.dimensions();
            Ok((
                Rgb {
                    w,
                    h,
                    data: img.into_raw(),
                },
                false,
            ))
        }
    }
}

fn decode_jpeg_fast(data: &[u8], target_long: u32) -> Result<(Rgb, bool)> {
    // Fastest path: DC-only 1/8 decode when that is still >= the target.
    if target_long > 0 {
        if let Some(i) = jpeg_info(data) {
            if matches!(i.sof, 0xC0 | 0xC1) && i.width.max(i.height).div_ceil(8) >= target_long {
                if let Some((w, h, d)) = super::dcjpeg::decode_eighth(data) {
                    return Ok((Rgb { w, h, data: d }, true));
                }
            }
        }
    }
    let mut dec = jpeg_decoder::Decoder::new(data);
    dec.read_info().context("jpeg header")?;
    let info = dec.info().ok_or_else(|| anyhow!("no jpeg info"))?;
    let (w, h) = (info.width as u32, info.height as u32);
    let long = w.max(h);
    let mut scaled = false;
    if target_long > 0 && target_long < long {
        let rw = (w as u64 * target_long as u64).div_ceil(long as u64).max(1) as u16;
        let rh = (h as u64 * target_long as u64).div_ceil(long as u64).max(1) as u16;
        let (sw, sh) = dec.scale(rw, rh)?;
        scaled = sw as u32 != w || sh as u32 != h;
    }
    let px = dec.decode()?;
    let info = dec.info().ok_or_else(|| anyhow!("no jpeg info"))?;
    let (ow, oh) = (info.width as u32, info.height as u32);
    let n = ow as usize * oh as usize;
    let data = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => px,
        jpeg_decoder::PixelFormat::L8 => {
            let mut v = Vec::with_capacity(n * 3);
            for &g in &px[..n] {
                v.extend_from_slice(&[g, g, g]);
            }
            v
        }
        jpeg_decoder::PixelFormat::L16 => {
            let mut v = Vec::with_capacity(n * 3);
            for c in px[..n * 2].as_chunks::<2>().0.iter() {
                let g = c[0];
                v.extend_from_slice(&[g, g, g]);
            }
            v
        }
        jpeg_decoder::PixelFormat::CMYK32 => {
            // Adobe CMYK JPEGs are stored inverted: r = c * k / 255.
            let mut v = Vec::with_capacity(n * 3);
            for p in px[..n * 4].as_chunks::<4>().0.iter() {
                let k = p[3] as u32;
                v.push((p[0] as u32 * k / 255) as u8);
                v.push((p[1] as u32 * k / 255) as u8);
                v.push((p[2] as u32 * k / 255) as u8);
            }
            v
        }
    };
    if data.len() < n * 3 {
        bail!("short JPEG decode buffer");
    }
    Ok((Rgb { w: ow, h: oh, data }, scaled))
}

fn decode_with_image(path: &Path) -> Result<Rgb> {
    let img = image::ImageReader::open(path)?
        .with_guessed_format()?
        .decode()
        .map_err(|e| anyhow!("decode {}: {e}", path.display()))?
        .to_rgb8();
    let (w, h) = img.dimensions();
    Ok(Rgb {
        w,
        h,
        data: img.into_raw(),
    })
}

/// Load pixels (not yet oriented) with long edge >= min(`long_edge`, original) where cheap.
fn load(
    path: &Path,
    format: ImageFormat,
    long_edge: u32,
    allow_embedded_thumb: bool,
) -> Result<(Rgb, ThumbSource)> {
    match format {
        ImageFormat::Jpeg => {
            let data = read_all(path)?;
            if allow_embedded_thumb {
                if let Some(info) = jpeg_info(&data) {
                    if let Some(tb) = info.exif_tiff.and_then(container::exif_thumbnail) {
                        if let Some((tw, th)) = container::usable_jpeg_dims(tb) {
                            let aspect_ok = info.width == 0
                                || ((tw as f64 / th as f64)
                                    - (info.width as f64 / info.height as f64))
                                    .abs()
                                    < 0.02 * (info.width as f64 / info.height as f64);
                            if tw.max(th) >= long_edge && aspect_ok {
                                if let Ok((rgb, _)) = decode_jpeg(tb, long_edge) {
                                    return Ok((rgb, ThumbSource::Embedded));
                                }
                            }
                        }
                    }
                }
            }
            let (rgb, scaled) = decode_jpeg(&data, long_edge)?;
            Ok((
                rgb,
                if scaled {
                    ThumbSource::DctScaled
                } else {
                    ThumbSource::FullDecode
                },
            ))
        }
        ImageFormat::Raw => {
            let data = open_data(path)?;
            let prev = container::raw_preview(&data).ok_or_else(|| {
                anyhow!(
                    "no embedded JPEG preview found in RAW file {}",
                    path.display()
                )
            })?;
            let (rgb, _) = decode_jpeg(prev, long_edge)?;
            Ok((rgb, ThumbSource::Embedded))
        }
        ImageFormat::Png | ImageFormat::Webp | ImageFormat::Tiff => {
            Ok((decode_with_image(path)?, ThumbSource::FullDecode))
        }
        ImageFormat::Heif | ImageFormat::Avif => {
            bail!(
                "HEIF/AVIF not supported yet (no pure-Rust decoder available): {}",
                path.display()
            )
        }
    }
}

fn upright(
    path: &Path,
    format: ImageFormat,
    orientation: u8,
    long_edge: u32,
    embedded: bool,
) -> Result<(Rgb, ThumbSource)> {
    if long_edge == 0 {
        bail!("long_edge must be > 0");
    }
    let (rgb, src) = load(path, format, long_edge, embedded)?;
    let (nw, nh) = fit_dims(rgb.w, rgb.h, long_edge);
    let rgb = resize_rgb(rgb, nw, nh)?;
    let (w, h, data) = orient::apply(rgb.w, rgb.h, rgb.data, orientation);
    Ok((Rgb { w, h, data }, src))
}

pub fn generate_thumbnail(
    path: &Path,
    format: ImageFormat,
    orientation: u8,
    long_edge: u32,
    quality: u8,
) -> Result<EncodedImage> {
    let (rgb, source) = upright(path, format, orientation, long_edge, true)?;
    let mut bytes = Vec::with_capacity(32 * 1024);
    let enc = jpeg_encoder::Encoder::new(&mut bytes, quality.clamp(1, 100));
    enc.encode(
        &rgb.data,
        rgb.w as u16,
        rgb.h as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(EncodedImage {
        bytes,
        width: rgb.w,
        height: rgb.h,
        source,
    })
}

pub fn decode_rgb8(
    path: &Path,
    format: ImageFormat,
    orientation: u8,
    max_long_edge: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    let (rgb, _) = upright(path, format, orientation, max_long_edge, false)?;
    Ok((rgb.w, rgb.h, rgb.data))
}
