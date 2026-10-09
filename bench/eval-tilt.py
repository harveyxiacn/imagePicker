"""Score the horizon-tilt estimate against the rotated tilt set (bench/make-tilt-set.py).

Usage:
    python bench/eval-tilt.py TILT_SET_DIR DATA_DIR [--library LIBRARY_DIR LIBRARY_DATA_DIR]
                              [--json OUT.json]

DATA_DIR is the --data-dir that `imagepicker analyze TILT_SET_DIR/jpeg --profile lite --data-dir
DATA_DIR` (or any other profile) wrote its catalog.db to; the optional library pair is the same
for the original synthetic library (bench/data/qwen-photo-library, see bench/eval-library.py).

The library photos are generated and assumed level, but some are visibly not (a tilted sea
horizon, a slanted window sill): "absolute" numbers treat every source as level, "relative"
numbers compare each rotated copy with the same photo's own 0-degree estimate, which isolates the
estimator from the source's own tilt. Reported:
  - angle error of confident estimates (confidence >= each grid value), absolute and relative
  - per true angle: share of confident estimates and of `tilted` flags (photo.issues bit 16)
  - precision / recall of the flag against |angle| >= 2 and >= 3 degrees, false-positive rate on
    the 0-degree copies and on the library
  - a sweep of (minimum confidence, minimum |tilt|) over the stored estimates, the basis of the
    thresholds in crates/ip-core/src/analysis/scoring.rs (TILTED_*)
Standard library only.
"""

import argparse
import json
import sqlite3
import statistics
import sys
from collections import defaultdict
from pathlib import Path

TILTED_BIT = 16
MAX_DEG = 10.0  # scoring.rs TILTED_MAX_DEG
CONF_GRID = [0.1, 0.15, 0.2, 0.25, 0.3, 0.4]
MIN_DEG_GRID = [1.5, 2.0, 2.5]


def norm(p: str) -> str:
    return p.replace("\\", "/").lower()


def catalog(data_dir: Path) -> dict[str, dict]:
    db = sqlite3.connect(f"file:{data_dir / 'catalog.db'}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    rows = db.execute(
        "SELECT p.rel_path, p.issues, a.tilt_deg, a.tilt_confidence"
        " FROM photo p LEFT JOIN analysis a ON a.photo_id = p.id"
    ).fetchall()
    return {norm(r["rel_path"]): dict(r) for r in rows}


def mean(xs):
    return statistics.fmean(xs) if xs else float("nan")


def ratio(a, b):
    return a / b if b else float("nan")


def tilt_set(set_dir: Path, data_dir: Path, out: dict) -> list[dict]:
    manifest = json.loads((set_dir / "manifest.json").read_text(encoding="utf-8"))
    cat = catalog(data_dir)
    rows, missing = [], 0
    for im in manifest["images"]:
        rel = norm(im["jpeg"])
        rel = rel[len("jpeg/"):] if rel.startswith("jpeg/") else rel
        r = cat.get(rel)
        if r is None or r["tilt_deg"] is None:
            missing += 1
            continue
        rows.append({**im, "est": r["tilt_deg"], "conf": r["tilt_confidence"] or 0.0,
                     "flag": bool((r["issues"] or 0) & TILTED_BIT)})
    print(f"tilt set: {len(rows)} analysed of {len(manifest['images'])} ({missing} missing)")
    zero = {r["name"]: r for r in rows if r["angle"] == 0}

    print("\nangle error of confident estimates (deg; relative = vs the photo's own 0-degree estimate)")
    out["error"] = {}
    for c in CONF_GRID:
        conf = [r for r in rows if r["conf"] >= c]
        absolute = [abs(r["est"] - r["angle"]) for r in conf]
        rel = [abs(r["est"] - zero[r["name"]]["est"] - r["angle"]) for r in conf
               if r["angle"] != 0 and r["name"] in zero and zero[r["name"]]["conf"] >= c]
        print(f"  conf >= {c:<4}  n={len(conf):<5} MAE {mean(absolute):.2f}  median {statistics.median(absolute) if absolute else float('nan'):.2f}"
              f"   relative: n={len(rel):<5} MAE {mean(rel):.2f}  median {statistics.median(rel) if rel else float('nan'):.2f}"
              f"  >1 deg {sum(e > 1 for e in rel)}")
        out["error"][str(c)] = {"n": len(conf), "mae": mean(absolute), "rel_n": len(rel), "rel_mae": mean(rel)}

    print("\nper true angle: flagged `tilted` / n   (lines = prompt promises a horizon or buildings)")
    by_angle = defaultdict(list)
    for r in rows:
        by_angle[r["angle"]].append(r)
    out["per_angle"] = {}
    for a in sorted(by_angle):
        rs = by_angle[a]
        lines = [r for r in rs if r["structure"] == "lines"]
        f_all, f_lines = sum(r["flag"] for r in rs), sum(r["flag"] for r in lines)
        print(f"  {a:+5.1f}: all {f_all:>3}/{len(rs):<4} ({ratio(f_all, len(rs)):.2f})"
              f"   lines {f_lines:>3}/{len(lines):<4} ({ratio(f_lines, len(lines)):.2f})")
        out["per_angle"][str(a)] = {"n": len(rs), "flagged": f_all, "lines_n": len(lines), "lines_flagged": f_lines}

    print("\n`tilted` flag vs truth |angle| >= T")
    out["flag"] = {}
    for t in (2.0, 3.0):
        tp = sum(r["flag"] and abs(r["angle"]) >= t for r in rows)
        fp = sum(r["flag"] and abs(r["angle"]) < t for r in rows)
        fn = sum(not r["flag"] and abs(r["angle"]) >= t for r in rows)
        pos_l = [r for r in rows if abs(r["angle"]) >= t and r["structure"] == "lines"]
        rec_l = ratio(sum(r["flag"] for r in pos_l), len(pos_l))
        print(f"  T={t}: TP {tp} FP {fp} FN {fn}  precision {ratio(tp, tp + fp):.2f}  recall {ratio(tp, tp + fn):.2f}"
              f"  (recall on 'lines' photos {rec_l:.2f})")
        out["flag"][str(t)] = {"tp": tp, "fp": fp, "fn": fn, "precision": ratio(tp, tp + fp),
                               "recall": ratio(tp, tp + fn), "recall_lines": rec_l}
    z = by_angle.get(0.0, [])
    fp0 = [r["name"] for r in z if r["flag"]]
    print(f"  false positives on the 0-degree copies: {len(fp0)}/{len(z)} ({ratio(len(fp0), len(z)):.3f}) {fp0}")
    out["fp_zero"] = {"n": len(z), "flagged": len(fp0), "names": fp0}
    return rows


def sweep(rows: list[dict], lib: list[dict], out: dict) -> None:
    print(f"\nthreshold sweep over the stored estimates (flag = conf >= c and d <= |tilt| <= {MAX_DEG})")
    print("   c     d    P(>=2)  R(>=2)  R(>=3)  R3 lines  FP 0deg  FP library")
    out["sweep"] = []
    for c in CONF_GRID:
        for d in MIN_DEG_GRID:
            def flag(r, c=c, d=d):
                return r["conf"] >= c and d <= abs(r["est"]) <= MAX_DEG
            pos2 = [r for r in rows if abs(r["angle"]) >= 2]
            pos3 = [r for r in rows if abs(r["angle"]) >= 3]
            pos3l = [r for r in pos3 if r["structure"] == "lines"]
            fl = [r for r in rows if flag(r)]
            p2 = ratio(sum(abs(r["angle"]) >= 2 for r in fl), len(fl))
            r2 = ratio(sum(flag(r) for r in pos2), len(pos2))
            r3 = ratio(sum(flag(r) for r in pos3), len(pos3))
            r3l = ratio(sum(flag(r) for r in pos3l), len(pos3l))
            z = [r for r in rows if r["angle"] == 0]
            fp0 = ratio(sum(flag(r) for r in z), len(z))
            fpl = ratio(sum(flag(r) for r in lib), len(lib)) if lib else float("nan")
            print(f"  {c:<5} {d:<4}  {p2:6.2f}  {r2:6.2f}  {r3:6.2f}  {r3l:8.2f}  {fp0:7.3f}  {fpl:9.3f}")
            out["sweep"].append({"conf": c, "min_deg": d, "precision2": p2, "recall2": r2, "recall3": r3,
                                 "recall3_lines": r3l, "fp_zero": fp0, "fp_library": fpl})


def library(lib_dir: Path, data_dir: Path, out: dict) -> list[dict]:
    manifest = json.loads((lib_dir / "manifest.json").read_text(encoding="utf-8"))
    cat = catalog(data_dir)
    rows = []
    for im in manifest["images"]:
        rel = norm(im["jpeg"])
        rel = rel[len("jpeg/"):] if rel.startswith("jpeg/") else rel
        r = cat.get(rel)
        if r is None or r["tilt_deg"] is None:
            continue
        rows.append({"name": im["name"], "phase": im.get("phase"), "tags": im.get("tags", []),
                     "est": r["tilt_deg"], "conf": r["tilt_confidence"] or 0.0,
                     "flag": bool((r["issues"] or 0) & TILTED_BIT)})
    level = [r for r in rows if "tilted" not in r["tags"]]
    flagged = [r for r in level if r["flag"]]
    by_phase = defaultdict(int)
    for r in flagged:
        by_phase[r["phase"]] += 1
    print(f"\nlibrary: {len(flagged)}/{len(level)} prompted-level photos flagged `tilted`"
          f" ({ratio(len(flagged), len(level)):.3f}) by phase {dict(by_phase)}")
    for r in sorted(flagged, key=lambda r: -r["conf"]):
        print(f"    {r['name']:<26} {r['est']:+6.2f} deg  conf {r['conf']:.3f}")
    tagged = [(r["name"], r["est"], r["conf"], r["flag"]) for r in rows if "tilted" in r["tags"]]
    print(f"  prompted 'tilted' (about 8 degrees): {tagged}")
    out["library"] = {"n": len(level), "flagged": len(flagged), "fp_rate": ratio(len(flagged), len(level)),
                      "by_phase": dict(by_phase), "prompted_tilted": tagged}
    return level


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("tilt_set", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("--library", type=Path, nargs=2, metavar=("LIBRARY_DIR", "DATA_DIR"))
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    out: dict = {}
    rows = tilt_set(a.tilt_set, a.data_dir, out)
    lib = library(a.library[0], a.library[1], out) if a.library else []
    sweep(rows, lib, out)
    if a.json:
        a.json.write_text(json.dumps(out, indent=2, default=str), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
