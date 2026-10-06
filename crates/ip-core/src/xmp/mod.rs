//! XMP interop (`docs/api-contract-m6.md` section C): sidecar and embedded XMP of rating, label
//! and keywords, compatible with Lightroom (`xmp:Rating`, `xmp:Label`, `dc:subject`,
//! `lr:hierarchicalSubject`) and darktable (`xmp:Rating = -1` for rejected,
//! `darktable:colorlabels`).

pub mod doc;
pub mod jpeg;
mod sync;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::Mutex;

pub use doc::{Xmp, XmpError};
pub use sync::{XmpSyncReport, XmpSyncRequest};

use crate::model::COLOR_LABELS;

/// Quiet period after the last rating/flag/label/keyword change before sidecars are written.
pub const DEFAULT_DEBOUNCE_MS: u64 = 1000;

/// Core-side state of the debounced writer.
pub struct XmpState {
    pub(crate) pending: Mutex<BTreeSet<i64>>,
    pub(crate) scheduled: AtomicBool,
    pub(crate) debounce_ms: AtomicU64,
}

impl Default for XmpState {
    fn default() -> Self {
        Self {
            pending: Mutex::new(BTreeSet::new()),
            scheduled: AtomicBool::new(false),
            debounce_ms: AtomicU64::new(DEFAULT_DEBOUNCE_MS),
        }
    }
}

/// `red` -> `Red` (the names Lightroom writes).
pub fn label_to_xmp(c: &str) -> Option<&'static str> {
    Some(match c {
        "red" => "Red",
        "yellow" => "Yellow",
        "green" => "Green",
        "blue" => "Blue",
        "purple" => "Purple",
        _ => return None,
    })
}

/// `Red` / `red` / ` RED ` -> `red`; labels the catalog does not know (`Review`, ...) -> `None`.
pub fn label_from_xmp(s: &str) -> Option<&'static str> {
    let t = s.trim().to_ascii_lowercase();
    COLOR_LABELS.iter().copied().find(|c| *c == t)
}

/// darktable colour label index (0 red .. 4 purple).
pub fn label_from_darktable(i: i64) -> Option<&'static str> {
    // darktable: 0 red, 1 yellow, 2 green, 3 blue, 4 purple
    COLOR_LABELS.get(usize::try_from(i).ok()?).copied()
}

/// `<stem>.xmp` (Adobe) and `<file>.<ext>.xmp` (darktable).
pub fn sidecar_candidates(photo: &Path) -> [PathBuf; 2] {
    let stem = photo.with_extension("xmp");
    let mut full = photo.as_os_str().to_os_string();
    full.push(".xmp");
    [stem, PathBuf::from(full)]
}

/// The sidecar that exists next to `photo` (`<stem>.xmp` wins over `<file>.xmp`).
pub fn find_sidecar(photo: &Path) -> Option<PathBuf> {
    sidecar_candidates(photo).into_iter().find(|p| p.is_file())
}

/// What a sidecar or embedded packet says about a photo, in catalog terms.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct XmpValues {
    /// `xmp:Rating` as written: -1 = rejected, 0..=5 (clamped).
    pub rating: Option<i64>,
    /// `xmpDM:pick`: 1 picked, -1 rejected.
    pub pick: Option<i64>,
    /// Catalog colour label.
    pub label: Option<&'static str>,
    pub keywords: Vec<String>,
}

pub fn values_of(x: &Xmp) -> XmpValues {
    let rating = x.rating().map(|r| r.clamp(-1, 5));
    let label = x.label().and_then(|l| label_from_xmp(&l)).or_else(|| {
        x.darktable_labels()
            .first()
            .and_then(|i| label_from_darktable(*i))
    });
    let mut keywords = x.keywords();
    if keywords.is_empty() {
        // leaves of the hierarchy: `Places|Japan|Kyoto` -> `Kyoto`
        keywords = x
            .hierarchical_keywords()
            .iter()
            .filter_map(|k| k.rsplit('|').next().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect();
    }
    XmpValues {
        rating,
        pick: x.pick().filter(|p| *p == 1 || *p == -1),
        label,
        keywords: normalize_tags(keywords),
    }
}

/// Trimmed, de-duplicated (order kept), at most 100 characters, no control characters.
pub fn normalize_tags<I: IntoIterator<Item = String>>(tags: I) -> Vec<String> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for t in tags {
        let t: String = t.chars().filter(|c| !c.is_control()).collect();
        let t = t.trim().to_string();
        if t.is_empty() || t.chars().count() > 100 {
            continue;
        }
        if seen.insert(t.clone()) {
            out.push(t);
        }
    }
    out
}

/// What the catalog knows about a photo, for writing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CatalogValues {
    pub user_rating: Option<i64>,
    pub flag: i64,
    pub color_label: Option<String>,
    pub tags: Vec<String>,
}

impl CatalogValues {
    /// `xmp:Rating` for the catalog state: rejected is -1 (darktable/Lightroom), else the stars.
    pub fn xmp_rating(&self) -> Option<i64> {
        if self.flag == -1 {
            Some(-1)
        } else {
            self.user_rating.filter(|r| *r > 0)
        }
    }
}

/// Applies the catalog values to a packet, touching only rating, label and keywords.
/// Returns whether anything changed.
pub fn apply_catalog(x: &mut Xmp, c: &CatalogValues) -> Result<bool, XmpError> {
    let before = x.serialize();
    // rating: keep an equivalent existing value (e.g. "3.0" or the 0 Lightroom writes for "none")
    let want = c.xmp_rating();
    let cur = x.rating();
    let norm = |r: Option<i64>| match r {
        Some(-1) => -1,
        Some(r) => r.max(0),
        None => 0,
    };
    if norm(cur) != norm(want) {
        x.set_rating(want)?;
    }
    let want_label = c.color_label.as_deref().and_then(label_to_xmp);
    let cur_label = x.label();
    if cur_label.as_deref().and_then(label_from_xmp) != want_label.and_then(label_from_xmp) {
        x.set_label(want_label)?;
    }
    let cur_kw = values_of(x).keywords;
    let cur_set: BTreeSet<&String> = cur_kw.iter().collect();
    let want_set: BTreeSet<&String> = c.tags.iter().collect();
    if cur_set != want_set {
        x.set_keywords(&c.tags)?;
        // hierarchical entries: keep the ones whose leaf survives, add flat entries for new tags
        let mut h: Vec<String> = x
            .hierarchical_keywords()
            .into_iter()
            .filter(|k| {
                let leaf = k.rsplit('|').next().unwrap_or(k).trim();
                c.tags.iter().any(|t| t == leaf)
            })
            .collect();
        for t in &c.tags {
            if !h
                .iter()
                .any(|k| k.rsplit('|').next().map(str::trim) == Some(t.as_str()))
            {
                h.push(t.clone());
            }
        }
        x.set_hierarchical_keywords(&h)?;
    }
    Ok(x.serialize() != before)
}

/// Writes `bytes` to `path` through a temporary file in the same directory.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(".{name}.ip-tmp-{}", std::process::id()));
    let res = (|| {
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_names() {
        assert_eq!(label_to_xmp("red"), Some("Red"));
        assert_eq!(label_to_xmp("pink"), None);
        assert_eq!(label_from_xmp(" GREEN "), Some("green"));
        assert_eq!(label_from_xmp("Review"), None);
        assert_eq!(label_from_darktable(0), Some("red"));
        assert_eq!(label_from_darktable(4), Some("purple"));
        assert_eq!(label_from_darktable(9), None);
        assert_eq!(label_from_darktable(-1), None);
    }

    #[test]
    fn sidecar_names() {
        let c = sidecar_candidates(Path::new("/p/IMG_1.CR2"));
        assert_eq!(c[0], PathBuf::from("/p/IMG_1.xmp"));
        assert_eq!(c[1], PathBuf::from("/p/IMG_1.CR2.xmp"));
    }

    #[test]
    fn values_map_to_catalog_terms() {
        let x = Xmp::parse(
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:lr="http://ns.adobe.com/lightroom/1.0/" xmp:Rating="9" xmp:Label="Purple"><lr:hierarchicalSubject><rdf:Bag><rdf:li>Places|Japan|Kyoto</rdf:li><rdf:li> Kyoto </rdf:li></rdf:Bag></lr:hierarchicalSubject></rdf:Description></rdf:RDF></x:xmpmeta>"#,
        )
        .unwrap();
        let v = values_of(&x);
        assert_eq!(v.rating, Some(5), "clamped");
        assert_eq!(v.label, Some("purple"));
        assert_eq!(
            v.keywords,
            ["Kyoto"],
            "leaf of the hierarchy, de-duplicated"
        );
    }

    #[test]
    fn rejected_is_minus_one_and_stars_survive_otherwise() {
        let mut c = CatalogValues {
            user_rating: Some(4),
            ..Default::default()
        };
        assert_eq!(c.xmp_rating(), Some(4));
        c.flag = -1;
        assert_eq!(c.xmp_rating(), Some(-1));
        c.flag = 0;
        c.user_rating = Some(0);
        assert_eq!(c.xmp_rating(), None);
    }

    #[test]
    fn apply_catalog_is_minimal_and_idempotent() {
        let mut x = Xmp::empty();
        let c = CatalogValues {
            user_rating: Some(3),
            flag: 0,
            color_label: Some("blue".into()),
            tags: vec!["a".into(), "b".into()],
        };
        assert!(apply_catalog(&mut x, &c).unwrap());
        assert!(
            !apply_catalog(&mut x, &c).unwrap(),
            "second pass changes nothing"
        );
        let v = values_of(&x);
        assert_eq!((v.rating, v.label), (Some(3), Some("blue")));
        assert_eq!(v.keywords, ["a", "b"]);
        let rejected = CatalogValues {
            flag: -1,
            ..c.clone()
        };
        assert!(apply_catalog(&mut x, &rejected).unwrap());
        assert_eq!(x.rating(), Some(-1));
        // "3.0" counts as 3
        let mut y = Xmp::empty();
        y.set_rating(Some(3)).unwrap();
        let s = y
            .serialize()
            .replace("xmp:Rating=\"3\"", "xmp:Rating=\"3.0\"");
        let mut y = Xmp::parse(&s).unwrap();
        let only_rating = CatalogValues {
            user_rating: Some(3),
            ..Default::default()
        };
        assert!(!apply_catalog(&mut y, &only_rating).unwrap());
    }

    #[test]
    fn tags_are_normalised() {
        let t = normalize_tags(
            ["  x ", "x", "", "a\u{0}b", &"l".repeat(101), "y"]
                .iter()
                .map(|s| s.to_string()),
        );
        assert_eq!(t, ["x", "ab", "y"]);
    }

    #[test]
    fn atomic_write_replaces_and_cleans_up() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.xmp");
        atomic_write(&p, b"one").unwrap();
        atomic_write(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        let n = std::fs::read_dir(d.path()).unwrap().count();
        assert_eq!(n, 1, "no temp file left behind");
    }
}
