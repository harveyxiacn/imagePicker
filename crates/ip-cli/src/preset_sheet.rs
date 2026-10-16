//! `imagepicker bench preset-sheet`: before/after contact sheet of every built-in preset on a
//! folder of real photos, so a person can judge the adaptive presets (docs/03 §6.1.1).
//!
//! Writes `<out>/<photo>/<preset>.jpg` (plus `original.jpg`) and `<out>/index.html`, a table with
//! one row per photo and one column per preset, captioned with the fitted exposure and the
//! clipped share before → after. Nothing is written next to the photos; no catalog is needed
//! (so face boxes are not used: portrait-specific face protection is not shown here).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use ip_core::ip_render::{self, EditStack, NoAssets, PresetContext, RenderRequest, RgbImage};
use ip_imaging::ImageFormat;

/// Long edge the presets are fitted on (same as the app's proxy).
const PROXY_EDGE: u32 = 1024;

pub struct SheetArgs {
    pub dir: PathBuf,
    pub out: PathBuf,
    pub cell: u32,
    pub amount: f32,
    pub presets: Option<Vec<String>>,
    pub limit: usize,
}

fn photos_in(dir: &Path, limit: usize) -> Result<Vec<(PathBuf, ImageFormat)>> {
    let mut v: Vec<(PathBuf, ImageFormat)> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .filter_map(|p| ImageFormat::from_path(&p).map(|f| (p, f)))
        .collect();
    v.sort();
    v.truncate(limit);
    Ok(v)
}

fn save_jpeg(img: &RgbImage, path: &Path) -> Result<()> {
    let buf = image::RgbImage::from_raw(img.width, img.height, img.data.clone())
        .context("image buffer size")?;
    buf.save_with_format(path, image::ImageFormat::Jpeg)
        .with_context(|| format!("writing {}", path.display()))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// File-name-safe stem, unique within the sheet.
fn slug(p: &Path, i: usize) -> String {
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let clean: String = stem
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("{i:03}_{clean}")
}

pub fn run(a: SheetArgs) -> Result<()> {
    let ids: Vec<String> = match &a.presets {
        Some(v) => v.clone(),
        None => ip_render::builtin_presets().into_iter().map(|p| p.0).collect(),
    };
    let photos = photos_in(&a.dir, a.limit)?;
    anyhow::ensure!(!photos.is_empty(), "no supported images in {}", a.dir.display());
    std::fs::create_dir_all(&a.out)?;
    let renderer = ip_render::create_renderer(false);
    let render = |src: &RgbImage, stack: &EditStack| -> Result<RgbImage> {
        renderer.render(&RenderRequest {
            source: src,
            stack,
            masks: &NoAssets,
            luts: &NoAssets,
            max_long_edge: Some(a.cell),
        })
    };

    let mut html = String::new();
    let _ = write!(
        html,
        "<!doctype html><meta charset=utf-8><title>Preset sheet</title>\
         <style>body{{font:13px system-ui;background:#111;color:#ddd}}td{{vertical-align:top;padding:4px}}\
         img{{max-width:{c}px;display:block}}th{{position:sticky;top:0;background:#111}}\
         small{{color:#999}}</style>\
         <p>Strength {pct}% · fitted on a {PROXY_EDGE}px proxy without face boxes · ev = solved exposure, \
         clip = share of pixels at ≥250/255 luminance before → after, prot = highlight protection used.</p>\
         <table><tr><th>photo</th><th>original</th>",
        c = a.cell,
        pct = (a.amount * 100.0).round()
    );
    for id in &ids {
        let _ = write!(html, "<th>{}</th>", esc(id));
    }
    html.push_str("</tr>\n");

    for (i, (path, fmt)) in photos.iter().enumerate() {
        let orientation = ip_imaging::read_metadata(path, *fmt)
            .map(|m| m.orientation)
            .unwrap_or(1);
        let (w, h, data) = match ip_imaging::decode_rgb8(path, *fmt, orientation, PROXY_EDGE) {
            Ok(x) => x,
            Err(e) => {
                eprintln!("skip {}: {e:#}", path.display());
                continue;
            }
        };
        let src = RgbImage {
            width: w,
            height: h,
            data,
        };
        let name = slug(path, i);
        let dir = a.out.join(&name);
        std::fs::create_dir_all(&dir)?;
        save_jpeg(&render(&src, &EditStack::default())?, &dir.join("original.jpg"))?;
        let _ = write!(
            html,
            "<tr><td><small>{}</small></td><td><img src=\"{name}/original.jpg\"></td>",
            esc(&path.file_name().unwrap_or_default().to_string_lossy())
        );
        for id in &ids {
            let Some(r) = ip_render::resolve_preset(id, &src, &PresetContext::default(), a.amount)
            else {
                let _ = write!(html, "<td><small>unknown preset</small></td>");
                continue;
            };
            save_jpeg(&render(&src, &r.stack)?, &dir.join(format!("{id}.jpg")))?;
            let _ = write!(
                html,
                "<td><img src=\"{name}/{id}.jpg\"><small>ev {:+.2} · clip {:.2}% → {:.2}% · prot {:.2}{}</small></td>",
                r.exposure,
                r.before.clipped * 100.0,
                r.after.clipped * 100.0,
                r.protection,
                if r.satisfied { "" } else { " · over budget" }
            );
        }
        html.push_str("</tr>\n");
        println!("{} ({}/{})", path.display(), i + 1, photos.len());
    }
    html.push_str("</table>\n");
    let index = a.out.join("index.html");
    std::fs::write(&index, html)?;
    println!("wrote {}", index.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_an_index_and_one_image_per_preset() {
        let src = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        for (i, k) in [0.3f32, 1.0].iter().enumerate() {
            let s = ip_render::testutil::tint(&ip_render::testutil::scene(320, 200, i as u64), [*k; 3]);
            save_jpeg(&s, &src.path().join(format!("p{i}.jpg"))).unwrap();
        }
        std::fs::write(src.path().join("notes.txt"), "x").unwrap();
        run(SheetArgs {
            dir: src.path().to_path_buf(),
            out: out.path().to_path_buf(),
            cell: 96,
            amount: 1.0,
            presets: Some(vec!["japanese_clean".into(), "natural".into()]),
            limit: 10,
        })
        .unwrap();
        let html = std::fs::read_to_string(out.path().join("index.html")).unwrap();
        assert_eq!(html.matches("<tr><td>").count(), 2, "{html}");
        for d in ["000_p0", "001_p1"] {
            for f in ["original.jpg", "japanese_clean.jpg", "natural.jpg"] {
                assert!(out.path().join(d).join(f).is_file(), "{d}/{f}");
            }
        }
    }
}
