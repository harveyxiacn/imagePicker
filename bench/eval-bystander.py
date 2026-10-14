"""Measure the P1-8 tags on an analysed catalog: bystander / statue-and-poster faces, bad expression.

Usage:
    python bench/eval-bystander.py LIBRARY_DIR DATA_DIR [--labels FACES.csv] [--json OUT.json]

LIBRARY_DIR holds manifest.json (bench/data/qwen-photo-library); DATA_DIR is the --data-dir of
`imagepicker analyze LIBRARY_DIR/jpeg --profile standard --data-dir DATA_DIR` (the faces step with
landmarks is needed: `fast` or `standard`; `lite` has no liveness, no expression). Nothing here
runs the analysis. Standard library only. Acceptance targets (docs/09 section 5.2, P1-8):

  * bystander misclassification <= 10% of labelled faces   (section 1, needs --labels)
  * bad_expression (mid-talk) recall >= 70%, false positives <= 5% of the other photos (section 3)

Section 1 - faces: predicted class of every face = `artificial` (face.artificial >= ARTIFICIAL_AT,
a statue / poster / screen / print), else `subject` (face.is_subject), else `bystander`. FACES.csv
labels the truth by hand or by generator intent, one row per face, header
`photo,x,y,w,h,kind` (photo = manifest `name` or catalog file name, x y w h = normalised box,
kind = subject | bystander | artificial); a catalog face is matched to the label of the same photo
with the highest IoU >= 0.3. The report gives the confusion matrix, the misclassification rate and
the number of artificial faces that ended up as bystanders (the failure the tag exists to avoid).
Without labels, manifest tags in ARTIFICIAL_TAGS (add `statue`, `poster`, ... to the prompts of the
generated photos) give the share of their faces flagged artificial, and every other photo gives
the share of real faces wrongly flagged (the false artificial rate). The count of photos with the
photo-level `bystander` tag (issue bit 32) is printed as well.

Section 2 - the artificial confidence distribution, to move ARTIFICIAL_AT / the weights in
ai-worker/imagepicker_ai/steps/liveness.py (and ip-core `scoring::ARTIFICIAL_AT`).

Section 3 - bad_expression (issue bit 128) against the manifest `mid-talk` variants, per variant,
plus a sweep of the mouth-open threshold (TALKING_MOUTH_OPEN, now 0.25) over the stored
`face.mouth_open` / `face.smile` so the owner can pick the value that meets recall >= 70% at
<= 5% false positives without rerunning the analysis.
"""

from __future__ import annotations

import argparse
import csv
import json
import sqlite3
import sys
from collections import Counter, defaultdict
from pathlib import Path

ISSUE_BYSTANDER = 32
ISSUE_BAD_EXPRESSION = 128
ARTIFICIAL_AT = 0.55  # ip-core analysis/scoring.rs ARTIFICIAL_AT
SMILE_BELOW = 0.35  # EXPR_SMILE_BELOW
HALF_BLINK = (0.45, 0.65)
ARTIFICIAL_TAGS = {"statue", "poster", "screen", "billboard", "painting", "print", "mannequin", "mural"}
VARIANTS = ("eyes-closed", "laugh", "gaze-away", "mid-talk", "blur", "shake", "best", "base")
TARGET_MISCLASS = 0.10
TARGET_RECALL = 0.70
TARGET_FP = 0.05


def norm(p: str) -> str:
    return p.replace("\\", "/").lower()


def ratio(a: float, b: float) -> float:
    return a / b if b else float("nan")


def iou(a, b) -> float:
    ix = max(0.0, min(a[0] + a[2], b[0] + b[2]) - max(a[0], b[0]))
    iy = max(0.0, min(a[1] + a[3], b[1] + b[3]) - max(a[1], b[1]))
    inter = ix * iy
    union = a[2] * a[3] + b[2] * b[3] - inter
    return inter / union if union > 0 else 0.0


def quantiles(xs: list[float]) -> str:
    xs = sorted(x for x in xs if x is not None)
    if not xs:
        return "n=0"
    q = lambda f: xs[min(len(xs) - 1, int(f * len(xs)))]  # noqa: E731
    return f"n={len(xs):<5} min={xs[0]:.2f} p10={q(.1):.2f} p50={q(.5):.2f} p90={q(.9):.2f} max={xs[-1]:.2f}"


def load(library: Path, data_dir: Path):
    manifest = json.loads((library / "manifest.json").read_text(encoding="utf-8"))
    by_path = {}
    for im in manifest["images"]:
        rel = norm(im["jpeg"])
        by_path[rel[len("jpeg/"):] if rel.startswith("jpeg/") else rel] = im
    db = sqlite3.connect(f"file:{data_dir / 'catalog.db'}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    cols = {r["name"] for r in db.execute("PRAGMA table_info(face)")}
    missing = {"artificial", "mouth_open"} - cols
    if missing:
        sys.exit(f"catalog has no face.{'/'.join(sorted(missing))}: analyse again with this build (schema v15)")
    photos = {}
    for r in db.execute("SELECT id, rel_path, file_name, issues, ai_score FROM photo"):
        im = by_path.get(norm(r["rel_path"]))
        if im is not None:
            photos[r["id"]] = (dict(r), im)
    faces = defaultdict(list)
    for r in db.execute(
        "SELECT id, photo_id, bbox_x, bbox_y, bbox_w, bbox_h, sharpness, eyes_open, smile, mouth_open,"
        " is_subject, is_bystander, artificial FROM face"
    ):
        if r["photo_id"] in photos:
            faces[r["photo_id"]].append(dict(r))
    comp = {
        r["photo_id"]: r["composition"]
        for r in db.execute("SELECT photo_id, composition FROM analysis")
        if r["photo_id"] in photos
    }
    return photos, faces, comp, len(by_path)


def predicted(f) -> str:
    if (f["artificial"] or 0.0) >= ARTIFICIAL_AT:
        return "artificial"
    return "subject" if f["is_subject"] else "bystander"


def load_labels(path: Path):
    out = defaultdict(list)
    with path.open(newline="", encoding="utf-8") as fh:
        for row in csv.DictReader(fh):
            box = [float(row[k]) for k in ("x", "y", "w", "h")]
            out[norm(row["photo"])].append((box, row["kind"].strip()))
    return out


def variant_of(tags: set[str]) -> str | None:
    return next((v for v in VARIANTS if v in tags), None)


def faces_section(photos, faces, labels, out) -> None:
    print("\n== 1. faces: subject / bystander / artificial")
    cls = Counter(predicted(f) for fs in faces.values() for f in fs)
    print(f"  faces {sum(cls.values())}: " + ", ".join(f"{k} {v}" for k, v in sorted(cls.items())))
    out["predicted"] = dict(cls)
    if labels:
        conf: Counter = Counter()
        for pid, fs in faces.items():
            row, im = photos[pid]
            cands = labels.get(norm(im["name"])) or labels.get(norm(row["file_name"])) or []
            for f in fs:
                box = [f["bbox_x"], f["bbox_y"], f["bbox_w"], f["bbox_h"]]
                best = max(cands, key=lambda c: iou(box, c[0]), default=None)
                if best is not None and iou(box, best[0]) >= 0.3:
                    conf[(best[1], predicted(f))] += 1
        n = sum(conf.values())
        wrong = sum(v for (t, p), v in conf.items() if t != p)
        print(f"  labelled faces matched: {n}")
        kinds = ("subject", "bystander", "artificial")
        print("  truth \\ predicted  " + "  ".join(f"{k:>10}" for k in kinds))
        for t in kinds:
            print(f"  {t:<17} " + "  ".join(f"{conf[(t, p)]:>10}" for p in kinds))
        # subject vs bystander is the subject_flags question (bench/eval-standard.py); the P1-8
        # risk is artificial faces tagged as people and real faces called artificial
        rate = ratio(wrong, n)
        print(f"  misclassified {wrong}/{n} = {rate:.1%}  (target <= {TARGET_MISCLASS:.0%}): "
              + ("PASS" if rate <= TARGET_MISCLASS else "FAIL"))
        art_as_by = conf[("artificial", "bystander")] + conf[("artificial", "subject")]
        print(f"  artificial faces treated as people: {art_as_by}/{sum(conf[('artificial', p)] for p in kinds)}")
        print(f"  real faces called artificial: {conf[('subject', 'artificial')] + conf[('bystander', 'artificial')]}")
        out["labelled"] = {"n": n, "wrong": wrong, "rate": rate,
                           "confusion": {f"{t}->{p}": v for (t, p), v in conf.items()}}
    # manifest-tag proxy
    tagged = flagged = other = other_flagged = 0
    for pid, fs in faces.items():
        tags = set(photos[pid][1].get("tags", []))
        art = bool(tags & ARTIFICIAL_TAGS)
        for f in fs:
            is_art = (f["artificial"] or 0.0) >= ARTIFICIAL_AT
            if art:
                tagged += 1
                flagged += is_art
            else:
                other += 1
                other_flagged += is_art
    print(f"  manifest-tag proxy: faces in photos tagged {sorted(ARTIFICIAL_TAGS)}: {flagged}/{tagged} flagged"
          f" ({ratio(flagged, tagged):.1%}); faces elsewhere flagged: {other_flagged}/{other} ({ratio(other_flagged, other):.1%})")
    out["proxy"] = {"tagged_faces": tagged, "tagged_flagged": flagged, "other_faces": other, "other_flagged": other_flagged}
    by_tag = sum(1 for pid in photos if photos[pid][0]["issues"] & ISSUE_BYSTANDER)
    print(f"  photos with the bystander tag (bit 32): {by_tag}/{len(photos)}")
    out["bystander_tagged_photos"] = by_tag


def artificial_section(faces, out) -> None:
    print("\n== 2. face.artificial distribution")
    xs = [f["artificial"] for fs in faces.values() for f in fs if f["artificial"] is not None]
    print("  all faces:   " + quantiles(xs))
    print(f"  at or above {ARTIFICIAL_AT}: {sum(x >= ARTIFICIAL_AT for x in xs)}/{len(xs)}")
    for lo in (0.3, 0.4, 0.5, 0.55, 0.6, 0.7, 0.8):
        print(f"    >= {lo:.2f}: {sum(x >= lo for x in xs)}")
    out["artificial_quantiles"] = quantiles(xs)


def talking(f, mouth_at: float) -> bool:
    if f["smile"] is None or f["smile"] >= SMILE_BELOW:
        return False
    if f["mouth_open"] is not None and f["mouth_open"] >= mouth_at:
        return True
    return f["eyes_open"] is not None and HALF_BLINK[0] <= f["eyes_open"] < HALF_BLINK[1]


def expression_section(photos, faces, out) -> None:
    print("\n== 3. bad_expression (bit 128) vs manifest `mid-talk`")
    rows = []
    for pid, (row, im) in photos.items():
        tags = set(im.get("tags", []))
        has_lm = any(f["is_subject"] and f["smile"] is not None for f in faces.get(pid, []))
        rows.append((pid, tags, has_lm, bool(row["issues"] & ISSUE_BAD_EXPRESSION)))
    truth = [r for r in rows if "mid-talk" in r[1] and r[2]]
    rest = [r for r in rows if "mid-talk" not in r[1] and r[2]]
    tp = sum(r[3] for r in truth)
    fp = sum(r[3] for r in rest)
    rec, fpr = ratio(tp, len(truth)), ratio(fp, len(rest))
    print(f"  photos with landmarks: mid-talk {len(truth)}, other {len(rest)}")
    print(f"  recall {tp}/{len(truth)} = {rec:.1%} (target >= {TARGET_RECALL:.0%}): " + ("PASS" if rec >= TARGET_RECALL else "FAIL"))
    print(f"  false positives {fp}/{len(rest)} = {fpr:.1%} (target <= {TARGET_FP:.0%}): " + ("PASS" if fpr <= TARGET_FP else "FAIL"))
    by_variant = defaultdict(Counter)
    for _pid, tags, has_lm, flagged in rows:
        if has_lm:
            v = variant_of(tags) or "other"
            by_variant[v]["n"] += 1
            by_variant[v]["flagged"] += flagged
    print("  flagged by variant: " + ", ".join(f"{v} {c['flagged']}/{c['n']}" for v, c in sorted(by_variant.items())))
    print("\n  mouth-open threshold sweep (smile < %.2f; half-blink rule unchanged), photo level:" % SMILE_BELOW)
    print("   mouth_at  recall   false-pos")
    sweep = []
    for at in (0.08, 0.10, 0.15, 0.20, 0.25, 0.30, 0.40, 0.50):
        t = sum(any(talking(f, at) for f in faces[r[0]] if f["is_subject"]) for r in truth)
        o = sum(any(talking(f, at) for f in faces[r[0]] if f["is_subject"]) for r in rest)
        sweep.append({"mouth_at": at, "recall": ratio(t, len(truth)), "fp": ratio(o, len(rest))})
        print(f"   {at:>7.2f}  {ratio(t, len(truth)):>6.1%}  {ratio(o, len(rest)):>9.1%}")
    mo = lambda rs: [f["mouth_open"] for r in rs for f in faces[r[0]] if f["is_subject"] and f["mouth_open"] is not None]  # noqa: E731
    print("  subject mouth_open, mid-talk photos: " + quantiles(mo(truth)))
    print("  subject mouth_open, other photos:    " + quantiles(mo(rest)))
    out["expression"] = {"recall": rec, "false_positive_rate": fpr, "sweep": sweep,
                         "by_variant": {k: dict(v) for k, v in by_variant.items()}}


def composition_section(photos, comp, out) -> None:
    print("\n== 4. composition (analysis.composition, informational)")
    xs = [c for c in comp.values() if c is not None]
    print("  all photos:  " + quantiles(xs))
    by_scene = defaultdict(list)
    for pid, c in comp.items():
        if c is not None:
            scene = next((t for t in photos[pid][1].get("tags", []) if t in ("portrait", "group", "landscape", "night", "food", "pet")), "other")
            by_scene[scene].append(c)
    for s, v in sorted(by_scene.items()):
        print(f"  {s:<10} " + quantiles(v))
    out["composition"] = quantiles(xs)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("library", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("--labels", type=Path, help="per-face truth CSV: photo,x,y,w,h,kind")
    ap.add_argument("--json", type=Path)
    a = ap.parse_args()
    photos, faces, comp, total = load(a.library, a.data_dir)
    print(f"matched {len(photos)} / {total} manifest images, {sum(len(v) for v in faces.values())} faces")
    out: dict = {"matched": len(photos), "manifest_images": total}
    faces_section(photos, faces, load_labels(a.labels) if a.labels else None, out)
    artificial_section(faces, out)
    expression_section(photos, faces, out)
    composition_section(photos, comp, out)
    if a.json:
        a.json.write_text(json.dumps(out, indent=2, default=str), encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
