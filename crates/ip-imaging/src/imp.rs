//! Implementation stubs — to be filled in by the imaging work package.
use super::*;

pub fn scan_dir(_root: &Path, _recursive: bool, _exclude: &[String]) -> Result<Vec<ScannedFile>> {
    anyhow::bail!("scan_dir: not implemented")
}

pub fn read_metadata(_path: &Path, _format: ImageFormat) -> Result<Metadata> {
    anyhow::bail!("read_metadata: not implemented")
}

pub fn content_key(_path: &Path) -> Result<String> {
    anyhow::bail!("content_key: not implemented")
}

pub fn generate_thumbnail(
    _path: &Path,
    _format: ImageFormat,
    _orientation: u8,
    _long_edge: u32,
    _quality: u8,
) -> Result<EncodedImage> {
    anyhow::bail!("generate_thumbnail: not implemented")
}

pub fn decode_rgb8(
    _path: &Path,
    _format: ImageFormat,
    _orientation: u8,
    _max_long_edge: u32,
) -> Result<(u32, u32, Vec<u8>)> {
    anyhow::bail!("decode_rgb8: not implemented")
}
