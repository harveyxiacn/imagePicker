"""Score an analysed catalog against the synthetic photo library's manifest.

Usage:
    python bench/eval-library.py LIBRARY_DIR DATA_DIR [--json OUT.json]

LIBRARY_DIR holds manifest.json (bench/data/qwen-photo-library); DATA_DIR is the --data-dir that
`imagepicker analyze LIBRARY_DIR/jpeg --profile lite --data-dir DATA_DIR` wrote its catalog.db to
(any profile; `closed_eyes` needs `fast` or `standard`, bench/eval-standard.py adds the people /
expression / scene metrics of `standard`).

Manifest tags are prompt intent, not verified ground truth (see the library's visual-review.json),
so the numbers rank threshold choices against each other; they are not an absolute accuracy.
Standard library only.
"""

import argparse
import csv
import json
import sqlite3
import sys
from collections import Counter, defaultdict
from pathlib import Path

ISSUE_BITS = {"closed_eyes": 1, "blurry": 2, "overexposed": 4, "underexposed": 8, "noisy": 64}
# manifest tag(s) that mean "this photo should carry the issue"
TRUTH = {
    # burst variants generated with someone's eyes closed (needs the faces step: fast / standard;
    # always 0 on lite). Group variants may close more people's eyes than the `victim:` tag says.
    "closed_eyes": lambda t, ph: "eyes-closed" in t,
    "overexposed": lambda t, ph: "overexposed" in t,
    "underexposed": lambda t, ph: "underexposed" in t,
    "noisy": lambda t, ph: "noisy" in t,
    # camera shake / heavy defocus in the defect set, and the motion-blurred burst variants (only
    # blurrier than their own base frame; also scored separately below)
    "blurry": lambda t, ph: "blur" in t and ph in ("defects", "bursts"),
}
METRICS = ["sharpness", "exposure", "noise", "mean_luminance", "clipped_highlights", "crushed_shadows"]


def norm(p: str) -> str:
    return p.replace("\\", "/").lower()


def load(library: Path, data_dir: Path):
    manifest = json.loads((library / "manifest.json").read_text(encoding="utf-8"))
    by_path = {}
    for im in manifest["images"]:
        rel = norm(im["jpeg"])
        rel = rel[len("jpeg/"):] if rel.startswith("jpeg/") else rel
        by_path[rel] = im
    db = sqlite3.connect(f"file:{data_dir / 'catalog.db'}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    rows = db.execute(
        "SELECT p.id, p.rel_path, p.issues, p.ai_rating, p.ai_score, p.burst_id, b.best_photo_id,"
        " a.sharpness, a.exposure, a.noise, a.mean_luminance, a.clipped_highlights, a.crushed_shadows"
        " FROM photo p LEFT JOIN analysis a ON a.photo_id = p.id"
        " LEFT JOIN burst b ON b.id = p.burst_id"
    ).fetchall()
    photos = []
    unmatched = 0
    for r in rows:
        im = by_path.get(norm(r["rel_path"]))
        if im is None:
            unmatched += 1
            continue
        photos.append((dict(r), im))
    return photos, unmatched, len(by_path)


def quantiles(xs):
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return "n=0"
    q = lambda f: xs[min(len(xs) - 1, int(f * len(xs)))]
    return f"n={len(xs):<4} min={xs[0]:.3f} p10={q(.1):.3f} p50={q(.5):.3f} p90={q(.9):.3f} max={xs[-1]:.3f}"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("library", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("--json", type=Path)
    ap.add_argument("--csv", type=Path, help="per-photo metrics and labels")
    a = ap.parse_args()
    photos, unmatched, total = load(a.library, a.data_dir)
    out = {"matched": len(photos), "unmatched_catalog_rows": unmatched, "manifest_images": total}
    print(f"matched {len(photos)} / {total} manifest images ({unmatched} catalog rows unmatched)")

    # ---- issue tags
    out["issues"] = {}
    print("\nissue          TP  FP  FN   precision recall   false positives by phase / tag")
    for issue, bit in ISSUE_BITS.items():
        tp = fp = fn = 0
        fp_where = Counter()
        fn_names = []
        for r, im in photos:
            t, ph = set(im.get("tags", [])), im.get("phase")
            truth = TRUTH[issue](t, ph)
            pred = bool((r["issues"] or 0) & bit)
            if pred and truth:
                tp += 1
            elif pred:
                fp += 1
                fp_where[ph] += 1
                for tag in t & {"document", "noon", "sunrise", "overcast", "blur", "thing", "place",
                                "laugh", "mid-talk", "gaze-away", "group"}:
                    fp_where["#" + tag] += 1
            elif truth:
                fn += 1
                fn_names.append(im["name"])
        prec = tp / (tp + fp) if tp + fp else float("nan")
        rec = tp / (tp + fn) if tp + fn else float("nan")
        where = ", ".join(f"{k} {v}" for k, v in fp_where.most_common(8))
        print(f"{issue:<13} {tp:>3} {fp:>3} {fn:>3}   {prec:>9.2f} {rec:>6.2f}   {where}")
        out["issues"][issue] = {"tp": tp, "fp": fp, "fn": fn, "precision": prec, "recall": rec,
                                "fp_where": dict(fp_where), "fn": fn_names}

    # burst motion-blur variants: flagged blurry, and blurrier than their burst's base frame?
    base_sharp = {}
    for r, im in photos:
        if im.get("burst") and "base" in im.get("tags", []):
            base_sharp[im["burst"]] = r["sharpness"]
    var = [(r, im) for r, im in photos if im.get("burst") and "blur" in im.get("tags", [])]
    flagged = sum(1 for r, _ in var if (r["issues"] or 0) & ISSUE_BITS["blurry"])
    ratios = [r["sharpness"] / base_sharp[im["burst"]] for r, im in var
              if r["sharpness"] is not None and base_sharp.get(im["burst"])]
    others = [r["sharpness"] / base_sharp[im["burst"]] for r, im in photos
              if im.get("burst") and "blur" not in im.get("tags", []) and "base" not in im.get("tags", [])
              and r["sharpness"] is not None and base_sharp.get(im["burst"])]
    print(f"\nburst motion-blur variants: {len(var)}, flagged blurry {flagged}")
    print(f"  sharpness / base frame, blur variants : {quantiles(ratios)}")
    print(f"  sharpness / base frame, other variants: {quantiles(others)}")
    out["burst_blur"] = {"variants": len(var), "flagged": flagged}

    # ---- raw metric distributions per class
    classes = defaultdict(list)
    for r, im in photos:
        t, ph = set(im.get("tags", [])), im.get("phase")
        hit = [k for k in ("overexposed", "underexposed", "noisy") if k in t]
        if ph == "defects":
            key = "defect:" + (hit[0] if hit else ("shake" if "shake" in t else "blur"))
        elif "document" in t:
            key = "document"
        elif im.get("burst") and "blur" in t:
            key = "burst-blur"
        elif ph in ("places",):
            key = "place:" + next((x for x in ("noon", "sunrise", "overcast") if x in t), "other")
        else:
            key = "clean:" + ph
        classes[key].append(r)
    print("\nmetric distributions per class")
    for m in METRICS:
        print(f"  {m}")
        for k in sorted(classes):
            print(f"    {k:<20} {quantiles([r[m] for r in classes[k]])}")

    # ---- burst grouping (pairwise over manifest bursts)
    truth_pairs = pred_pairs = both = 0
    members = defaultdict(list)
    for r, im in photos:
        if im.get("burst"):
            members[im["burst"]].append(r)
    by_photo = {r["id"]: (r, im) for r, im in photos}
    for ms in members.values():
        for i in range(len(ms)):
            for j in range(i + 1, len(ms)):
                truth_pairs += 1
                if ms[i]["burst_id"] is not None and ms[i]["burst_id"] == ms[j]["burst_id"]:
                    both += 1
    groups = defaultdict(list)
    for r, _ in photos:
        if r["burst_id"] is not None:
            groups[r["burst_id"]].append(r["id"])
    for ids in groups.values():
        pred_pairs += len(ids) * (len(ids) - 1) // 2
    stacked = sum(1 for ids in groups.values() if len(ids) >= 2)
    best_hits = sum(1 for r, im in photos if "best" in im.get("tags", []) and r["best_photo_id"] == r["id"])
    # a frame split off into a group of its own is trivially that group's best
    best_alone = sum(1 for r, im in photos if "best" in im.get("tags", []) and r["best_photo_id"] == r["id"]
                     and len(groups.get(r["burst_id"], ())) < 2)
    best_total = sum(1 for _, im in photos if "best" in im.get("tags", []))
    gp = both / pred_pairs if pred_pairs else float("nan")
    gr = both / truth_pairs if truth_pairs else float("nan")
    print(f"\nbursts: {len(members)} in manifest, {stacked} stacked groups found;"
          f" pair precision {gp:.2f} recall {gr:.2f}; best frame picked {best_hits}/{best_total}"
          f" ({best_alone} of them alone in their group)")
    out["bursts"] = {"manifest": len(members), "stacked": stacked, "pair_precision": gp,
                     "pair_recall": gr, "best_hits": best_hits, "best_total": best_total,
                     "best_hits_alone": best_alone}

    # Duplicate-aware variant: the exact-duplicate / missing-lens edge fixtures are copies of a
    # library photo with the same capture time, so stacking them with their source is correct
    # although the manifest gives them no `burst`. (no-exif / clock-offset copies have no usable
    # capture time and are not expected to stack.) Pairs of such copies count as true pairs.
    source = {}
    for _, im in photos:
        if im.get("phase") != "edge":
            source.setdefault((im.get("png"), im.get("taken_at")), im)

    def copy_of(im):
        mode = (im.get("metadata") or {}).get("metadata_mode")
        if im.get("phase") == "edge" and mode in ("normal", "missing-lens"):
            return source.get((im.get("png"), im.get("taken_at")))
        return None

    copied = {src["name"] for src in (copy_of(im) for _, im in photos) if src}

    def stack_key(im):
        src = copy_of(im) or (im if im["name"] in copied else None)
        if src is not None:
            return src.get("burst") or "dup:" + src["name"]
        return im.get("burst")

    keys = defaultdict(list)
    for r, im in photos:
        k = stack_key(im)
        if k:
            keys[k].append(r)
    truth2 = both2 = 0
    for ms in keys.values():
        for i in range(len(ms)):
            for j in range(i + 1, len(ms)):
                truth2 += 1
                if ms[i]["burst_id"] is not None and ms[i]["burst_id"] == ms[j]["burst_id"]:
                    both2 += 1
    gp2 = both2 / pred_pairs if pred_pairs else float("nan")
    gr2 = both2 / truth2 if truth2 else float("nan")
    print(f"bursts + exact-duplicate/missing-lens copies with their source: {len(keys)} stacks expected;"
          f" pair precision {gp2:.2f} recall {gr2:.2f}")
    out["bursts_with_copies"] = {"expected": len(keys), "pair_precision": gp2, "pair_recall": gr2}

    stars = Counter(r["ai_rating"] for r, _ in photos)
    print("\nai stars: " + "  ".join(f"{k}*:{v}" for k, v in sorted(stars.items(), key=lambda kv: (kv[0] is None, kv[0]))))
    out["stars"] = {str(k): v for k, v in stars.items()}
    if a.json:
        a.json.write_text(json.dumps(out, indent=2, default=str), encoding="utf-8")
    if a.csv:
        cols = ["name", "phase", "tags", "burst", "issues", "ai_rating", "burst_id", "best_photo_id", "id"] + METRICS
        with a.csv.open("w", encoding="utf-8", newline="") as f:
            w = csv.writer(f)
            w.writerow(cols)
            for r, im in photos:
                w.writerow([im["name"], im.get("phase"), " ".join(im.get("tags", [])), im.get("burst", "")]
                           + [r[c] for c in cols[4:]])
    return 0


if __name__ == "__main__":
    sys.exit(main())
