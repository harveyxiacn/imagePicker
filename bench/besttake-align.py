"""Global alignment statistics of the synthetic bursts (why Best Take says `camera_moved`).

Usage (worker venv):
    ai-worker/.venv/Scripts/python bench/besttake-align.py LIBRARY_DIR DATA_DIR [--json OUT]

For every manifest burst, aligns each frame onto the burst's `base` frame with the worker's own
`besttake.align.align_global` (faces from the catalog masked out, as `besttake.compose` does) and
prints inliers / residual / shift / scale / rotation / background NCC against the thresholds.
"""

from __future__ import annotations

import argparse
import json
import sqlite3
import sys
from collections import Counter, defaultdict
from pathlib import Path

import numpy as np
from PIL import Image

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ai-worker"))
from imagepicker_ai.besttake import align  # noqa: E402


def failing(ga) -> list[str]:
    why = []
    if ga.inliers < align.MIN_INLIERS or (ga.matches and ga.inliers / ga.matches < align.MIN_INLIER_FRAC):
        why.append("inliers")
    if ga.residual_px > align.MAX_RESIDUAL_PX:
        why.append("residual")
    if ga.shift_frac > align.MAX_SHIFT_FRAC:
        why.append("shift")
    if abs(ga.scale - 1) > align.MAX_SCALE_DEV:
        why.append("scale")
    if abs(ga.rot_deg) > align.MAX_ROT_DEG:
        why.append("rotation")
    if ga.inliers >= align.MIN_INLIERS and ga.ncc < align.MIN_BG_NCC:
        why.append("ncc")
    return why


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("library", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("--json", type=Path)
    ap.add_argument("--margin", type=float, nargs=2, metavar=("W", "H"),
                    help="experiment: exclude only the face box grown by these factors instead of the"
                         " worker's head/hair/neck box (1.3 x 1.5 half sizes, shifted down)")
    a = ap.parse_args()
    if a.margin:
        import cv2

        mw, mh = a.margin

        def tight(shape, boxes, s):
            m = np.full(shape, 255, np.uint8)
            for x, y, w, h in boxes:
                cx, cy = (x + w / 2) * s, (y + h / 2) * s
                cv2.rectangle(m, (int(cx - w * s * mw / 2), int(cy - h * s * mh / 2)),
                              (int(cx + w * s * mw / 2), int(cy + h * s * mh / 2)), 0, -1)
            return m

        align._exclusion_mask = tight
    manifest = json.loads((a.library / "manifest.json").read_text(encoding="utf-8"))
    db = sqlite3.connect(f"file:{a.data_dir / 'catalog.db'}?mode=ro", uri=True)
    faces = defaultdict(list)
    for name, x, y, w, h in db.execute(
        "SELECT p.file_name, f.bbox_x, f.bbox_y, f.bbox_w, f.bbox_h FROM face f JOIN photo p ON p.id = f.photo_id"
    ):
        faces[name].append((x, y, w, h))
    bursts = defaultdict(list)
    for im in manifest["images"]:
        if im.get("burst"):
            bursts[im["burst"]].append(im)
    rows = []
    excl = []
    for bid, ims in sorted(bursts.items()):
        base = next(i for i in ims if "base" in i["tags"])
        bimg = np.asarray(Image.open(a.library / base["jpeg"]).convert("RGB"))
        bh, bw = bimg.shape[:2]
        bf = [(x * bw, y * bh, w * bw, h * bh) for x, y, w, h in faces[Path(base["jpeg"]).name]]
        excl.append(("group" in base["tags"], float((align._exclusion_mask((bh, bw), bf, 1.0) == 0).mean())))
        for im in ims:
            if im is base:
                continue
            simg = np.asarray(Image.open(a.library / im["jpeg"]).convert("RGB"))
            sh, sw = simg.shape[:2]
            sf = [(x * sw, y * sh, w * sw, h * sh) for x, y, w, h in faces[Path(im["jpeg"]).name]]
            ga = align.align_global(bimg, simg, bf, sf)
            variant = next((t for t in ("eyes-closed", "laugh", "gaze-away", "mid-talk", "blur", "best")
                            if t in im["tags"]), "?")
            rows.append({"burst": bid, "group": "group" in im["tags"], "frame": im["name"], "variant": variant,
                         "ok": ga.ok, "why": failing(ga) if not ga.ok else [], "inliers": ga.inliers,
                         "matches": ga.matches, "residual_px": round(ga.residual_px, 2),
                         "shift": round(ga.shift_frac, 4), "scale": round(ga.scale, 4),
                         "rot": round(ga.rot_deg, 2), "ncc": round(ga.ncc, 3)})
    for kind in (False, True):
        sel = [r for r in rows if r["group"] == kind]
        okn = sum(r["ok"] for r in sel)
        why = Counter(w for r in sel for w in r["why"])
        print(f"{'group' if kind else 'single'} bursts: {okn}/{len(sel)} frames align with their base;"
              f" failing criteria: {dict(why)}")
        for k in ("inliers", "residual_px", "ncc", "shift"):
            xs = sorted(r[k] for r in sel)
            if xs:
                print(f"   {k:<12} p10 {xs[len(xs) // 10]}  median {xs[len(xs) // 2]}  p90 {xs[9 * len(xs) // 10]}")
    for kind in (False, True):
        xs = sorted(v for g, v in excl if g == kind)
        if xs:
            print(f"{'group' if kind else 'single'} base frames: share of the frame excluded from matching"
                  f" median {xs[len(xs) // 2]:.2f}, max {xs[-1]:.2f}")
    for r in rows[:6]:
        print(json.dumps(r))
    if a.json:
        a.json.write_text(json.dumps(rows, indent=1), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
