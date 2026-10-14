//! Data directory resolution (see contract "数据目录").

use std::path::{Path, PathBuf};

pub const ENV_DATA_DIR: &str = "IMAGEPICKER_DATA_DIR";

/// Marker file next to the executable that switches on portable mode (an empty file is enough).
pub const PORTABLE_MARKER: &str = "imagepicker.portable";
/// In portable mode the data lives in this folder next to the executable. A `data/` folder that
/// already exists there switches portable mode on as well (no marker needed).
pub const PORTABLE_DATA_DIR: &str = "data";

/// Where the data directory came from, highest priority first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirSource {
    /// `--data-dir` / `CoreConfig::data_dir` / `ServerConfig::data_dir`.
    Explicit,
    /// `IMAGEPICKER_DATA_DIR`.
    Env,
    /// `imagepicker.portable` (or a `data/` folder) next to the executable.
    Portable,
    /// `%APPDATA%\imagePicker`, `~/Library/Application Support/imagePicker`, `$XDG_DATA_HOME/imagePicker`.
    Platform,
}

impl DataDirSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Env => "env",
            Self::Portable => "portable",
            Self::Platform => "platform",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDirChoice {
    pub dir: PathBuf,
    pub source: DataDirSource,
}

/// The data folder of portable mode (`<exe dir>/data`) if the executable folder carries the
/// marker file or already has a `data/` folder. Never on Android.
pub fn portable_data_dir(exe_dir: &Path) -> Option<PathBuf> {
    if cfg!(target_os = "android") {
        return None;
    }
    let data = exe_dir.join(PORTABLE_DATA_DIR);
    (exe_dir.join(PORTABLE_MARKER).is_file() || data.is_dir()).then_some(data)
}

/// The folder of the running executable (`None` when the OS cannot tell).
pub fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// Pure resolution with every input injected.
///
/// Priority: `explicit` (`--data-dir`) > `env_dir` (`IMAGEPICKER_DATA_DIR`, empty = unset) >
/// portable folder next to `exe_dir` > `platform_default`.
pub fn resolve_data_dir_with(
    explicit: Option<&Path>,
    env_dir: Option<&std::ffi::OsStr>,
    exe_dir: Option<&Path>,
    platform_default: impl FnOnce() -> PathBuf,
) -> DataDirChoice {
    if let Some(p) = explicit {
        return DataDirChoice {
            dir: p.to_path_buf(),
            source: DataDirSource::Explicit,
        };
    }
    if let Some(p) = env_dir.filter(|v| !v.is_empty()) {
        return DataDirChoice {
            dir: PathBuf::from(p),
            source: DataDirSource::Env,
        };
    }
    if let Some(p) = exe_dir.and_then(portable_data_dir) {
        return DataDirChoice {
            dir: p,
            source: DataDirSource::Portable,
        };
    }
    DataDirChoice {
        dir: platform_default(),
        source: DataDirSource::Platform,
    }
}

pub fn platform_data_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("imagePicker")
}

/// [`resolve_data_dir_with`] with the real environment and executable folder.
pub fn resolve_data_dir_choice(explicit: Option<&Path>, exe_dir: Option<&Path>) -> DataDirChoice {
    resolve_data_dir_with(
        explicit,
        std::env::var_os(ENV_DATA_DIR).as_deref(),
        exe_dir,
        platform_data_dir,
    )
}

/// On Android there is no platform data dir to ask for: the shell (Tauri plugin) passes the app's
/// private files dir as `CoreConfig::data_dir` / `ServerConfig::data_dir` (or sets
/// `IMAGEPICKER_DATA_DIR`); [`data_dir_resolvable`] lets callers fail fast otherwise.
///
/// Priority: explicit argument > `IMAGEPICKER_DATA_DIR` > portable mode (`imagepicker.portable`
/// or `data/` next to the executable) > platform default
/// (`%APPDATA%\imagePicker`, `~/Library/Application Support/imagePicker`, `$XDG_DATA_HOME/imagePicker`).
pub fn resolve_data_dir(explicit: Option<&Path>) -> PathBuf {
    resolve_data_dir_choice(explicit, current_exe_dir().as_deref()).dir
}

/// `false` only on Android when neither an explicit dir nor `IMAGEPICKER_DATA_DIR` is given.
pub fn data_dir_resolvable(explicit: Option<&Path>) -> bool {
    !cfg!(target_os = "android")
        || explicit.is_some()
        || std::env::var_os(ENV_DATA_DIR).is_some_and(|v| !v.is_empty())
}

#[derive(Debug, Clone)]
pub struct DataDirs {
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub thumbs: PathBuf,
    pub previews: PathBuf,
    /// Full-resolution images for the loupe at 100% (originals and rendered edits), trimmed to
    /// [`crate::thumbs::FULL_CACHE_BUDGET`]; counted as previews in the cache statistics.
    pub full: PathBuf,
    pub logs: PathBuf,
    /// Crash records (`crash-*.txt` from Rust panics, `worker-*` from the AI worker), see `logging`.
    pub crashes: PathBuf,
    /// Imported `.cube` LUTs (`/api/luts/import`).
    pub luts: PathBuf,
    /// AI mask PNGs generated by the worker.
    pub masks: PathBuf,
    /// Person geometry (skin/body masks + JSON) prepared by the worker for portrait retouching.
    pub beauty: PathBuf,
    /// Rendered thumbnails / previews of edited photos (keyed by edit hash).
    pub edited_thumbs: PathBuf,
    pub edited_previews: PathBuf,
    /// Generated patch assets (`<edits>/<photo_id>/<asset>.png`, M5).
    pub edits: PathBuf,
    /// Scratch space handed to the worker for generative requests.
    pub gen: PathBuf,
    /// Catalog backups (`backup`: daily `VACUUM INTO` copies, replaced and unreadable catalogs).
    pub backups: PathBuf,
}

impl DataDirs {
    pub fn new(root: PathBuf) -> Self {
        Self {
            catalog: root.join("catalog.db"),
            thumbs: root.join("cache").join("thumbs"),
            previews: root.join("cache").join("previews"),
            full: root.join("cache").join("full"),
            logs: root.join("logs"),
            crashes: root.join("crashes"),
            luts: root.join("luts"),
            masks: root.join("cache").join("masks"),
            beauty: root.join("cache").join("beauty"),
            edited_thumbs: root.join("cache").join("edited-thumbs"),
            edited_previews: root.join("cache").join("edited-previews"),
            edits: root.join("edits"),
            gen: root.join("cache").join("gen"),
            backups: root.join("backups"),
            root,
        }
    }

    pub fn create(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.thumbs)?;
        std::fs::create_dir_all(&self.previews)?;
        std::fs::create_dir_all(&self.full)?;
        std::fs::create_dir_all(&self.luts)?;
        std::fs::create_dir_all(&self.masks)?;
        std::fs::create_dir_all(&self.beauty)?;
        std::fs::create_dir_all(&self.edited_thumbs)?;
        std::fs::create_dir_all(&self.edited_previews)?;
        std::fs::create_dir_all(&self.edits)?;
        std::fs::create_dir_all(&self.gen)?;
        std::fs::create_dir_all(&self.backups)?;
        std::fs::create_dir_all(&self.crashes)?;
        std::fs::create_dir_all(&self.logs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn plat() -> PathBuf {
        PathBuf::from("/profile/imagePicker")
    }

    fn exe_dir(marker: bool, data: bool) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        if marker {
            std::fs::write(d.path().join(PORTABLE_MARKER), b"").unwrap();
        }
        if data {
            std::fs::create_dir(d.path().join(PORTABLE_DATA_DIR)).unwrap();
        }
        d
    }

    #[test]
    fn explicit_wins() {
        let p = resolve_data_dir(Some(Path::new("/x/y")));
        assert_eq!(p, PathBuf::from("/x/y"));
        let d = DataDirs::new(p);
        assert!(d.thumbs.ends_with("cache/thumbs") || d.thumbs.ends_with("cache\\thumbs"));
    }

    #[test]
    fn precedence_explicit_env_portable_platform() {
        let exe = exe_dir(true, false);
        let (e, env) = (Some(exe.path()), Some(OsStr::new("/from/env")));
        let c = resolve_data_dir_with(Some(Path::new("/arg")), env, e, plat);
        assert_eq!((c.dir, c.source), ("/arg".into(), DataDirSource::Explicit));
        let c = resolve_data_dir_with(None, env, e, plat);
        assert_eq!((c.dir, c.source), ("/from/env".into(), DataDirSource::Env));
        let c = resolve_data_dir_with(None, None, e, plat);
        assert_eq!(c.source, DataDirSource::Portable);
        assert_eq!(c.dir, exe.path().join("data"));
        // an empty variable counts as unset
        let c = resolve_data_dir_with(None, Some(OsStr::new("")), e, plat);
        assert_eq!(c.source, DataDirSource::Portable);
        let c = resolve_data_dir_with(None, None, None, plat);
        assert_eq!((c.dir, c.source), (plat(), DataDirSource::Platform));
    }

    #[test]
    fn portable_needs_marker_or_data_folder() {
        let none = exe_dir(false, false);
        assert_eq!(portable_data_dir(none.path()), None);
        let marker = exe_dir(true, false);
        assert_eq!(
            portable_data_dir(marker.path()),
            Some(marker.path().join("data"))
        );
        let folder = exe_dir(false, true);
        assert_eq!(
            portable_data_dir(folder.path()),
            Some(folder.path().join("data"))
        );
        // a *folder* named like the marker is not a marker
        let fake = exe_dir(false, false);
        std::fs::create_dir(fake.path().join(PORTABLE_MARKER)).unwrap();
        assert_eq!(portable_data_dir(fake.path()), None);
        let c = resolve_data_dir_with(None, None, Some(none.path()), plat);
        assert_eq!(c.source, DataDirSource::Platform);
    }
}
