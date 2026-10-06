//! Persisted security settings: LAN mode, password hashes (Argon2id) and filesystem roots.
//!
//! Stored in `<data_dir>/security.json`. The file is the single source of truth: every
//! [`SecurityStore`] instance (server, CLI, the settings API) re-reads it when it changes on disk,
//! so a password change made by one component is seen by all of them. Writes are atomic
//! (temp file + rename).
//!
//! Password hashes never leave this module except through [`SecurityStore::snapshot`], which the
//! settings API must not serialise (use [`SecuritySnapshot::public_lan`] / `roots`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, Result};

pub const FILE_NAME: &str = "security.json";
pub const DEFAULT_PORT: u16 = 7878;
pub const MIN_PASSWORD_LEN: usize = 8;
pub const MAX_PASSWORD_LEN: usize = 256;

/// `settings.lan` of the API contract (the password is set via `POST /api/auth/password`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanSettings {
    pub enabled: bool,
    pub port: u16,
    pub guest_enabled: bool,
}

impl Default for LanSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            port: DEFAULT_PORT,
            guest_enabled: false,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SecuritySnapshot {
    pub lan: LanSettings,
    /// PHC string (`$argon2id$...`).
    pub owner_hash: Option<String>,
    pub guest_hash: Option<String>,
    /// Bumped on every password change; login sessions of an older epoch are rejected.
    pub auth_epoch: u64,
    /// Additional allowed filesystem roots (`settings.roots`).
    pub roots: Vec<String>,
}

impl SecuritySnapshot {
    pub fn has_owner_password(&self) -> bool {
        self.owner_hash.is_some()
    }
    pub fn has_guest_password(&self) -> bool {
        self.guest_hash.is_some()
    }
}

struct Inner {
    path: PathBuf,
    cache: Mutex<Cache>,
}

struct Cache {
    snap: SecuritySnapshot,
    stamp: Option<(SystemTime, u64)>,
}

/// Handle on `<data_dir>/security.json`. Cheap to clone.
#[derive(Clone)]
pub struct SecurityStore {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for SecurityStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecurityStore")
            .field("path", &self.inner.path)
            .finish_non_exhaustive()
    }
}

fn stamp_of(path: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len()))
}

impl SecurityStore {
    /// Opens (or lazily creates) the store in `data_dir`. A corrupt file is treated as empty
    /// defaults (LAN off, no passwords) rather than failing the app.
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join(FILE_NAME);
        let store = Self {
            inner: Arc::new(Inner {
                path,
                cache: Mutex::new(Cache {
                    snap: SecuritySnapshot::default(),
                    stamp: None,
                }),
            }),
        };
        store.refresh();
        store
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.inner.cache.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn refresh(&self) {
        let mut c = self.lock();
        self.refresh_locked(&mut c);
    }

    fn refresh_locked(&self, c: &mut Cache) {
        let stamp = stamp_of(&self.inner.path);
        if stamp == c.stamp {
            return;
        }
        c.snap = match std::fs::read(&self.inner.path) {
            Ok(b) => serde_json::from_slice(&b).unwrap_or_else(|e| {
                tracing::warn!(error = %e, "security.json is corrupt; using defaults");
                SecuritySnapshot::default()
            }),
            Err(_) => SecuritySnapshot::default(),
        };
        c.stamp = stamp;
    }

    /// Current settings (re-read from disk if the file changed).
    pub fn snapshot(&self) -> SecuritySnapshot {
        let mut c = self.lock();
        self.refresh_locked(&mut c);
        c.snap.clone()
    }

    fn update<R>(&self, f: impl FnOnce(&mut SecuritySnapshot) -> Result<R>) -> Result<R> {
        let mut c = self.lock();
        self.refresh_locked(&mut c);
        let mut next = c.snap.clone();
        let r = f(&mut next)?;
        let bytes = serde_json::to_vec_pretty(&next)
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("serialise security.json: {e}")))?;
        let tmp = self.inner.path.with_extension("json.tmp");
        if let Some(dir) = self.inner.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        std::fs::write(&tmp, &bytes)
            .and_then(|_| std::fs::rename(&tmp, &self.inner.path))
            .map_err(|e| CoreError::Internal(anyhow::anyhow!("write security.json: {e}")))?;
        c.snap = next;
        c.stamp = stamp_of(&self.inner.path);
        Ok(r)
    }

    // ------------------------------------------------------------------ LAN

    pub fn lan(&self) -> LanSettings {
        self.snapshot().lan
    }

    pub fn set_lan(&self, lan: LanSettings) -> Result<()> {
        if lan.port == 0 {
            return Err(CoreError::bad_request("lan.port must be 1..65535"));
        }
        if lan.enabled && !self.has_owner_password() {
            return Err(CoreError::bad_request(
                "set a password (POST /api/auth/password) before enabling LAN access",
            ));
        }
        self.update(|s| {
            s.lan = lan;
            Ok(())
        })
    }

    // ---------------------------------------------------------------- roots

    pub fn roots(&self) -> Vec<String> {
        self.snapshot().roots
    }

    /// Replaces the configured roots (duplicates and blanks are dropped).
    pub fn set_roots(&self, roots: Vec<String>) -> Result<()> {
        let mut seen = std::collections::HashSet::new();
        let roots: Vec<String> = roots
            .into_iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty() && seen.insert(r.clone()))
            .collect();
        self.update(|s| {
            s.roots = roots;
            Ok(())
        })
    }

    pub fn add_root(&self, root: &Path) -> Result<()> {
        let r = root.to_string_lossy().into_owned();
        self.update(|s| {
            if !s.roots.contains(&r) {
                s.roots.push(r);
            }
            Ok(())
        })
    }

    // ------------------------------------------------------------ passwords

    pub fn has_owner_password(&self) -> bool {
        self.snapshot().has_owner_password()
    }

    pub fn auth_epoch(&self) -> u64 {
        self.snapshot().auth_epoch
    }

    /// Sets the owner password and, if `guest` is `Some`, the guest password (`Some("")`
    /// removes it; `None` keeps it). Bumps the auth epoch so every login session is invalidated.
    pub fn set_passwords(&self, owner: &str, guest: Option<&str>) -> Result<()> {
        validate_password(owner)?;
        if let Some(g) = guest.filter(|g| !g.is_empty()) {
            validate_password(g)?;
            if g == owner {
                return Err(CoreError::bad_request(
                    "the guest password must differ from the owner password",
                ));
            }
        }
        let owner_hash = hash_password(owner)?;
        let guest_hash = match guest {
            Some("") => Some(None),
            Some(g) => Some(Some(hash_password(g)?)),
            None => None,
        };
        self.update(|s| {
            s.owner_hash = Some(owner_hash);
            if let Some(g) = guest_hash {
                s.guest_hash = g;
            }
            s.auth_epoch += 1;
            Ok(())
        })
    }

    pub fn verify_owner(&self, password: &str) -> bool {
        let snap = self.snapshot();
        verify_password(password, snap.owner_hash.as_deref())
    }

    /// Always `false` unless `lan.guest_enabled` and a guest password is set.
    pub fn verify_guest(&self, password: &str) -> bool {
        let snap = self.snapshot();
        snap.lan.guest_enabled && verify_password(password, snap.guest_hash.as_deref())
    }
}

fn validate_password(p: &str) -> Result<()> {
    let n = p.chars().count();
    if n < MIN_PASSWORD_LEN {
        return Err(CoreError::bad_request(format!(
            "password must have at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if p.len() > MAX_PASSWORD_LEN {
        return Err(CoreError::bad_request("password is too long"));
    }
    Ok(())
}

/// Argon2id hash as a PHC string (random salt).
pub fn hash_password(password: &str) -> Result<String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| CoreError::Internal(anyhow::anyhow!("password hashing failed: {e}")))
}

/// Verifies against a PHC string; the comparison inside Argon2 is constant time. With no stored
/// hash a dummy verification still runs so timing does not reveal whether a password is set.
pub fn verify_password(password: &str, hash: Option<&str>) -> bool {
    match hash {
        Some(h) => Argon2::default()
            .verify_password(password.as_bytes(), h)
            .is_ok(),
        None => {
            let _ = hash_password(password);
            false
        }
    }
}

/// Constant-time equality of two secrets (compares fixed-length BLAKE3 digests).
pub fn ct_eq(a: &str, b: &str) -> bool {
    blake3::hash(a.as_bytes()) == blake3::hash(b.as_bytes())
}

/// 256 random bits as 64 hex chars (session ids, tokens).
pub fn random_token() -> String {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf).expect("os rng");
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip_and_epoch() {
        let d = tempfile::tempdir().unwrap();
        let s = SecurityStore::open(d.path());
        assert!(!s.has_owner_password());
        assert!(!s.verify_owner("whatever1"));
        s.set_passwords("correct horse", Some("guest pass 1"))
            .unwrap();
        assert_eq!(s.auth_epoch(), 1);
        assert!(s.verify_owner("correct horse"));
        assert!(!s.verify_owner("wrong horse!"));
        assert!(!s.verify_guest("guest pass 1"), "guest disabled by default");
        s.set_lan(LanSettings {
            enabled: true,
            port: 9000,
            guest_enabled: true,
        })
        .unwrap();
        assert!(s.verify_guest("guest pass 1"));
        // second instance sees the same file
        let s2 = SecurityStore::open(d.path());
        assert_eq!(s2.lan().port, 9000);
        s.set_passwords("another secret", None).unwrap();
        assert_eq!(s2.auth_epoch(), 2);
        assert!(s2.verify_guest("guest pass 1"), "guest kept when None");
        s.set_passwords("another secret", Some("")).unwrap();
        assert!(!s2.verify_guest("guest pass 1"));
    }

    #[test]
    fn rejects_weak_and_lan_without_password() {
        let d = tempfile::tempdir().unwrap();
        let s = SecurityStore::open(d.path());
        assert!(s.set_passwords("short", None).is_err());
        assert!(s
            .set_passwords("long enough pw", Some("long enough pw"))
            .is_err());
        let lan = LanSettings {
            enabled: true,
            ..Default::default()
        };
        assert!(s.set_lan(lan).is_err());
    }

    #[test]
    fn roots_are_deduped_and_persisted() {
        let d = tempfile::tempdir().unwrap();
        let s = SecurityStore::open(d.path());
        s.set_roots(vec!["/a".into(), " /a ".into(), "".into(), "/b".into()])
            .unwrap();
        assert_eq!(SecurityStore::open(d.path()).roots(), vec!["/a", "/b"]);
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join(FILE_NAME), b"{not json").unwrap();
        let s = SecurityStore::open(d.path());
        assert!(!s.has_owner_password());
        assert_eq!(s.lan(), LanSettings::default());
    }

    #[test]
    fn tokens_and_ct_eq() {
        let a = random_token();
        assert_eq!(a.len(), 64);
        assert_ne!(a, random_token());
        assert!(ct_eq(&a, &a.clone()));
        assert!(!ct_eq(&a, "x"));
    }
}
