//! Wire types of `docs/api-contract-m2.md` section C.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::scoring::{Contribution, Issue};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Fast,
    Standard,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Standard => "standard",
        }
    }
    /// Stored in `photo.analysis_version`.
    pub fn level(self) -> i64 {
        match self {
            Self::Fast => 1,
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
    /// CLI only: lets the worker download missing models. The HTTP route never sets it.
    #[serde(skip)]
    pub allow_download: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RunState {
    Idle,
    Running,
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
        }
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub task: Vec<String>,
    pub size_mb: f64,
    pub license: String,
    pub noncommercial: bool,
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
    pub tier: Option<String>,
    pub device: Option<String>,
    pub providers: Vec<String>,
    pub gpu: Option<GpuOut>,
    pub error: Option<String>,
}

pub type IssueList = Vec<Issue>;
