//! Helpers shared by the `tests_*` modules that are identical wherever they are used.

use std::fmt::Write as _;
use std::path::Path;
use std::sync::Arc;

use sha2::{Digest, Sha256};

use crate::Core;

/// Lowercase hex SHA-256 of a file's bytes. Tests compare this before and after an operation
/// to prove the original was not touched.
pub(crate) fn sha256(p: &Path) -> String {
    let digest = Sha256::digest(std::fs::read(p).unwrap());
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest.iter() {
        write!(out, "{b:02x}").unwrap();
    }
    out
}

/// A single-integer value from the catalog (`SELECT count(*) ...` and friends).
pub(crate) async fn count(core: &Arc<Core>, sql: &'static str) -> i64 {
    core.db
        .call(move |c| Ok(c.query_row(sql, [], |r| r.get::<_, i64>(0))?))
        .await
        .unwrap()
}
