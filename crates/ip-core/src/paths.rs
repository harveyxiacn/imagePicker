//! Data directory resolution (see contract "数据目录").

use std::path::{Path, PathBuf};

pub const ENV_DATA_DIR: &str = "IMAGEPICKER_DATA_DIR";

/// Priority: explicit argument > `IMAGEPICKER_DATA_DIR` > platform default
/// (`%APPDATA%\imagePicker`, `~/Library/Application Support/imagePicker`, `$XDG_DATA_HOME/imagePicker`).
pub fn resolve_data_dir(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Some(p) = std::env::var_os(ENV_DATA_DIR).filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("imagePicker")
}

#[derive(Debug, Clone)]
pub struct DataDirs {
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub thumbs: PathBuf,
    pub previews: PathBuf,
    pub logs: PathBuf,
}

impl DataDirs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            catalog: root.join("catalog.db"),
            thumbs: root.join("cache").join("thumbs"),
            previews: root.join("cache").join("previews"),
            logs: root.join("logs"),
            root,
        }
    }

    pub fn create(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.thumbs)?;
        std::fs::create_dir_all(&self.previews)?;
        std::fs::create_dir_all(&self.logs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_wins() {
        let p = resolve_data_dir(Some(Path::new("/x/y")));
        assert_eq!(p, PathBuf::from("/x/y"));
        let d = DataDirs::new(p);
        assert!(d.thumbs.ends_with("cache/thumbs") || d.thumbs.ends_with("cache\\thumbs"));
    }
}
