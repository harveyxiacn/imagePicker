"""Hair transfer on your own photos: before / donor / after contact sheets to judge by eye.

Usage (worker venv, models installed or `--download`):
    ai-worker/.venv/Scripts/python bench/hair-transfer-demo.py PHOTO_DIR [--out OUT_DIR]
        [--target NAME ...] [--donors 3] [--max 60] [--download]

PHOTO_DIR should hold photos of ONE person from one day or event (the app enforces "same person" with
face identity; this script does not, it takes the largest face of every photo). For each target (by
default the 3 photos whose hair rates worst, else the names given with `--target`) it ranks every
other photo with the same cues as `GET /api/photos/{id}/hair/donors` (`hair.rank`: hair area,
shine, roughness, sharpness, pose gate), composes the best `--donors` with `hair.compose`'s own code
(`besttake.hair.hair_compose_arrays`, LaMa fill when installed) and writes
OUT_DIR/hair_<target>.jpg: one row per donor = target | donor | result | blend mask, labelled with
the donor's score, cues and warnings, plus OUT_DIR/summary.json (metrics, timings, reasons).

This needs your eyes: the automated tests only use synthetic heads. Look at the hair edge at full
size (stray strands, halo, background behind the hair), skin and lighting continuity at the
forehead, and whether the face below the brows really is untouched.
"""

from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

import cv2
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "ai-worker"))
from imagepicker_ai.besttake import hair as hm  # noqa: E402
from imagepicker_ai.imgio import PhotoRef, load_rgb  # noqa: E402
from imagepicker_ai.masks.generator import YUNET  # noqa: E402
from imagepicker_ai.service import WorkerService  # noqa: E402

EXTS = {".jpg", ".jpeg", ".png", ".heic", ".heif", ".tif", ".tiff", ".webp", ".dng", ".cr2", ".cr3",
        ".nef", ".arw", ".raf", ".orf", ".rw2"}  # fmt: skip
RANK_SIDE = 2048


def score(m: dict, med_r: float, med_s: float, delta: dict) -> float:
    """Same formula as `ip_core::hair::score_donors`."""
    vol = min(max(m["hair_ratio"] / med_r, 0), 1.3) / 1.3 if m.get("hair_ratio") and med_r else 0.5
    dry = 1 - min(max(m["shine"] / 0.05, 0), 1) if m.get("shine") is not None else 0.5
    tidy = 1 - min(max(m["roughness"] / 0.3, 0), 1) if m.get("roughness") is not None else 0.5
    sharp = min(max(m["sharpness"] / med_s, 0), 1.5) / 1.5 if m.get("sharpness") and med_s else 0.5
    worst = min(1.0, max(delta["yaw"] / hm.HAIR_MAX_YAW, delta["pitch"] / hm.HAIR_MAX_PITCH,
                         delta["roll"] / hm.HAIR_MAX_ROLL))  # fmt: skip
    return (0.35 * vol + 0.25 * dry + 0.2 * tidy + 0.2 * sharp) * (1 - 0.3 * worst)


def cues(m: dict, t: dict) -> list[str]:
    out = []
    if m.get("hair_ratio") and t.get("hair_ratio") and m["hair_ratio"] >= t["hair_ratio"] * 1.08:
        out.append("fuller")
    ms, ts = m.get("shine"), t.get("shine")
    if ms is not None and ts is not None and ms < ts * 0.7 and ts - ms >= 0.003:
        out.append("less_shine")
    mr, tr = m.get("roughness"), t.get("roughness")
    if mr is not None and tr is not None and mr < tr - 0.03:
        out.append("tidier")
    if m.get("sharpness") and t.get("sharpness") and m["sharpness"] > t["sharpness"] * 1.15:
        out.append("sharper")
    return out


def label(img: np.ndarray, lines: list[str]) -> np.ndarray:
    img = img.copy()
    y = 22
    for s in lines:
        cv2.putText(img, s, (8, y), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (0, 0, 0), 3, cv2.LINE_AA)
        cv2.putText(img, s, (8, y), cv2.FONT_HERSHEY_SIMPLEX, 0.55, (255, 255, 255), 1, cv2.LINE_AA)
        y += 22
    return img


def tile(img: np.ndarray, rect: tuple[int, int, int, int], side: int = 420) -> np.ndarray:
    x0, y0, x1, y1 = rect
    crop = img[y0:y1, x0:x1]
    k = side / max(crop.shape[:2])
    crop = cv2.resize(crop, None, fx=k, fy=k, interpolation=cv2.INTER_AREA)
    out = np.zeros((side, side, 3), np.uint8)
    out[: crop.shape[0], : crop.shape[1]] = crop
    return out


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("photos", type=Path)
    ap.add_argument("--out", type=Path, default=Path("hair-demo-out"))
    ap.add_argument("--target", action="append", default=[])
    ap.add_argument("--donors", type=int, default=3)
    ap.add_argument("--max", type=int, default=60)
    ap.add_argument("--download", action="store_true", help="download missing models first")
    a = ap.parse_args()
    a.out.mkdir(parents=True, exist_ok=True)
    files = sorted(p for p in a.photos.iterdir() if p.suffix.lower() in EXTS)[: a.max]
    if len(files) < 2:
        print("need at least two photos", file=sys.stderr)
        return 2
    svc = WorkerService(idle_unload_s=0)
    need = svc.hair.required_models() + [YUNET]
    for mid in need:
        if not svc.manager.is_installed(mid):
            if not a.download:
                print(f"model {mid} is not installed (use --download)", file=sys.stderr)
                return 2
            svc.manager.ensure(mid)
    comp = svc.hair._components()
    fill = svc.hair._fill()
    fa = svc.masks._face_analyzer()

    t0 = time.perf_counter()
    rows: dict[str, dict] = {}
    for f in files:
        rgb = load_rgb(PhotoRef(f.name, str(f), None), RANK_SIDE)
        h, w = rgb.shape[:2]
        faces = fa.detect(rgb)
        if not faces:
            rows[f.name] = {"ok": False, "reason": "no_face"}
            continue
        b = max(faces, key=lambda d: d["bbox"][2] * d["bbox"][3])["bbox"]
        m = hm.hair_metrics(rgb, (b[0] * w, b[1] * h, b[2] * w, b[3] * h), comp)
        m["bbox"] = [float(v) for v in b]
        rows[f.name] = m
    t_rank = time.perf_counter() - t0
    ok = {k: v for k, v in rows.items() if v.get("ok")}
    med_r = float(np.median([v["hair_ratio"] for v in ok.values()])) if ok else 0.0
    sh = [v["sharpness"] for v in ok.values() if v.get("sharpness")]
    med_s = float(np.median(sh)) if sh else 0.0
    zero = {"yaw": 0.0, "pitch": 0.0, "roll": 0.0}
    own = {k: score(v, med_r, med_s, zero) for k, v in ok.items()}
    targets = a.target or sorted(own, key=own.get)[:3]
    print(f"rated {len(files)} photos in {t_rank:.1f} s; targets: {targets}")

    summary: dict = {"photos": rows, "rank_seconds": round(t_rank, 2), "targets": {}}
    for tname in targets:
        if tname not in ok:
            print(f"{tname}: not measurable ({rows.get(tname, {}).get('reason')})")
            continue
        tm = ok[tname]
        ranked = []
        for dname, dm in ok.items():
            if dname == tname:
                continue
            delta = hm.pose_delta((tm["yaw"], tm["pitch"], tm["roll"]), (dm["yaw"], dm["pitch"], dm["roll"]))
            ranked.append((hm.pose_gate(delta), score(dm, med_r, med_s, delta), dname, delta))
        ranked.sort(key=lambda r: (not r[0], -r[1]))
        tfull = load_rgb(PhotoRef(tname, str(a.photos / tname), None))
        th, tw = tfull.shape[:2]
        tb = tm["bbox"]
        tface = (tb[0] * tw, tb[1] * th, tb[2] * tw, tb[3] * th)
        sheet_rows = []
        info = []
        for gate, sc, dname, delta in ranked[: a.donors]:
            dfull = load_rgb(PhotoRef(dname, str(a.photos / dname), None))
            dh, dw = dfull.shape[:2]
            db = ok[dname]["bbox"]
            t1 = time.perf_counter()
            out = hm.hair_compose_arrays(
                tfull, dfull, tface, (db[0] * dw, db[1] * dh, db[2] * dw, db[3] * dh), comp, fill
            )
            secs = time.perf_counter() - t1
            entry = {"donor": dname, "pose_ok": gate, "score": round(sc, 3), "cues": cues(ok[dname], tm),
                     "pose_delta": delta, "seconds": round(secs, 2), "reason": out.reason,
                     "quality": out.quality, "align": out.align}  # fmt: skip
            info.append(entry)
            print(f"  {tname} <- {dname}: score {sc:.2f} {entry['cues']} "
                  f"{out.reason or out.quality} in {secs:.1f} s")  # fmt: skip
            if out.patch is None:
                continue
            x, y, w, h = out.rect
            res = tfull.copy()
            al = out.patch[..., 3:4].astype(np.float32) / 255
            res[y : y + h, x : x + w] = np.rint(res[y : y + h, x : x + w] * (1 - al) + out.patch[..., :3] * al).astype(np.uint8)
            mask = np.zeros_like(tfull)
            mask[y : y + h, x : x + w] = out.patch[..., 3:4]
            crop = out.debug["crop"]
            donor_view = out.debug["aligned_donor"]
            pad = np.zeros_like(tfull)
            pad[crop[1] : crop[3], crop[0] : crop[2]] = donor_view
            row = np.hstack([
                label(tile(tfull, crop), [f"target {tname}"]),
                label(tile(pad, crop), [f"donor {dname}", f"score {sc:.2f}", " ".join(entry["cues"])]),
                label(tile(res, crop), ["result", " ".join(out.quality.get("warnings", [])) or "no warnings"]),
                label(tile(mask, crop), ["blend mask"]),
            ])  # fmt: skip
            sheet_rows.append(row)
        summary["targets"][tname] = info
        if sheet_rows:
            sheet = np.vstack(sheet_rows)
            p = a.out / f"hair_{Path(tname).stem}.jpg"
            cv2.imwrite(str(p), cv2.cvtColor(sheet, cv2.COLOR_RGB2BGR), [cv2.IMWRITE_JPEG_QUALITY, 90])
            print(f"  -> {p}")
    (a.out / "summary.json").write_text(json.dumps(summary, indent=1, default=float), encoding="utf-8")
    print(f"summary: {a.out / 'summary.json'}")
    svc.masks.shutdown()
    svc.besttake.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
