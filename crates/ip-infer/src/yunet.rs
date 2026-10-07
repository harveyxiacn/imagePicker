//! YuNet decoding, ported from OpenCV `FaceDetectorYNImpl` (modules/objdetect/src/face_detect.cpp).

use std::path::Path;
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use ort::session::Session;
use ort::value::Tensor;

const STRIDES: [usize; 3] = [8, 16, 32];

/// One detected face. Everything is normalised to the input image (`bbox` = `[x, y, w, h]`).
#[derive(Debug, Clone, PartialEq)]
pub struct Detection {
    pub bbox: [f32; 4],
    pub score: f32,
    /// Right eye, left eye, nose tip, right mouth corner, left mouth corner (YuNet order; "right"
    /// is the subject's right, i.e. image left).
    pub landmarks: [[f32; 2]; 5],
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub score_threshold: f32,
    pub nms_threshold: f32,
    pub top_k: usize,
}

impl Default for Options {
    /// Same values as the Python worker's `FaceAnalyzer`.
    fn default() -> Self {
        Self {
            score_threshold: 0.7,
            nms_threshold: 0.3,
            top_k: 5000,
        }
    }
}

pub struct FaceDetector {
    // `Session::run` needs `&mut`; detection is cheap enough to serialise per detector.
    session: Mutex<Session>,
    input: String,
    /// `(height, width)` when the model input is static (the OpenCV Zoo 2023mar model is 640x640);
    /// `None` for dynamic models, which are fed the image padded to a multiple of 32 like OpenCV.
    fixed: Option<(usize, usize)>,
    opts: Options,
}

impl FaceDetector {
    pub fn load(model_path: impl AsRef<Path>) -> Result<Self> {
        Self::load_with(model_path, Options::default())
    }

    pub fn load_with(model_path: impl AsRef<Path>, opts: Options) -> Result<Self> {
        let path = model_path.as_ref();
        // CPU execution provider only (the ort default). Add NNAPI / XNNPACK here later.
        let session = Session::builder()
            .map_err(|e| anyhow!("ort session builder: {e}"))?
            .with_intra_threads(1)
            .map_err(|e| anyhow!("ort threads: {e}"))?
            .commit_from_file(path)
            .map_err(|e| anyhow!("load {}: {e}", path.display()))?;
        let input = session
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .context("model has no inputs")?;
        let fixed = session
            .inputs()
            .first()
            .and_then(|i| i.dtype().tensor_shape().map(|s| s.to_vec()))
            .and_then(|d| match d.as_slice() {
                [_, 3, h, w] if *h > 0 && *w > 0 => Some((*h as usize, *w as usize)),
                _ => None,
            });
        Ok(Self {
            session: Mutex::new(session),
            input,
            fixed,
            opts,
        })
    }

    /// Detects faces in an upright RGB8 image. Sorted by descending score; empty on failure.
    pub fn detect(&self, rgb: &[u8], width: usize, height: usize) -> Vec<Detection> {
        self.try_detect(rgb, width, height).unwrap_or_default()
    }

    pub fn try_detect(&self, rgb: &[u8], width: usize, height: usize) -> Result<Vec<Detection>> {
        if width == 0 || height == 0 || rgb.len() != width * height * 3 {
            return Err(anyhow!("bad rgb buffer for {width}x{height}"));
        }
        // Network input: fixed-size models get the image scaled to fit and padded bottom/right
        // with zeros (scale `sc`); dynamic ones get it unscaled, padded to a multiple of 32
        // (what OpenCV's `FaceDetectorYN` does).
        let (ph, pw, sc) = match self.fixed {
            Some((fh, fw)) => (
                fh,
                fw,
                (fw as f32 / width as f32).min(fh as f32 / height as f32),
            ),
            None => (height.div_ceil(32) * 32, width.div_ceil(32) * 32, 1.0),
        };
        let (dw, dh) = (
            ((width as f32 * sc).round() as usize).clamp(1, pw),
            ((height as f32 * sc).round() as usize).clamp(1, ph),
        );
        let mut blob = vec![0f32; 3 * pw * ph];
        let plane = pw * ph;
        let (xr, yr) = (width as f32 / dw as f32, height as f32 / dh as f32);
        for y in 0..dh {
            // bilinear, half-pixel centres (cv2.resize INTER_LINEAR)
            let fy = ((y as f32 + 0.5) * yr - 0.5).clamp(0.0, (height - 1) as f32);
            let (y0, ty) = (fy as usize, fy.fract());
            let y1 = (y0 + 1).min(height - 1);
            for x in 0..dw {
                let fx = ((x as f32 + 0.5) * xr - 0.5).clamp(0.0, (width - 1) as f32);
                let (x0, tx) = (fx as usize, fx.fract());
                let x1 = (x0 + 1).min(width - 1);
                let o = y * pw + x;
                for ch in 0..3 {
                    let px = |yy: usize, xx: usize| rgb[(yy * width + xx) * 3 + ch] as f32;
                    let top = px(y0, x0) * (1.0 - tx) + px(y0, x1) * tx;
                    let bot = px(y1, x0) * (1.0 - tx) + px(y1, x1) * tx;
                    // BGR plane order
                    blob[(2 - ch) * plane + o] = top * (1.0 - ty) + bot * ty;
                }
            }
        }
        let tensor = Tensor::from_array(([1usize, 3, ph, pw], blob))
            .map_err(|e| anyhow!("input tensor: {e}"))?;
        let mut session = self
            .session
            .lock()
            .map_err(|_| anyhow!("session poisoned"))?;
        let outputs = session
            .run(ort::inputs![self.input.as_str() => tensor])
            .map_err(|e| anyhow!("inference: {e}"))?;
        let get = |name: String| -> Result<Vec<f32>> {
            let (_, data) = outputs[name.as_str()]
                .try_extract_tensor::<f32>()
                .map_err(|e| anyhow!("output {name}: {e}"))?;
            Ok(data.to_vec())
        };
        let mut raw: Vec<(f32, [f32; 14])> = Vec::new(); // score, x y w h + 10 kps (pixels)
        for &s in &STRIDES {
            let cls = get(format!("cls_{s}"))?;
            let obj = get(format!("obj_{s}"))?;
            let bbox = get(format!("bbox_{s}"))?;
            let kps = get(format!("kps_{s}"))?;
            let (cols, rows) = (pw / s, ph / s);
            if cls.len() < rows * cols
                || obj.len() < rows * cols
                || bbox.len() < rows * cols * 4
                || kps.len() < rows * cols * 10
            {
                return Err(anyhow!("unexpected YuNet output size at stride {s}"));
            }
            let sf = s as f32;
            for r in 0..rows {
                for c in 0..cols {
                    let i = r * cols + c;
                    let score = (cls[i].clamp(0.0, 1.0) * obj[i].clamp(0.0, 1.0)).sqrt();
                    if score < self.opts.score_threshold {
                        continue;
                    }
                    let cx = (c as f32 + bbox[i * 4]) * sf;
                    let cy = (r as f32 + bbox[i * 4 + 1]) * sf;
                    let w = bbox[i * 4 + 2].exp() * sf;
                    let h = bbox[i * 4 + 3].exp() * sf;
                    let mut v = [0f32; 14];
                    v[0] = cx - w / 2.0;
                    v[1] = cy - h / 2.0;
                    v[2] = w;
                    v[3] = h;
                    for n in 0..5 {
                        v[4 + 2 * n] = (kps[i * 10 + 2 * n] + c as f32) * sf;
                        v[5 + 2 * n] = (kps[i * 10 + 2 * n + 1] + r as f32) * sf;
                    }
                    raw.push((score, v));
                }
            }
        }
        let keep = nms(&raw, self.opts.nms_threshold, self.opts.top_k);
        let (fw, fh) = (dw as f32, dh as f32);
        Ok(keep
            .into_iter()
            .map(|i| {
                let (score, v) = &raw[i];
                let mut landmarks = [[0f32; 2]; 5];
                for (n, l) in landmarks.iter_mut().enumerate() {
                    *l = [v[4 + 2 * n] / fw, v[5 + 2 * n] / fh];
                }
                Detection {
                    bbox: [v[0] / fw, v[1] / fh, v[2] / fw, v[3] / fh],
                    score: *score,
                    landmarks,
                }
            })
            .collect())
    }
}

/// Greedy NMS over score-sorted candidates (IoU above the threshold suppresses).
fn nms(raw: &[(f32, [f32; 14])], thr: f32, top_k: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..raw.len()).collect();
    order.sort_by(|&a, &b| raw[b].0.total_cmp(&raw[a].0));
    order.truncate(top_k);
    let mut keep: Vec<usize> = Vec::new();
    for &i in &order {
        if keep.iter().all(|&j| iou(&raw[i].1, &raw[j].1) <= thr) {
            keep.push(i);
        }
    }
    keep
}

fn iou(a: &[f32; 14], b: &[f32; 14]) -> f32 {
    let iw = ((a[0] + a[2]).min(b[0] + b[2]) - a[0].max(b[0])).max(0.0);
    let ih = ((a[1] + a[3]).min(b[1] + b[3]) - a[1].max(b[1])).max(0.0);
    let inter = iw * ih;
    let union = a[2] * a[3] + b[2] * b[3] - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nms_suppresses_overlaps() {
        let mk = |x: f32, s: f32| {
            let mut v = [0f32; 14];
            v[0] = x;
            v[2] = 10.0;
            v[3] = 10.0;
            (s, v)
        };
        let raw = vec![mk(0.0, 0.8), mk(1.0, 0.9), mk(50.0, 0.75)];
        assert_eq!(nms(&raw, 0.3, 100), vec![1, 2]);
    }
}
