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
    /// The user read the purpose explanation and agreed to face detection / recognition
    /// (docs/02 §8: face features are sensitive personal data). Until then no face is
    /// detected and no identity embedding is computed, whatever `enabled` says.
    pub consented: bool,
}

impl Default for FacesSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            consented: false,
        }
    }
}

impl FacesSettings {
    /// Faces may be detected and recognised: switched on and agreed to.
    pub fn allowed(&self) -> bool {
        self.enabled && self.consented
    }
}

/// `xmp_mode` that also writes the XMP packet embedded in original JPEGs. The only mode in
/// which the app changes an original's bytes; switching to it needs
/// [`CONFIRM_MODIFY_ORIGINALS`] in the same patch.
pub const XMP_MODIFY_ORIGINALS: &str = "modify_originals";
/// Settings patch key that confirms switching `xmp_mode` to [`XMP_MODIFY_ORIGINALS`] (the UI
/// sends it after its second confirmation). Never stored.
pub const CONFIRM_MODIFY_ORIGINALS: &str = "confirm_modify_originals";

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

/// `settings.remote_ai` (docs/api-contract-m8.md section C): use a home PC's AI worker. The
/// device token is never part of the settings; it lives in `<data dir>/remote.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RemoteAiSettings {
    pub enabled: bool,
    /// Normalised `scheme://host[:port]` of the host, or empty.
    pub host_url: String,
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
    /// `off` | `sidecar` | `modify_originals` (sidecar + the JPEG's embedded XMP; rewrites
    /// originals, see [`XMP_MODIFY_ORIGINALS`])
    pub xmp_mode: String,
    pub assistant: AssistantSettings,
    pub remote_ai: RemoteAiSettings,
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
            assistant: AssistantSettings::default(),
            remote_ai: RemoteAiSettings::default(),
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
            &["off", "sidecar", XMP_MODIFY_ORIGINALS],
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
        if !self.remote_ai.host_url.is_empty() {
            let norm = ip_worker_client::normalize_host_url(&self.remote_ai.host_url)
                .map_err(|e| CoreError::Unprocessable(format!("remote_ai.host_url: {e}")))?;
            if norm != self.remote_ai.host_url {
                return Err(CoreError::Unprocessable(
                    "remote_ai.host_url must look like http://host:port".into(),
                ));
            }
        }
        if self.models.dir.contains('\0') || self.models.dir.len() > 1024 {
            return Err(CoreError::Unprocessable(
                "models.dir is not a valid path".into(),
            ));
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

/// Rewrites values of older builds in a stored settings object. Returns whether anything changed.
///
/// `xmp_mode = "sidecar_and_embedded"` was described as "also read the embedded XMP" but
/// rewrote original JPEGs; nobody agreed to that, so it becomes `sidecar` (the explicit
/// `modify_originals` mode needs a confirmation).
fn upgrade_legacy(v: &mut Value) -> bool {
    if v.get("xmp_mode").and_then(Value::as_str) == Some("sidecar_and_embedded") {
        v["xmp_mode"] = Value::from("sidecar");
        return true;
    }
    false
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
        let mut upgraded = false;
        let mut s = match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str::<Value>(&text)
                .and_then(|mut v| {
                    upgraded = upgrade_legacy(&mut v);
                    serde_json::from_value::<Settings>(v)
                })
                .map_err(|e| e.to_string())
                .and_then(|s| s.validate().map(|_| s).map_err(|e| e.to_string()))
            {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(error = %e, "settings file is invalid; using defaults");
                    let _ = std::fs::rename(&path, root.join("settings.json.bad"));
                    upgraded = false;
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        };
        if s.models.dir.trim().is_empty() {
            s.models.dir = default_models_dir.clone();
        }
        let store = Self {
            path,
            default_models_dir,
            cur: RwLock::new(Arc::new(s)),
        };
        if upgraded {
            tracing::warn!(
                "xmp_mode \"sidecar_and_embedded\" (rewrote original JPEGs) is now \"sidecar\"; \
                 \"modify_originals\" has to be chosen and confirmed again"
            );
            if let Err(e) = store.save(&store.get()) {
                tracing::warn!(error = %e, "cannot store the upgraded settings");
            }
        }
        store
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
    ///
    /// Switching `xmp_mode` to `modify_originals` (the only mode that rewrites original files)
    /// needs `"confirm_modify_originals": true` in the same patch.
    pub fn patch(&self, patch: &Value) -> Result<(Arc<Settings>, Arc<Settings>)> {
        let Value::Object(fields) = patch else {
            return Err(CoreError::bad_request("the body must be a JSON object"));
        };
        let mut fields = fields.clone();
        let confirmed = match fields.remove(CONFIRM_MODIFY_ORIGINALS) {
            None => false,
            Some(Value::Bool(b)) => b,
            Some(_) => {
                return Err(CoreError::bad_request(format!(
                    "{CONFIRM_MODIFY_ORIGINALS} must be true or false"
                )))
            }
        };
        let patch = Value::Object(fields);
        let old = self.get();
        let mut v = serde_json::to_value(&*old)?;
        merge(&mut v, &patch);
        let mut new: Settings = serde_json::from_value(v)
            .map_err(|e| CoreError::bad_request(format!("invalid settings: {e}")))?;
        if new.models.dir.trim().is_empty() {
            new.models.dir = self.default_models_dir.clone();
        }
        new.validate()?;
        if new.xmp_mode == XMP_MODIFY_ORIGINALS
            && old.xmp_mode != XMP_MODIFY_ORIGINALS
            && !confirmed
        {
            return Err(CoreError::Unprocessable(format!(
                "xmp_mode \"{XMP_MODIFY_ORIGINALS}\" rewrites the XMP inside original JPEG files; \
                 send \"{CONFIRM_MODIFY_ORIGINALS}\": true with the change to confirm it"
            )));
        }
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

    /// Broadcasts `settings.updated` (the server adds the security-store parts, `lan` / `roots`).
    pub fn emit_settings_updated(&self, settings: Value) {
        self.events.emit(Event::SettingsUpdated { settings });
    }

    /// `PATCH /api/settings` (without `lan` / `roots`, which live in the security store):
    /// merge, validate, persist, apply live effects.
    pub async fn patch_settings(self: &Arc<Self>, patch: Value) -> Result<Arc<Settings>> {
        let (old, new) = self.settings.patch(&patch)?;
        self.apply_settings(Some(&old), &new);
        Ok(new)
    }

    /// Effects of settings on running parts. `old == None` at start-up.
    pub(crate) fn apply_settings(self: &Arc<Self>, old: Option<&Settings>, new: &Settings) {
        self.remote.refresh(new);
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
        assert_eq!(
            v["faces"]["consented"], false,
            "faces need an explicit consent"
        );
        assert!(!s.get().faces.allowed());
        assert_eq!(v["privacy"]["allow_network"], true);
        assert_eq!(v["cache"]["max_gb"], 20.0);
        assert_eq!(v["xmp_mode"], "off");
        assert_eq!(v["assistant"]["engine"], "auto");
        assert_eq!(v["models"]["source"], "auto");
        assert!(v["models"]["dir"].as_str().unwrap().ends_with("models"));
    }

    #[test]
    fn patch_merges_validates_and_persists() {
        let (d, s) = store();
        let (_, new) = s
            .patch(&json!({"theme":"dark","faces":{"enabled":false}}))
            .unwrap();
        assert_eq!(new.theme, "dark");
        assert!(!new.faces.enabled);
        assert!(new.privacy.allow_network, "untouched sections survive");
        for bad in [
            json!({"theme":"purple"}),
            json!({"cache":{"max_gb":0}}),
            json!({"cache":{"max_gb":-3}}),
            json!({"xmp_mode":"both"}),
            json!({"render":{"backend":"tpu"}}),
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
    fn modify_originals_needs_a_confirmation() {
        let (_d, s) = store();
        let e = s
            .patch(&json!({"xmp_mode": XMP_MODIFY_ORIGINALS}))
            .unwrap_err();
        assert!(matches!(e, CoreError::Unprocessable(_)), "{e:?}");
        assert_eq!(s.get().xmp_mode, "off");
        assert!(matches!(
            s.patch(&json!({"xmp_mode": XMP_MODIFY_ORIGINALS, CONFIRM_MODIFY_ORIGINALS: "yes"}))
                .unwrap_err(),
            CoreError::BadRequest(_)
        ));
        let (_, new) = s
            .patch(&json!({"xmp_mode": XMP_MODIFY_ORIGINALS, CONFIRM_MODIFY_ORIGINALS: true}))
            .unwrap();
        assert_eq!(new.xmp_mode, XMP_MODIFY_ORIGINALS);
        // already on: other changes need no new confirmation; the flag is never stored
        s.patch(&json!({"theme": "light"})).unwrap();
        let text = std::fs::read_to_string(&s.path).unwrap();
        assert!(!text.contains(CONFIRM_MODIFY_ORIGINALS), "{text}");
        // the old mode name is gone
        assert!(s
            .patch(&json!({"xmp_mode": "sidecar_and_embedded"}))
            .is_err());
        s.patch(&json!({"xmp_mode": "sidecar"})).unwrap();
        assert!(s.patch(&json!({"xmp_mode": XMP_MODIFY_ORIGINALS})).is_err());
    }

    #[test]
    fn legacy_embedded_mode_becomes_sidecar() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join(SETTINGS_FILE),
            r#"{"theme":"dark","xmp_mode":"sidecar_and_embedded"}"#,
        )
        .unwrap();
        let s = SettingsStore::load(d.path());
        assert_eq!(s.get().xmp_mode, "sidecar");
        assert_eq!(s.get().theme, "dark", "the rest of the file is kept");
        assert!(!d.path().join("settings.json.bad").exists());
        let stored = std::fs::read_to_string(d.path().join(SETTINGS_FILE)).unwrap();
        assert!(!stored.contains("sidecar_and_embedded"), "{stored}");
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
