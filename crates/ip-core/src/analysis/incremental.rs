//! Incremental analysis steps: after a model is installed only the steps it enables are
//! computed for photos that were analysed without them.
//!
//! The decision is made from what the analysis rows store: `analysis.model_versions` holds the
//! model of every step that ran (`{step: model id}`), so a step of the profile that is missing
//! there is a candidate. It is only redone when a model for it is installed now. A step the
//! worker reported as skipped is remembered in `model_versions._skipped` with a fingerprint of
//! the installed models; it is not tried again until that set changes (a worker may skip a step
//! for reasons a download cannot fix, e.g. a missing runtime library).

use std::collections::{BTreeMap, BTreeSet};

use ip_worker_client::ModelsListing;
use rusqlite::Connection;
use serde_json::Value;

use super::types::Profile;
use crate::error::Result;

/// Steps that can be recomputed on their own: each writes independent columns of the analysis
/// row. (`faces` and `identity` belong together with the face rows and are not incremental.)
pub const INCREMENTAL_STEPS: &[&str] = &["embed", "aesthetic", "iqa", "scene"];
/// Key of `model_versions` that remembers skipped steps (`{step: fingerprint}`).
pub const SKIPPED_KEY: &str = "_skipped";

/// Identifies the set of installed models (FNV-1a over the sorted ids).
pub fn fingerprint(l: &ModelsListing) -> String {
    let ids: BTreeSet<&str> = l
        .models
        .iter()
        .filter(|m| m.installed)
        .map(|m| m.id.as_str())
        .collect();
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for id in ids {
        for b in id.bytes().chain(std::iter::once(b',')) {
            h ^= b as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    format!("{h:016x}")
}

/// Incremental steps of `profile` that an installed model enables right now, in profile order.
/// A model that the face detector needs too does not count: it is installed for every profile.
pub fn ready_steps(l: &ModelsListing, profile: Profile) -> Vec<String> {
    let Some(p) = l.profiles.get(profile.as_str()) else {
        return Vec::new();
    };
    let in_use = |m: &ip_worker_client::WorkerModel, step: &str| {
        m.installed
            && m.required_for.iter().any(|s| s == step)
            && !m.required_for.iter().any(|s| s == "faces")
    };
    p.steps
        .iter()
        .filter(|s| INCREMENTAL_STEPS.contains(&s.as_str()))
        .filter(|s| l.models.iter().any(|m| in_use(m, s)))
        .cloned()
        .collect()
}

/// Steps of `ready` that the stored `model_versions` of an analysis lacks and that were not
/// skipped under the current model set (`fp`).
pub fn missing_steps(model_versions: Option<&str>, ready: &[String], fp: &str) -> Vec<String> {
    let v: Value = model_versions
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(Value::Null);
    ready
        .iter()
        .filter(|s| v.get(s.as_str()).map(Value::is_null).unwrap_or(true))
        .filter(|s| {
            v.get(SKIPPED_KEY)
                .and_then(|k| k.get(s.as_str()))
                .and_then(Value::as_str)
                != Some(fp)
        })
        .cloned()
        .collect()
}

/// Whether any analysed photo of the session lacks one of the incremental steps in its stored
/// `model_versions` (a cheap check done before the worker is asked which models are installed).
pub fn any_candidates(c: &Connection, session_id: i64, profile: Profile) -> Result<bool> {
    let lacks: Vec<String> = INCREMENTAL_STEPS
        .iter()
        .map(|s| format!("json_extract(a.model_versions, '$.{s}') IS NULL"))
        .collect();
    let sql = format!(
        "SELECT EXISTS(SELECT 1 FROM analysis a
           JOIN photo p ON p.id=a.photo_id
           JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1
           WHERE a.profile=?2 AND COALESCE(p.analysis_version,0) >= ?3
             AND (a.model_versions IS NULL OR NOT json_valid(a.model_versions) OR {}))",
        lacks.join(" OR ")
    );
    Ok(c.query_row(
        &sql,
        rusqlite::params![session_id, profile.as_str(), profile.level()],
        |r| r.get(0),
    )?)
}

/// Analysed photos of the session (rows of `profile` that count as analysed at `level`) grouped
/// by the steps they lack: `(steps, photo ids)`, photos in capture order.
pub fn stale_groups(
    c: &Connection,
    session_id: i64,
    profile: Profile,
    ready: &[String],
    fp: &str,
) -> Result<Vec<(Vec<String>, Vec<i64>)>> {
    if ready.is_empty() {
        return Ok(Vec::new());
    }
    let mut st = c.prepare(
        "SELECT a.photo_id, a.model_versions FROM analysis a
         JOIN photo p ON p.id=a.photo_id
         JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?1
         WHERE a.profile=?2 AND COALESCE(p.analysis_version,0) >= ?3
         ORDER BY COALESCE(p.taken_at, p.mtime), p.id",
    )?;
    let rows = st
        .query_map(
            rusqlite::params![session_id, profile.as_str(), profile.level()],
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut groups: BTreeMap<Vec<String>, Vec<i64>> = BTreeMap::new();
    for (id, versions) in rows {
        let missing = missing_steps(versions.as_deref(), ready, fp);
        if !missing.is_empty() {
            groups.entry(missing).or_default().push(id);
        }
    }
    Ok(groups.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ip_worker_client::{ProfileInfo, WorkerModel};

    fn model(id: &str, installed: bool, req: &[&str]) -> WorkerModel {
        WorkerModel {
            id: id.into(),
            task: vec![],
            size_mb: 1.0,
            license: "MIT".into(),
            noncommercial: false,
            usage: "commercial-ok".into(),
            license_info: Default::default(),
            installed,
            recommended: true,
            optional: false,
            tiers: vec![],
            required_for: req.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn listing(models: Vec<WorkerModel>) -> ModelsListing {
        let mut profiles = BTreeMap::new();
        profiles.insert(
            "standard".to_string(),
            ProfileInfo {
                steps: ["phash", "faces", "embed", "aesthetic", "scene"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                models: vec![],
            },
        );
        ModelsListing {
            models,
            profiles,
            beauty_models: Value::Null,
            extra: Default::default(),
        }
    }

    #[test]
    fn ready_steps_need_an_installed_model_of_their_own() {
        let l = listing(vec![
            model("yunet", true, &["faces", "identity"]),
            model("siglip2-base", true, &["embed", "scene"]),
            model("nima-aesthetic", false, &["aesthetic"]),
        ]);
        assert_eq!(ready_steps(&l, Profile::Standard), vec!["embed", "scene"]);
        // a profile the worker does not list has no incremental steps
        assert!(ready_steps(&l, Profile::Fast).is_empty());
        let l2 = listing(vec![
            model("siglip2-base", true, &["embed", "scene"]),
            model("nima-aesthetic", true, &["aesthetic"]),
        ]);
        assert_eq!(
            ready_steps(&l2, Profile::Standard),
            vec!["embed", "aesthetic", "scene"]
        );
        assert_ne!(fingerprint(&l), fingerprint(&l2));
        assert_eq!(fingerprint(&l), fingerprint(&listing(l.models.clone())));
    }

    #[test]
    fn missing_steps_follow_the_stored_model_versions() {
        let ready: Vec<String> = ["embed", "aesthetic", "scene"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let v = r#"{"embed":"siglip","faces":"yunet"}"#;
        assert_eq!(
            missing_steps(Some(v), &ready, "fp1"),
            vec!["aesthetic", "scene"]
        );
        assert_eq!(missing_steps(None, &ready, "fp1"), ready);
        // skipped under the same model set: not tried again; a new set tries again
        let skipped = r#"{"embed":"siglip","_skipped":{"aesthetic":"fp1"}}"#;
        assert_eq!(missing_steps(Some(skipped), &ready, "fp1"), vec!["scene"]);
        assert_eq!(
            missing_steps(Some(skipped), &ready, "fp2"),
            vec!["aesthetic", "scene"]
        );
        let all = r#"{"embed":"a","aesthetic":"b","scene":"c"}"#;
        assert!(missing_steps(Some(all), &ready, "fp1").is_empty());
    }
}
