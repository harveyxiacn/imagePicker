//! XMP inside JPEG files: the `http://ns.adobe.com/xap/1.0/\0` APP1 segment.
//!
//! Rewriting touches exactly one segment: the bytes before it and everything after it (the
//! remaining metadata, the quantisation/Huffman tables and the entropy-coded pixel data) are
//! copied verbatim.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const XMP_HEADER: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";
const EXT_HEADER: &[u8] = b"http://ns.adobe.com/xmp/extension/\0";
/// Largest XMP payload one APP1 segment can carry.
pub const MAX_XMP: usize = 0xFFFF - 2 - 29;

/// A marker segment: `start..end` covers marker + length + payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub marker: u8,
    pub start: usize,
    pub end: usize,
}

/// Segments up to (not including) the start of scan.
pub fn segments(jpeg: &[u8]) -> Option<Vec<Segment>> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut out = Vec::new();
    let mut pos = 2;
    while pos + 4 <= jpeg.len() {
        if jpeg[pos] != 0xFF {
            return None;
        }
        let marker = jpeg[pos + 1];
        if marker == 0xFF {
            pos += 1;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            pos += 2;
            continue;
        }
        let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        if len < 2 || pos + 2 + len > jpeg.len() {
            return None;
        }
        out.push(Segment {
            marker,
            start: pos,
            end: pos + 2 + len,
        });
        pos += 2 + len;
    }
    Some(out)
}

fn payload<'a>(jpeg: &'a [u8], s: &Segment) -> &'a [u8] {
    &jpeg[s.start + 4..s.end]
}

fn is_xmp(jpeg: &[u8], s: &Segment) -> bool {
    s.marker == 0xE1 && payload(jpeg, s).starts_with(XMP_HEADER)
}

/// The XMP packet (UTF-8 text) of a JPEG held in memory.
pub fn extract(jpeg: &[u8]) -> Option<String> {
    let segs = segments(jpeg)?;
    let s = segs.iter().find(|s| is_xmp(jpeg, s))?;
    let p = &payload(jpeg, s)[XMP_HEADER.len()..];
    Some(String::from_utf8_lossy(p).into_owned())
}

/// Reads the XMP of a JPEG file without loading the pixel data.
pub fn read_file(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut soi = [0u8; 2];
    f.read_exact(&mut soi).ok()?;
    if soi != [0xFF, 0xD8] {
        return None;
    }
    loop {
        let mut m = [0u8; 2];
        f.read_exact(&mut m).ok()?;
        if m[0] != 0xFF {
            return None;
        }
        let marker = m[1];
        if marker == 0xFF {
            f.seek(SeekFrom::Current(-1)).ok()?;
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            continue;
        }
        let mut l = [0u8; 2];
        f.read_exact(&mut l).ok()?;
        let len = u16::from_be_bytes(l) as usize;
        if len < 2 {
            return None;
        }
        let body = len - 2;
        if marker == 0xE1 && body >= XMP_HEADER.len() {
            let mut buf = vec![0u8; body];
            f.read_exact(&mut buf).ok()?;
            if buf.starts_with(XMP_HEADER) {
                return Some(String::from_utf8_lossy(&buf[XMP_HEADER.len()..]).into_owned());
            }
        } else {
            f.seek(SeekFrom::Current(body as i64)).ok()?;
        }
    }
}

/// `jpeg` with its XMP segment replaced by `xmp` (inserted after the leading APPn segments
/// when there is none). Everything else is byte-identical. `None` when `jpeg` is not a valid
/// JPEG header or the packet does not fit one segment.
pub fn replace(jpeg: &[u8], xmp: &str) -> Option<Vec<u8>> {
    if xmp.len() > MAX_XMP {
        return None;
    }
    let segs = segments(jpeg)?;
    let mut seg = vec![0xFF, 0xE1];
    seg.extend(((XMP_HEADER.len() + xmp.len() + 2) as u16).to_be_bytes());
    seg.extend_from_slice(XMP_HEADER);
    seg.extend_from_slice(xmp.as_bytes());
    let (cut_start, cut_end) = match segs.iter().find(|s| is_xmp(jpeg, s)) {
        Some(s) => (s.start, s.end),
        None => {
            // after SOI and the JFIF/EXIF/ICC style APPn segments, before everything else; the
            // extended-XMP chunks must follow the main packet, so go before them if present
            let mut at = 2;
            for s in &segs {
                let ext = s.marker == 0xE1 && payload(jpeg, s).starts_with(EXT_HEADER);
                if (0xE0..=0xEF).contains(&s.marker) && !ext {
                    at = s.end;
                } else {
                    break;
                }
            }
            (at, at)
        }
    };
    let mut out = Vec::with_capacity(jpeg.len() + seg.len());
    out.extend_from_slice(&jpeg[..cut_start]);
    out.extend_from_slice(&seg);
    out.extend_from_slice(&jpeg[cut_end..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SOI, APP0, APP1 Exif, DQT, SOS + entropy data (with a 0xFF00 stuffed byte), EOI.
    fn sample(with_xmp: Option<&str>) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        v.extend([0xFF, 0xE0, 0x00, 0x10]);
        v.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        let exif = b"Exif\0\0MM\0*\0\0\0\x08\0\0";
        v.extend([0xFF, 0xE1]);
        v.extend(((exif.len() + 2) as u16).to_be_bytes());
        v.extend(exif);
        if let Some(x) = with_xmp {
            v.extend([0xFF, 0xE1]);
            v.extend(((XMP_HEADER.len() + x.len() + 2) as u16).to_be_bytes());
            v.extend(XMP_HEADER);
            v.extend(x.as_bytes());
        }
        v.extend([0xFF, 0xDB, 0x00, 0x05, 1, 2, 3]);
        v.extend([0xFF, 0xDA, 0x00, 0x04, 0x01, 0x02]);
        v.extend([0x12, 0x34, 0xFF, 0x00, 0x56, 0xFF, 0xE1, 0x99, 0xFF, 0xD9]);
        v
    }

    #[test]
    fn replaces_only_the_xmp_segment() {
        let old = sample(Some("<old/>"));
        let new = replace(&old, "<new>xmp</new>").unwrap();
        assert_eq!(extract(&new).as_deref(), Some("<new>xmp</new>"));
        let tail = |v: &[u8]| v[v.len() - 14..].to_vec();
        assert_eq!(tail(&old), tail(&new), "scan data untouched");
        // everything before the segment is identical
        let cut = 2 + 18 + 2 + 2 + 16;
        assert_eq!(old[..cut], new[..cut]);
        // exactly one XMP segment
        let segs = segments(&new).unwrap();
        assert_eq!(
            segs.iter().filter(|s| is_xmp(&new, s)).count(),
            1,
            "{segs:?}"
        );
    }

    #[test]
    fn inserts_after_the_leading_app_segments() {
        let old = sample(None);
        assert!(extract(&old).is_none());
        let new = replace(&old, "<x/>").unwrap();
        assert_eq!(extract(&new).as_deref(), Some("<x/>"));
        let segs = segments(&new).unwrap();
        let order: Vec<u8> = segs.iter().map(|s| s.marker).collect();
        assert_eq!(order, [0xE0, 0xE1, 0xE1, 0xDB]);
        // removing the inserted segment gives the original back
        let s = segs.iter().find(|s| is_xmp(&new, s)).unwrap();
        let mut back = new.clone();
        back.drain(s.start..s.end);
        assert_eq!(back, old);
    }

    #[test]
    fn rejects_non_jpeg_and_oversized_packets() {
        assert!(replace(b"\x89PNG....", "<x/>").is_none());
        assert!(replace(&sample(None), &"x".repeat(70_000)).is_none());
        assert!(segments(&[0xFF, 0xD8, 0xFF]).is_none());
        // a truncated segment length
        assert!(segments(&[0xFF, 0xD8, 0xFF, 0xE1, 0xFF, 0xFF, 0, 0]).is_none());
    }

    #[test]
    fn reads_from_a_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.jpg");
        std::fs::write(&p, sample(Some("<in-file/>"))).unwrap();
        assert_eq!(read_file(&p).as_deref(), Some("<in-file/>"));
        std::fs::write(&p, sample(None)).unwrap();
        assert!(read_file(&p).is_none());
        std::fs::write(&p, b"not a jpeg").unwrap();
        assert!(read_file(&p).is_none());
    }
}
