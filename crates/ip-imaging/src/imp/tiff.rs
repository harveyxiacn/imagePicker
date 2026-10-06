//! Minimal, bounds-checked TIFF/EXIF IFD reader (also handles ORF/RW2 header variants).

pub const TAG_IMAGE_WIDTH: u16 = 0x0100;
pub const TAG_IMAGE_LENGTH: u16 = 0x0101;
pub const TAG_COMPRESSION: u16 = 0x0103;
pub const TAG_MAKE: u16 = 0x010F;
pub const TAG_MODEL: u16 = 0x0110;
pub const TAG_STRIP_OFFSETS: u16 = 0x0111;
pub const TAG_ORIENTATION: u16 = 0x0112;
pub const TAG_STRIP_BYTES: u16 = 0x0117;
pub const TAG_SUB_IFDS: u16 = 0x014A;
pub const TAG_JPEG_OFFSET: u16 = 0x0201;
pub const TAG_JPEG_LEN: u16 = 0x0202;
pub const TAG_EXIF_IFD: u16 = 0x8769;
pub const TAG_GPS_IFD: u16 = 0x8825;
/// Panasonic RW2 JpgFromRaw.
pub const TAG_RW2_JPG: u16 = 0x002E;

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// Position in the TIFF buffer where the value bytes live.
    pub pos: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Ifd {
    pub entries: Vec<Entry>,
}

impl Ifd {
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }
}

pub struct Tiff<'a> {
    pub data: &'a [u8],
    pub le: bool,
}

fn type_size(t: u16) -> usize {
    match t {
        1 | 2 | 6 | 7 => 1,
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 0,
    }
}

impl<'a> Tiff<'a> {
    /// `data` must start at the TIFF header.
    pub fn new(data: &'a [u8]) -> Option<Self> {
        if data.len() < 8 {
            return None;
        }
        let le = match &data[0..2] {
            b"II" => true,
            b"MM" => false,
            _ => return None,
        };
        let t = Tiff { data, le };
        let magic = t.u16(2)?;
        // 42 = TIFF, 0x4F52/0x5352 = Olympus ORF, 0x55 = Panasonic RW2.
        if !matches!(magic, 42 | 0x4F52 | 0x5352 | 0x55) {
            return None;
        }
        Some(t)
    }

    pub fn u16(&self, pos: usize) -> Option<u16> {
        let b = self.data.get(pos..pos.checked_add(2)?)?;
        let a = [b[0], b[1]];
        Some(if self.le {
            u16::from_le_bytes(a)
        } else {
            u16::from_be_bytes(a)
        })
    }

    pub fn u32(&self, pos: usize) -> Option<u32> {
        let b = self.data.get(pos..pos.checked_add(4)?)?;
        let a = [b[0], b[1], b[2], b[3]];
        Some(if self.le {
            u32::from_le_bytes(a)
        } else {
            u32::from_be_bytes(a)
        })
    }

    pub fn first_ifd_offset(&self) -> Option<usize> {
        Some(self.u32(4)? as usize)
    }

    pub fn read_ifd(&self, off: usize) -> Option<(Ifd, usize)> {
        let n = self.u16(off)? as usize;
        if n > 4096 {
            return None;
        }
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let p = off + 2 + i * 12;
            let tag = self.u16(p)?;
            let typ = self.u16(p + 2)?;
            let count = self.u32(p + 4)?;
            let sz = type_size(typ).checked_mul(count as usize)?;
            let pos = if sz <= 4 {
                p + 8
            } else {
                self.u32(p + 8)? as usize
            };
            if sz > 0 && pos.checked_add(sz)? > self.data.len() {
                continue;
            }
            entries.push(Entry {
                tag,
                typ,
                count,
                pos,
            });
        }
        let next = self.u32(off + 2 + n * 12).unwrap_or(0) as usize;
        Some((Ifd { entries }, next))
    }

    /// Unsigned integer at index `i` (SHORT/LONG/BYTE).
    pub fn uint(&self, e: &Entry, i: usize) -> Option<u32> {
        if i >= e.count as usize {
            return None;
        }
        match e.typ {
            1 | 7 => self.data.get(e.pos + i).map(|&b| b as u32),
            3 => self.u16(e.pos + i * 2).map(u32::from),
            4 | 13 => self.u32(e.pos + i * 4),
            _ => None,
        }
    }

    pub fn first_uint(&self, e: &Entry) -> Option<u32> {
        self.uint(e, 0)
    }

    pub fn rational(&self, e: &Entry, i: usize) -> Option<f64> {
        if i >= e.count as usize {
            return None;
        }
        match e.typ {
            5 => {
                let n = self.u32(e.pos + i * 8)? as f64;
                let d = self.u32(e.pos + i * 8 + 4)? as f64;
                (d != 0.0).then(|| n / d)
            }
            10 => {
                let n = self.u32(e.pos + i * 8)? as i32 as f64;
                let d = self.u32(e.pos + i * 8 + 4)? as i32 as f64;
                (d != 0.0).then(|| n / d)
            }
            3 | 4 => self.uint(e, i).map(f64::from),
            _ => None,
        }
    }

    pub fn ascii(&self, e: &Entry) -> Option<String> {
        if e.typ != 2 && e.typ != 7 && e.typ != 1 {
            return None;
        }
        let b = self.data.get(e.pos..e.pos.checked_add(e.count as usize)?)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        let s = String::from_utf8_lossy(&b[..end]).trim().to_string();
        (!s.is_empty()).then_some(s)
    }

    /// Walk the IFD0 chain plus all SubIFD / Exif / GPS children.
    pub fn walk(&self) -> Walked {
        let mut w = Walked::default();
        let mut visited = Vec::new();
        let Some(mut off) = self.first_ifd_offset() else {
            return w;
        };
        for _ in 0..8 {
            if off == 0 || visited.contains(&off) {
                break;
            }
            visited.push(off);
            let Some((ifd, next)) = self.read_ifd(off) else {
                break;
            };
            w.chain.push(ifd);
            off = next;
        }
        for ci in 0..w.chain.len() {
            let ifd = w.chain[ci].clone();
            self.collect_children(&ifd, ci == 0, &mut w, &mut visited, 0);
        }
        w
    }

    fn collect_children(
        &self,
        ifd: &Ifd,
        top: bool,
        w: &mut Walked,
        visited: &mut Vec<usize>,
        depth: u8,
    ) {
        if depth > 3 {
            return;
        }
        for e in &ifd.entries {
            if !matches!(e.tag, TAG_EXIF_IFD | TAG_GPS_IFD | TAG_SUB_IFDS) {
                continue;
            }
            for i in 0..(e.count.min(8) as usize) {
                let Some(off) = self.uint(e, i) else { continue };
                let off = off as usize;
                if off == 0 || visited.contains(&off) {
                    continue;
                }
                visited.push(off);
                let Some((child, _)) = self.read_ifd(off) else {
                    continue;
                };
                match e.tag {
                    TAG_EXIF_IFD if top && w.exif.is_none() => w.exif = Some(child.clone()),
                    TAG_GPS_IFD if top && w.gps.is_none() => w.gps = Some(child.clone()),
                    _ => w.subs.push(child.clone()),
                }
                self.collect_children(&child, false, w, visited, depth + 1);
            }
        }
    }

    /// All IFDs (chain + children).
    pub fn all_ifds<'w>(&self, w: &'w Walked) -> impl Iterator<Item = &'w Ifd> {
        w.chain.iter().chain(w.exif.iter()).chain(w.subs.iter())
    }
}

#[derive(Default, Clone)]
pub struct Walked {
    pub chain: Vec<Ifd>,
    pub exif: Option<Ifd>,
    pub gps: Option<Ifd>,
    pub subs: Vec<Ifd>,
}
