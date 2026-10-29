//! Wire types of `docs/api-contract-m2.md` section C.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::scoring::{Contribution, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    /// On-device analysis without the AI worker (`docs/api-contract-m8.md` section B):
    /// pHash + sharpness/exposure/noise only, no faces or embeddings.
    Lite,
    Fast,
    Standard,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lite => "lite",
            Self::Fast => "fast",
            Self::Standard => "standard",
        }
    }
    /// Stored in `photo.analysis_version`.
    pub fn level(self) -> i64 {
        match self {
            // same level as `fast`: a later `fast` run still re-analyses lite results (the
            // run query checks `analysis.profile`), a `lite` run skips anything analysed
            Self::Lite | Self::Fast => 1,
            Self::Standard => 2,
        }
    }
    pub fn from_level(l: i64) -> Option<Self> {
        match l {
            1 => Some(Self::Fast),
            2 => Some(Self::Standard),
            _ => None,
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "lite" => Some(Self::Lite),
            "fast" => Some(Self::Fast),
            "standard" => Some(Self::Standard),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnalysisRunRequest {
    pub session_id: i64,
    pub profile: Profile,
    #[serde(default)]
    pub photo_ids: Option<Vec<i64>>,
    /// Re-analyse photos that already have an analysis of this profile (not in the contract).
    #[serde(default)]
    pub force: bool,
    /// When the AI components or a model are unavailable (no runtime, models missing, worker
    /// unreachable), carry on with the on-device `lite` profile instead of failing.
    #[serde(default)]
    pub fallback_lite: bool,
    /// CLI only: lets the worker download missing models. The HTTP route never sets it.
    #[serde(skip)]
    pub allow_download: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Idle,
    Running,
    /// Stopped between two batches on request (or restored as such after a restart); `continue`
    /// takes it up again with the photos that are left.
    Paused,
    Done,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisStatus {
    pub state: RunState,
    pub profile: Option<Profile>,
    pub done: i64,
    pub total: i64,
    /// `analyzing` | `grouping` | `scoring` | `clustering`
    pub stage: Option<String>,
    pub error: Option<String>,
    /// Steps the worker skipped (missing models); not part of the M2 contract.
    pub skipped_steps: Vec<String>,
    /// Machine-readable cause of `error` (`worker_unavailable` | `models_missing`): the UI offers
    /// the `lite` profile for these.
    pub error_code: Option<String>,
    /// The profile that was asked for when the run switched to `lite` (`fallback_lite`).
    pub fallback_from: Option<Profile>,
    /// Photos redone for single steps because a model was installed since their analysis.
    pub incremental: i64,
    /// The run was picked up again after the app had exited.
    pub resumed: bool,
}

impl AnalysisStatus {
    pub fn idle() -> Self {
        Self {
            state: RunState::Idle,
            profile: None,
            done: 0,
            total: 0,
            stage: None,
            error: None,
            skipped_steps: Vec::new(),
            error_code: None,
            fallback_from: None,
            incremental: 0,
            resumed: false,
        }
    }
}

/// `GET /api/analysis/plan`: what a run of `profile` would do now.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnalysisPlan {
    /// Photos without an analysis of this level (they get every step).
    pub new_photos: i64,
    /// Photos that only lack single steps whose models have been installed since.
    pub incremental_photos: i64,
    /// The steps those photos get, in profile order.
    pub incremental_steps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Face {
    pub id: i64,
    pub photo_id: i64,
    pub person_id: Option<i64>,
    pub person_name: Option<String>,
    pub bbox: [f64; 4],
    pub eyes_open: Option<f64>,
    pub smile: Option<f64>,
    pub gaze: Option<f64>,
    pub yaw: Option<f64>,
    pub pitch: Option<f64>,
    pub roll: Option<f64>,
    pub sharpness: Option<f64>,
    pub expression_score: Option<f64>,
    pub is_subject: bool,
    /// 0-1 confidence that the face is on a statue / poster / screen / print; such a face is never
    /// a subject or a bystander (it can still be removed by hand).
    #[serde(default)]
    pub artificial: Option<f64>,
    #[serde(default)]
    pub likely_artificial: bool,
    /// Not recognised as a human face (a pet; docs/03 §4.3): no person, never a subject or a
    /// bystander.
    #[serde(default)]
    pub nonhuman: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Scores {
    pub sharpness: Option<f64>,
    pub exposure: Option<f64>,
    pub noise: Option<f64>,
    pub iqa: Option<f64>,
    pub aesthetic: Option<f64>,
    pub face: Option<f64>,
    pub composition: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReasonOut {
    pub key: String,
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisDetail {
    pub photo_id: i64,
    pub analyzed: bool,
    pub profile: Option<String>,
    pub scores: Option<Scores>,
    pub ai_score: Option<f64>,
    pub ai_rating: Option<f64>,
    pub contributions: Vec<Contribution>,
    pub reasons: Vec<ReasonOut>,
    pub scene_type: Option<String>,
    pub faces: Vec<Face>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BurstOut {
    pub id: i64,
    pub best_photo_id: Option<i64>,
    pub photo_ids: Vec<i64>,
    pub size: i64,
    pub start_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SceneOut {
    pub id: i64,
    pub start_at: i64,
    pub end_at: i64,
    pub bursts: Vec<BurstOut>,
    /// Only in `GET /api/sessions/{id}/timeline`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<i64>,
}

/// One event of the timeline (docs/09 P1-1): scenes of one session that are not separated by a
/// pause of more than 2 h or a move of more than 5 km.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EventOut {
    pub id: i64,
    /// Display name: `title` when set, else the local date and time span (`2024-05-03 10:12-14:40`).
    pub name: String,
    pub title: Option<String>,
    /// Place name; not set automatically (the coordinates are `gps_lat` / `gps_lon`).
    pub place: Option<String>,
    /// Capture time (UTC ms) of the first and last photo.
    pub start_at: i64,
    pub end_at: i64,
    /// UTC offset (minutes) the first photo with a known offset was taken at.
    pub offset_min: Option<i32>,
    /// Mean position of the event's geotagged photos.
    pub gps_lat: Option<f64>,
    pub gps_lon: Option<f64>,
    pub photo_count: i64,
    pub scenes: Vec<SceneOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimelineOut {
    pub events: Vec<EventOut>,
}

/// `GET /api/photos/{id}/duplicates`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DuplicatesOut {
    /// `None` = the photo is not a duplicate of anything.
    pub group_id: Option<i64>,
    /// Every photo of the group (catalog wide, every session), the asked one included.
    pub photo_ids: Vec<i64>,
    /// The members that are byte-identical copies of another member.
    pub exact_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroupsOut {
    pub scenes: Vec<SceneOut>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SplitRequest {
    pub burst_id: i64,
    pub at_photo_id: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MergeBurstsRequest {
    pub burst_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackOut {
    pub track_id: i64,
    pub person_id: Option<i64>,
    pub person_name: Option<String>,
    pub cells: BTreeMap<String, Option<Face>>,
    pub best_photo_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurstFacesOut {
    pub photo_ids: Vec<i64>,
    pub tracks: Vec<TrackOut>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Person {
    pub id: i64,
    pub name: Option<String>,
    pub cover_face_id: Option<i64>,
    pub photo_count: i64,
    pub hidden: bool,
    /// A subject seen in a single photo only (docs/api-contract-m5.md D): selectable in the
    /// portrait panel, hidden from `GET /api/people` unless `include_singletons=1`.
    pub singleton: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PersonPatch {
    #[serde(default, deserialize_with = "crate::model::double_option")]
    pub name: Option<Option<String>>,
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MergePeopleRequest {
    pub ids: Vec<i64>,
    pub into: i64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetFacePersonRequest {
    #[serde(default)]
    pub person_id: Option<i64>,
}

/// `POST /api/people`: "this is a new person" (search by face). The faces leave whatever person
/// they had and are locked to the new one, so re-clustering keeps them there.
#[derive(Debug, Clone, Deserialize)]
pub struct CreatePersonRequest {
    pub face_ids: Vec<i64>,
    /// Optional name; blank = unnamed.
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub task: Vec<String>,
    pub size_mb: f64,
    pub license: String,
    pub noncommercial: bool,
    /// `commercial-ok` | `research-only` | `non-commercial` | `unknown` (never empty)
    pub usage: String,
    pub license_info: ip_worker_client::LicenseInfo,
    pub installed: bool,
    pub required_for: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuOut {
    pub name: String,
    pub vram_mb: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerOut {
    pub state: String,
    /// The tier in effect (the manual override, if any).
    pub tier: Option<String>,
    /// What detection found; `None` until the worker has reported.
    pub tier_detected: Option<String>,
    pub tier_overridden: bool,
    pub device: Option<String>,
    pub providers: Vec<String>,
    pub gpu: Option<GpuOut>,
    pub error: Option<String>,
}

pub type IssueList = Vec<Issue>;
