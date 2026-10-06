//! Patch assets (docs/api-contract-m5.md section A): generated RGBA PNG layers stored under
//! `<data_dir>/edits/<photo_id>/<asset>.png`, decoded on demand for the renderer through an
//! LRU, and cleaned up with their photo.
//!
//! Patches are specific to the photo they were generated for, so they are never copied by
//! `sync` / paste / presets (see `edit::sync`).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ip_render::RgbaImage;
use serde_json::Value;

use crate::error::{CoreError, Result};

/// Longest accepted asset id.
pub const MAX_ASSET_LEN: usize = 96;
/// Memory budget of the decoded-patch cache.
const LRU_BYTES: usize = 192 * 1024 * 1024;

/// Asset ids are file names: ASCII letters, digits, `_` and `-` only.
pub fn valid_asset_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= MAX_ASSET_LEN
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

type LruEntry = ((i64, String), Arc<RgbaImage>);

pub struct PatchStore {
    root: PathBuf,
    lru: Mutex<Vec<LruEntry>>,
    seq: AtomicU64,
}

impl PatchStore {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            lru: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Directory holding the assets of one photo.
    pub fn dir(&self, photo_id: i64) -> PathBuf {
        self.root.join(photo_id.to_string())
    }

    /// File of an asset; `None` when the id is not a valid asset id.
    pub fn path(&self, photo_id: i64, asset: &str) -> Option<PathBuf> {
        valid_asset_id(asset).then(|| self.dir(photo_id).join(format!("{asset}.png")))
    }

    pub fn exists(&self, photo_id: i64, asset: &str) -> bool {
        self.path(photo_id, asset).is_some_and(|p| p.is_file())
    }

    /// The PNG bytes of an asset (`GET /api/assets/{photo_id}/{asset}`).
    pub fn read_png(&self, photo_id: i64, asset: &str) -> Result<Vec<u8>> {
        let p = self
            .path(photo_id, asset)
            .ok_or_else(|| CoreError::bad_request("invalid asset id"))?;
        std::fs::read(&p).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                CoreError::not_found(format!("asset {asset} of photo {photo_id} not found"))
            } else {
                e.into()
            }
        })
    }

    fn fresh_id(&self, prefix: &str, photo_id: i64, tag: &str) -> String {
        let ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let n = self.seq.fetch_add(1, Ordering::Relaxed);
        let tag: String = tag
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .take(16)
            .collect();
        let tag = if tag.is_empty() {
            String::new()
        } else {
            format!("{tag}_")
        };
        format!(
            "{prefix}_{photo_id}_{tag}{:x}",
            ms.wrapping_mul(1024) + (n & 1023)
        )
    }

    /// Stores the PNG the worker produced as a new asset of `photo_id` (re-encoded as 8-bit
    /// RGBA, so whatever the worker wrote, the renderer gets what the contract promises).
    /// Returns the asset id and the patch's pixel size.
    pub fn import(
        &self,
        photo_id: i64,
        prefix: &str,
        tag: &str,
        src: &Path,
    ) -> Result<(String, (u32, u32))> {
        let img = image::open(src)
            .map_err(|e| {
                CoreError::Internal(anyhow::anyhow!(
                    "the worker produced an unreadable patch {}: {e}",
                    src.display()
                ))
            })?
            .to_rgba8();
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Err(CoreError::Internal(anyhow::anyhow!("empty patch image")));
        }
        let dir = self.dir(photo_id);
        std::fs::create_dir_all(&dir)?;
        let id = self.fresh_id(prefix, photo_id, tag);
        let path = dir.join(format!("{id}.png"));
        let tmp = dir.join(format!("{id}.tmp"));
        img.save_with_format(&tmp, image::ImageFormat::Png)
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("write patch: {e}")))?;
        std::fs::rename(&tmp, &path)?;
        Ok((id, (w, h)))
    }

    /// Decoded RGBA of an asset (`Ok(None)` when it does not exist or is unreadable, so a lost
    /// file skips the patch instead of failing every render).
    pub fn load(&self, photo_id: i64, asset: &str) -> Result<Option<Arc<RgbaImage>>> {
        let key = (photo_id, asset.to_string());
        {
            let mut l = self.lru.lock().unwrap();
            if let Some(i) = l.iter().position(|(k, _)| *k == key) {
                let e = l.remove(i);
                let img = e.1.clone();
                l.push(e);
                return Ok(Some(img));
            }
        }
        let Some(path) = self.path(photo_id, asset) else {
            return Ok(None);
        };
        if !path.is_file() {
            return Ok(None);
        }
        let img = match image::open(&path) {
            Ok(i) => i.to_rgba8(),
            Err(e) => {
                tracing::warn!(asset, error = %e, "unreadable patch asset; skipping it");
                return Ok(None);
            }
        };
        let rgba = Arc::new(RgbaImage {
            width: img.width(),
            height: img.height(),
            data: img.into_raw(),
        });
        let size = rgba.data.len();
        if size <= LRU_BYTES / 2 {
            let mut l = self.lru.lock().unwrap();
            l.push((key, rgba.clone()));
            let mut total: usize = l.iter().map(|(_, i)| i.data.len()).sum();
            while total > LRU_BYTES && l.len() > 1 {
                let old = l.remove(0);
                total -= old.1.data.len();
            }
        }
        Ok(Some(rgba))
    }

    fn forget(&self, photo_id: i64, only: Option<&HashSet<String>>) {
        self.lru
            .lock()
            .unwrap()
            .retain(|((p, a), _)| *p != photo_id || only.is_some_and(|k| k.contains(a)));
    }

    /// Deletes every asset of a photo (the photo itself was deleted).
    pub fn remove_photo(&self, photo_id: i64) {
        self.forget(photo_id, None);
        let _ = std::fs::remove_dir_all(self.dir(photo_id));
    }

    /// Deletes the assets of `photo_id` that are not in `keep`.
    pub fn gc(&self, photo_id: i64, keep: &HashSet<String>) {
        self.forget(photo_id, Some(keep));
        let Ok(rd) = std::fs::read_dir(self.dir(photo_id)) else {
            return;
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let stem = name.strip_suffix(".png").unwrap_or(&name);
            if !keep.contains(stem) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// Asset ids referenced by the `patch` ops of a raw stack.
pub fn assets_in_stack(raw: &Value) -> Vec<String> {
    raw.get("ops")
        .and_then(Value::as_array)
        .map(|ops| {
            ops.iter()
                .filter(|o| o.get("type").and_then(Value::as_str) == Some("patch"))
                .filter_map(|o| o.get("asset").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_ids_are_file_safe() {
        assert!(valid_asset_id("bt_4231_881"));
        assert!(valid_asset_id("a-b_C9"));
        assert!(!valid_asset_id(""));
        assert!(!valid_asset_id("../x"));
        assert!(!valid_asset_id("a/b"));
        assert!(!valid_asset_id("a.png"));
        assert!(!valid_asset_id(&"x".repeat(MAX_ASSET_LEN + 1)));
    }
}
