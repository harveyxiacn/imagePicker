"""Score a `standard`-profile catalog against the synthetic photo library's manifest.

Usage:
    python bench/eval-standard.py LIBRARY_DIR DATA_DIR [--gpu-csv GPU.csv] [--json OUT.json]

LIBRARY_DIR holds manifest.json (bench/data/qwen-photo-library); DATA_DIR is the --data-dir of
`imagepicker analyze LIBRARY_DIR/jpeg --profile standard --data-dir DATA_DIR`. Complements
bench/eval-library.py (issue tags, burst pairs, best frame) with what only the AI profiles produce:
closed eyes per variant, face -> person clustering against the actor ids a1..a8, expression
signals (eyes_open / smile / gaze) of the burst variants against their base frame, best frame
vs the `best` tag, burst and scene grouping pairs, scene types, stars, and GPU use.

Manifest tags are prompt intent, not verified ground truth (see the library's visual-review.json:
one `eyes-closed` group variant closed two people's eyes), so treat the numbers as indicative.
Standard library only.
"""

from __future__ import annotations

import argparse
import csv
import json
import sqlite3
import statistics
import sys
from collections import Counter, defaultdict
from pathlib import Path

EYES_CLOSED_BELOW = 0.45  # ip-core analysis/scoring.rs EYES_CLOSED_BELOW
VARIANTS = ("eyes-closed", "laugh", "gaze-away", "mid-talk", "blur", "shake", "best", "base")
ACTORS = [f"a{i}" for i in range(1, 9)]


def norm(p: str) -> str:
    return p.replace("\\", "/").lower()


def fmt(x, nd=2):
    if x is None:
        return "-"
    if isinstance(x, float):
        return f"{x:.{nd}f}"
    return str(x)


def ratio(a, b):
    return a / b if b else float("nan")


def stats(xs):
    xs = [x for x in xs if x is not None]
    if not xs:
        return {"n": 0}
    xs.sort()
    return {"n": len(xs), "mean": statistics.fmean(xs), "p10": xs[int(0.1 * (len(xs) - 1))],
            "median": statistics.median(xs), "p90": xs[int(0.9 * (len(xs) - 1))]}


def sfmt(s):
    if not s.get("n"):
        return "n=0"
    return f"n={s['n']:<4} mean={s['mean']:.3f} p10={s['p10']:.3f} median={s['median']:.3f} p90={s['p90']:.3f}"


def variant_of(tags: set[str]) -> str | None:
    for v in VARIANTS:
        if v in tags:
            return v
    return None


def victims(tags: set[str]) -> list[str]:
    return [t.split(":", 1)[1] for t in tags if t.startswith("victim:")]


def iou(a, b):
    ax, ay, aw, ah = a
    bx, by, bw, bh = b
    ix = max(0.0, min(ax + aw, bx + bw) - max(ax, bx))
    iy = max(0.0, min(ay + ah, by + bh) - max(ay, by))
    inter = ix * iy
    u = aw * ah + bw * bh - inter
    return inter / u if u > 0 else 0.0


def pair_counts(items, truth_key, pred_key):
    """Pairwise precision / recall of a clustering: items are dicts, keys give the labels (None
    = singleton)."""
    tp = tpairs = ppairs = 0
    n = len(items)
    for i in range(n):
        ti, pi = truth_key(items[i]), pred_key(items[i])
        for j in range(i + 1, n):
            tj, pj = truth_key(items[j]), pred_key(items[j])
            t = ti is not None and ti == tj
            p = pi is not None and pi == pj
            tpairs += t
            ppairs += p
            tp += t and p
    prec, rec = ratio(tp, ppairs), ratio(tp, tpairs)
    f1 = 2 * prec * rec / (prec + rec) if prec == prec and rec == rec and prec + rec else float("nan")
    return {"truth_pairs": tpairs, "pred_pairs": ppairs, "both": tp, "precision": prec,
            "recall": rec, "f1": f1}


def load(library: Path, data_dir: Path):
    manifest = json.loads((library / "manifest.json").read_text(encoding="utf-8"))
    by_path = {}
    for im in manifest["images"]:
        rel = norm(im["jpeg"])
        by_path[rel[len("jpeg/"):] if rel.startswith("jpeg/") else rel] = im
    db = sqlite3.connect(f"file:{data_dir / 'catalog.db'}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    photos = {}
    for r in db.execute(
        "SELECT p.id, p.rel_path, p.issues, p.ai_rating, p.ai_score, p.burst_id, p.rank_in_burst,"
        " p.scene_type, p.face_count, p.subject_face_count, b.best_photo_id, b.scene_id, b.size AS burst_size,"
        " a.profile, a.aesthetic, a.iqa, a.sharpness, a.model_versions, a.analyzed_at"
        " FROM photo p LEFT JOIN analysis a ON a.photo_id = p.id LEFT JOIN burst b ON b.id = p.burst_id"
    ):
        im = by_path.get(norm(r["rel_path"]))
        if im is not None:
            photos[r["id"]] = (dict(r), im)
    faces = defaultdict(list)
    for f in db.execute(
        "SELECT f.*, pe.singleton, pe.name AS person_name FROM face f"
        " LEFT JOIN person pe ON pe.id = f.person_id ORDER BY f.photo_id, f.idx"
    ):
        d = dict(f)
        d.pop("embedding", None)
        d["bbox"] = (d["bbox_x"], d["bbox_y"], d["bbox_w"], d["bbox_h"])
        d["area"] = (d["bbox_w"] or 0) * (d["bbox_h"] or 0)
        faces[d["photo_id"]].append(d)
    persons = {r["id"]: dict(r) for r in db.execute("SELECT id, name, singleton, hidden FROM person")}
    return manifest, photos, faces, persons


def main_face(fs):
    """The subject face of a single-person photo: the largest subject face, else the largest."""
    if not fs:
        return None
    subj = [f for f in fs if f["is_subject"]]
    return max(subj or fs, key=lambda f: f["area"])


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("library", type=Path)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("--gpu-csv", type=Path, help="nvidia-smi -lms log: timestamp,util,mem,power")
    ap.add_argument("--json", type=Path)
    ap.add_argument("--faces-csv", type=Path, help="per-face dump with manifest labels")
    a = ap.parse_args()
    manifest, photos, faces, persons = load(a.library, a.data_dir)
    out: dict = {"matched": len(photos), "manifest_images": len(manifest["images"])}
    print(f"matched {len(photos)} / {len(manifest['images'])} manifest images")
    analyzed = [r for r, _ in photos.values() if r["analyzed_at"]]
    prof = Counter(r["profile"] for r in analyzed)
    print(f"analysed {len(analyzed)} ({dict(prof)})")
    mv = Counter()
    for r in analyzed:
        for k, v in json.loads(r["model_versions"] or "{}").items():
            mv[f"{k}={v}"] += 1
    print("model versions: " + ", ".join(f"{k} ({v})" for k, v in sorted(mv.items())))
    out["model_versions"] = dict(mv)

    # ------------------------------------------------------------------ closed eyes
    print("\n== closed_eyes (issue bit 1) vs manifest `eyes-closed`")
    rows = []
    for pid, (r, im) in photos.items():
        t = set(im.get("tags", []))
        rows.append((pid, r, im, t, "eyes-closed" in t, bool((r["issues"] or 0) & 1)))
    tp = sum(1 for *_, tr, pr in rows if tr and pr)
    fp = sum(1 for *_, tr, pr in rows if pr and not tr)
    fn = sum(1 for *_, tr, pr in rows if tr and not pr)
    print(f"  TP {tp} FP {fp} FN {fn}  precision {ratio(tp, tp + fp):.2f} recall {ratio(tp, tp + fn):.2f}")
    by_kind = defaultdict(Counter)
    for _pid, _r, im, t, _tr, pr in rows:
        if im.get("phase") == "bursts":
            kind = ("group-burst " if "group" in t else "single-burst ") + (variant_of(t) or "?")
        else:
            kind = im.get("phase")
        by_kind[kind]["n"] += 1
        by_kind[kind]["flagged"] += pr
    print("  flagged closed_eyes by phase / burst variant:")
    for k in sorted(by_kind):
        c = by_kind[k]
        print(f"    {k:<26} {c['flagged']:>3}/{c['n']:<3}")
    fn_names = [im["name"] for _, r, im, t, tr, pr in rows if tr and not pr]
    fp_names = [im["name"] for _, r, im, t, tr, pr in rows if pr and not tr]
    print("  missed: " + ", ".join(fn_names[:40]))
    print("  false positives: " + ", ".join(fp_names[:60]))
    out["closed_eyes"] = {"tp": tp, "fp": fp, "fn": fn, "precision": ratio(tp, tp + fp),
                          "recall": ratio(tp, tp + fn),
                          "by_kind": {k: dict(v) for k, v in by_kind.items()},
                          "missed": fn_names, "false_positives": fp_names}

    # ------------------------------------------------------------------ faces / people
    print("\n== faces and people")
    nfaces = sum(len(v) for v in faces.values())
    with_person = sum(1 for v in faces.values() for f in v if f["person_id"] is not None)
    subj = sum(1 for v in faces.values() for f in v if f["is_subject"])
    nonsingle = {p for p, d in persons.items() if not d["singleton"]}
    print(f"  faces {nfaces} (subject {subj}), assigned to a person {with_person}, unknown (no person) {nfaces - with_person}")
    print(f"  persons {len(persons)} ({len(nonsingle)} multi-photo, {len(persons) - len(nonsingle)} singleton)")
    # face count vs expected people
    fc_rows = []
    for pid, (_r, im) in photos.items():
        t = set(im.get("tags", []))
        ph = im.get("phase")
        exp = None
        if ph in ("actors", "people") or (ph == "bursts" and "group" not in t) or (ph == "edge" and "single" in t):
            exp = 1
        elif "group" in t:
            nums = [int(x) for x in t if x.isdigit()]
            exp = nums[0] if nums else len(im.get("actors", []))
        elif ph in ("places", "things", "defects"):
            exp = None
        if exp is not None:
            fc_rows.append((im, exp, len(faces.get(pid, [])), sum(1 for f in faces.get(pid, []) if f["is_subject"])))
    def fc_summary(sel, label):
        sel = list(sel)
        exact = sum(1 for _, e, n, _ in sel if n == e)
        more = sum(1 for _, e, n, _ in sel if n > e)
        less = sum(1 for _, e, n, _ in sel if n < e)
        print(f"  {label:<28} photos {len(sel):<4} faces==expected {exact:<4} more {more:<4} fewer {less}")
        return {"photos": len(sel), "exact": exact, "more": more, "fewer": less}
    out["face_count"] = {
        "single": fc_summary((x for x in fc_rows if x[1] == 1), "single-person photos"),
        "group": fc_summary((x for x in fc_rows if x[1] > 1), "group photos (tag count)"),
    }
    # people photos with no face at all
    zero = [im["name"] for im, e, n, _ in fc_rows if n == 0]
    print(f"  people photos with no face detected: {len(zero)} {zero[:20]}")
    nofaces_expected = [(im, len(faces.get(pid, []))) for pid, (r, im) in photos.items()
                        if im.get("phase") in ("places", "things", "defects")]
    print(f"  places/things/defects with faces: {sum(1 for _, n in nofaces_expected if n)} / {len(nofaces_expected)}")

    # subject / bystander split in group photos whose people are all actors (everyone is meant)
    gs_faces = gs_by = gs_photos = gs_photos_by = 0
    by_pos = Counter()
    for pid, (_r, im) in photos.items():
        t = set(im.get("tags", []))
        nums = [int(x) for x in t if x.isdigit()]
        n_people = nums[0] if nums else len(im.get("actors") or [])
        if "group" not in t or not im.get("actors") or n_people != len(im["actors"]):
            continue
        fs = faces.get(pid, [])
        if not fs:
            continue
        gs_photos += 1
        gs_faces += len(fs)
        nb = sum(1 for f in fs if not f["is_subject"])
        gs_by += nb
        gs_photos_by += nb > 0
        for f in fs:
            cx = f["bbox_x"] + f["bbox_w"] / 2
            pos = "edge" if cx < 0.2 or cx > 0.8 else "centre"
            by_pos[(pos, bool(f["is_subject"]))] += 1
    print(f"  group photos where every person is an actor: {gs_photos}; faces {gs_faces}, not subject"
          f" (bystander) {gs_by} ({ratio(gs_by, gs_faces):.2f}); photos with >=1 member demoted {gs_photos_by}")
    print(f"    by horizontal position: edge (cx<0.2|>0.8) subject {by_pos[('edge', True)]} / not {by_pos[('edge', False)]};"
          f" centre subject {by_pos[('centre', True)]} / not {by_pos[('centre', False)]}")
    out["group_subjects"] = {"photos": gs_photos, "faces": gs_faces, "not_subject": gs_by,
                             "photos_with_demoted": gs_photos_by,
                             "by_pos": {f"{k[0]}:{'subject' if k[1] else 'not'}": v for k, v in by_pos.items()}}
    # closed-eyes misses: a face with eyes_open below the threshold that is not a subject
    expl = []
    for name in fn_names:
        pid = next(p for p, (r, im) in photos.items() if im["name"] == name)
        fs = faces.get(pid, [])
        closed = [f for f in fs if f["eyes_open"] is not None and f["eyes_open"] < EYES_CLOSED_BELOW]
        expl.append(f"{name}: faces {len(fs)}, eyes_open<{EYES_CLOSED_BELOW} on {len(closed)}"
                    f" (subject {sum(1 for f in closed if f['is_subject'])}), min eyes_open "
                    f"{fmt(min((f['eyes_open'] for f in fs if f['eyes_open'] is not None), default=None))}")
    print("  closed-eyes misses: " + "; ".join(expl))
    out["closed_eyes"]["missed_detail"] = expl

    # single-person photos: label the subject face with the actor
    labelled = []
    for pid, (_r, im) in photos.items():
        acts = im.get("actors") or [t for t in im.get("tags", []) if t in ACTORS]
        t = set(im.get("tags", []))
        if len(acts) != 1 or "group" in t:
            continue
        f = main_face(faces.get(pid, []))
        if f is None:
            continue
        labelled.append({**f, "actor": acts[0], "phase": im.get("phase"), "name": im["name"]})
    print(f"\n  single-person photos with a face: {len(labelled)}")
    pc = pair_counts(labelled, lambda f: f["actor"], lambda f: f["person_id"])
    print(f"  pairwise (same actor <-> same person): precision {pc['precision']:.3f} recall {pc['recall']:.3f} F1 {pc['f1']:.3f}"
          f"  (truth pairs {pc['truth_pairs']}, predicted {pc['pred_pairs']})")
    clusters = defaultdict(Counter)
    for f in labelled:
        clusters[f["person_id"] if f["person_id"] is not None else ("none", f["id"])][f["actor"]] += 1
    n = len(labelled)
    purity = sum(max(c.values()) for c in clusters.values()) / n if n else float("nan")
    by_actor = defaultdict(Counter)
    for f in labelled:
        by_actor[f["actor"]][f["person_id"] if f["person_id"] is not None else ("none", f["id"])] += 1
    completeness = sum(max(c.values()) for c in by_actor.values()) / n if n else float("nan")
    nclusters = len([k for k in clusters if not isinstance(k, tuple)])
    unknown = sum(1 for f in labelled if f["person_id"] is None)
    print(f"  purity {purity:.3f}  completeness (inverse purity) {completeness:.3f}  clusters {nclusters}"
          f"  unassigned subject faces {unknown}")
    # actor -> clusters, cluster -> actors
    actor_map = {}
    print("  actor  faces  clusters (largest first: person_id x faces)")
    for act in ACTORS:
        c = by_actor.get(act, Counter())
        top = ", ".join(f"{k if not isinstance(k, tuple) else 'none'}x{v}" for k, v in c.most_common(6))
        print(f"    {act:<5} {sum(c.values()):>5}  {len(c):>3}: {top}")
    cluster_actor = {}
    for k, c in clusters.items():
        if isinstance(k, tuple):
            continue
        act, cnt = c.most_common(1)[0]
        cluster_actor[k] = act
    mixed = {k: dict(c) for k, c in clusters.items() if not isinstance(k, tuple) and len(c) > 1}
    print(f"  clusters mixing actors: {len(mixed)} {list(mixed.items())[:10]}")
    out["people_single"] = {**pc, "faces": n, "purity": purity, "completeness": completeness,
                            "clusters": nclusters, "unassigned": unknown, "mixed_clusters": mixed,
                            "actor_clusters": {a: len(by_actor.get(a, ())) for a in ACTORS}}
    actor_map = cluster_actor

    # group photos: actors found through the clusters learned on single-person photos
    grp = []
    for pid, (_r, im) in photos.items():
        t = set(im.get("tags", []))
        if "group" not in t:
            continue
        exp = set(im.get("actors") or [])
        fs = faces.get(pid, [])
        mapped = [actor_map.get(f["person_id"]) for f in fs]
        found = {m for m in mapped if m}
        wrong = sum(1 for m in mapped if m and m not in exp)
        dup = sum(v - 1 for v in Counter(m for m in mapped if m).values() if v > 1)
        grp.append({"name": im["name"], "phase": im["phase"], "expected": len(exp), "faces": len(fs),
                    "found": len(found & exp), "wrong": wrong, "dup": dup,
                    "unassigned": sum(1 for f in fs if f["person_id"] is None),
                    "other_cluster": sum(1 for f, m in zip(fs, mapped, strict=False) if f["person_id"] is not None and not m)})
    if grp:
        e = sum(g["expected"] for g in grp)
        print(f"\n  group photos {len(grp)}: actors expected {e}, found via clusters {sum(g['found'] for g in grp)}"
              f" ({ratio(sum(g['found'] for g in grp), e):.2f}); faces {sum(g['faces'] for g in grp)};"
              f" mapped to an actor not in the photo {sum(g['wrong'] for g in grp)};"
              f" same actor twice {sum(g['dup'] for g in grp)}; unassigned {sum(g['unassigned'] for g in grp)};"
              f" in clusters without a single-photo actor {sum(g['other_cluster'] for g in grp)}")
        out["people_group"] = {"photos": len(grp), "expected": e,
                               "found": sum(g["found"] for g in grp),
                               "wrong": sum(g["wrong"] for g in grp), "dup": sum(g["dup"] for g in grp),
                               "unassigned": sum(g["unassigned"] for g in grp),
                               "other_cluster": sum(g["other_cluster"] for g in grp)}
    # all faces (singles + groups) labelled through manifest where unambiguous is above; the
    # cluster sizes:
    sizes = Counter()
    for v in faces.values():
        for f in v:
            if f["person_id"] is not None:
                sizes[f["person_id"]] += 1
    big = sorted(sizes.values(), reverse=True)
    print(f"  person sizes (faces): top {big[:12]}  persons with >=5 faces {sum(1 for s in big if s >= 5)}")

    # ------------------------------------------------------------------ expressions
    print("\n== expression signals of burst variants vs their base frame")
    bursts = defaultdict(list)
    for pid, (r, im) in photos.items():
        if im.get("burst"):
            bursts[im["burst"]].append((pid, r, im))
    expr = defaultdict(list)
    for _bid, members in bursts.items():
        base = next(((p, r, im) for p, r, im in members if "base" in im.get("tags", [])), None)
        if base is None:
            continue
        bfaces = faces.get(base[0], [])
        group = "group" in base[2].get("tags", [])
        for p, _r, im in members:
            if p == base[0]:
                continue
            t = set(im.get("tags", []))
            v = variant_of(t)
            vf = faces.get(p, [])
            if not group:
                f, b = main_face(vf), main_face(bfaces)
                if f is None or b is None:
                    expr[("single", v)].append(None)
                    continue
                expr[("single", v)].append((f, b))
            else:
                vic = victims(t)
                # victim face: the face of the victim's cluster, else unknown
                cands = [f for f in vf if actor_map.get(f["person_id"]) in vic]
                f = max(cands, key=lambda x: x["area"]) if cands else None
                b = None
                if f is not None and bfaces:
                    b = max(bfaces, key=lambda x: iou(x["bbox"], f["bbox"]))
                    if iou(b["bbox"], f["bbox"]) < 0.3:
                        b = None
                expr[("group-victim", v)].append((f, b) if f is not None and b is not None else None)
                # biggest change among IoU-matched faces (identity-free)
                best = None
                for ff in vf:
                    if not bfaces:
                        break
                    bb = max(bfaces, key=lambda x: iou(x["bbox"], ff["bbox"]))
                    if iou(bb["bbox"], ff["bbox"]) < 0.3:
                        continue
                    key = {"eyes-closed": "eyes_open", "laugh": "smile", "gaze-away": "gaze"}.get(v, "eyes_open")
                    if ff[key] is None or bb[key] is None:
                        continue
                    d = ff[key] - bb[key]
                    d = -d if v in ("eyes-closed", "gaze-away") else d
                    if best is None or d > best[0]:
                        best = (d, ff, bb)
                expr[("group-maxchange", v)].append((best[1], best[2]) if best else None)
    print("  kind            variant      n  missing  eyes_open(var/base)  smile(var/base)  gaze(var/base)   expected-direction hits")
    out["expressions"] = {}
    for (kind, v) in sorted(expr, key=lambda k: (k[0], str(k[1]))):
        pairs = expr[(kind, v)]
        ok = [x for x in pairs if x]
        def m(field, i, ok=ok):
            xs = [x[i][field] for x in ok if x[i][field] is not None]
            return statistics.fmean(xs) if xs else None
        hits = None
        if v == "eyes-closed":
            hits = sum(1 for f, b in ok if f["eyes_open"] is not None and f["eyes_open"] < EYES_CLOSED_BELOW
                       and (b["eyes_open"] or 0) >= EYES_CLOSED_BELOW)
        elif v == "laugh":
            hits = sum(1 for f, b in ok if f["smile"] is not None and b["smile"] is not None and f["smile"] > b["smile"] + 0.1)
        elif v == "gaze-away":
            hits = sum(1 for f, b in ok if f["gaze"] is not None and b["gaze"] is not None and f["gaze"] < b["gaze"] - 0.1)
        line = (f"  {kind:<15} {str(v):<11} {len(pairs):>3} {len(pairs) - len(ok):>6}   "
                f"{fmt(m('eyes_open', 0))}/{fmt(m('eyes_open', 1))}            {fmt(m('smile', 0))}/{fmt(m('smile', 1))}        "
                f"{fmt(m('gaze', 0))}/{fmt(m('gaze', 1))}        {'' if hits is None else f'{hits}/{len(ok)}'}")
        print(line)
        out["expressions"][f"{kind}:{v}"] = {
            "n": len(pairs), "missing": len(pairs) - len(ok),
            "eyes_open": [m("eyes_open", 0), m("eyes_open", 1)], "smile": [m("smile", 0), m("smile", 1)],
            "gaze": [m("gaze", 0), m("gaze", 1)], "hits": hits}
    # distributions over all subject faces of base frames vs variants (single bursts)
    for field in ("eyes_open", "smile", "gaze"):
        for v in ("base", "best", "eyes-closed", "laugh", "gaze-away", "mid-talk"):
            xs = []
            for _bid, members in bursts.items():
                for p, _r, im in members:
                    t = set(im.get("tags", []))
                    if "group" in t or variant_of(t) != v:
                        continue
                    f = main_face(faces.get(p, []))
                    if f is not None:
                        xs.append(f[field])
            print(f"  single-burst {field:<9} {v:<11} {sfmt(stats(xs))}")

    # ------------------------------------------------------------------ best frame / bursts
    print("\n== bursts: grouping and best frame")
    db_bursts = defaultdict(list)
    for pid, (r, _im) in photos.items():
        if r["burst_id"] is not None:
            db_bursts[r["burst_id"]].append(pid)
    items = [{"t": im.get("burst"), "p": r["burst_id"] if len(db_bursts.get(r["burst_id"], ())) > 1 else None}
             for pid, (r, im) in photos.items()]
    pc = pair_counts(items, lambda x: x["t"], lambda x: x["p"])
    stacked = sum(1 for v in db_bursts.values() if len(v) > 1)
    print(f"  manifest bursts {len(bursts)}, stacks (2+) found {stacked}; pair precision {pc['precision']:.3f}"
          f" recall {pc['recall']:.3f} F1 {pc['f1']:.3f}")
    complete = 0
    for _bid, members in bursts.items():
        ids = {r["burst_id"] for _, r, _ in members}
        if len(ids) == 1 and None not in ids and len(db_bursts[next(iter(ids))]) == len(members):
            complete += 1
    print(f"  manifest bursts recovered exactly (same members): {complete}/{len(bursts)}")
    out["bursts"] = {**pc, "manifest": len(bursts), "stacks": stacked, "exact": complete}
    chosen = Counter()
    strict = lenient = total_best = 0
    best_rank = []
    for _bid, members in bursts.items():
        # the DB burst holding most of this manifest burst
        c = Counter(r["burst_id"] for _, r, _ in members if r["burst_id"] is not None)
        if not c:
            continue
        dbb = c.most_common(1)[0][0]
        best_pid = next((r["best_photo_id"] for _, r, _ in members if r["burst_id"] == dbb), None)
        chosen_im = photos[best_pid][1] if best_pid in photos else None
        tags = set(chosen_im.get("tags", [])) if chosen_im else set()
        kind = ("group " if "group" in tags else "single ") + str(variant_of(tags))
        chosen[kind] += 1
        has_best = any("best" in im.get("tags", []) for _, _, im in members)
        if has_best:
            total_best += 1
            strict += "best" in tags
        lenient += bool(tags & {"best", "base"})
        for _p, r, im in members:
            if "best" in im.get("tags", []) and r["burst_id"] == dbb:
                best_rank.append(r["rank_in_burst"])
    print(f"  best frame == `best` tag: {strict}/{total_best} bursts that have a `best` frame;"
          f" best frame is `best` or `base`: {lenient}/{len(bursts)}")
    print(f"  rank of the `best` frame inside its stack: {Counter(best_rank)}")
    print("  chosen best frame by variant: " + ", ".join(f"{k} {v}" for k, v in chosen.most_common()))
    out["best_frame"] = {"strict": strict, "strict_total": total_best, "lenient": lenient,
                         "bursts": len(bursts), "chosen": dict(chosen), "best_rank": dict(Counter(best_rank))}

    # ------------------------------------------------------------------ scenes
    print("\n== scene grouping (manifest `scene`) and scene types")
    scene_of = {}
    for pid, (r, _im) in photos.items():
        scene_of[pid] = r["scene_id"]
    # photos without a burst have no scene id here; give them their own singleton
    items = [{"t": im.get("scene"), "p": r["scene_id"]} for pid, (r, im) in photos.items()]
    pc = pair_counts(items, lambda x: x["t"], lambda x: x["p"])
    nscenes = len({x["p"] for x in items if x["p"] is not None})
    print(f"  manifest scenes {len({x['t'] for x in items})}, app scenes {nscenes}; pair precision {pc['precision']:.3f}"
          f" recall {pc['recall']:.3f} F1 {pc['f1']:.3f}")
    out["scenes"] = {**pc, "manifest": len({x["t"] for x in items}), "app": nscenes}
    conf = defaultdict(Counter)
    for _pid, (r, im) in photos.items():
        t = set(im.get("tags", []))
        cls = im.get("phase")
        if cls == "bursts":
            cls = "bursts-group" if "group" in t else "bursts-single"
        if "document" in t:
            cls = "things-document"
        conf[cls][r["scene_type"]] += 1
    types = ["portrait", "group", "landscape", "food", "architecture", "night", "pet", "other", None]
    print("  phase \\ scene_type   " + " ".join(f"{str(x)[:6]:>6}" for x in types))
    for cls in sorted(conf):
        print(f"  {cls:<20} " + " ".join(f"{conf[cls].get(x, 0):>6}" for x in types))
    out["scene_types"] = {k: {str(x): v for x, v in c.items()} for k, c in conf.items()}

    # ------------------------------------------------------------------ stars / scores
    print("\n== stars and scores")
    stars = Counter(r["ai_rating"] for r, _ in photos.values())
    print("  ai stars: " + "  ".join(f"{k}*:{v}" for k, v in sorted(stars.items(), key=lambda kv: (kv[0] is None, kv[0] or 0))))
    out["stars"] = {str(k): v for k, v in stars.items()}
    per = defaultdict(list)
    for _pid, (r, im) in photos.items():
        t = set(im.get("tags", []))
        if im.get("phase") == "defects":
            k = "defects"
        elif im.get("phase") == "bursts":
            k = "burst " + str(variant_of(t))
        else:
            k = im.get("phase")
        per[k].append(r)
    print("  class                 n   mean*   aesthetic  iqa")
    out["stars_by_class"] = {}
    for k in sorted(per):
        rs = per[k]
        ms = [x["ai_rating"] for x in rs if x["ai_rating"] is not None]
        ae = [x["aesthetic"] for x in rs if x["aesthetic"] is not None]
        iq = [x["iqa"] for x in rs if x["iqa"] is not None]
        print(f"  {k:<20} {len(rs):>4}  {fmt(statistics.fmean(ms) if ms else None)}    "
              f"{fmt(statistics.fmean(ae) if ae else None, 3)}      {fmt(statistics.fmean(iq) if iq else None, 3)}")
        out["stars_by_class"][k] = {"n": len(rs), "stars": statistics.fmean(ms) if ms else None,
                                    "aesthetic": statistics.fmean(ae) if ae else None,
                                    "iqa": statistics.fmean(iq) if iq else None}

    # ------------------------------------------------------------------ GPU
    if a.gpu_csv and a.gpu_csv.exists():
        util, mem, pw = [], [], []
        for row in csv.reader(a.gpu_csv.read_text(encoding="utf-8", errors="replace").splitlines()):
            try:
                util.append(float(row[1]))
                mem.append(float(row[2]))
                pw.append(float(row[3]))
            except (ValueError, IndexError):
                pass
        if util:
            busy = [u for u in util if u > 5]
            print(f"\n== GPU ({len(util)} samples @0.5 s): util mean {statistics.fmean(util):.1f}% "
                  f"(busy samples {len(busy)}, mean when busy {statistics.fmean(busy) if busy else 0:.1f}%), "
                  f"max {max(util):.0f}%; memory min {min(mem):.0f} max {max(mem):.0f} MiB "
                  f"(delta {max(mem) - min(mem):.0f}); power max {max(pw):.0f} W")
            out["gpu"] = {"samples": len(util), "util_mean": statistics.fmean(util), "util_max": max(util),
                          "busy_samples": len(busy), "mem_min": min(mem), "mem_max": max(mem), "power_max": max(pw)}

    if a.faces_csv:
        with a.faces_csv.open("w", encoding="utf-8", newline="") as fh:
            w = csv.writer(fh)
            w.writerow(["name", "phase", "tags", "face_id", "idx", "person_id", "singleton", "is_subject",
                        "bbox_x", "bbox_y", "bbox_w", "bbox_h", "det_score", "eyes_open", "smile", "gaze",
                        "yaw", "pitch", "expression_score"])
            for pid, (_r, im) in sorted(photos.items(), key=lambda kv: kv[1][1]["name"]):
                for f in faces.get(pid, []):
                    w.writerow([im["name"], im.get("phase"), " ".join(im.get("tags", [])), f["id"], f["idx"],
                                f["person_id"], f["singleton"], f["is_subject"]] +
                               [f[k] for k in ("bbox_x", "bbox_y", "bbox_w", "bbox_h", "det_score", "eyes_open",
                                               "smile", "gaze", "yaw", "pitch", "expression_score")])
    if a.json:
        a.json.write_text(json.dumps(out, indent=2, default=str, ensure_ascii=False), encoding="utf-8")
    return 0


if __name__ == "__main__":
    sys.exit(main())
