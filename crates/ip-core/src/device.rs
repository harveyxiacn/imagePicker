//! Camera / phone device naming: vendor spellings are normalised to one canonical make, the
//! model loses a repeated make prefix, and the pair is classified into a [`DeviceKind`].
//! Everything here is pure; the raw EXIF strings stay in the `device` table.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Phone,
    Camera,
    Drone,
    Action,
    Unknown,
}

impl DeviceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Phone => "phone",
            Self::Camera => "camera",
            Self::Drone => "drone",
            Self::Action => "action",
            Self::Unknown => "unknown",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "phone" => Self::Phone,
            "camera" => Self::Camera,
            "drone" => Self::Drone,
            "action" => Self::Action,
            "unknown" => Self::Unknown,
            _ => return None,
        })
    }
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Canonical vendor name. Known spellings map to a fixed name; a single ALL-CAPS token is
/// title-cased; anything else is kept as written (trimmed).
pub fn normalize_make(raw: &str) -> String {
    let t = squash(raw);
    let canon = match t.to_lowercase().as_str() {
        "nikon corporation" | "nikon" => "Nikon",
        "sony" | "sony corporation" => "Sony",
        "canon" | "canon inc." | "canon inc" => "Canon",
        "fujifilm" | "fuji photo film co., ltd." | "fujifilm corporation" => "Fujifilm",
        "olympus imaging corp." | "olympus corporation" | "olympus" => "Olympus",
        "om digital solutions" | "om system" | "om digital solutions corporation" => "OM System",
        "panasonic" => "Panasonic",
        "ricoh imaging company, ltd." | "ricoh" => "Ricoh",
        "leica camera ag" | "leica" => "Leica",
        "samsung" | "samsung electronics" => "Samsung",
        "huawei" => "Huawei",
        "xiaomi" => "Xiaomi",
        "oppo" => "OPPO",
        "vivo" => "vivo",
        "honor" => "HONOR",
        "oneplus" => "OnePlus",
        "apple" => "Apple",
        "google" => "Google",
        "dji" => "DJI",
        "gopro" => "GoPro",
        "insta360" => "Insta360",
        "hmd global" | "nokia" => "Nokia",
        "realme" => "realme",
        "meizu" => "Meizu",
        _ => "",
    };
    if !canon.is_empty() {
        return canon.to_string();
    }
    let single_caps = !t.is_empty()
        && !t.contains(' ')
        && t.chars().any(|c| c.is_alphabetic())
        && !t.chars().any(|c| c.is_lowercase());
    if single_caps {
        let mut it = t.chars();
        let first = it.next().map(|c| c.to_uppercase().to_string());
        return first.unwrap_or_default() + &it.as_str().to_lowercase();
    }
    t
}

/// Model without a leading repeat of the make (case-insensitive, whole word), whitespace collapsed.
pub fn normalize_model(make: &str, raw: &str) -> String {
    let model = squash(raw);
    let canon = normalize_make(make);
    let raw_make = squash(make);
    for prefix in [canon.as_str(), raw_make.as_str()] {
        if prefix.is_empty() || model.len() <= prefix.len() || !model.is_char_boundary(prefix.len())
        {
            continue;
        }
        let (head, tail) = model.split_at(prefix.len());
        if head.eq_ignore_ascii_case(prefix) && tail.starts_with(' ') {
            return tail.trim_start().to_string();
        }
    }
    model
}

/// `"{make} {model}"`, or whichever part exists (empty when neither does).
pub fn display_name(make: &str, model: &str) -> String {
    match (make.is_empty(), model.is_empty()) {
        (false, false) => format!("{make} {model}"),
        (false, true) => make.to_string(),
        (true, false) => model.to_string(),
        (true, true) => String::new(),
    }
}

/// Normalised display name straight from raw EXIF strings; `None` when both are empty.
pub fn display_from_raw(make: Option<&str>, model: Option<&str>) -> Option<String> {
    let mk = normalize_make(make.unwrap_or(""));
    let md = normalize_model(make.unwrap_or(""), model.unwrap_or(""));
    Some(display_name(&mk, &md)).filter(|s| !s.is_empty())
}

/// Phone / camera / drone / action camera, from make and model (raw or normalised).
pub fn classify(make: &str, model: &str) -> DeviceKind {
    let mk = normalize_make(make);
    let md = normalize_model(make, model).to_lowercase();
    let has = |s: &str| md.contains(s);
    let starts = |s: &str| md.starts_with(s);
    match mk.as_str() {
        "Apple" => DeviceKind::Phone,
        "Samsung" => {
            if starts("sm-") || has("galaxy") {
                DeviceKind::Phone
            } else if md.is_empty() {
                DeviceKind::Unknown
            } else {
                DeviceKind::Camera
            }
        }
        "Xiaomi" | "Huawei" | "HONOR" | "OPPO" | "vivo" | "OnePlus" | "realme" | "Meizu"
        | "Nokia" => DeviceKind::Phone,
        "Google" => {
            if has("pixel") {
                DeviceKind::Phone
            } else {
                DeviceKind::Unknown
            }
        }
        "Sony" => {
            if starts("xq-") || has("xperia") {
                DeviceKind::Phone
            } else {
                DeviceKind::Camera
            }
        }
        "DJI" => {
            if has("osmo") || has("action") || has("pocket") {
                DeviceKind::Action
            } else {
                // Mavic / Mini / Air / Phantom and the FCxxxx drone camera codes.
                DeviceKind::Drone
            }
        }
        "GoPro" | "Insta360" => DeviceKind::Action,
        "Nikon" | "Canon" | "Fujifilm" | "Olympus" | "OM System" | "Panasonic" | "Ricoh"
        | "Leica" | "Pentax" | "Hasselblad" | "Sigma" | "Kodak" | "Casio" | "Minolta" => {
            DeviceKind::Camera
        }
        _ => DeviceKind::Unknown,
    }
}

/// `(name, kind)` for a device row's raw make/model.
pub fn describe(make: Option<&str>, model: Option<&str>) -> (String, DeviceKind) {
    let name = display_from_raw(make, model).unwrap_or_default();
    (name, classify(make.unwrap_or(""), model.unwrap_or("")))
}

/// Fills `device.name` / `device.kind` for rows that predate them and refreshes the denormalised
/// `photo.camera` of their photos. A no-op once every device has a name.
pub fn backfill(conn: &Connection) -> Result<()> {
    let rows: Vec<(i64, Option<String>, Option<String>)> = {
        let mut st =
            conn.prepare("SELECT id, make, model FROM device WHERE name IS NULL OR kind IS NULL")?;
        let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        it.collect::<rusqlite::Result<_>>()?
    };
    if rows.is_empty() {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for (id, make, model) in rows {
        let (name, kind) = describe(make.as_deref(), model.as_deref());
        tx.execute(
            "UPDATE device SET name=?2, kind=?3 WHERE id=?1",
            rusqlite::params![id, name, kind.as_str()],
        )?;
        tx.execute(
            "UPDATE photo SET camera=?2 WHERE device_id=?1 AND camera IS NOT ?2",
            rusqlite::params![id, Some(name).filter(|n| !n.is_empty())],
        )?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use DeviceKind::*;

    #[test]
    fn normalises_and_classifies() {
        // (raw make, raw model, make, model, display, kind)
        let t: &[(&str, &str, &str, &str, &str, DeviceKind)] = &[
            (
                "NIKON CORPORATION",
                "NIKON Z 6",
                "Nikon",
                "Z 6",
                "Nikon Z 6",
                Camera,
            ),
            (
                "Canon",
                "Canon EOS R5",
                "Canon",
                "EOS R5",
                "Canon EOS R5",
                Camera,
            ),
            (
                "SONY",
                "ILCE-7M4",
                "Sony",
                "ILCE-7M4",
                "Sony ILCE-7M4",
                Camera,
            ),
            (
                "SONY",
                "SONY ILCE-7M4",
                "Sony",
                "ILCE-7M4",
                "Sony ILCE-7M4",
                Camera,
            ),
            ("SONY", "XQ-DQ72", "Sony", "XQ-DQ72", "Sony XQ-DQ72", Phone),
            (
                "Apple",
                "iPhone 15 Pro",
                "Apple",
                "iPhone 15 Pro",
                "Apple iPhone 15 Pro",
                Phone,
            ),
            (
                "samsung",
                "SM-S928B",
                "Samsung",
                "SM-S928B",
                "Samsung SM-S928B",
                Phone,
            ),
            (
                "SAMSUNG",
                "NX500",
                "Samsung",
                "NX500",
                "Samsung NX500",
                Camera,
            ),
            ("DJI", "FC3582", "DJI", "FC3582", "DJI FC3582", Drone),
            (
                "DJI",
                "Osmo Action 4",
                "DJI",
                "Osmo Action 4",
                "DJI Osmo Action 4",
                Action,
            ),
            ("DJI", "Mavic 3", "DJI", "Mavic 3", "DJI Mavic 3", Drone),
            (
                "GoPro",
                "HERO12 Black",
                "GoPro",
                "HERO12 Black",
                "GoPro HERO12 Black",
                Action,
            ),
            (
                "Google",
                "Pixel 8",
                "Google",
                "Pixel 8",
                "Google Pixel 8",
                Phone,
            ),
            (
                "HMD Global",
                "Nokia G42",
                "Nokia",
                "G42",
                "Nokia G42",
                Phone,
            ),
            (
                "OM Digital Solutions",
                "OM-1",
                "OM System",
                "OM-1",
                "OM System OM-1",
                Camera,
            ),
            (
                "OLYMPUS IMAGING CORP.",
                "E-M1",
                "Olympus",
                "E-M1",
                "Olympus E-M1",
                Camera,
            ),
            (
                "RICOH IMAGING COMPANY, LTD.",
                "PENTAX K-3",
                "Ricoh",
                "PENTAX K-3",
                "Ricoh PENTAX K-3",
                Camera,
            ),
            ("FAKE", "FAKE Cam 1", "Fake", "Cam 1", "Fake Cam 1", Unknown),
            (
                "Acme Labs",
                "  X  100 ",
                "Acme Labs",
                "X 100",
                "Acme Labs X 100",
                Unknown,
            ),
            ("", "", "", "", "", Unknown),
            ("", "Mystery 1", "", "Mystery 1", "Mystery 1", Unknown),
        ];
        for (rm, rmd, mk, md, disp, kind) in t {
            let nm = normalize_make(rm);
            let nmd = normalize_model(rm, rmd);
            assert_eq!((nm.as_str(), nmd.as_str()), (*mk, *md), "{rm}/{rmd}");
            assert_eq!(display_name(&nm, &nmd), *disp, "{rm}/{rmd}");
            assert_eq!(classify(rm, rmd), *kind, "{rm}/{rmd}");
            assert_eq!(classify(&nm, &nmd), *kind, "{rm}/{rmd} normalised");
        }
    }

    #[test]
    fn make_prefix_needs_a_word_boundary() {
        assert_eq!(normalize_model("Sony", "Sonyx 1"), "Sonyx 1");
        assert_eq!(display_from_raw(None, None), None);
        assert_eq!(
            display_from_raw(Some("Canon"), None).as_deref(),
            Some("Canon")
        );
    }

    #[test]
    fn kind_serialises_lowercase() {
        assert_eq!(
            serde_json::to_string(&DeviceKind::Action).unwrap(),
            "\"action\""
        );
        assert_eq!(DeviceKind::parse("drone"), Some(DeviceKind::Drone));
    }

    #[test]
    fn backfill_fills_old_rows_and_photo_camera() {
        let mut conn = Connection::open_in_memory().unwrap();
        crate::db::migrate_to(&mut conn, 8).unwrap();
        conn.execute_batch(
            "INSERT INTO root_folder(id, path, added_at) VALUES(1, '/r', 0);
             INSERT INTO device(id, make, model) VALUES(1, 'NIKON CORPORATION', 'NIKON Z 6');
             INSERT INTO photo(id, root_id, rel_path, file_name, fast_key, device_id, camera)
               VALUES(1, 1, 'a.jpg', 'a.jpg', 'k', 1, 'NIKON Z 6');",
        )
        .unwrap();
        crate::db::migrate(&mut conn).unwrap();
        backfill(&conn).unwrap();
        let (name, kind): (String, String) = conn
            .query_row("SELECT name, kind FROM device WHERE id=1", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((name.as_str(), kind.as_str()), ("Nikon Z 6", "camera"));
        let cam: String = conn
            .query_row("SELECT camera FROM photo WHERE id=1", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cam, "Nikon Z 6");
    }
}
