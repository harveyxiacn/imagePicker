//! Wire/domain types. Field names match `docs/api-contract-m1.md` exactly.

use serde::{Deserialize, Deserializer, Serialize};

pub const COLOR_LABELS: [&str; 5] = ["red", "yellow", "green", "blue", "purple"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Photo {
    pub id: i64,
    pub session_id: i64,
    pub path: String,
    pub file_name: String,
    pub format: String,
    pub file_size: i64,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub taken_at: Option<i64>,
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub focal_mm: Option<f64>,
    pub aperture: Option<f64>,
    pub shutter_s: Option<f64>,
    pub iso: Option<i64>,
    pub user_rating: Option<i64>,
    pub ai_rating: Option<f64>,
    pub flag: i64,
    pub color_label: Option<String>,
    pub burst_id: Option<i64>,
    pub thumb_ready: bool,
    pub thumb_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportState {
    Scanning,
    Thumbnailing,
    Ready,
}

impl ImportState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Scanning => "scanning",
            Self::Thumbnailing => "thumbnailing",
            Self::Ready => "ready",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "scanning" => Self::Scanning,
            "thumbnailing" => Self::Thumbnailing,
            _ => Self::Ready,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Session {
    pub id: i64,
    pub title: String,
    pub root_path: String,
    pub created_at: i64,
    pub photo_count: i64,
    pub picked_count: i64,
    pub rejected_count: i64,
    pub rated_count: i64,
    pub cover_photo_id: Option<i64>,
    pub import_state: ImportState,
}

/// `{"id":..,"v":".."}` entry of `thumbs.ready`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbItem {
    pub id: i64,
    pub v: String,
}

/// Entry of `photos.updated`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PhotoUpdate {
    pub id: i64,
    pub user_rating: Option<i64>,
    pub flag: i64,
    pub color_label: Option<String>,
}

/// Distinguishes an absent JSON field (outer `None`) from an explicit `null` (`Some(None)`).
pub fn double_option<'de, T, D>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct PatchRequest {
    pub ids: Vec<i64>,
    #[serde(default, deserialize_with = "double_option")]
    pub user_rating: Option<Option<i64>>,
    pub flag: Option<i64>,
    #[serde(default, deserialize_with = "double_option")]
    pub color_label: Option<Option<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ImportRequest {
    pub path: String,
    #[serde(default = "default_true")]
    pub recursive: bool,
    pub title: Option<String>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExportRequest {
    pub ids: Vec<i64>,
    pub dest: String,
    #[serde(default)]
    pub long_edge: Option<u32>,
    #[serde(default = "default_quality")]
    pub quality: u8,
    #[serde(default = "default_template")]
    pub name_template: String,
}

fn default_quality() -> u8 {
    90
}
fn default_template() -> String {
    "{name}".into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlagFilter {
    #[default]
    Any,
    Picked,
    Rejected,
    Unflagged,
    NotRejected,
}

impl FlagFilter {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "picked" => Self::Picked,
            "rejected" => Self::Rejected,
            "unflagged" => Self::Unflagged,
            "not_rejected" => Self::NotRejected,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    #[default]
    TakenAt,
    TakenAtDesc,
    Name,
    Rating,
}

impl SortKey {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "taken_at" => Self::TakenAt,
            "-taken_at" => Self::TakenAtDesc,
            "name" => Self::Name,
            "rating" => Self::Rating,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct PhotoQuery {
    pub session_id: i64,
    pub rating_gte: Option<i64>,
    pub flag: FlagFilter,
    pub color_label: Option<String>,
    pub sort: SortKey,
    pub cursor: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhotosPage {
    pub photos: Vec<Photo>,
    pub total: i64,
    pub next_cursor: Option<String>,
}
