//! `imagepicker bench`: synthetic libraries for the performance benchmarks (`bench/`).
//!
//! * `gen`  writes N JPEGs with realistic EXIF (capture times in bursts and scenes, mixed sizes).
//! * `seed` fills a catalog directly (no image files) with N photo rows, bursts, faces and
//!   people, to benchmark queries at 20k / 100k rows.
//!
//! Everything is generated from a seeded PRNG; no real photos are ever used.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;
use ip_core::db::Db;
use ip_core::jpegmeta;
use ip_imaging::Metadata;
use rayon::prelude::*;
use rusqlite::params;

#[derive(Subcommand, Debug)]
pub enum BenchCmd {
    /// Write synthetic JPEGs (with EXIF) into a directory.
    Gen {
        dir: PathBuf,
        #[arg(long, default_value_t = 1000)]
        count: usize,
        /// Megapixels of the common frame (12 = 4000x3000); ~8% of files are 1.5x larger.
        #[arg(long, default_value_t = 12)]
        megapixels: u32,
        #[arg(long, default_value_t = 1)]
        seed: u64,
    },
    /// Seed a catalog with synthetic rows (no files on disk).
    Seed {
        /// Data directory of the catalog to fill.
        #[arg(long)]
        data_dir: PathBuf,
        /// Total photo rows.
        #[arg(long, default_value_t = 20_000)]
        rows: usize,
        /// Rows per session (the library is split into `rows / session-size` sessions).
        #[arg(long, default_value_t = 20_000)]
        session_size: usize,
        /// Number of people (faces are assigned to them).
        #[arg(long, default_value_t = 12)]
        people: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Point the rows at real files: `IMG_00001.jpg`.. from this directory (made by `gen`),
        /// cycling through `--files-count` of them. Every row is still a distinct photo (own
        /// fast key and root alias), so thumbnails are generated per row on demand.
        #[arg(long)]
        files_dir: Option<PathBuf>,
        #[arg(long, default_value_t = 1000)]
        files_count: usize,
    },
}

pub fn run(cmd: BenchCmd) -> Result<()> {
    match cmd {
        BenchCmd::Gen {
            dir,
            count,
            megapixels,
            seed,
        } => gen(&dir, count, megapixels, seed),
        BenchCmd::Seed {
            data_dir,
            rows,
            session_size,
            people,
            seed,
            files_dir,
            files_count,
        } => seed_catalog(
            &data_dir,
            rows,
            session_size.max(1),
            people,
            seed,
            files_dir.map(|d| (d, files_count.max(1))),
        ),
    }
}

/// xorshift64*: tiny deterministic PRNG.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
    pub fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// A capture timeline: moments (single shots or bursts) separated by gaps, with the occasional
/// long pause that becomes a scene boundary.
pub fn timeline(n: usize, rng: &mut Rng) -> Vec<i64> {
    // 2025-06-01 08:00:00 UTC in ms
    let mut t: i64 = 1_748_764_800_000;
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let shots = if rng.below(100) < 55 {
            1
        } else {
            3 + rng.below(6) as usize
        };
        for _ in 0..shots {
            if out.len() == n {
                break;
            }
            out.push(t);
            t += 180 + rng.below(160) as i64;
        }
        t += if rng.below(100) < 4 {
            20 * 60_000 + rng.below(40 * 60_000) as i64
        } else {
            4_000 + rng.below(110_000) as i64
        };
    }
    out
}

fn base_image(w: u32, h: u32, variant: u64) -> image::RgbImage {
    let mut rng = Rng::new(variant + 100);
    let (fx, fy) = (0.5 + rng.unit() * 3.0, 0.5 + rng.unit() * 3.0);
    let (r0, g0, b0) = (
        rng.below(120) as f64,
        rng.below(120) as f64,
        rng.below(120) as f64,
    );
    let rows: Vec<Vec<u8>> = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut r = Rng::new(variant * 7919 + y as u64);
            let mut row = Vec::with_capacity(w as usize * 3);
            let yy = y as f64 / h as f64;
            for x in 0..w {
                let xx = x as f64 / w as f64;
                let base = (xx * fx * std::f64::consts::TAU).sin() * 40.0
                    + (yy * fy * std::f64::consts::TAU).cos() * 40.0;
                // a soft "subject" blob
                let d = ((xx - 0.5).powi(2) + (yy - 0.55).powi(2)).sqrt();
                let blob = (1.0 - (d * 3.0).min(1.0)) * 70.0;
                let noise = (r.below(13) as f64) - 6.0;
                let px = |c0: f64| (c0 + 70.0 + base + blob + noise).clamp(0.0, 255.0) as u8;
                row.extend([px(r0), px(g0), px(b0)]);
            }
            row
        })
        .collect();
    let raw: Vec<u8> = rows.into_iter().flatten().collect();
    image::RgbImage::from_raw(w, h, raw).expect("buffer size")
}

fn encode(img: &image::RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 88)
        .encode_image(img)
        .expect("jpeg encode");
    out
}

fn gen(dir: &Path, count: usize, megapixels: u32, seed: u64) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let started = std::time::Instant::now();
    let scale = ((megapixels as f64 * 1_000_000.0) / 12_000_000.0).sqrt();
    let dim = |v: f64| ((v * scale) as u32).max(64) & !1;
    let shapes = [
        (dim(4000.0), dim(3000.0)),
        (dim(4000.0), dim(3000.0)),
        (dim(4000.0), dim(3000.0)),
        (dim(3000.0), dim(4000.0)),
        (dim(6000.0), dim(4000.0)),
    ];
    let bases: Vec<(u32, u32, Arc<Vec<u8>>)> = shapes
        .iter()
        .enumerate()
        .map(|(i, &(w, h))| (w, h, Arc::new(encode(&base_image(w, h, i as u64 + seed)))))
        .collect();
    eprintln!(
        "base images ready in {:.1}s ({} KB .. {} KB)",
        started.elapsed().as_secs_f64(),
        bases.iter().map(|b| b.2.len()).min().unwrap_or(0) / 1024,
        bases.iter().map(|b| b.2.len()).max().unwrap_or(0) / 1024
    );
    let mut rng = Rng::new(seed);
    let times = timeline(count, &mut rng);
    let cams = [
        ("Canon", "Canon EOS R6", "RF24-70mm F2.8 L IS USM"),
        ("SONY", "ILCE-7M4", "FE 35mm F1.8"),
        ("NIKON CORPORATION", "NIKON Z 6_2", "NIKKOR Z 50mm f/1.8 S"),
    ];
    // One base image per moment: frames of a burst (< 1.2 s apart) share it, like a real burst
    // looks alike; 8% of the moments use the large frame, 20% are portrait.
    let mut plan: Vec<(usize, usize, i64, u64)> = Vec::with_capacity(times.len());
    let (mut last_t, mut base) = (i64::MIN, 0usize);
    for (i, &t) in times.iter().enumerate() {
        if t - last_t >= 1200 {
            let r = rng.below(100);
            base = if r < 8 {
                4
            } else if r < 28 {
                3
            } else {
                rng.below(3) as usize
            };
        }
        last_t = t;
        plan.push((i, base, t, rng.next()));
    }
    plan.par_iter()
        .try_for_each(|&(i, b, t, r)| -> Result<()> {
            let (w, h, bytes) = &bases[b];
            let mut rr = Rng::new(r);
            let cam = cams[(i / 400) % cams.len()];
            let md = Metadata {
                width: Some(*w),
                height: Some(*h),
                orientation: 1,
                taken_at_ms: Some(t),
                taken_at_offset_min: Some(480),
                camera_make: Some(cam.0.into()),
                camera_model: Some(cam.1.into()),
                camera_serial: None,
                lens: Some(cam.2.into()),
                focal_mm: Some([24.0, 35.0, 50.0, 70.0][rr.below(4) as usize]),
                aperture: Some([1.8, 2.8, 4.0, 5.6][rr.below(4) as usize]),
                shutter_s: Some(1.0 / [60.0, 125.0, 250.0, 500.0, 1000.0][rr.below(5) as usize]),
                iso: Some([100, 200, 400, 800, 1600][rr.below(5) as usize]),
                gps_lat: None,
                gps_lon: None,
            };
            let exif = jpegmeta::synth_exif(&md, (*w, *h), false);
            let jpg = jpegmeta::insert_segments(bytes, exif.as_deref(), None);
            let path = dir.join(format!("IMG_{:05}.jpg", i + 1));
            std::fs::write(&path, &jpg).with_context(|| format!("write {}", path.display()))?;
            let f = std::fs::OpenOptions::new().write(true).open(&path)?;
            f.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_millis(t as u64))?;
            Ok(())
        })?;
    eprintln!(
        "wrote {count} JPEGs to {} in {:.1}s",
        dir.display(),
        started.elapsed().as_secs_f64()
    );
    println!("{}", dir.display());
    Ok(())
}

fn seed_catalog(
    data_dir: &Path,
    rows: usize,
    session_size: usize,
    people: usize,
    seed: u64,
    files: Option<(PathBuf, usize)>,
) -> Result<()> {
    std::fs::create_dir_all(data_dir)?;
    let db = Db::open(&data_dir.join("catalog.db")).context("open catalog")?;
    let started = std::time::Instant::now();
    let mut rng = Rng::new(seed);
    let now = ip_core::catalog::now_ms();
    let cams = ["Canon EOS R6", "SONY ILCE-7M4", "NIKON Z 6_2"];
    let scenes = ["portrait", "landscape", "food", "street", "indoor", "night"];
    let sessions = rows.div_ceil(session_size);
    let mut session_ids = Vec::new();
    let person_ids: Vec<i64> = db.with(|c| {
        let tx = c.transaction()?;
        let mut ids = Vec::new();
        for pid in 0..people {
            tx.execute(
                "INSERT INTO person(name, hidden, created_at) VALUES(?1, 0, ?2)",
                params![format!("Person {pid}"), now],
            )?;
            ids.push(tx.last_insert_rowid());
        }
        tx.commit()?;
        Ok(ids)
    })?;
    for s in 0..sessions {
        let n = session_size.min(rows - s * session_size);
        let times = timeline(n, &mut rng);
        let sid = db.with(|c| {
            let tx = c.transaction()?;
            // One root per `files_count` rows. With real files the roots are aliases of the same
            // directory (`dir/.`, `dir/./.`, ...), which keeps (root, rel_path) unique.
            let n_roots = files.as_ref().map_or(1, |(_, c)| n.div_ceil(*c));
            let mut roots = Vec::with_capacity(n_roots);
            for k in 0..n_roots {
                let root = match &files {
                    Some((dir, _)) => format!(
                        "{}{}",
                        dir.to_string_lossy().trim_end_matches(['/', '\\']),
                        "/.".repeat(s * n_roots + k)
                    ),
                    None => format!("bench-root-{s}"),
                };
                tx.execute(
                    "INSERT INTO root_folder(path, added_at) VALUES(?1, ?2)",
                    params![root, now],
                )?;
                roots.push(tx.last_insert_rowid());
            }
            let root_id = roots[0];
            tx.execute(
                "INSERT INTO session(title, created_at, root_id, import_state) VALUES(?1, ?2, ?3, 'ready')",
                params![format!("Bench session {s}"), now - s as i64, root_id],
            )?;
            let sid = tx.last_insert_rowid();
            {
                let mut ins = tx.prepare(
                    "INSERT INTO photo(root_id, rel_path, file_name, file_size, mtime, fast_key, content_key,
                        format, width, height, orientation, taken_at, taken_at_offset_min, camera, lens, focal, aperture,
                        shutter, iso, user_rating, flag, ai_score, ai_rating, issues, scene_type, thumb_state,
                        analysis_version, meta_done, face_count, subject_face_count, burst_id, rank_in_burst)
                     VALUES(?1,?2,?3,?4,?5,?6,?7,'jpeg',?8,?9,1,?10,480,?11,'lens',50.0,2.8,0.008,200,?12,?13,?14,?15,?16,?17,2,1,1,?18,?18,?19,?20)",
                )?;
                let mut link =
                    tx.prepare("INSERT INTO session_photo(session_id, photo_id) VALUES(?1, ?2)")?;
                let mut face = tx.prepare(
                    "INSERT INTO face(photo_id, idx, person_id, bbox_x, bbox_y, bbox_w, bbox_h, det_score,
                        eyes_open, smile, gaze, is_subject, is_bystander)
                     VALUES(?1, ?2, ?3, 0.3, 0.3, 0.2, 0.2, 0.99, ?4, ?5, ?6, ?7, ?8)",
                )?;
                let mut burst_ins = tx.prepare(
                    "INSERT INTO burst(session_id, best_photo_id, size, start_at, end_at) VALUES(?1, NULL, ?2, ?3, ?4)",
                )?;
                // burst grouping: consecutive photos closer than 1.2 s
                let mut i = 0;
                let mut seq = 0usize;
                while i < n {
                    let mut j = i + 1;
                    while j < n && times[j] - times[j - 1] < 1200 {
                        j += 1;
                    }
                    let size = j - i;
                    let burst_id = if size >= 2 {
                        burst_ins.execute(params![sid, size as i64, times[i], times[j - 1]])?;
                        Some(tx.last_insert_rowid())
                    } else {
                        None
                    };
                    for (k, &t) in times.iter().enumerate().take(j).skip(i) {
                        let rated = rng.below(100) < 30;
                        let rating: Option<i64> = rated.then(|| 1 + rng.below(5) as i64);
                        let flag: i64 = match rng.below(100) {
                            0..=7 => 1,
                            8..=13 => -1,
                            _ => 0,
                        };
                        let ai = rng.unit();
                        let mask: i64 = if rng.below(100) < 35 {
                            1 << rng.below(6)
                        } else {
                            0
                        };
                        let nfaces = if rng.below(100) < 60 {
                            1 + rng.below(3)
                        } else {
                            0
                        } as i64;
                        let w = if rng.below(5) == 0 { 3000 } else { 4000 };
                        let h = 7000 - w;
                        let (name, row_root) = match &files {
                            Some((_, c)) => (format!("IMG_{:05}.jpg", seq % c + 1), roots[seq / c]),
                            None => (format!("IMG_{:05}.jpg", seq + 1), root_id),
                        };
                        let size_bytes = 4_000_000i64 + rng.below(3_000_000) as i64;
                        let fk = format!("fk{s:03}{seq:09}{:016x}", rng.next());
                        let ck = format!("ck{s:03}{seq:09}{:016x}", rng.next());
                        let scene = scenes[rng.below(scenes.len() as u64) as usize];
                        let pid = ins.insert(params![
                            row_root,
                            name,
                            name,
                            size_bytes,
                            t,
                            fk,
                            ck,
                            w,
                            h,
                            t,
                            cams[(seq / 400) % 3],
                            rating,
                            flag,
                            ai,
                            ((ai * 4.0 + 1.0) * 2.0).round() / 2.0,
                            mask,
                            scene,
                            nfaces,
                            burst_id,
                            burst_id.map(|_| (k - i) as i64),
                        ])?;
                        link.execute(params![sid, pid])?;
                        for fi in 0..nfaces {
                            let who = person_ids[rng.below(person_ids.len() as u64) as usize];
                            face.execute(params![
                                pid,
                                fi,
                                who,
                                rng.unit(),
                                rng.unit(),
                                rng.unit(),
                                (fi == 0) as i64,
                                (fi != 0) as i64
                            ])?;
                        }
                        seq += 1;
                    }
                    if let Some(b) = burst_id {
                        tx.execute(
                            "UPDATE burst SET best_photo_id=(SELECT id FROM photo WHERE burst_id=?1 AND rank_in_burst=0) WHERE id=?1",
                            [b],
                        )?;
                    }
                    i = j;
                }
            }
            tx.execute(
                "UPDATE session SET cover_photo_id=(SELECT MIN(photo_id) FROM session_photo WHERE session_id=?1) WHERE id=?1",
                [sid],
            )?;
            tx.commit()?;
            Ok(sid)
        })?;
        session_ids.push(sid);
        eprintln!(
            "session {sid}: {n} rows ({:.1}s)",
            started.elapsed().as_secs_f64()
        );
    }
    db.with(|c| {
        c.execute_batch("ANALYZE")?;
        Ok(())
    })?;
    println!(
        "{}",
        serde_json::json!({"rows": rows, "sessions": session_ids, "people": person_ids,
                           "seconds": started.elapsed().as_secs_f64()})
    );
    Ok(())
}
