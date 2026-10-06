//! A tiny deterministic stand-in for the real renderer (tests only; feature `testutil`).
//!
//! It honours crop rects, global exposure / temp / tint (per-channel gains in linear light),
//! AI/radial masks on local exposure, and `max_long_edge`; it asks the LUT provider for every
//! LUT op (so provider errors surface) but does not apply it. Enough for pixels to depend on
//! the stack, which is what the API and sync tests need.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

use ip_render::{Backend, EditStack, MaskRef, Op, RenderRequest, Renderer, Result, RgbImage};

pub struct FakeRenderer {
    pub backend: Backend,
    pub calls: AtomicUsize,
    /// Dimensions of the source of the last call.
    pub last_source: Mutex<Option<(u32, u32)>>,
    pub luts_seen: Mutex<Vec<String>>,
    /// `person_id`s `MaskProvider::people()` returned for each beauty/warp op rendered.
    pub people_seen: Mutex<Vec<Vec<Option<i64>>>>,
    pub fail: AtomicBool,
}

impl Default for FakeRenderer {
    fn default() -> Self {
        Self::new(Backend::Cpu)
    }
}

impl FakeRenderer {
    pub fn new(backend: Backend) -> Self {
        Self {
            backend,
            calls: AtomicUsize::new(0),
            last_source: Mutex::new(None),
            luts_seen: Mutex::new(Vec::new()),
            people_seen: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        }
    }
}

fn to_linear(v: u8) -> f32 {
    let c = v as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn to_srgb(l: f32) -> u8 {
    let l = l.clamp(0.0, 1.0);
    let c = if l <= 0.0031308 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    };
    (c * 255.0 + 0.5) as u8
}

/// Per-channel linear gains of an adjustment: exposure in EV, temperature warms red / cools
/// blue, tint trades green against magenta.
fn gains(exposure: f32, temp: f32, tint: f32) -> [f32; 3] {
    let g = 2f32.powf(exposure);
    [
        g * 2f32.powf(temp / 3000.0 * 0.4),
        g * 2f32.powf(-tint / 100.0 * 0.2),
        g * 2f32.powf(-temp / 3000.0 * 0.4),
    ]
}

impl Renderer for FakeRenderer {
    fn backend(&self) -> Backend {
        self.backend
    }

    fn render(&self, req: &RenderRequest<'_>) -> Result<RgbImage> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            anyhow::bail!("fake renderer failure");
        }
        let src = req.source;
        *self.last_source.lock().unwrap() = Some((src.width, src.height));
        let stack: &EditStack = req.stack;

        // crop
        let (mut x0, mut y0, mut cw, mut ch) = (0u32, 0u32, src.width, src.height);
        for op in &stack.ops {
            if let Op::Crop(c) = op {
                x0 = ((c.rect[0] * src.width as f32).round() as u32).min(src.width - 1);
                y0 = ((c.rect[1] * src.height as f32).round() as u32).min(src.height - 1);
                cw = ((c.rect[2] * src.width as f32).round() as u32).clamp(1, src.width - x0);
                ch = ((c.rect[3] * src.height as f32).round() as u32).clamp(1, src.height - y0);
                break;
            }
        }
        let (mut g, mut locals) = ([1.0f32; 3], Vec::new());
        for op in &stack.ops {
            match op {
                Op::Global(a) => {
                    let k = gains(a.exposure, a.temp, a.tint);
                    for i in 0..3 {
                        g[i] *= k[i];
                    }
                }
                Op::Local(l) => {
                    let mask = match &l.mask {
                        MaskRef::Ai { target, person_id } => req.masks.mask(*target, *person_id)?,
                        MaskRef::Radial { .. } | MaskRef::Linear { .. } => None,
                    };
                    if let Some(m) = mask {
                        locals.push((m, l.amount, l.invert, l.adjust.exposure));
                    }
                }
                Op::Beauty(b) => {
                    // asks for the geometry (so provider errors surface); "whiten" brightens
                    // everything when somebody it targets is in the photo
                    let people = req.masks.people()?;
                    self.people_seen
                        .lock()
                        .unwrap()
                        .push(people.iter().map(|p| p.person_id).collect());
                    let hit = people
                        .iter()
                        .any(|p| b.person_id.is_none() || p.person_id == b.person_id);
                    if hit {
                        for c in &mut g {
                            *c *= 1.0 + b.whiten / 500.0;
                        }
                    }
                }
                Op::Warp(_) => {
                    let people = req.masks.people()?;
                    self.people_seen
                        .lock()
                        .unwrap()
                        .push(people.iter().map(|p| p.person_id).collect());
                }
                Op::Lut(l) => {
                    self.luts_seen.lock().unwrap().push(l.file.clone());
                    let _ = req.luts.lut(&l.file)?;
                }
                _ => {}
            }
        }
        let mut data = Vec::with_capacity((cw * ch * 3) as usize);
        for y in 0..ch {
            for x in 0..cw {
                let i = (((y0 + y) * src.width + x0 + x) * 3) as usize;
                let mut k = g;
                for (m, amount, invert, ev) in &locals {
                    let mx = (((x as f32 + 0.5) / cw as f32) * m.width as f32) as u32;
                    let my = (((y as f32 + 0.5) / ch as f32) * m.height as f32) as u32;
                    let v = m.data[(my.min(m.height - 1) * m.width + mx.min(m.width - 1)) as usize]
                        as f32
                        / 255.0;
                    let w = if *invert { 1.0 - v } else { v } * amount;
                    let f = 2f32.powf(ev * w);
                    for c in &mut k {
                        *c *= f;
                    }
                }
                for (c, gain) in k.iter().enumerate() {
                    data.push(to_srgb(to_linear(src.data[i + c]) * gain));
                }
            }
        }
        let mut out = RgbImage {
            width: cw,
            height: ch,
            data,
        };
        if let Some(max) = req.max_long_edge {
            let long = out.width.max(out.height);
            if long > max && max > 0 {
                let s = max as f32 / long as f32;
                let nw = ((out.width as f32 * s).round() as u32).max(1);
                let nh = ((out.height as f32 * s).round() as u32).max(1);
                let mut d = Vec::with_capacity((nw * nh * 3) as usize);
                for y in 0..nh {
                    for x in 0..nw {
                        let sx = (x * out.width / nw).min(out.width - 1);
                        let sy = (y * out.height / nh).min(out.height - 1);
                        let i = ((sy * out.width + sx) * 3) as usize;
                        d.extend_from_slice(&out.data[i..i + 3]);
                    }
                }
                out = RgbImage {
                    width: nw,
                    height: nh,
                    data: d,
                };
            }
        }
        Ok(out)
    }
}
