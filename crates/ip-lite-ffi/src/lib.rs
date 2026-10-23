//! C ABI of [`ip_lite::analyze_quality`] for the Python worker (`ai-worker/imagepicker_ai/steps/
//! lite_native.py` loads it with `ctypes`, which releases the GIL, so the worker's decode threads
//! run the analysis in parallel).
//!
//! The worker keeps its pure-Python `quality` step as the fallback: without this library (an
//! installer without the desktop bundle, a worker started by hand) or on a layout mismatch it
//! never loads, and `analyze_quality` behaves exactly as before.

use std::panic::{catch_unwind, AssertUnwindSafe};

/// Layout of [`imagepicker_lite_quality`]'s result; mirrored by the ctypes `Structure` in
/// `lite_native.py`. Every value is already rounded like the Python step (`round(x, 5)`, and the
/// tilt estimator's own 2 / 3 decimals).
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub struct QualityOut {
    /// 0 on success; the call's return value repeats it.
    pub status: i32,
    pub sharpness: f64,
    pub sharpness_center: f64,
    pub laplacian_var: f64,
    pub tenengrad: f64,
    pub exposure: f64,
    pub mean_luminance: f64,
    pub clipped_highlights: f64,
    pub crushed_shadows: f64,
    pub noise: f64,
    pub noise_sigma: f64,
    pub tilt_deg: f64,
    pub tilt_confidence: f64,
}

/// Version of the ABI and of the struct layout; the loader refuses a different value.
pub const ABI_VERSION: u32 = 1;

/// `1` for this build. The Python loader compares it before the first call so that an old library
/// left next to a new worker (or the reverse) is ignored instead of read with the wrong layout.
#[no_mangle]
pub extern "C" fn imagepicker_lite_abi_version() -> u32 {
    ABI_VERSION
}

/// Sharpness / exposure / noise / tilt of an upright RGB8 buffer (`w * h * 3` bytes).
///
/// Returns 0 on success and writes `*out`. Non-zero: 1 = the analysis panicked, 2 = a null
/// pointer or an empty image, 3 = `w * h * 3` overflows.
///
/// # Safety
///
/// `rgb` must point to at least `w * h * 3` readable bytes and `out` to a writable
/// [`QualityOut`]; neither may be NULL.
#[no_mangle]
pub unsafe extern "C" fn imagepicker_lite_quality(
    rgb: *const u8,
    w: u32,
    h: u32,
    out: *mut QualityOut,
) -> i32 {
    if rgb.is_null() || out.is_null() || w == 0 || h == 0 {
        return 2;
    }
    let Some(len) = (w as usize)
        .checked_mul(h as usize)
        .and_then(|n| n.checked_mul(3))
    else {
        return 3;
    };
    let buf = std::slice::from_raw_parts(rgb, len);
    match catch_unwind(AssertUnwindSafe(|| {
        ip_lite::analyze_quality(buf, w as usize, h as usize)
    })) {
        Ok(q) => {
            out.write(QualityOut {
                status: 0,
                sharpness: q.sharpness,
                sharpness_center: q.sharpness_center,
                laplacian_var: q.laplacian_var,
                tenengrad: q.tenengrad,
                exposure: q.exposure,
                mean_luminance: q.mean_luminance,
                clipped_highlights: q.clipped_highlights,
                crushed_shadows: q.crushed_shadows,
                noise: q.noise,
                noise_sigma: q.noise_sigma,
                tilt_deg: q.tilt_deg,
                tilt_confidence: q.tilt_confidence,
            });
            0
        }
        Err(_) => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(w: usize, h: usize, v: u8) -> Vec<u8> {
        vec![v; w * h * 3]
    }

    #[test]
    fn result_matches_the_rust_api() {
        let (w, h) = (96usize, 72usize);
        let mut img = flat(w, h, 100);
        for y in 0..h {
            for x in 0..w {
                if (x / 8 + y / 8) % 2 == 0 {
                    let i = (y * w + x) * 3;
                    img[i..i + 3].copy_from_slice(&[200, 40, 30]);
                }
            }
        }
        let want = ip_lite::analyze_quality(&img, w, h);
        let mut out = QualityOut::default();
        let rc = unsafe { imagepicker_lite_quality(img.as_ptr(), w as u32, h as u32, &mut out) };
        assert_eq!(rc, 0);
        assert_eq!(out.status, 0);
        assert_eq!(out.sharpness, want.sharpness);
        assert_eq!(out.sharpness_center, want.sharpness_center);
        assert_eq!(out.laplacian_var, want.laplacian_var);
        assert_eq!(out.tenengrad, want.tenengrad);
        assert_eq!(out.exposure, want.exposure);
        assert_eq!(out.mean_luminance, want.mean_luminance);
        assert_eq!(out.clipped_highlights, want.clipped_highlights);
        assert_eq!(out.crushed_shadows, want.crushed_shadows);
        assert_eq!(out.noise, want.noise);
        assert_eq!(out.noise_sigma, want.noise_sigma);
        assert_eq!(out.tilt_deg, want.tilt_deg);
        assert_eq!(out.tilt_confidence, want.tilt_confidence);
    }

    #[test]
    fn bad_arguments_are_reported() {
        let mut out = QualityOut::default();
        let img = flat(4, 4, 0);
        unsafe {
            assert_eq!(
                imagepicker_lite_quality(std::ptr::null(), 4, 4, &mut out),
                2
            );
            assert_eq!(imagepicker_lite_quality(img.as_ptr(), 0, 4, &mut out), 2);
            assert_eq!(
                imagepicker_lite_quality(img.as_ptr(), 4, 4, std::ptr::null_mut()),
                2
            );
        }
        assert_eq!(imagepicker_lite_abi_version(), ABI_VERSION);
    }
}
