#!/usr/bin/env python3
"""Turn the ComfyUI evaluation set (PNG, no metadata) into camera-like JPEGs with EXIF.

Grouping needs capture times: every category becomes a "scene" a few minutes apart, every
burst's frames are 0.4 s apart, everything else 25 s apart. Camera model, exposure and ISO are
filled in so the UI / XMP code paths see realistic metadata.

Usage (no venv needed):
  uv run --with pillow --with piexif scripts/comfy-testset-exif.py \
      [--src ~/ai/ComfyUI/output/imagepicker-testset] [--dst ~/ai/ComfyUI/output/imagepicker-testset-jpg]
"""
from __future__ import annotations

import argparse
import json
from datetime import datetime, timedelta
from pathlib import Path

import piexif
from PIL import Image

# A scene is shot on one device; scenes rotate over a camera and two phones so the device
# filter / multi-device time alignment have something to chew on. Raw vendor spellings on purpose.
DEVICES = [
    ("SONY", "ILCE-7M4", "FE 35mm F1.8", 35.0, 2.8),
    ("Apple", "iPhone 15 Pro", "iPhone 15 Pro back triple camera 6.765mm f/1.78", 6.765, 1.78),
    ("Xiaomi", "Xiaomi 14", "", 6.0, 1.6),
]


def exif_bytes(t: datetime, subsec_ms: int, iso: int, shutter: tuple[int, int], dev: tuple, w: int, h: int) -> bytes:
    make, model, lens, focal, f = dev
    dt = t.strftime("%Y:%m:%d %H:%M:%S")
    zeroth = {
        piexif.ImageIFD.Make: make,
        piexif.ImageIFD.Model: model,
        piexif.ImageIFD.Orientation: 1,
        piexif.ImageIFD.DateTime: dt,
        piexif.ImageIFD.Software: "imagePicker testset",
    }
    exif = {
        piexif.ExifIFD.DateTimeOriginal: dt,
        piexif.ExifIFD.DateTimeDigitized: dt,
        piexif.ExifIFD.SubSecTimeOriginal: f"{subsec_ms:03d}",
        piexif.ExifIFD.OffsetTimeOriginal: "+08:00",
        piexif.ExifIFD.ISOSpeedRatings: iso,
        piexif.ExifIFD.ExposureTime: shutter,
        piexif.ExifIFD.FNumber: (int(f * 10), 10),
        piexif.ExifIFD.FocalLength: (int(focal * 1000), 1000),
        piexif.ExifIFD.PixelXDimension: w,
        piexif.ExifIFD.PixelYDimension: h,
        piexif.ExifIFD.ColorSpace: 1,
    }
    if lens:
        exif[piexif.ExifIFD.LensModel] = lens
    return piexif.dump({"0th": zeroth, "Exif": exif})


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default=str(Path.home() / "ai/ComfyUI/output/imagepicker-testset"))
    ap.add_argument("--dst", default=str(Path.home() / "ai/ComfyUI/output/imagepicker-testset-jpg"))
    ap.add_argument("--quality", type=int, default=92)
    args = ap.parse_args()
    src, dst = Path(args.src), Path(args.dst)
    manifest = json.loads((src / "manifest.json").read_text())
    t = datetime(2026, 10, 3, 9, 0, 0)
    last_cat = None
    last_burst = None
    out_manifest = []
    for m in manifest["images"]:
        # v2 manifests carry explicit scene / burst keys; v1 derives the burst from the name
        burst = m.get("burst") or (m["name"].rsplit("-", 1)[0] if m["category"] == "bursts" else None)
        scene = m.get("scene", m["category"])
        if scene != last_cat:
            t += timedelta(minutes=12) if m.get("scene") is None else timedelta(minutes=3)
            last_cat = scene
        elif burst and burst == last_burst:
            t += timedelta(milliseconds=400)
        else:
            t += timedelta(seconds=25)
        last_burst = burst
        tags = set(m.get("tags", []))
        iso = 3200 if {"low-light", "night", "noisy"} & tags else 200
        shutter = (1, 60) if iso > 400 else (1, 500)
        dev = DEVICES[sum(map(ord, scene)) % len(DEVICES)]
        # exported / screenshot-like images carry no EXIF at all
        no_exif = "document" in tags
        for f in m["files"]:
            p = src.parent / f
            if not p.exists():
                print("missing", p)
                continue
            img = Image.open(p).convert("RGB")
            out = dst / m["category"] / (Path(f).stem.replace("_00001_", "") + ".jpg")
            out.parent.mkdir(parents=True, exist_ok=True)
            if no_exif:
                img.save(out, "JPEG", quality=args.quality)
            else:
                img.save(out, "JPEG", quality=args.quality, exif=exif_bytes(t, t.microsecond // 1000, iso, shutter, dev, img.width, img.height))
            out_manifest.append({**m, "jpeg": str(out.relative_to(dst)), "taken_at": None if no_exif else t.isoformat(timespec="milliseconds"), "device": None if no_exif else f"{dev[0]} {dev[1]}"})
    (dst / "manifest.json").write_text(json.dumps({"images": out_manifest}, ensure_ascii=False, indent=1))
    print(f"{len(out_manifest)} JPEGs in {dst}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
