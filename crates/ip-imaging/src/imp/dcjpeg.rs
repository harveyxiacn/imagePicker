//! DC-only (1/8 scale) decoder for baseline/extended-sequential Huffman JPEGs.
//!
//! The DC coefficient of a block is 8x the block mean, so a 1/8 downscale is exact box
//! filtering for free. AC coefficients are entropy-skipped (code looked up, value bits dropped),
//! no IDCT and no coefficient storage -> ~3x faster than a general decoder at 1/8 scale.
//! Returns `None` for anything unusual (progressive, 12-bit, CMYK, multi-scan, truncated data);
//! callers fall back to a general decoder.

const LUT_BITS: u32 = 10;

struct Huff {
    lut: Vec<u16>,
    max_code: [i32; 17],
    first_code: [i32; 17],
    first_idx: [i32; 17],
    vals: Vec<u8>,
}

impl Huff {
    fn build(counts: &[u8; 16], vals: &[u8]) -> Option<Self> {
        let total: usize = counts.iter().map(|&c| c as usize).sum();
        if total > vals.len() || total > 256 {
            return None;
        }
        let mut h = Huff {
            lut: vec![0; 1 << LUT_BITS],
            max_code: [-1; 17],
            first_code: [0; 17],
            first_idx: [0; 17],
            vals: vals[..total].to_vec(),
        };
        let mut code = 0u32;
        let mut k = 0usize;
        for l in 1..=16usize {
            h.first_code[l] = code as i32;
            h.first_idx[l] = k as i32;
            for _ in 0..counts[l - 1] {
                if l as u32 <= LUT_BITS {
                    let prefix = (code << (LUT_BITS - l as u32)) as usize;
                    let e = ((l as u16) << 8) | vals[k] as u16;
                    for j in 0..(1usize << (LUT_BITS - l as u32)) {
                        *h.lut.get_mut(prefix | j)? = e;
                    }
                }
                code += 1;
                k += 1;
            }
            h.max_code[l] = if counts[l - 1] > 0 {
                code as i32 - 1
            } else {
                -1
            };
            if code > (1 << l) {
                return None;
            }
            code <<= 1;
        }
        Some(h)
    }
}

struct Br<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
    hit: bool,
    /// Number of synthetic (zero) bits at the tail of `buf`.
    pad: u32,
}

impl<'a> Br<'a> {
    fn new(d: &'a [u8], pos: usize) -> Self {
        Br {
            d,
            pos,
            buf: 0,
            n: 0,
            hit: false,
            pad: 0,
        }
    }

    #[inline(always)]
    fn refill(&mut self) {
        while self.n <= 56 {
            let mut b = 0u8;
            if !self.hit && self.pos < self.d.len() {
                let x = self.d[self.pos];
                if x == 0xFF {
                    match self.d.get(self.pos + 1) {
                        Some(0) => {
                            self.pos += 2;
                            b = 0xFF;
                        }
                        _ => {
                            self.hit = true;
                            self.pad += 8;
                        }
                    }
                } else {
                    self.pos += 1;
                    b = x;
                }
            } else {
                self.hit = true;
                self.pad += 8;
            }
            self.buf |= (b as u64) << (56 - self.n);
            self.n += 8;
        }
    }

    #[inline(always)]
    fn skip(&mut self, k: u32) {
        self.buf <<= k;
        self.n -= k;
    }

    #[inline(always)]
    fn get(&mut self, k: u32) -> i32 {
        if k == 0 {
            return 0;
        }
        let v = (self.buf >> (64 - k)) as i32;
        self.skip(k);
        v
    }

    #[inline(always)]
    fn sym(&mut self, h: &Huff) -> Option<u8> {
        if self.n < 32 {
            self.refill();
        }
        let e = h.lut[(self.buf >> (64 - LUT_BITS)) as usize];
        if e != 0 {
            self.skip((e >> 8) as u32);
            return Some(e as u8);
        }
        let v = (self.buf >> 48) as i32;
        for l in (LUT_BITS as usize + 1)..=16 {
            let c = v >> (16 - l);
            if h.max_code[l] >= 0 && c <= h.max_code[l] && c >= h.first_code[l] {
                self.skip(l as u32);
                return h
                    .vals
                    .get((h.first_idx[l] + c - h.first_code[l]) as usize)
                    .copied();
            }
        }
        None
    }

    fn truncated(&self) -> bool {
        self.n < self.pad
    }

    /// Move past the next RSTn marker and reset state.
    fn restart(&mut self) -> bool {
        let mut p = self.pos;
        loop {
            let Some(&b) = self.d.get(p) else {
                return false;
            };
            if b != 0xFF {
                p += 1;
                continue;
            }
            match self.d.get(p + 1) {
                Some(0) | Some(0xFF) => p += if self.d[p + 1] == 0 { 2 } else { 1 },
                Some(m) if (0xD0..=0xD7).contains(m) => {
                    self.pos = p + 2;
                    self.buf = 0;
                    self.n = 0;
                    self.hit = false;
                    self.pad = 0;
                    return true;
                }
                _ => return false,
            }
        }
    }
}

struct Comp {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    td: usize,
    ta: usize,
}

/// Returns `(width, height, rgb)` at 1/8 scale (`ceil(W/8) x ceil(H/8)`).
pub fn decode_eighth(data: &[u8]) -> Option<(u32, u32, Vec<u8>)> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return None;
    }
    let mut q0 = [0i32; 4];
    let mut dc_t: [Option<Huff>; 4] = [None, None, None, None];
    let mut ac_t: [Option<Huff>; 4] = [None, None, None, None];
    let mut comps: Vec<Comp> = Vec::new();
    let (mut width, mut height) = (0usize, 0usize);
    let mut ri = 0usize;
    let mut adobe_transform: Option<u8> = None;
    let mut p = 2usize;
    let scan_start = loop {
        if p + 4 > data.len() {
            return None;
        }
        if data[p] != 0xFF {
            p += 1;
            continue;
        }
        let m = data[p + 1];
        if m == 0xFF {
            p += 1;
            continue;
        }
        if m == 0 || m == 1 || (0xD0..=0xD8).contains(&m) {
            p += 2;
            continue;
        }
        let len = u16::from_be_bytes([data[p + 2], data[p + 3]]) as usize;
        let seg = data.get(p + 4..p + 2 + len)?;
        match m {
            0xDB => {
                let mut s = seg;
                while !s.is_empty() {
                    let (pq, tq) = (s[0] >> 4, (s[0] & 15) as usize);
                    let sz = if pq == 0 { 64 } else { 128 };
                    if tq > 3 || s.len() < 1 + sz {
                        return None;
                    }
                    q0[tq] = if pq == 0 {
                        s[1] as i32
                    } else {
                        u16::from_be_bytes([s[1], s[2]]) as i32
                    };
                    s = &s[1 + sz..];
                }
            }
            0xC4 => {
                let mut s = seg;
                while s.len() >= 17 {
                    let (tc, th) = (s[0] >> 4, (s[0] & 15) as usize);
                    let counts: [u8; 16] = s[1..17].try_into().ok()?;
                    let total: usize = counts.iter().map(|&c| c as usize).sum();
                    let vals = s.get(17..17 + total)?;
                    if th > 3 || tc > 1 {
                        return None;
                    }
                    let h = Huff::build(&counts, vals)?;
                    if tc == 0 {
                        dc_t[th] = Some(h);
                    } else {
                        ac_t[th] = Some(h);
                    }
                    s = &s[17 + total..];
                }
            }
            0xC0 | 0xC1 => {
                if seg.len() < 6 || seg[0] != 8 || !comps.is_empty() {
                    return None;
                }
                height = u16::from_be_bytes([seg[1], seg[2]]) as usize;
                width = u16::from_be_bytes([seg[3], seg[4]]) as usize;
                let nc = seg[5] as usize;
                if (nc != 1 && nc != 3) || seg.len() < 6 + nc * 3 || width == 0 || height == 0 {
                    return None;
                }
                for i in 0..nc {
                    let c = &seg[6 + i * 3..9 + i * 3];
                    let (h, v) = ((c[1] >> 4) as usize, (c[1] & 15) as usize);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) || c[2] > 3 {
                        return None;
                    }
                    comps.push(Comp {
                        id: c[0],
                        h,
                        v,
                        tq: c[2] as usize,
                        td: 0,
                        ta: 0,
                    });
                }
            }
            0xC2 | 0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return None,
            0xDD => {
                if seg.len() < 2 {
                    return None;
                }
                ri = u16::from_be_bytes([seg[0], seg[1]]) as usize;
            }
            0xEE if seg.len() >= 12 && &seg[..5] == b"Adobe" => adobe_transform = Some(seg[11]),
            0xDA => {
                if comps.is_empty()
                    || seg.is_empty()
                    || seg[0] as usize != comps.len()
                    || seg.len() < 1 + comps.len() * 2 + 3
                {
                    return None;
                }
                for i in 0..comps.len() {
                    let id = seg[1 + i * 2];
                    let t = seg[2 + i * 2];
                    let c = comps.iter_mut().find(|c| c.id == id)?;
                    c.td = (t >> 4) as usize;
                    c.ta = (t & 15) as usize;
                    if c.td > 3 || c.ta > 3 {
                        return None;
                    }
                }
                break p + 2 + len;
            }
            _ => {}
        }
        p += 2 + len;
    };

    let nc = comps.len();
    if nc == 1 {
        comps[0].h = 1;
        comps[0].v = 1;
    }
    let hmax = comps.iter().map(|c| c.h).max()?;
    let vmax = comps.iter().map(|c| c.v).max()?;
    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    let planes_w: Vec<usize> = comps.iter().map(|c| mcux * c.h).collect();
    let planes_h: Vec<usize> = comps.iter().map(|c| mcuy * c.v).collect();
    let mut planes: Vec<Vec<u8>> = (0..nc)
        .map(|i| vec![0u8; planes_w[i] * planes_h[i]])
        .collect();
    for c in &comps {
        dc_t[c.td].as_ref()?;
        ac_t[c.ta].as_ref()?;
        if q0[c.tq] == 0 {
            return None;
        }
    }

    let mut br = Br::new(data, scan_start);
    let mut pred = [0i32; 4];
    let mut mcu_count = 0usize;
    for my in 0..mcuy {
        for mx in 0..mcux {
            if ri > 0 && mcu_count > 0 && mcu_count.is_multiple_of(ri) {
                if !br.restart() {
                    return None;
                }
                pred = [0; 4];
            }
            mcu_count += 1;
            for (ci, c) in comps.iter().enumerate() {
                let dct = dc_t[c.td].as_ref()?;
                let act = ac_t[c.ta].as_ref()?;
                let q = q0[c.tq];
                for j in 0..c.v {
                    for i in 0..c.h {
                        // DC
                        let s = br.sym(dct)? as u32;
                        if s > 15 {
                            return None;
                        }
                        let diff = if s == 0 {
                            0
                        } else {
                            let v = br.get(s);
                            if v < (1 << (s - 1)) {
                                v - (1 << s) + 1
                            } else {
                                v
                            }
                        };
                        pred[ci] += diff;
                        // AC: skip
                        let mut k = 1;
                        while k < 64 {
                            let rs = br.sym(act)?;
                            let r = (rs >> 4) as usize;
                            let sz = (rs & 15) as u32;
                            if sz == 0 {
                                if r == 15 {
                                    k += 16;
                                    continue;
                                }
                                break;
                            }
                            k += r + 1;
                            if br.n < sz {
                                br.refill();
                            }
                            br.skip(sz);
                        }
                        let val = ((pred[ci] * q + 4) >> 3) + 128;
                        let bx = mx * c.h + i;
                        let by = my * c.v + j;
                        planes[ci][by * planes_w[ci] + bx] = val.clamp(0, 255) as u8;
                    }
                }
            }
        }
        if br.truncated() {
            return None;
        }
    }

    let ow = width.div_ceil(8);
    let oh = height.div_ceil(8);
    let mut out = vec![0u8; ow * oh * 3];
    let xmap: Vec<Vec<usize>> = comps
        .iter()
        .map(|c| (0..ow).map(|x| x * c.h / hmax).collect())
        .collect();
    let rgb_mode = nc == 3
        && (adobe_transform == Some(0)
            || (comps[0].id == b'R' && comps[1].id == b'G' && comps[2].id == b'B'));
    for y in 0..oh {
        let rows: Vec<usize> = comps
            .iter()
            .enumerate()
            .map(|(i, c)| (y * c.v / vmax) * planes_w[i])
            .collect();
        let o = &mut out[y * ow * 3..(y + 1) * ow * 3];
        if nc == 1 {
            for x in 0..ow {
                let g = planes[0][rows[0] + x];
                o[x * 3..x * 3 + 3].copy_from_slice(&[g, g, g]);
            }
        } else {
            for x in 0..ow {
                let a = planes[0][rows[0] + xmap[0][x]] as i32;
                let b = planes[1][rows[1] + xmap[1][x]] as i32;
                let c = planes[2][rows[2] + xmap[2][x]] as i32;
                let px = &mut o[x * 3..x * 3 + 3];
                if rgb_mode {
                    px.copy_from_slice(&[a as u8, b as u8, c as u8]);
                } else {
                    let (cb, cr) = (b - 128, c - 128);
                    px[0] = (a + ((91_881 * cr + 32_768) >> 16)).clamp(0, 255) as u8;
                    px[1] = (a - ((22_554 * cb + 46_802 * cr + 32_768) >> 16)).clamp(0, 255) as u8;
                    px[2] = (a + ((116_130 * cb + 32_768) >> 16)).clamp(0, 255) as u8;
                }
            }
        }
    }
    Some((ow as u32, oh as u32, out))
}
