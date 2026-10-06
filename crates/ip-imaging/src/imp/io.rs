//! File access helpers.
use std::fs::File;
use std::io::Read;
use std::ops::Deref;
use std::path::Path;

use crate::Result;

pub enum Data {
    Mem(Vec<u8>),
    Map(memmap2::Mmap),
}

impl Deref for Data {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Data::Mem(v) => v,
            Data::Map(m) => m,
        }
    }
}

pub fn read_all(path: &Path) -> Result<Vec<u8>> {
    Ok(std::fs::read(path)?)
}

/// Read at most `n` bytes from the start of the file.
pub fn read_head(path: &Path, n: usize) -> Result<Vec<u8>> {
    let f = File::open(path)?;
    let mut v = Vec::with_capacity(n.min(1 << 20));
    f.take(n as u64).read_to_end(&mut v)?;
    Ok(v)
}

/// Whole file: small files are read, big ones (RAW) are memory-mapped so that only the touched
/// pages (IFDs, embedded preview) are ever read from disk.
pub fn open_data(path: &Path) -> Result<Data> {
    let f = File::open(path)?;
    let len = f.metadata()?.len();
    if len < (1 << 20) {
        return Ok(Data::Mem(std::fs::read(path)?));
    }
    // SAFETY: read-only mapping; concurrent truncation by another process is the usual mmap caveat.
    let m = unsafe { memmap2::Mmap::map(&f)? };
    Ok(Data::Map(m))
}
