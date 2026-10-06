//! Wire types of docs/api-contract-m8.md section A. Field names are part of the contract.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PermissionState {
    /// Some media access is available (full, or partial on Android 14).
    pub granted: bool,
    /// Android 14 "Selected photos" access.
    #[serde(default)]
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Album {
    pub id: String,
    pub name: String,
    pub path: String,
    pub count: u32,
    pub cover_path: String,
    pub latest_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AlbumList {
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathsArgs {
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareResult {
    pub shared: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrashResult {
    pub trashed: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishResult {
    pub published: u32,
    pub uris: Vec<String>,
}

/// Calling `start_foreground` again while the service runs updates the notification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForegroundArgs {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    /// 0..=100; omitted = indeterminate progress bar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeepAwakeArgs {
    pub on: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn permission_state_roundtrip() {
        let v: PermissionState = serde_json::from_value(json!({"granted": true})).unwrap();
        assert_eq!(
            v,
            PermissionState {
                granted: true,
                partial: false
            }
        );
        assert_eq!(
            serde_json::to_value(PermissionState {
                granted: true,
                partial: true
            })
            .unwrap(),
            json!({"granted": true, "partial": true})
        );
    }

    #[test]
    fn album_list_matches_contract() {
        let raw = json!({"albums": [{
            "id": "123", "name": "Camera", "path": "/storage/emulated/0/DCIM/Camera",
            "count": 3, "cover_path": "/storage/emulated/0/DCIM/Camera/a.jpg", "latest_ms": 1700000000000i64
        }]});
        let list: AlbumList = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(list.albums[0].count, 3);
        assert_eq!(serde_json::to_value(&list).unwrap(), raw);
    }

    #[test]
    fn args_serialize_as_contract() {
        assert_eq!(
            serde_json::to_value(PathsArgs {
                paths: vec!["/a.jpg".into()]
            })
            .unwrap(),
            json!({"paths": ["/a.jpg"]})
        );
        assert_eq!(
            serde_json::to_value(KeepAwakeArgs { on: true }).unwrap(),
            json!({"on": true})
        );
        let f: ForegroundArgs = serde_json::from_value(json!({"title": "t", "text": "x"})).unwrap();
        assert_eq!(f.progress, None);
        assert_eq!(
            serde_json::to_value(&f).unwrap(),
            json!({"title": "t", "text": "x"})
        );
        let r: TrashResult = serde_json::from_value(json!({"trashed": 2})).unwrap();
        assert_eq!(r.trashed, 2);
    }
}
