use super::*;
use ip_imaging::ImageFormat;
use std::path::PathBuf;

const LAYOUTS: [CollageLayout; 3] = [
    CollageLayout::Grid,
    CollageLayout::Story,
    CollageLayout::Contact,
];

fn r(x: u32, y: u32, w: u32, h: u32) -> Rect {
    Rect { x, y, w, h }
}

fn plain(r: Rect) -> Cell {
    Cell {
        photo: r,
        caption: None,
    }
}

/// The rectangle a cell occupies including its caption band.
fn full(c: &Cell) -> Rect {
    match c.caption {
        Some(cap) => Rect {
            h: c.photo.h + cap.h,
            ..c.photo
        },
        None => c.photo,
    }
}

// The same golden rectangles are asserted in `web/src/lib/collageLayout.test.ts`.
#[test]
fn golden_rectangles() {
    let cells = compute_layout(CollageLayout::Grid, 1000, 1000, 10, 20, 0, &[1.5; 4]).unwrap();
    assert_eq!(
        cells,
        [
            plain(r(20, 20, 475, 475)),
            plain(r(505, 20, 475, 475)),
            plain(r(20, 505, 475, 475)),
            plain(r(505, 505, 475, 475)),
        ]
    );
    let cells = compute_layout(CollageLayout::Story, 1000, 500, 10, 0, 0, &[1.5; 4]).unwrap();
    assert_eq!(
        cells,
        [
            plain(r(0, 0, 660, 500)),
            plain(r(670, 0, 330, 160)),
            plain(r(670, 170, 330, 160)),
            plain(r(670, 340, 330, 160)),
        ]
    );
    let cells = compute_layout(CollageLayout::Contact, 400, 300, 10, 10, 20, &[1.5; 2]).unwrap();
    assert_eq!(
        cells,
        [
            Cell {
                photo: r(10, 10, 185, 260),
                caption: Some(r(10, 270, 185, 20)),
            },
            Cell {
                photo: r(205, 10, 185, 260),
                caption: Some(r(205, 270, 185, 20)),
            },
        ]
    );
    // a portrait canvas puts the large photo on top
    let cells = compute_layout(CollageLayout::Story, 500, 1000, 10, 0, 0, &[1.0; 2]).unwrap();
    assert_eq!(
        cells,
        [plain(r(0, 0, 500, 660)), plain(r(0, 670, 500, 330))]
    );
    // one photo fills the area inside the margin
    for l in LAYOUTS {
        let one = compute_layout(l, 300, 200, 10, 5, 0, &[1.5]).unwrap();
        assert_eq!(one, [plain(r(5, 5, 290, 190))], "{l:?}");
    }
}

/// Every cell lies inside the margin, neighbours are exactly `gap` apart, nothing overlaps and
/// the cells (with their captions) reach the margin on every side.
fn check_tiling(cells: &[Cell], w: u32, h: u32, gap: u32, margin: u32) {
    let rects: Vec<Rect> = cells.iter().map(full).collect();
    for (i, a) in rects.iter().enumerate() {
        assert!(a.w > 0 && a.h > 0, "empty cell {a:?}");
        assert!(a.x >= margin && a.y >= margin, "{a:?}");
        assert!(a.x + a.w <= w - margin && a.y + a.h <= h - margin, "{a:?}");
        for b in &rects[i + 1..] {
            let apart =
                a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
            assert!(apart, "overlap {a:?} {b:?}");
        }
        // nearest neighbour to the right / below that shares rows / columns
        let right = rects
            .iter()
            .filter(|b| b.x >= a.x + a.w && b.y < a.y + a.h && a.y < b.y + b.h)
            .map(|b| b.x - (a.x + a.w))
            .min();
        match right {
            Some(d) => assert_eq!(d, gap, "horizontal gap next to {a:?}"),
            None => assert_eq!(a.x + a.w, w - margin, "right edge of {a:?}"),
        }
        let below = rects
            .iter()
            .filter(|b| b.y >= a.y + a.h && b.x < a.x + a.w && a.x < b.x + b.w)
            .map(|b| b.y - (a.y + a.h))
            .min();
        match below {
            Some(d) => assert_eq!(d, gap, "vertical gap below {a:?}"),
            None => assert_eq!(a.y + a.h, h - margin, "bottom edge of {a:?}"),
        }
        if !rects
            .iter()
            .any(|b| b.x + b.w + gap <= a.x && b.y < a.y + a.h && a.y < b.y + b.h)
        {
            assert_eq!(a.x, margin, "left edge of {a:?}");
        }
        if !rects
            .iter()
            .any(|b| b.y + b.h + gap <= a.y && b.x < a.x + a.w && a.x < b.x + b.w)
        {
            assert_eq!(a.y, margin, "top edge of {a:?}");
        }
    }
    // no pixel is used twice and nothing leaves the area inside the margin
    let (iw, ih) = ((w - 2 * margin) as u64, (h - 2 * margin) as u64);
    let mut covered = vec![0u8; (w * h) as usize];
    for a in &rects {
        for y in a.y..a.y + a.h {
            for x in a.x..a.x + a.w {
                covered[(y * w + x) as usize] += 1;
            }
        }
    }
    assert!(covered.iter().all(|c| *c <= 1));
    let cells_area: u64 = covered.iter().map(|c| *c as u64).sum();
    assert!(cells_area <= iw * ih);
}

#[test]
fn layouts_tile_the_canvas_without_overlap_and_with_exact_gaps() {
    let aspects = |n: usize| -> Vec<f64> {
        (0..n)
            .map(|i| [1.5, 0.667, 1.0, 1.778, 0.75][i % 5])
            .collect()
    };
    for layout in LAYOUTS {
        for n in 1..=30usize {
            for (w, h, gap, margin) in [
                (240, 180, 4, 6),
                (150, 260, 0, 0),
                (300, 300, 7, 3),
                (400, 120, 2, 10),
            ] {
                let caption = if layout == CollageLayout::Contact {
                    9
                } else {
                    0
                };
                match compute_layout(layout, w, h, gap, margin, caption, &aspects(n)) {
                    Ok(cells) => {
                        assert_eq!(cells.len(), n);
                        check_tiling(&cells, w, h, gap, margin);
                        for c in &cells {
                            assert_eq!(c.caption.is_some(), caption > 0);
                        }
                    }
                    // a canvas too small for that many cells is refused, never mis-tiled
                    Err(e) => assert!(e.contains("too small"), "{e}"),
                }
            }
        }
    }
}

#[test]
fn layout_is_deterministic_and_follows_the_order() {
    let aspects = [1.5, 0.667, 1.0, 1.778, 0.75, 1.5, 1.0];
    for layout in LAYOUTS {
        let a = compute_layout(layout, 1080, 1350, 12, 24, 20, &aspects).unwrap();
        let b = compute_layout(layout, 1080, 1350, 12, 24, 20, &aspects).unwrap();
        assert_eq!(a, b, "{layout:?}");
    }
    // the first photo is the large one of a story
    let cells = compute_layout(CollageLayout::Story, 1080, 1350, 12, 24, 0, &aspects).unwrap();
    let area = |c: &Cell| c.photo.w as u64 * c.photo.h as u64;
    assert!(cells[1..].iter().all(|c| area(c) < area(&cells[0])));
    // a wide photo set picks a wide arrangement, a tall one a tall arrangement
    let wide = compute_layout(CollageLayout::Grid, 1000, 1000, 0, 0, 0, &[3.0; 3]).unwrap();
    assert_eq!(
        wide.iter().map(|c| c.photo.w).collect::<Vec<_>>(),
        [1000, 1000, 1000]
    );
    let tall = compute_layout(CollageLayout::Grid, 1000, 1000, 0, 0, 0, &[0.33; 3]).unwrap();
    assert_eq!(
        tall.iter().map(|c| c.photo.h).collect::<Vec<_>>(),
        [1000, 1000, 1000]
    );
}

#[test]
fn too_small_canvases_are_refused() {
    assert!(compute_layout(CollageLayout::Grid, 100, 100, 10, 50, 0, &[1.0]).is_err());
    assert!(compute_layout(CollageLayout::Grid, 100, 100, 40, 0, 0, &[1.0; 30]).is_err());
    assert!(compute_layout(CollageLayout::Contact, 100, 100, 0, 0, 100, &[1.0; 2]).is_err());
    assert!(compute_layout(CollageLayout::Story, 50, 50, 60, 0, 0, &[1.0; 3]).is_err());
}

#[test]
fn cover_window_follows_the_faces_and_stays_inside() {
    // 300x100 picture into a square cell: a 100x100 window
    assert_eq!(cover_window(300, 100, 50, 50, None), (100, 0, 100, 100));
    assert_eq!(
        cover_window(300, 100, 50, 50, Some((0.9, 0.5))),
        (200, 0, 100, 100)
    );
    assert_eq!(
        cover_window(300, 100, 50, 50, Some((0.0, 0.0))),
        (0, 0, 100, 100)
    );
    assert_eq!(
        cover_window(300, 100, 50, 50, Some((1.0, 1.0))),
        (200, 0, 100, 100)
    );
    // portrait picture into a wide cell: the window is a full-width band
    assert_eq!(
        cover_window(100, 300, 200, 100, Some((0.5, 0.25))),
        (0, 50, 100, 50)
    );
    // same aspect: everything
    assert_eq!(
        cover_window(400, 300, 40, 30, Some((0.1, 0.9))),
        (0, 0, 400, 300)
    );
    assert_eq!(
        cover_window(100, 100, 10, 10, Some((f64::NAN, 0.5))),
        (0, 0, 100, 100)
    );
}

#[test]
fn captions_name_and_stars() {
    assert_eq!(caption_text("a.jpg", Some(3), true, true), "a.jpg  ★★★");
    assert_eq!(caption_text("a.jpg", Some(0), true, true), "a.jpg");
    assert_eq!(caption_text("a.jpg", Some(4), false, true), "★★★★");
    assert_eq!(caption_text("a.jpg", None, true, false), "a.jpg");
    assert_eq!(caption_text("a.jpg", Some(4), false, false), "");
}

// ------------------------------------------------------------------ rendering

fn photo(dir: &Path, id: i64, w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> PhotoRef {
    let path: PathBuf = dir.join(format!("p{id}.png"));
    let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb(f(x, y)));
    img.save(&path).unwrap();
    PhotoRef {
        id,
        path,
        file_name: format!("p{id}.png"),
        format: ImageFormat::Png,
        orientation: 1,
        fast_key: "0".repeat(32),
        taken_at: None,
        mtime_ms: None,
        edit_hash: None,
        width: Some(w),
        height: Some(h),
    }
}

const COLORS: [[u8; 3]; 4] = [[220, 30, 30], [30, 200, 40], [40, 60, 230], [230, 210, 20]];
const BG: [u8; 3] = [10, 20, 30];

fn flat(dir: &Path, id: i64, w: u32, h: u32) -> PhotoRef {
    photo(dir, id, w, h, |_, _| COLORS[(id - 1) as usize])
}

/// Mean colour of the pixels in `rect` (inclusive of nothing outside it).
fn mean(img: &RgbImage, rect: Rect) -> [f64; 3] {
    let mut sum = [0f64; 3];
    for y in rect.y..rect.y + rect.h {
        for x in rect.x..rect.x + rect.w {
            let i = ((y * img.width + x) * 3) as usize;
            for c in 0..3 {
                sum[c] += img.data[i + c] as f64;
            }
        }
    }
    let n = (rect.w * rect.h) as f64;
    sum.map(|v| v / n)
}

fn close(a: [f64; 3], b: [u8; 3], tol: f64) -> bool {
    (0..3).all(|c| (a[c] - b[c] as f64).abs() <= tol)
}

fn job<'a>(
    cells: &'a [Cell],
    (width, height): (u32, u32),
    fit: CollageFit,
    focus: &'a HashMap<i64, (f64, f64)>,
    captions: &'a [String],
    cancel: Option<&'a CancelToken>,
) -> CollageJob<'a> {
    CollageJob {
        cells,
        width,
        height,
        background: BG,
        fit,
        focus,
        captions,
        cancel,
    }
}

#[test]
fn every_cell_shows_its_own_photo() {
    let dir = tempfile::tempdir().unwrap();
    // different shapes: wide, tall, square, wide
    let refs = vec![
        flat(dir.path(), 1, 300, 200),
        flat(dir.path(), 2, 200, 300),
        flat(dir.path(), 3, 240, 240),
        flat(dir.path(), 4, 400, 200),
    ];
    let aspects: Vec<f64> = refs.iter().map(aspect_of).collect();
    for layout in [CollageLayout::Grid, CollageLayout::Story] {
        for fit in [CollageFit::Cover, CollageFit::Contain] {
            let cells = compute_layout(layout, 400, 400, 8, 8, 0, &aspects).unwrap();
            let none = HashMap::new();
            let steps = std::sync::Mutex::new(Vec::new());
            let canvas = render_collage(
                &crate::imaging::RealImaging,
                &refs,
                None,
                &job(&cells, (400, 400), fit, &none, &[], None),
                &|d| steps.lock().unwrap().push(d),
            )
            .unwrap();
            assert_eq!((canvas.width, canvas.height), (400, 400));
            let mut steps = steps.into_inner().unwrap();
            steps.sort_unstable();
            assert_eq!(steps, [1, 2, 3, 4]);
            for (i, c) in cells.iter().enumerate() {
                // contain leaves bars; look at the middle of the cell
                let inner = Rect {
                    x: c.photo.x + c.photo.w / 3,
                    y: c.photo.y + c.photo.h / 3,
                    w: (c.photo.w / 3).max(1),
                    h: (c.photo.h / 3).max(1),
                };
                assert!(
                    close(mean(&canvas, inner), COLORS[i], 3.0),
                    "{layout:?} {fit:?} cell {i}"
                );
            }
            // margin and gaps stay background
            assert!(close(mean(&canvas, r(0, 0, 400, 8)), BG, 0.01));
            assert!(close(mean(&canvas, r(0, 392, 400, 8)), BG, 0.01));
            assert!(close(mean(&canvas, r(0, 0, 8, 400)), BG, 0.01));
            if layout == CollageLayout::Grid {
                // 2x2 grid: the cross between the cells
                assert!(close(
                    mean(&canvas, r(8 + cells[0].photo.w, 8, 8, 384)),
                    BG,
                    0.01
                ));
            }
        }
    }
}

#[test]
fn cover_fills_cells_and_contain_letterboxes() {
    let dir = tempfile::tempdir().unwrap();
    // a wide picture in a square cell: cover has no bars, contain has bars above and below
    let refs = vec![flat(dir.path(), 1, 400, 100), flat(dir.path(), 2, 400, 100)];
    let cells = [plain(r(0, 0, 100, 100)), plain(r(100, 0, 100, 100))];
    let none = HashMap::new();
    let cover = render_collage(
        &crate::imaging::RealImaging,
        &refs,
        None,
        &job(&cells, (200, 100), CollageFit::Cover, &none, &[], None),
        &|_| {},
    )
    .unwrap();
    assert!(close(mean(&cover, r(0, 0, 100, 100)), COLORS[0], 3.0));
    let contain = render_collage(
        &crate::imaging::RealImaging,
        &refs,
        None,
        &job(&cells, (200, 100), CollageFit::Contain, &none, &[], None),
        &|_| {},
    )
    .unwrap();
    assert!(
        close(mean(&contain, r(0, 0, 100, 36)), BG, 0.01),
        "bar above"
    );
    assert!(
        close(mean(&contain, r(0, 64, 100, 36)), BG, 0.01),
        "bar below"
    );
    assert!(
        close(mean(&contain, r(0, 40, 100, 20)), COLORS[0], 3.0),
        "picture in the middle"
    );
}

#[test]
fn cover_centres_on_the_faces() {
    let dir = tempfile::tempdir().unwrap();
    // left half red, right half blue
    let two_tone = |x: u32, _| if x < 150 { COLORS[0] } else { COLORS[2] };
    let refs = vec![
        photo(dir.path(), 1, 300, 100, two_tone),
        photo(dir.path(), 2, 300, 100, two_tone),
    ];
    let cells = [plain(r(0, 0, 50, 50)), plain(r(50, 0, 50, 50))];
    let mut focus = HashMap::new();
    focus.insert(2, (0.95, 0.5)); // faces on the right of photo 2 only
    let canvas = render_collage(
        &crate::imaging::RealImaging,
        &refs,
        None,
        &job(&cells, (100, 50), CollageFit::Cover, &focus, &[], None),
        &|_| {},
    )
    .unwrap();
    // photo 1: centred, so half red half blue
    assert!(close(mean(&canvas, r(0, 0, 20, 50)), COLORS[0], 6.0));
    assert!(close(mean(&canvas, r(30, 0, 20, 50)), COLORS[2], 6.0));
    // photo 2: moved to the faces, all blue
    assert!(close(mean(&canvas, r(50, 0, 50, 50)), COLORS[2], 3.0));
}

#[test]
fn contact_sheet_draws_captions_in_the_band_only() {
    let dir = tempfile::tempdir().unwrap();
    let refs = vec![flat(dir.path(), 1, 300, 200), flat(dir.path(), 2, 300, 200)];
    let cells = compute_layout(CollageLayout::Contact, 400, 300, 10, 10, 20, &[1.5; 2]).unwrap();
    let captions = vec!["p1.png  ★★★".to_string(), String::new()];
    let none = HashMap::new();
    let mut j = job(
        &cells,
        (400, 300),
        CollageFit::Cover,
        &none,
        &captions,
        None,
    );
    j.background = [255, 255, 255];
    let canvas = render_collage(&crate::imaging::RealImaging, &refs, None, &j, &|_| {}).unwrap();
    let ink = |rect: Rect| {
        (rect.y..rect.y + rect.h)
            .flat_map(|y| (rect.x..rect.x + rect.w).map(move |x| (x, y)))
            .filter(|(x, y)| canvas.data[((y * canvas.width + x) * 3) as usize] < 128)
            .count()
    };
    assert!(ink(cells[0].caption.unwrap()) > 20, "the caption is drawn");
    assert_eq!(
        ink(cells[1].caption.unwrap()),
        0,
        "an empty caption draws nothing"
    );
    // the picture above the first caption is untouched
    assert!(close(mean(&canvas, cells[0].photo), COLORS[0], 3.0));
}

#[test]
fn cancelling_stops_the_collage() {
    let dir = tempfile::tempdir().unwrap();
    let refs: Vec<PhotoRef> = (1..=4).map(|i| flat(dir.path(), i, 100, 100)).collect();
    let cells = compute_layout(CollageLayout::Grid, 200, 200, 0, 0, 0, &[1.0; 4]).unwrap();
    let none = HashMap::new();
    // cancelled before it starts: nothing is rendered
    let token = CancelToken::new();
    token.cancel();
    let rendered = std::sync::atomic::AtomicUsize::new(0);
    let res = render_collage(
        &crate::imaging::RealImaging,
        &refs,
        None,
        &job(
            &cells,
            (200, 200),
            CollageFit::Cover,
            &none,
            &[],
            Some(&token),
        ),
        &|_| {
            rendered.fetch_add(1, Ordering::SeqCst);
        },
    );
    assert_eq!(res.unwrap_err(), CANCELLED);
    assert_eq!(rendered.load(Ordering::SeqCst), 0);
    // cancelled while running: no canvas comes out either
    let token = CancelToken::new();
    let res = render_collage(
        &crate::imaging::RealImaging,
        &refs,
        None,
        &job(
            &cells,
            (200, 200),
            CollageFit::Cover,
            &none,
            &[],
            Some(&token),
        ),
        &|_| token.cancel(),
    );
    assert_eq!(res.unwrap_err(), CANCELLED);
}

#[test]
fn an_unreadable_photo_fails_the_collage_with_its_name() {
    let dir = tempfile::tempdir().unwrap();
    let mut bad = flat(dir.path(), 1, 100, 100);
    bad.path = dir.path().join("missing.png");
    bad.file_name = "missing.png".into();
    let refs = vec![bad, flat(dir.path(), 2, 100, 100)];
    let cells = compute_layout(CollageLayout::Grid, 200, 100, 0, 0, 0, &[1.0; 2]).unwrap();
    let none = HashMap::new();
    let err = render_collage(
        &crate::imaging::RealImaging,
        &refs,
        None,
        &job(&cells, (200, 100), CollageFit::Cover, &none, &[], None),
        &|_| {},
    )
    .unwrap_err();
    assert!(err.starts_with("missing.png: "), "{err}");
}

#[test]
fn request_validation() {
    let ok = || CollageRequest {
        ids: vec![1, 2, 3],
        dest: "/tmp/x".into(),
        layout: CollageLayout::Grid,
        width: 1080,
        height: 1350,
        gap: 16,
        margin: 16,
        background: "#fff".into(),
        fit: CollageFit::Cover,
        caption_name: true,
        caption_rating: true,
        format: ExportFormat::Jpeg,
        quality: 90,
        name_template: "collage".into(),
        apply_edits: true,
        watermark: None,
    };
    assert!(ok().validate().is_ok());
    let bad = |f: &dyn Fn(&mut CollageRequest)| {
        let mut r = ok();
        f(&mut r);
        r.validate().unwrap_err().to_string()
    };
    assert!(bad(&|r| r.ids = vec![1]).contains("2..30"));
    assert!(bad(&|r| r.ids = (1..=31).collect()).contains("2..30"));
    assert!(bad(&|r| r.ids = vec![1, 1]).contains("repeat"));
    assert!(bad(&|r| r.dest = " ".into()).contains("dest"));
    assert!(bad(&|r| r.width = 10).contains("width"));
    assert!(bad(&|r| r.height = 12_001).contains("height"));
    assert!(bad(&|r| {
        r.width = 12_000;
        r.height = 12_000
    })
    .contains("megapixels"));
    assert!(bad(&|r| r.gap = 2_001).contains("gap"));
    assert!(bad(&|r| r.margin = 2_001).contains("margin"));
    assert!(bad(&|r| r.background = "red".into()).contains("background"));
    assert!(bad(&|r| r.quality = 0).contains("quality"));
}
