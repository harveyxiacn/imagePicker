//! Portable mode, part 2: catalog roots that live on the application's own drive.
//!
//! In portable mode (see [`crate::paths::DataDirSource::Portable`]) the whole folder may sit on a
//! removable disk whose drive letter changes between computers (`E:` -> `F:`). A root folder on
//! the same drive as the app is flagged `root_folder.portable = 1`; its stored path keeps the
//! drive letter it had at the time, but at startup [`rebase_roots`] rewrites the letter to the
//! drive the app runs from now. Photos are addressed as `root.path + rel_path`, so nothing else
//! changes; files that are still not found are handled by the missing-file / relocate logic
//! (`files`). Roots on other drives are never touched.
//!
//! All path handling is plain string work, so it behaves the same (and is testable) on every OS.

use std::sync::RwLock;

use rusqlite::{params, Connection};

use crate::error::Result;

/// The drive (`"E:"`) of a running portable app, `None` when not portable or not on a lettered drive.
static APP_DRIVE: RwLock<Option<String>> = RwLock::new(None);

pub fn set_app_drive(drive: Option<String>) {
    *APP_DRIVE.write().unwrap() = drive;
}

pub fn app_drive() -> Option<String> {
    APP_DRIVE.read().unwrap().clone()
}

const VERBATIM: &str = r"\\?\";

/// Length of the `\\?\` prefix when present (the form `canonicalize` returns on Windows).
fn prefix_len(path: &str) -> usize {
    if path.starts_with(VERBATIM) {
        VERBATIM.len()
    } else {
        0
    }
}

/// Upper-case drive designator (`"E:"`) of a Windows path, also for `\\?\E:\...`.
/// `None` for UNC shares, POSIX paths and relative paths.
pub fn drive_of(path: &str) -> Option<String> {
    let rest = &path[prefix_len(path)..];
    let b = rest.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        if b.len() > 2 && b[2] != b'\\' && b[2] != b'/' {
            return None; // `E:foo` is relative to the current directory of E:, not a root path
        }
        return Some(format!("{}:", (b[0] as char).to_ascii_uppercase()));
    }
    None
}

/// `path` with its drive letter replaced by `drive` (`"F:"`); `None` if it has no drive.
pub fn rebase(path: &str, drive: &str) -> Option<String> {
    drive_of(path)?;
    let p = prefix_len(path);
    Some(format!("{}{}{}", &path[..p], drive, &path[p + 2..]))
}

/// Whether a new root at `path` belongs to the portable drive `app_drive`.
pub fn is_on_drive(path: &str, app_drive: Option<&str>) -> bool {
    matches!((drive_of(path), app_drive), (Some(a), Some(b)) if a.eq_ignore_ascii_case(b))
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RebaseReport {
    /// Portable roots whose drive letter was rewritten.
    pub moved: usize,
    /// Roots on the app drive that were flagged portable now.
    pub adopted: usize,
    /// Roots left alone because the rewritten path already belongs to another root.
    pub conflicts: usize,
}

/// Startup step of portable mode on `drive` (`"F:"`): follow the app to its current drive letter
/// and flag every root that is on that drive.
pub fn rebase_roots(conn: &Connection, drive: &str) -> Result<RebaseReport> {
    let rows: Vec<(i64, String, bool)> = {
        let mut st = conn.prepare("SELECT id, path, COALESCE(portable,0) FROM root_folder")?;
        let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0)))?;
        rows.collect::<std::result::Result<_, _>>()?
    };
    let mut rep = RebaseReport::default();
    for (id, path, portable) in rows {
        let Some(old) = drive_of(&path) else { continue };
        if portable && !old.eq_ignore_ascii_case(drive) {
            let Some(new) = rebase(&path, drive) else {
                continue;
            };
            let taken: bool = conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM root_folder WHERE path=?1 AND id<>?2)",
                params![new, id],
                |r| r.get(0),
            )?;
            if taken {
                rep.conflicts += 1;
                tracing::warn!(root = %path, "portable root not rebased: {new} is another root");
                continue;
            }
            conn.execute(
                "UPDATE root_folder SET path=?2 WHERE id=?1",
                params![id, new],
            )?;
            rep.moved += 1;
        } else if !portable && old.eq_ignore_ascii_case(drive) {
            conn.execute("UPDATE root_folder SET portable=1 WHERE id=?1", [id])?;
            rep.adopted += 1;
        }
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_detection() {
        assert_eq!(drive_of(r"E:\Photos").as_deref(), Some("E:"));
        assert_eq!(drive_of("e:/Photos").as_deref(), Some("E:"));
        assert_eq!(drive_of(r"\\?\e:\Photos").as_deref(), Some("E:"));
        assert_eq!(drive_of("E:").as_deref(), Some("E:"));
        assert_eq!(drive_of(r"E:Photos"), None);
        assert_eq!(drive_of(r"\\server\share\x"), None);
        assert_eq!(drive_of("/mnt/e/Photos"), None);
        assert_eq!(drive_of(""), None);
    }

    #[test]
    fn rebase_keeps_the_rest() {
        assert_eq!(
            rebase(r"E:\Photos\2024", "F:").as_deref(),
            Some(r"F:\Photos\2024")
        );
        assert_eq!(
            rebase(r"\\?\E:\Photos", "F:").as_deref(),
            Some(r"\\?\F:\Photos")
        );
        assert_eq!(rebase("/mnt/e", "F:"), None);
        assert!(is_on_drive(r"e:\a", Some("E:")));
        assert!(!is_on_drive(r"C:\a", Some("E:")));
        assert!(!is_on_drive(r"E:\a", None));
    }

    fn catalog() -> Connection {
        let mut c = Connection::open_in_memory().unwrap();
        crate::db::migrate(&mut c).unwrap();
        c
    }

    fn add(c: &Connection, path: &str, portable: i64) -> i64 {
        c.execute(
            "INSERT INTO root_folder(path, portable, added_at) VALUES(?1, ?2, 0)",
            params![path, portable],
        )
        .unwrap();
        c.last_insert_rowid()
    }

    fn path_of(c: &Connection, id: i64) -> String {
        c.query_row("SELECT path FROM root_folder WHERE id=?1", [id], |r| {
            r.get(0)
        })
        .unwrap()
    }

    /// Fake drive mapping: the app was on `E:`, the same disk now shows up as `F:`.
    #[test]
    fn portable_roots_follow_the_drive_letter() {
        let c = catalog();
        let on_disk = add(&c, r"E:\Photos\2024", 1);
        let other_disk = add(&c, r"D:\Elsewhere", 0);
        let fresh = add(&c, r"F:\Trips", 0); // already on the new letter, never flagged
        let rep = rebase_roots(&c, "F:").unwrap();
        assert_eq!(path_of(&c, on_disk), r"F:\Photos\2024");
        assert_eq!(path_of(&c, other_disk), r"D:\Elsewhere");
        assert_eq!(path_of(&c, fresh), r"F:\Trips");
        assert_eq!(
            rep,
            RebaseReport {
                moved: 1,
                adopted: 1,
                conflicts: 0
            }
        );
        let flagged: i64 = c
            .query_row(
                "SELECT portable FROM root_folder WHERE id=?1",
                [fresh],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(flagged, 1);
        // running again changes nothing
        assert_eq!(rebase_roots(&c, "F:").unwrap(), RebaseReport::default());
    }

    #[test]
    fn rebase_never_merges_two_roots() {
        let c = catalog();
        let a = add(&c, r"E:\Photos", 1);
        add(&c, r"F:\Photos", 0);
        let rep = rebase_roots(&c, "F:").unwrap();
        assert_eq!(rep.conflicts, 1);
        assert_eq!(path_of(&c, a), r"E:\Photos");
    }

    #[test]
    fn photo_paths_resolve_through_the_new_letter() {
        let c = catalog();
        let root = add(&c, r"E:\Photos", 1);
        c.execute(
            "INSERT INTO photo(root_id, rel_path, file_name, fast_key) VALUES(?1, 'a/b.jpg', 'b.jpg', 'k')",
            [root],
        )
        .unwrap();
        rebase_roots(&c, "G:").unwrap();
        let (r, rel): (String, String) = c
            .query_row(
                "SELECT r.path, p.rel_path FROM photo p JOIN root_folder r ON r.id=p.root_id",
                [],
                |x| Ok((x.get(0)?, x.get(1)?)),
            )
            .unwrap();
        assert_eq!((r.as_str(), rel.as_str()), (r"G:\Photos", "a/b.jpg"));
    }
}
