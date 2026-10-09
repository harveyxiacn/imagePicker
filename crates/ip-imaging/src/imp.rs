//! Implementation of the public imaging API (see `lib.rs` for the contract).
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use super::*;

mod color;
mod container;
mod copy_exif;
mod dcjpeg;
mod exif;
mod io;
mod orient;
mod scan;
mod thumb;
mod tiff;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_dc;

use io::{open_data, read_head};
use tiff::Tiff;

pub use copy_exif::read_exif;
pub use scan::scan_dir;
pub use thumb::{decode_rgb8, generate_thumbnail};

fn jpeg_meta(data: &[u8], md: &mut Metadata) {
    let Some(info) = container::jpeg_info(data) else {
        return;
    };
    let mut dims = (info.width > 0).then_some((info.width, info.height));
    if let Some(t) = info.exif_tiff.and_then(Tiff::new) {
        let d = exif::fill_from_tiff(md, &t);
        dims = dims.or(d);
    }
    if let Some((w, h)) = dims {
        md.width = Some(w);
        md.height = Some(h);
    }
}

fn header_dims(path: &Path, md: &mut Metadata) {
    if let Ok((w, h)) = image::image_dimensions(path) {
        md.width = Some(w);
        md.height = Some(h);
    }
}

fn set_dims(md: &mut Metadata, d: Option<(u32, u32)>) {
    if let Some((w, h)) = d {
        md.width = Some(w);
        md.height = Some(h);
    }
}

fn raw_meta(path: &Path, md: &mut Metadata) -> Result<()> {
    let data = open_data(path)?;
    if container::is_raf(&data) {
        if let Some(j) = container::raf_jpeg(&data) {
            jpeg_meta(j, md);
        }
        return Ok(());
    }
    if container::is_cr3(&data) {
        let c = container::parse_cr3(&data);
        let mut dims = None;
        if let Some(t) = c.cmt1.and_then(Tiff::new) {
            exif::fill_from_tiff(md, &t);
        }
        if let Some(t) = c.cmt2.and_then(Tiff::new) {
            if let Some((ifd, _)) = t.first_ifd_offset().and_then(|o| t.read_ifd(o)) {
                exif::fill_exif(md, &t, &ifd);
                let x = ifd.get(0xA002).and_then(|e| t.first_uint(e));
                let y = ifd.get(0xA003).and_then(|e| t.first_uint(e));
                if let (Some(x), Some(y)) = (x, y) {
                    dims = Some((x, y));
                }
            }
        }
        if let Some(t) = c.cmt4.and_then(Tiff::new) {
            if let Some((ifd, _)) = t.first_ifd_offset().and_then(|o| t.read_ifd(o)) {
                exif::fill_gps(md, &t, &ifd);
            }
        }
        if dims.is_none() {
            dims = c.prvw.and_then(container::usable_jpeg_dims);
        }
        set_dims(md, dims);
        return Ok(());
    }
    let mut dims = None;
    if let Some(t) = Tiff::new(&data) {
        dims = exif::fill_from_tiff(md, &t);
    }
    // Fill gaps (ORF/RW2/PEF keep little in IFD0) from the embedded preview's EXIF.
    if md.taken_at_ms.is_none() || md.camera_make.is_none() || dims.is_none() {
        if let Some(p) = container::raw_preview(&data) {
            let mut pm = Metadata::default();
            jpeg_meta(p, &mut pm);
            if md.taken_at_ms.is_none() {
                md.taken_at_ms = pm.taken_at_ms;
            }
            md.camera_make = md.camera_make.take().or(pm.camera_make);
            md.camera_model = md.camera_model.take().or(pm.camera_model);
            md.camera_serial = md.camera_serial.take().or(pm.camera_serial);
            md.lens = md.lens.take().or(pm.lens);
            md.focal_mm = md.focal_mm.or(pm.focal_mm);
            md.aperture = md.aperture.or(pm.aperture);
            md.shutter_s = md.shutter_s.or(pm.shutter_s);
            md.iso = md.iso.or(pm.iso);
            md.gps_lat = md.gps_lat.or(pm.gps_lat);
            md.gps_lon = md.gps_lon.or(pm.gps_lon);
            if md.orientation == 0 {
                md.orientation = pm.orientation;
            }
            if dims.is_none() {
                dims = pm.width.zip(pm.height);
            }
        }
    }
    set_dims(md, dims);
    Ok(())
}

pub fn read_metadata(path: &Path, format: ImageFormat) -> Result<Metadata> {
    let mut md = Metadata::default();
    match format {
        ImageFormat::Jpeg => {
            let head = read_head(path, 1 << 20)?;
            jpeg_meta(&head, &mut md);
            if md.width.is_none() {
                header_dims(path, &mut md);
            }
        }
        ImageFormat::Tiff => {
            let data = open_data(path)?;
            if let Some(t) = Tiff::new(&data) {
                let d = exif::fill_from_tiff(&mut md, &t);
                set_dims(&mut md, d);
            }
            if md.width.is_none() {
                header_dims(path, &mut md);
            }
        }
        ImageFormat::Raw => {
            raw_meta(path, &mut md)?;
        }
        ImageFormat::Png | ImageFormat::Webp => {
            std::fs::metadata(path)?;
            header_dims(path, &mut md);
        }
        ImageFormat::Heif | ImageFormat::Avif => {
            let data = open_data(path)?;
            let h = container::parse_heif(&data);
            if h.width > 0 {
                md.width = Some(h.width);
                md.height = Some(h.height);
            }
            if let Some(t) = h.exif_tiff.and_then(Tiff::new) {
                exif::fill_from_tiff(&mut md, &t);
            }
        }
    }
    if !(1..=8).contains(&md.orientation) {
        md.orientation = 1;
    }
    Ok(md)
}

pub fn content_key(path: &Path) -> Result<String> {
    const N: u64 = 64 * 1024;
    let mut f = std::fs::File::open(path)?;
    let len = f.metadata()?.len();
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; N.min(len) as usize];
    f.read_exact(&mut buf)?;
    h.update(&buf);
    if len > N {
        let start = len - N;
        f.seek(SeekFrom::Start(start))?;
        f.read_exact(&mut buf)?;
        h.update(&buf);
    }
    h.update(&len.to_le_bytes());
    Ok(h.finalize().to_hex()[..32].to_string())
}
