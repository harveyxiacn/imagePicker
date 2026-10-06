//! Cross-check against the Python worker: `ai-worker/scripts/gen_lite_fixtures.py` runs the real
//! `phash.py` / `quality.py` on synthetic images and stores the results next to the PNGs.

use std::path::PathBuf;

use ip_lite::{analyze_quality, hamming_hex, phash_hex};
use serde_json::Value;

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn load(name: &str) -> (usize, usize, Vec<u8>) {
    let img = image::open(fixtures().join(format!("{name}.png")))
        .expect("fixture png")
        .to_rgb8();
    (img.width() as usize, img.height() as usize, img.into_raw())
}

fn expected() -> serde_json::Map<String, Value> {
    let text = std::fs::read_to_string(fixtures().join("expected.json")).expect("expected.json");
    serde_json::from_str::<Value>(&text)
        .unwrap()
        .as_object()
        .unwrap()
        .clone()
}

#[test]
fn phash_is_bit_exact() {
    let mut mismatches = Vec::new();
    for (name, e) in expected() {
        let (w, h, rgb) = load(&name);
        assert_eq!(
            (w as u64, h as u64),
            (e["width"].as_u64().unwrap(), e["height"].as_u64().unwrap())
        );
        let want = e["phash"].as_str().unwrap();
        let got = phash_hex(&rgb, w, h);
        if got != want {
            mismatches.push(format!(
                "{name}: rust {got} python {want} (hamming {})",
                hamming_hex(&got, want).unwrap()
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "pHash differs:\n{}",
        mismatches.join("\n")
    );
}

/// Relative tolerance on the unbounded metrics, absolute on the 0..1 scores.
fn close(name: &str, key: &str, got: f64, want: f64, abs: f64, rel: f64) -> Option<String> {
    let tol = abs.max(rel * want.abs());
    ((got - want).abs() > tol)
        .then(|| format!("{name}.{key}: rust {got} python {want} (tol {tol})"))
}

#[test]
fn quality_matches_worker_formulas() {
    let mut bad = Vec::new();
    let mut max_dev = 0f64;
    for (name, e) in expected() {
        let (w, h, rgb) = load(&name);
        let q = analyze_quality(&rgb, w, h);
        let want = &e["quality"];
        let pairs = [
            ("sharpness", q.sharpness, 2e-3, 4e-3),
            ("sharpness_center", q.sharpness_center, 2e-3, 4e-3),
            ("laplacian_var", q.laplacian_var, 0.02, 4e-3),
            ("tenengrad", q.tenengrad, 0.5, 4e-3),
            ("exposure", q.exposure, 1e-4, 0.0),
            ("mean_luminance", q.mean_luminance, 1e-4, 0.0),
            ("clipped_highlights", q.clipped_highlights, 1e-4, 0.0),
            ("crushed_shadows", q.crushed_shadows, 1e-4, 0.0),
            ("noise", q.noise, 5e-3, 0.0),
            ("noise_sigma", q.noise_sigma, 0.05, 0.0),
        ];
        for (key, got, abs, rel) in pairs {
            let w = want[key].as_f64().unwrap();
            max_dev = max_dev.max((got - w).abs() / w.abs().max(1.0));
            bad.extend(close(&name, key, got, w, abs, rel));
        }
    }
    eprintln!("max relative deviation: {max_dev:.2e}");
    assert!(bad.is_empty(), "quality differs:\n{}", bad.join("\n"));
}
