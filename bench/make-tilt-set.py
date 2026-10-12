"""Builds the tilt evaluation set from the synthetic photo library.

Usage:
    python bench/make-tilt-set.py [LIBRARY_DIR] [OUT_DIR] [--angles 0,1,-1,...] [--long-edge 1024]

LIBRARY_DIR defaults to bench/data/qwen-photo-library, OUT_DIR to bench/data/tilt-set (both
gitignored). Each selected library JPEG is rotated by known angles about its centre, centre-cropped
to the largest rectangle of the original aspect ratio that holds no rotated border, resized to
`--long-edge` and written as `jpeg/<angle>/<name>.jpg`, plus `manifest.json` with the true angle.

Angle convention (same as `analysis.tilt_deg`): positive = the content is rotated clockwise, i.e.
the horizon falls to the right; rotating the photo counter-clockwise by the angle levels it.

Selection: every `places` photo (horizons, buildings, streets, but also forests, dunes, canyons)
except the three prompted "tilted" harbours, and a deterministic subsample of `things`, `people`
and `groups` (portraits and objects, many without any straight structure). `structure` in the
manifest is a prompt-based guess (`lines` / `none`), not verified ground truth.
The library images are assumed level (no tilt label exists).

Needs Pillow; standard library otherwise.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent
DEFAULT_LIBRARY = ROOT / "data" / "qwen-photo-library"
DEFAULT_OUT = ROOT / "data" / "tilt-set"
DEFAULT_ANGLES = [0, 1, -1, 2, -2, 3, -3, 5, -5, 8, -8]
# subsample step per phase (every n-th photo in manifest order)
PHASE_STEP = {"places": 1, "things": 2, "people": 5, "groups": 3}
# places whose prompt promises a horizon or man-made straight lines (buildings, roads, fences)
LINE_PLACES = {
    "pl01", "pl02", "pl03", "pl05", "pl09", "pl11", "pl13", "pl14", "pl15", "pl16", "pl17",
    "pl18", "pl20", "pl22", "pl26", "pl27", "pl28", "pl31", "pl32", "pl33", "pl34", "pl38",
    "pl39", "pl40",
}


def crop_scale(w: int, h: int, deg: float) -> float:
    """Largest s such that a centred s*w x s*h rectangle fits inside the w x h image rotated by deg."""
    a = math.radians(abs(deg))
    c, s = math.cos(a), math.sin(a)
    return min(w / (w * c + h * s), h / (w * s + h * c))


def tilt(img: Image.Image, deg: float, long_edge: int) -> Image.Image:
    """Content rotated clockwise by `deg`, border-free centre crop, long edge resized."""
    w, h = img.size
    # PIL rotates counter-clockwise for positive angles
    rot = img.rotate(-deg, resample=Image.Resampling.BICUBIC, expand=False) if deg else img
    k = crop_scale(w, h, deg)
    cw, ch = w * k, h * k
    box = ((w - cw) / 2, (h - ch) / 2, (w + cw) / 2, (h + ch) / 2)
    box = (math.ceil(box[0]), math.ceil(box[1]), math.floor(box[2]), math.floor(box[3]))
    out = rot.crop(box)
    s = long_edge / max(out.size)
    size = (max(1, round(out.size[0] * s)), max(1, round(out.size[1] * s)))
    return out.resize(size, Image.Resampling.LANCZOS)


def select(manifest: dict) -> list[dict]:
    picked, seen = [], {}
    for im in manifest["images"]:
        ph = im.get("phase")
        step = PHASE_STEP.get(ph)
        if step is None or "tilted" in im.get("tags", []):
            continue
        n = seen.get(ph, 0)
        seen[ph] = n + 1
        if n % step:
            continue
        scene = im["name"].split("-")[0]
        lines = ph == "places" and scene in LINE_PLACES or "document" in im.get("tags", [])
        picked.append({**im, "structure": "lines" if lines else "none"})
    return picked


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("library", type=Path, nargs="?", default=DEFAULT_LIBRARY)
    ap.add_argument("out", type=Path, nargs="?", default=DEFAULT_OUT)
    ap.add_argument("--angles", default=",".join(str(a) for a in DEFAULT_ANGLES))
    ap.add_argument("--long-edge", type=int, default=1024)
    ap.add_argument("--quality", type=int, default=90)
    a = ap.parse_args()
    angles = [float(x) for x in a.angles.split(",") if x.strip()]
    manifest = json.loads((a.library / "manifest.json").read_text(encoding="utf-8"))
    sources = select(manifest)
    items = []
    for i, im in enumerate(sources):
        with Image.open(a.library / im["jpeg"]) as src:
            src = src.convert("RGB")
            for deg in angles:
                tag = f"{deg:+g}".replace("+0", "0")
                rel = f"jpeg/{tag}/{im['name']}.jpg"
                path = a.out / rel
                path.parent.mkdir(parents=True, exist_ok=True)
                tilt(src, deg, a.long_edge).save(path, quality=a.quality)
                items.append({
                    "jpeg": rel, "name": im["name"], "source": im["jpeg"], "phase": im["phase"],
                    "structure": im["structure"], "angle": deg,
                })
        if (i + 1) % 25 == 0:
            print(f"{i + 1}/{len(sources)} sources", flush=True)
    out = {
        "library": str(a.library), "angles": angles, "long_edge": a.long_edge,
        "convention": "angle > 0: content rotated clockwise (horizon falls to the right)",
        "sources": len(sources), "images": items,
    }
    (a.out / "manifest.json").write_text(json.dumps(out, indent=1), encoding="utf-8")
    print(f"wrote {len(items)} images from {len(sources)} sources to {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
