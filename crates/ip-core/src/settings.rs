//! Persisted settings (`docs/api-contract-m6.md` section D): defaults, validation, atomic
//! persistence in the data directory and the live effects of a change.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use ip_worker_client::WorkerOptions;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::analysis::grouping::GroupParams;
use crate::error::{CoreError, Result};
use crate::events::Event;
use crate::Core;

pub const SETTINGS_FILE: &str = "settings.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AnalysisSettings {
    /// `fast` | `standard`
    pub default_profile: String,
    pub auto_analyze_on_import: bool,
    /// `loose` | `normal` | `strict`
    pub group_strictness: String,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            default_profile: "standard".into(),
            auto_analyze_on_import: false,
            group_strictness: "normal".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FacesSettings {
    pub enabled: bool,
}

impl Default for FacesSettings {
    fn default() -> Self {
        Self { enabled: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PrivacySettings {
    pub allow_network: bool,
}

impl Default for PrivacySettings {
    fn default() -> Self {
        Self {
            allow_network: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ModelsSettings {
    /// Empty = `<data dir>/models` (filled in on load).
    pub dir: String,
    /// `auto` | `hf` | `hf-mirror` | `modelscope`
    pub source: String,
}

impl Default for ModelsSettings {
    fn default() -> Self {
        Self {
            dir: String::new(),
            source: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CacheSettings {
    pub max_gb: f64,
}

impl Default for CacheSettings {
    fn default() -> Self {
        Self { max_gb: 20.0 }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RenderSettings {
    /// `auto` | `gpu` | `cpu`
    pub backend: String,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            backend: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LanSettings {
    pub enabled: bool,
    pub port: u16,
    pub guest_enabled: bool,
}

impl Default for LanSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 7878,
            guest_enabled: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AssistantSettings {
    /// `auto` | `rules` | `llm`
    pub engine: String,
}

impl Default for AssistantSettings {
    fn default() -> Self {
        Self {
            engine: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub language: String,
    /// `dark` | `light` | `system`
    pub theme: String,
    pub analysis: AnalysisSettings,
    pub faces: FacesSettings,
    pub privacy: PrivacySettings,
    pub models: ModelsSettings,
    pub cache: CacheSettings,
    pub render: RenderSettings,
    /// `off` | `sidecar` | `sidecar_and_embedded`
    pub xmp_mode: String,
    pub lan: LanSettings,
    pub roots: Vec<String>,
    pub assistant: AssistantSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            language: "zh-CN".into(),
            theme: "system".into(),
            analysis: AnalysisSettings::default(),
            faces: FacesSettings::default(),
            privacy: PrivacySettings::default(),
            models: ModelsSettings::default(),
            cache: CacheSettings::default(),
            render: RenderSettings::default(),
            xmp_mode: "off".into(),
            lan: LanSettings::default(),
            roots: Vec::new(),
            assistant: AssistantSettings::default(),
        }
    }
}

fn one_of(name: &str, v: &str, allowed: &[&str]) -> Result<()> {
    if allowed.contains(&v) {
        Ok(())
    } else {
        Err(CoreError::Unprocessable(format!(
            "{name} must be one of {}",
            allowed.join("|")
        )))
    }
}

impl Settings {
    pub fn validate(&self) -> Result<()> {
        one_of("language", &self.language, &["zh-CN", "en"])?;
        one_of("theme", &self.theme, &["dark", "light", "system"])?;
        one_of(
            "analysis.default_profile",
            &self.analysis.default_profile,
            &["fast", "standard"],
        )?;
        one_of(
            "analysis.group_strictness",
            &self.analysis.group_strictness,
            &["loose", "normal", "strict"],
        )?;
        one_of(
            "models.source",
            &self.models.source,
            &["auto", "hf", "hf-mirror", "modelscope"],
        )?;
        one_of(
            "render.backend",
            &self.render.backend,
            &["auto", "gpu", "cpu"],
        )?;
        one_of(
            "xmp_mode",
            &self.xmp_mode,
            &["off", "sidecar", "sidecar_and_embedded"],
        )?;
        one_of(
            "assistant.engine",
            &self.assistant.engine,
            &["auto", "rules", "llm"],
        )?;
        if !self.cache.max_gb.is_finite() || !(0.1..=100_000.0).contains(&self.cache.max_gb) {
            return Err(CoreError::Unprocessable(
                "cache.max_gb must be within 0.1..100000".into(),
            ));
        }
        if self.lan.port == 0 {
            return Err(CoreError::Unprocessable(
                "lan.port must be within 1..65535".into(),
            ));
        }
        if self.models.dir.contains('\0') || self.models.dir.len() > 1024 {
            return Err(CoreError::Unprocessable(
                "models.dir is not a valid path".into(),
            ));
        }
        if self.roots.len() > 256 {
            return Err(CoreError::Unprocessable("at most 256 roots".into()));
        }
        for r in &self.roots {
            if r.trim().is_empty() || r.contains('\0') || r.len() > 1024 {
                return Err(CoreError::Unprocessable(
                    "roots must be non-empty paths".into(),
                ));
            }
        }
        Ok(())
    }

    /// Group parameters of `analysis.group_strictness`.
    pub fn group_params(&self) -> GroupParams {
        GroupParams::for_strictness(&self.analysis.group_strictness)
    }

    /// Process options of the AI worker implied by these settings (applied on its next start).
    pub fn worker_options(&self) -> WorkerOptions {
        let mut env: Vec<(String, String)> = Vec::new();
        match self.models.source.as_str() {
            "hf" => env.push(("HF_ENDPOINT".into(), "https://huggingface.co".into())),
            "hf-mirror" => env.push(("HF_ENDPOINT".into(), "https://hf-mirror.com".into())),
            _ => {}
        }
        env.push((
            "IMAGEPICKER_MODEL_SOURCE".into(),
            self.models.source.clone(),
        ));
        if !self.privacy.allow_network {
            env.push(("HF_HUB_OFFLINE".into(), "1".into()));
            env.push(("IMAGEPICKER_OFFLINE".into(), "1".into()));
        }
        WorkerOptions {
            models_dir: (!self.models.dir.trim().is_empty())
                .then(|| PathBuf::from(&self.models.dir)),
            env,
        }
    }
}

impl GroupParams {
    /// `loose` groups more photos into one burst, `strict` fewer.
    pub fn for_strictness(s: &str) -> Self {
        let base = GroupParams::default();
        match s {
            "loose" => GroupParams {
                theta_burst: 0.80,
                theta_strict: 0.88,
                theta_drift: 0.68,
                ..base
            },
            "strict" => GroupParams {
                theta_burst: 0.90,
                theta_strict: 0.95,
                theta_drift: 0.82,
                ..base
            },
            _ => base,
        }
    }
}

/// RFC 7396 merge patch of settings objects: `null` resets a key to its default.
fn merge(cur: &mut Value, patch: &Value) {
    match (cur, patch) {
        (Value::Object(c), Value::Object(p)) => {
            for (k, v) in p {
                if v.is_null() {
                    c.remove(k);
                } else {
                    merge(c.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        (c, p) => *c = p.clone(),
    }
}

pub struct SettingsStore {
    path: PathBuf,
    default_models_dir: String,
    cur: RwLock<Arc<Settings>>,
}

impl SettingsStore {
    /// Loads `<root>/settings.json`; a missing file gives the defaults, a corrupt one is moved
    /// aside (`settings.json.bad`) and replaced by the defaults.
    pub fn load(root: &Path) -> Self {
        let path = root.join(SETTINGS_FILE);
        let default_models_dir = std::env::var_os("IMAGEPICKER_MODELS_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join("models"))
            .to_string_lossy()
            .into_owned();
        let mut s = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Settings>(&text)
                .map_err(|e| e.to_string())
                .and_then(|s| s.validate().map(|_| s).map_err(|e| e.to_string()))
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "settings file is invalid; using defaults");
                    let _ = std::fs::rename(&path, root.join("settings.json.bad"));
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        };
        if s.models.dir.trim().is_empty() {
            s.models.dir = default_models_dir.clone();
        }
        Self {
            path,
            default_models_dir,
            cur: RwLock::new(Arc::new(s)),
        }
    }

    pub fn get(&self) -> Arc<Settings> {
        self.cur.read().unwrap().clone()
    }

    pub fn default_models_dir(&self) -> &str {
        &self.default_models_dir
    }

    fn save(&self, s: &Settings) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }

    /// Validates `patch` against the current settings and stores the result.
    /// Returns `(old, new)`.
    pub fn patch(&self, patch: &Value) -> Result<(Arc<Settings>, Arc<Settings>)> {
        let Value::Object(_) = patch else {
            return Err(CoreError::bad_request("the body must be a JSON object"));
        };
        let old = self.get();
        let mut v = serde_json::to_value(&*old)?;
        merge(&mut v, patch);
        let mut new: Settings = serde_json::from_value(v)
            .map_err(|e| CoreError::bad_request(format!("invalid settings: {e}")))?;
        if new.models.dir.trim().is_empty() {
            new.models.dir = self.default_models_dir.clone();
        }
        new.validate()?;
        self.save(&new)?;
        let new = Arc::new(new);
        *self.cur.write().unwrap() = new.clone();
        Ok((old, new))
    }
}

impl Core {
    pub fn settings(&self) -> Arc<Settings> {
        self.settings.get()
    }

    /// `PATCH /api/settings`: merge, validate, persist, apply live effects, broadcast.
    pub async fn patch_settings(self: &Arc<Self>, patch: Value) -> Result<Arc<Settings>> {
        let (old, new) = self.settings.patch(&patch)?;
        self.apply_settings(Some(&old), &new);
        self.events.emit(Event::SettingsUpdated {
            settings: serde_json::to_value(&*new)?,
        });
        Ok(new)
    }

    /// Effects of settings on running parts. `old == None` at start-up.
    pub(crate) fn apply_settings(self: &Arc<Self>, old: Option<&Settings>, new: &Settings) {
        let opts = new.worker_options();
        if old.map(|o| o.worker_options()).as_ref() != Some(&opts) {
            // takes effect when the worker process starts next
            self.worker.configure(&opts);
        }
        let render_changed = match old {
            Some(o) => o.render.backend != new.render.backend,
            // at start-up "auto" keeps whatever the core was opened with
            None => new.render.backend != "auto",
        };
        if render_changed {
            self.apply_render_backend(&new.render.backend);
        }
        if let Some(o) = old {
            if o.cache.max_gb != new.cache.max_gb {
                self.spawn_cache_eviction();
            }
            if o.xmp_mode != new.xmp_mode {
                self.xmp_mode_changed(&new.xmp_mode);
            }
        }
    }

    fn apply_render_backend(&self, backend: &str) {
        use ip_render::Backend;
        let current = self.render.backend();
        let env_cpu = std::env::var("IMAGEPICKER_RENDER")
            .map(|v| v.trim().eq_ignore_ascii_case("cpu"))
            .unwrap_or(false);
        let want_gpu = match backend {
            "cpu" => false,
            "gpu" => true,
            _ => !env_cpu,
        };
        if (!want_gpu && current == Backend::Cpu) || (want_gpu && current == Backend::Gpu) {
            return;
        }
        let r: Arc<dyn ip_render::Renderer> = Arc::from(ip_render::create_renderer(want_gpu));
        tracing::info!(backend = ?r.backend(), "render backend switched");
        self.render.set_renderer(r);
    }

    /// Refuses network use when `privacy.allow_network` is off.
    pub(crate) fn require_network(&self, what: &str) -> Result<()> {
        if self.settings().privacy.allow_network {
            Ok(())
        } else {
            Err(CoreError::Conflict(format!(
                "network access is disabled (settings: privacy.allow_network = false); {what} needs the network"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, SettingsStore) {
        let d = tempfile::tempdir().unwrap();
        let s = SettingsStore::load(d.path());
        (d, s)
    }

    #[test]
    fn defaults_match_the_contract() {
        let (_d, s) = store();
        let v = serde_json::to_value(&*s.get()).unwrap();
        assert_eq!(v["language"], "zh-CN");
        assert_eq!(v["faces"]["enabled"], true);
        assert_eq!(v["privacy"]["allow_network"], true);
        assert_eq!(v["cache"]["max_gb"], 20.0);
        assert_eq!(v["xmp_mode"], "off");
        assert_eq!(v["lan"]["port"], 7878);
        assert_eq!(v["assistant"]["engine"], "auto");
        assert_eq!(v["models"]["source"], "auto");
        assert!(v["models"]["dir"].as_str().unwrap().ends_with("models"));
    }

    #[test]
    fn patch_merges_validates_and_persists() {
        let (d, s) = store();
        let (_, new) = s
            .patch(&json!({"theme":"dark","faces":{"enabled":false},"roots":["C:/a"]}))
            .unwrap();
        assert_eq!(new.theme, "dark");
        assert!(!new.faces.enabled);
        assert!(new.privacy.allow_network, "untouched sections survive");
        for bad in [
            json!({"theme":"purple"}),
            json!({"cache":{"max_gb":0}}),
            json!({"cache":{"max_gb":-3}}),
            json!({"lan":{"port":0}}),
            json!({"lan":{"port":70000}}),
            json!({"xmp_mode":"both"}),
            json!({"render":{"backend":"tpu"}}),
            json!({"roots":[""]}),
        ] {
            let e = s.patch(&bad).unwrap_err();
            assert!(
                matches!(e, CoreError::Unprocessable(_) | CoreError::BadRequest(_)),
                "{bad}: {e:?}"
            );
        }
        assert!(matches!(
            s.patch(&json!({"nope":1})).unwrap_err(),
            CoreError::BadRequest(_)
        ));
        assert!(matches!(
            s.patch(&json!({"faces":{"enabled":"yes"}})).unwrap_err(),
            CoreError::BadRequest(_)
        ));
        assert!(matches!(
            s.patch(&json!([1])).unwrap_err(),
            CoreError::BadRequest(_)
        ));
        assert_eq!(s.get().theme, "dark", "failed patches change nothing");
        let again = SettingsStore::load(d.path());
        assert_eq!(again.get().theme, "dark");
        assert_eq!(again.get().roots, vec!["C:/a".to_string()]);
        s.patch(&json!({"theme": null})).unwrap();
        assert_eq!(s.get().theme, "system");
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(SETTINGS_FILE), "{ not json").unwrap();
        let s = SettingsStore::load(d.path());
        assert_eq!(s.get().language, "zh-CN");
        assert!(d.path().join("settings.json.bad").exists());
    }

    #[test]
    fn worker_options_follow_source_and_privacy() {
        let mut s = Settings::default();
        s.models.source = "hf-mirror".into();
        let o = s.worker_options();
        assert!(o
            .env
            .contains(&("HF_ENDPOINT".into(), "https://hf-mirror.com".into())));
        assert!(!o.env.iter().any(|(k, _)| k == "HF_HUB_OFFLINE"));
        s.privacy.allow_network = false;
        assert!(s
            .worker_options()
            .env
            .contains(&("HF_HUB_OFFLINE".into(), "1".into())));
        s.models.source = "modelscope".into();
        let o = s.worker_options();
        assert!(!o.env.iter().any(|(k, _)| k == "HF_ENDPOINT"));
        assert!(o
            .env
            .contains(&("IMAGEPICKER_MODEL_SOURCE".into(), "modelscope".into())));
    }

    #[test]
    fn strictness_orders_thresholds() {
        let (l, n, s) = (
            GroupParams::for_strictness("loose"),
            GroupParams::for_strictness("normal"),
            GroupParams::for_strictness("strict"),
        );
        assert!(l.theta_burst < n.theta_burst && n.theta_burst < s.theta_burst);
    }
}
