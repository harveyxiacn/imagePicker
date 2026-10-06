//! Throughput benchmark for the imaging fast paths.
//!
//! cargo run --release -p ip-imaging --example bench_thumbs -- <dir> [--synth N]
//! cargo run --release -p ip-imaging --example bench_thumbs -- --synth 200
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use ip_imaging::*;
use rayon::prelude::*;

fn synth_jpeg(seed: u64) -> Vec<u8> {
    let (w, h) = (6000usize, 4000usize);
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut rgb = vec![0u8; w * h * 3];
    let (fx, fy) = (1.0 + (seed % 7) as f32, 1.0 + (seed % 5) as f32);
    for y in 0..h {
        let gy = (y as f32 / h as f32 * fy * std::f32::consts::TAU).sin() * 60.0 + 128.0;
        for x in 0..w {
            let gx = (x as f32 / w as f32 * fx * std::f32::consts::TAU).cos() * 60.0;
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let n = (s & 0xFF) as f32 / 255.0 * 24.0 - 12.0; // sensor-like noise
            let v = gy + gx + n;
            let i = (y * w + x) * 3;
            rgb[i] = v.clamp(0.0, 255.0) as u8;
            rgb[i + 1] = (v * 0.8 + 20.0).clamp(0.0, 255.0) as u8;
            rgb[i + 2] = (255.0 - v * 0.7).clamp(0.0, 255.0) as u8;
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 90)
        .encode(&rgb, w as u16, h as u16, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    out
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir: Option<PathBuf> = None;
    let mut synth = 0usize;
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--synth" {
            synth = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(100);
            i += 1;
        } else {
            dir = Some(PathBuf::from(&args[i]));
        }
        i += 1;
    }
    let _tmp;
    let root = if synth > 0 {
        let t = tempfile::Builder::new().prefix("ip-bench-").tempdir()?;
        let r = dir.clone().unwrap_or_else(|| t.path().to_path_buf());
        std::fs::create_dir_all(&r)?;
        let t0 = Instant::now();
        (0..synth).into_par_iter().for_each(|n| {
            let sub = r.join(format!("d{}", n % 8));
            std::fs::create_dir_all(&sub).unwrap();
            std::fs::write(sub.join(format!("SYN_{n:05}.jpg")), synth_jpeg(n as u64)).unwrap();
        });
        println!(
            "generated {synth} synthetic 24MP JPEGs in {:.1}s -> {}",
            t0.elapsed().as_secs_f64(),
            r.display()
        );
        _tmp = t;
        r
    } else {
        dir.ok_or_else(|| anyhow::anyhow!("usage: bench_thumbs <dir> [--synth N]"))?
    };

    let t0 = Instant::now();
    let files = scan_dir(&root, true, &[])?;
    let dt = t0.elapsed().as_secs_f64();
    let total_mb: f64 = files.iter().map(|f| f.size as f64).sum::<f64>() / 1e6;
    println!(
        "scan:      {:>6} files  {:>8.3}s  {:>10.0} files/s  ({:.0} MB total)",
        files.len(),
        dt,
        files.len() as f64 / dt,
        total_mb
    );
    if files.is_empty() {
        return Ok(());
    }
    println!("threads: {}", rayon::current_num_threads());

    let t0 = Instant::now();
    let metas: Vec<Metadata> = files
        .par_iter()
        .map(|f| read_metadata(&f.path, f.format).unwrap_or_default())
        .collect();
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "metadata:  {:>6} files  {:>8.3}s  {:>10.0} files/s",
        files.len(),
        dt,
        files.len() as f64 / dt
    );

    let t0 = Instant::now();
    let keys: Vec<String> = files
        .par_iter()
        .filter_map(|f| content_key(&f.path).ok())
        .collect();
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "content_key:{:>5} files  {:>8.3}s  {:>10.0} files/s",
        keys.len(),
        dt,
        keys.len() as f64 / dt
    );

    for (label, edge, quality) in [("thumb 256", 256u32, 80u8), ("preview 2048", 2048, 85)] {
        // single-thread latency (first up-to-20 files)
        let sample: Vec<_> = files.iter().zip(&metas).take(20).collect();
        let t0 = Instant::now();
        for (f, m) in &sample {
            let _ = generate_thumbnail(&f.path, f.format, m.orientation, edge, quality);
        }
        let per = t0.elapsed().as_secs_f64() * 1000.0 / sample.len() as f64;

        let t0 = Instant::now();
        let results: Vec<Option<EncodedImage>> = files
            .par_iter()
            .zip(metas.par_iter())
            .map(|(f, m)| generate_thumbnail(&f.path, f.format, m.orientation, edge, quality).ok())
            .collect();
        let dt = t0.elapsed().as_secs_f64();
        let ok = results.iter().flatten().count();
        let bytes: usize = results.iter().flatten().map(|e| e.bytes.len()).sum();
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for r in results.iter().flatten() {
            *by.entry(format!("{:?}", r.source)).or_default() += 1;
        }
        let pct: Vec<String> = by
            .iter()
            .map(|(k, v)| format!("{k} {:.0}%", *v as f64 * 100.0 / ok.max(1) as f64))
            .collect();
        println!(
            "{label:<12} {ok:>5}/{} ok  {dt:>8.3}s  {:>8.1} files/s  1-thread {per:>6.1} ms/img  avg {:.1} KB  [{}]",
            files.len(),
            ok as f64 / dt,
            bytes as f64 / ok.max(1) as f64 / 1024.0,
            pct.join(", ")
        );
    }
    Ok(())
}
