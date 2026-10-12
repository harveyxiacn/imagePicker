"""Exercise the AI features end to end through the HTTP API of a running `imagepicker serve`.

Usage (worker venv python; the server must already run on --base):
    ai-worker/.venv/Scripts/python bench/api-features.py --base http://127.0.0.1:7911 \
        --out target/eval/api/out --library bench/data/qwen-photo-library/jpeg [--only masks,beauty]

Each feature prints one JSON line {"feature", "status": "works" | "fails" | "not_available",
"seconds", "evidence" | "error"} and saves its images under --out (previews of the original and the
edited photo, masks, patches, exports) with a numeric check where possible (mean absolute
difference inside / outside the edited region, noise / sharpness measures, output size).
Photo, burst and person ids are looked up by file name in the catalog through the API.
"""

from __future__ import annotations

import argparse
import asyncio
import io
import json
import shutil
import threading
import time
import traceback
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path

import numpy as np
from PIL import Image

BASE = "http://127.0.0.1:7911"
EVENTS: list[dict] = []
EV_LOCK = threading.Lock()


# ------------------------------------------------------------------ http / events
def http(method: str, path: str, body=None, raw: bytes | None = None, ctype: str | None = None,
         timeout: float = 600):
    data = None
    headers = {}
    if body is not None:
        data = json.dumps(body).encode()
        headers["Content-Type"] = "application/json"
    elif raw is not None:
        data = raw
        headers["Content-Type"] = ctype or "application/octet-stream"
    req = urllib.request.Request(BASE + path, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            payload = r.read()
            ct = r.headers.get("Content-Type", "")
            return r.status, (json.loads(payload) if "json" in ct and payload else payload), dict(r.headers)
    except urllib.error.HTTPError as e:
        payload = e.read()
        try:
            payload = json.loads(payload)
        except Exception:
            payload = payload.decode("utf-8", "replace")
        return e.code, payload, dict(e.headers)


def ok(status, payload, what):
    if status >= 400:
        raise RuntimeError(f"{what}: HTTP {status} {json.dumps(payload, ensure_ascii=False)[:600]}")
    return payload


def events_thread(stop: threading.Event):
    import websockets

    async def run():
        uri = BASE.replace("http", "ws") + "/api/events"
        while not stop.is_set():
            try:
                async with websockets.connect(uri, max_size=None) as ws:
                    while not stop.is_set():
                        try:
                            msg = await asyncio.wait_for(ws.recv(), 0.5)
                        except TimeoutError:
                            continue
                        try:
                            ev = json.loads(msg)
                        except Exception:
                            continue
                        evs = ev if isinstance(ev, list) else [ev]
                        with EV_LOCK:
                            for e in evs:
                                e["_t"] = time.time()
                                EVENTS.append(e)
            except Exception:
                await asyncio.sleep(0.5)

    asyncio.run(run())


def wait_event(pred, since: float, timeout: float = 900):
    t_end = time.time() + timeout
    while time.time() < t_end:
        with EV_LOCK:
            for e in EVENTS:
                if e["_t"] >= since and pred(e):
                    return e
        time.sleep(0.2)
    raise TimeoutError(f"no matching event within {timeout:.0f} s")


def wait_task(task_id: str, since: float, timeout: float = 1800):
    e = wait_event(lambda e: e.get("type") == "task.progress" and e.get("task_id") == task_id
                   and e.get("state") in ("done", "failed"), since, timeout)
    return e


# ------------------------------------------------------------------ helpers
def img(payload: bytes) -> np.ndarray:
    return np.asarray(Image.open(io.BytesIO(payload)).convert("RGB")).astype(np.float32)


def render(photo_id: int, original: bool, long_edge: int = 1600, stack=None) -> tuple[np.ndarray, bytes, dict]:
    body = {"photo_id": photo_id, "long_edge": long_edge, "original": original}
    if stack is not None:
        body["stack"] = stack
    st, p, h = http("POST", "/api/render/preview", body)
    ok(st, p, "render/preview")
    return img(p), p, h


def rect_mask(shape, rect):
    h, w = shape[:2]
    x, y, rw, rh = rect
    m = np.zeros((h, w), bool)
    m[max(0, int(y * h)):min(h, int(round((y + rh) * h))), max(0, int(x * w)):min(w, int(round((x + rw) * w)))] = True
    return m


def diff_in_out(a: np.ndarray, b: np.ndarray, m: np.ndarray):
    d = np.abs(a - b).mean(axis=2)
    return {"mad_inside": round(float(d[m].mean()), 3) if m.any() else None,
            "mad_outside": round(float(d[~m].mean()), 3) if (~m).any() else None,
            "changed_px_inside_pct": round(float((d[m] > 4).mean() * 100), 2) if m.any() else None,
            "changed_px_outside_pct": round(float((d[~m] > 4).mean() * 100), 2) if (~m).any() else None}


def noise_sigma(a: np.ndarray) -> float:
    """Immerkaer fast noise estimate on the luma (8-bit units)."""
    g = a @ np.array([0.299, 0.587, 0.114], np.float32)
    k = np.array([[1, -2, 1], [-2, 4, -2], [1, -2, 1]], np.float32)
    h, w = g.shape
    conv = sum(k[i, j] * g[i:h - 2 + i, j:w - 2 + j] for i in range(3) for j in range(3))
    return float(np.sqrt(np.pi / 2) * np.abs(conv).sum() / (6 * (w - 2) * (h - 2)))


def lap_var(a: np.ndarray, m: np.ndarray | None = None) -> float:
    g = a @ np.array([0.299, 0.587, 0.114], np.float32)
    lap = g[1:-1, :-2] + g[1:-1, 2:] + g[:-2, 1:-1] + g[2:, 1:-1] - 4 * g[1:-1, 1:-1]
    if m is not None:
        lap = lap[m[1:-1, 1:-1]]
    return float(lap.var())


def save(out: Path, name: str, payload: bytes) -> str:
    p = out / name
    p.write_bytes(payload)
    return f"{p} ({p.stat().st_size} B)"


class Ctx:
    def __init__(self, out: Path, library: Path):
        self.out = out
        self.library = library
        self.session = None
        self.by_name: dict[str, dict] = {}

    def load(self):
        st, p, _ = http("GET", "/api/sessions")
        sess = ok(st, p, "sessions")["sessions"]
        lib = [s for s in sess if "qwen-photo-library" in (s.get("root_path") or s.get("path") or json.dumps(s))]
        self.session = (lib or sess)[-1]["id"]
        cursor = None
        while True:
            q = f"/api/photos?session_id={self.session}&limit=2000" + (f"&cursor={cursor}" if cursor else "")
            st, p, _ = http("GET", q)
            p = ok(st, p, "photos")
            for ph in p["photos"]:
                self.by_name[ph["file_name"]] = ph
            cursor = p.get("next_cursor")
            if not cursor:
                break

    def pid(self, name: str) -> int:
        return self.by_name[name]["id"]


def analysis(pid):
    st, p, _ = http("GET", f"/api/photos/{pid}/analysis")
    return ok(st, p, "analysis")


# ------------------------------------------------------------------ features
def f_hardware(c: Ctx):
    hw = ok(*http("GET", "/api/system/hardware")[:2], "hardware")
    st = ok(*http("GET", "/api/assistant/status")[:2], "assistant/status")
    models = ok(*http("GET", "/api/models")[:2], "models")["models"]
    inst = [m["id"] for m in models if m.get("installed")]
    return {"hardware": hw, "assistant": st, "installed_models": inst}


ENSURE = ["birefnet-lite-fp16", "skyseg-u2net", "mediapipe-selfie-multiclass",
          "mediapipe-pose-landmarker-full", "lama-big-fp32", "scunet-color-real-psnr", "gfpgan-v1.4",
          "realesrgan-x2-fp16", "realesrgan-x4-fp16"]


def f_models(c: Ctx, ids=None):
    ids = ids or ENSURE
    t0 = time.time()
    p = ok(*http("POST", "/api/models/ensure", {"ids": ids})[:2], "models/ensure")
    e = wait_task(p["task_id"], t0 - 1, 3600)
    if e.get("state") != "done":
        raise RuntimeError(f"models.ensure failed: {e}")
    models = ok(*http("GET", "/api/models")[:2], "models")["models"]
    sizes = {m["id"]: m.get("size_mb") for m in models if m["id"] in ids}
    return {"task": p["task_id"], "ids": ids, "installed": all(m.get("installed") for m in models if m["id"] in ids),
            "size_mb": sizes, "total_mb": round(sum(v or 0 for v in sizes.values()), 1),
            "bytes": e.get("total")}


def f_masks(c: Ctx):
    res = {}
    pid = c.pid("beach-walk-a1.jpg")
    an = analysis(pid)
    person = next((f["person_id"] for f in an["faces"] if f.get("is_subject")), None)
    for target in ["subject", "sky", "person", "skin", "hair", "clothes", "background"]:
        q = f"/api/masks/{pid}?target={target}" + (f"&person_id={person}" if target == "person" and person else "")
        t0 = time.time()
        st, p, h = http("GET", q)
        if st >= 400:
            res[target] = {"error": p, "status": st}
            continue
        m = np.asarray(Image.open(io.BytesIO(p)).convert("L")).astype(np.float32) / 255
        hh = m.shape[0]
        res[target] = {"seconds": round(time.time() - t0, 2), "size": list(m.shape[::-1]),
                       "coverage": round(float(m.mean()), 3),
                       "top_third": round(float(m[: hh // 3].mean()), 3),
                       "bottom_third": round(float(m[2 * hh // 3:].mean()), 3),
                       "file": save(c.out, f"mask_{pid}_{target}.png", p)}
    # two people of one group photo: their person masks should not overlap much
    gid = c.pid("bg01-family-dinner-00.jpg")
    faces = [f for f in analysis(gid)["faces"] if f.get("person_id")]
    pm = {}
    for f in faces[:4]:
        st, p, _ = http("GET", f"/api/masks/{gid}?target=person&person_id={f['person_id']}")
        if st < 400:
            pm[f["person_id"]] = np.asarray(Image.open(io.BytesIO(p)).convert("L")) > 127
            save(c.out, f"mask_{gid}_person{f['person_id']}.png", p)
    ious = []
    keys = list(pm)
    for i in range(len(keys)):
        for j in range(i + 1, len(keys)):
            a, b = pm[keys[i]], pm[keys[j]]
            ious.append(round(float((a & b).sum() / max(1, (a | b).sum())), 4))
    res["group_person_masks"] = {"photo": "bg01-family-dinner-00", "people": keys,
                                 "coverage": {k: round(float(v.mean()), 3) for k, v in pm.items()},
                                 "pairwise_iou": ious}
    failed = [k for k, v in res.items() if isinstance(v, dict) and "error" in v]
    if failed:
        raise RuntimeError(f"targets failed: {failed}: {json.dumps({k: res[k] for k in failed}, ensure_ascii=False)[:800]}")
    return {"photo": "beach-walk-a1.jpg", "photo_id": pid, **res}


def get_stack(pid):
    return ok(*http("GET", f"/api/edits/{pid}")[:2], "edits")["stack"]


def patch_ops(stack, kind=None):
    return [o for o in stack.get("ops", []) if o.get("type") == "patch" and (kind is None or o.get("kind") == kind)]


def f_besttake(c: Ctx, burst_name="bg01-family-dinner-00.jpg"):
    burst = c.by_name[burst_name]["burst_id"]
    plan = ok(*http("GET", f"/api/bursts/{burst}/besttake")[:2], "besttake plan")
    base = plan["base_photo_id"]
    t0 = time.time()
    ok(*http("POST", f"/api/bursts/{burst}/besttake/auto")[:2], "besttake/auto")
    e = wait_event(lambda e: e.get("type") == "besttake.done" and e.get("photo_id") == base, t0 - 1, 900)
    secs = time.time() - t0
    stack = get_stack(base)
    patches = patch_ops(stack, "best_take")
    orig, ob, _ = render(base, True)
    edit, eb, _ = render(base, False)
    m = np.zeros(orig.shape[:2], bool)
    for pt in patches:
        m |= rect_mask(orig.shape, pt["rect"])
    ev = {"burst_id": burst, "base_photo_id": base, "base_name": next((n for n, v in c.by_name.items() if v["id"] == base), None),
          "plan_people": [{"base_face": pp["base_face_id"], "best_photo": pp["best_photo_id"],
                           "candidates": [(cc["photo_id"], cc["expression_score"], cc["composable"], cc["reason"]) for cc in pp["candidates"]]}
                          for pp in plan["people"]],
          "base_choice": plan.get("base_choice"),
          "results": e.get("results"), "patches": [{"asset": pt["asset"], "rect": pt["rect"], "source": pt.get("source_photo_id")} for pt in patches],
          "seconds": round(secs, 1), **diff_in_out(orig, edit, m),
          "original": save(c.out, f"besttake_{base}_original.jpg", ob),
          "edited": save(c.out, f"besttake_{base}_edited.jpg", eb)}
    for pt in patches[:4]:
        st, png, _ = http("GET", f"/api/assets/{base}/{pt['asset']}")
        if st < 400:
            save(c.out, f"besttake_{base}_{pt['asset']}.png", png)
    if not any(r.get("ok") for r in (e.get("results") or [])) or not patches:
        raise RuntimeError(f"no best-take patch composed: {json.dumps(ev, ensure_ascii=False)[:1500]}")
    return ev


def f_besttake_all(c: Ctx):
    """besttake/auto on every burst whose base frame has 2+ faces: how often does it compose?"""
    bursts = {}
    for n, ph in c.by_name.items():
        if n.startswith("bg") and ph.get("burst_id") is not None:
            bursts.setdefault(ph["burst_id"], n.rsplit("-", 1)[0])
    out = {"bursts": len(bursts), "people_planned": 0, "choices": 0, "ok": 0, "reasons": {}, "warnings": {},
           "per_burst": {}}
    for bid, label in sorted(bursts.items()):
        plan = ok(*http("GET", f"/api/bursts/{bid}/besttake")[:2], "plan")
        out["people_planned"] += len(plan["people"])
        t0 = time.time()
        st, p, _ = http("POST", f"/api/bursts/{bid}/besttake/auto")
        if st >= 400:
            out["per_burst"][label] = {"http": st, "error": p}
            continue
        try:
            e = wait_event(lambda e, base=plan["base_photo_id"]: e.get("type") == "besttake.done"
                           and e.get("photo_id") == base, t0 - 1, 600)
        except TimeoutError:
            # nothing to compose: the task finishes without a besttake.done for an empty choice list
            out["per_burst"][label] = {"timeout": True}
            continue
        res = e.get("results") or []
        out["choices"] += len(res)
        out["ok"] += sum(1 for r in res if r.get("ok"))
        for r in res:
            if not r.get("ok"):
                out["reasons"][r.get("reason")] = out["reasons"].get(r.get("reason"), 0) + 1
            for w in r.get("warnings") or []:
                out["warnings"][w] = out["warnings"].get(w, 0) + 1
        bc = plan.get("base_choice") or {}
        if bc:
            out.setdefault("base_is_group_best", 0)
            out["base_is_group_best"] += plan["base_photo_id"] == bc.get("group_best_photo_id")
        out["per_burst"][label] = {"seconds": round(time.time() - t0, 1), "base": plan["base_photo_id"],
                                   "base_reason": bc.get("reason"),
                                   "results": [(r["base_face_id"], r["ok"], r.get("reason"), r.get("warnings")) for r in res]}
    if out["ok"] == 0:
        raise RuntimeError(f"no best take composed on any burst: {json.dumps(out, ensure_ascii=False)[:2500]}")
    return out


def f_besttake_fix(c: Ctx, variant="eyes-closed"):
    """Base = the burst frame where someone has `variant` (manual base override): auto best take
    should replace that person's face from another frame of the burst."""
    manifest = json.loads((c.library.parent / "manifest.json").read_text(encoding="utf-8"))
    frames = [im for im in manifest["images"] if im.get("burst", "").startswith("bg") and variant in im["tags"]]
    out = {"variant": variant, "bursts": 0, "choices": 0, "ok": 0, "victim_replaced": 0, "reasons": {},
           "warnings": {}, "quality": [], "per_burst": {}}
    for im in frames:
        name = Path(im["jpeg"]).name
        ph = c.by_name.get(name)
        if not ph or ph.get("burst_id") is None:
            continue
        bid, base = ph["burst_id"], ph["id"]
        out["bursts"] += 1
        plan = ok(*http("GET", f"/api/bursts/{bid}/besttake?base_photo_id={base}")[:2], "plan base")
        t0 = time.time()
        ok(*http("POST", f"/api/bursts/{bid}/besttake/auto", {"base_photo_id": base})[:2], "auto base")
        e = wait_event(lambda e, base=base: e.get("type") == "besttake.done" and e.get("photo_id") == base,
                       t0 - 1, 600)
        res = e.get("results") or []
        out["choices"] += len(res)
        out["ok"] += sum(1 for r in res if r.get("ok"))
        # the victim: the face whose eyes are closed in the base frame (analysis)
        faces = analysis(base)["faces"]
        closed = {f["id"] for f in faces if f.get("eyes_open") is not None and f["eyes_open"] < 0.45}
        out["victim_replaced"] += any(r.get("ok") and r["base_face_id"] in closed for r in res)
        for r in res:
            if not r.get("ok"):
                out["reasons"][r.get("reason")] = out["reasons"].get(r.get("reason"), 0) + 1
            for w in r.get("warnings") or []:
                out["warnings"][w] = out["warnings"].get(w, 0) + 1
        patches = patch_ops(get_stack(base), "best_take")
        rec = {"base": name, "seconds": round(time.time() - t0, 1), "closed_faces": sorted(closed),
               "planned": [(p["base_face_id"], p["best_photo_id"]) for p in plan["people"] if p["best_photo_id"] != base],
               "results": [(r["base_face_id"], r["ok"], r.get("reason"), r.get("warnings")) for r in res],
               "patches": len(patches)}
        if patches and len(out["quality"]) < 6:
            orig, ob, _ = render(base, True)
            edit, eb, _ = render(base, False)
            m = np.zeros(orig.shape[:2], bool)
            for pt in patches:
                m |= rect_mask(orig.shape, pt["rect"])
            rec.update(diff_in_out(orig, edit, m))
            rec["files"] = [save(c.out, f"besttake_fix_{base}_original.jpg", ob),
                            save(c.out, f"besttake_fix_{base}_edited.jpg", eb)]
            out["quality"].append(rec)
        out["per_burst"][name] = rec
    if out["ok"] == 0:
        raise RuntimeError(f"nothing composed: {json.dumps(out, ensure_ascii=False)[:2500]}")
    return out


def f_inpaint_strokes(c: Ctx, name="pl05-noon.jpg"):
    pid = c.pid(name)
    strokes = [{"points": [[0.45, 0.55], [0.55, 0.6]], "radius": 0.03}]
    t0 = time.time()
    ok(*http("POST", f"/api/photos/{pid}/inpaint", {"strokes": strokes})[:2], "inpaint strokes")
    e = wait_event(lambda e: e.get("type") == "inpaint.done" and e.get("photo_id") == pid, t0 - 1, 900)
    if not e.get("ok"):
        raise RuntimeError(f"inpaint.done not ok: {e}")
    patches = patch_ops(get_stack(pid), "inpaint")
    orig, ob, _ = render(pid, True)
    edit, eb, _ = render(pid, False)
    m = np.zeros(orig.shape[:2], bool)
    for pt in patches:
        m |= rect_mask(orig.shape, pt["rect"])
    return {"photo": name, "seconds": round(time.time() - t0, 1), "strokes": strokes,
            "patches": [{"asset": p["asset"], "rect": p["rect"]} for p in patches], **diff_in_out(orig, edit, m),
            "original": save(c.out, f"inpaint_strokes_{pid}_original.jpg", ob),
            "edited": save(c.out, f"inpaint_strokes_{pid}_edited.jpg", eb)}


def f_inpaint(c: Ctx, name="noon-market-a4.jpg"):
    pid = c.pid(name)
    bys = ok(*http("GET", f"/api/photos/{pid}/bystanders")[:2], "bystanders")["faces"]
    group_bys = ok(*http("GET", f"/api/photos/{c.pid('bg01-family-dinner-00.jpg')}/bystanders")[:2], "bystanders")["faces"]
    t0 = time.time()
    ok(*http("POST", f"/api/photos/{pid}/inpaint", {"bystanders": True})[:2], "inpaint")
    e = wait_event(lambda e: e.get("type") == "inpaint.done" and e.get("photo_id") == pid, t0 - 1, 900)
    secs = time.time() - t0
    if not e.get("ok"):
        raise RuntimeError(f"inpaint.done not ok: {e}")
    patches = patch_ops(get_stack(pid), "inpaint")
    orig, ob, _ = render(pid, True)
    edit, eb, _ = render(pid, False)
    m = np.zeros(orig.shape[:2], bool)
    for b in bys:
        m |= rect_mask(orig.shape, b["bbox"])
    pm = np.zeros(orig.shape[:2], bool)
    for pt in patches:
        pm |= rect_mask(orig.shape, pt["rect"])
    return {"photo": name, "photo_id": pid, "bystanders": bys, "seconds": round(secs, 1),
            "patches": [{"asset": p["asset"], "rect": p["rect"]} for p in patches],
            "bystander_box": diff_in_out(orig, edit, m), "patch_rect": diff_in_out(orig, edit, pm),
            "group_photo_bystanders(bg01-family-dinner-00)": group_bys,
            "original": save(c.out, f"inpaint_{pid}_original.jpg", ob),
            "edited": save(c.out, f"inpaint_{pid}_edited.jpg", eb)}


def f_enhance(c: Ctx, op: str, name: str, strength=None):
    pid = c.pid(name)
    body = {"op": op}
    if strength is not None:
        body["strength"] = strength
    t0 = time.time()
    ok(*http("POST", f"/api/photos/{pid}/enhance", body)[:2], f"enhance {op}")
    e = wait_event(lambda e: e.get("type") == "enhance.done" and e.get("photo_id") == pid and e.get("op") == op, t0 - 1, 900)
    secs = time.time() - t0
    if not e.get("ok"):
        raise RuntimeError(f"enhance.done not ok: {e}")
    patches = patch_ops(get_stack(pid), op)
    orig, ob, _ = render(pid, True, 2048)
    edit, eb, _ = render(pid, False, 2048)
    ev = {"photo": name, "photo_id": pid, "seconds": round(secs, 1),
          "patches": [{"asset": p["asset"], "rect": p["rect"]} for p in patches],
          "original": save(c.out, f"{op}_{pid}_original.jpg", ob),
          "edited": save(c.out, f"{op}_{pid}_edited.jpg", eb)}
    if op == "denoise":
        ev["noise_sigma"] = [round(noise_sigma(orig), 2), round(noise_sigma(edit), 2)]
        ev["mad"] = round(float(np.abs(orig - edit).mean()), 2)
    else:
        m = np.zeros(orig.shape[:2], bool)
        for p in patches:
            m |= rect_mask(orig.shape, p["rect"])
        ev.update(diff_in_out(orig, edit, m))
        ev["face_laplacian_var"] = [round(lap_var(orig, m), 1), round(lap_var(edit, m), 1)] if m.any() else None
    if not patches:
        raise RuntimeError(f"no {op} patch in the edit stack: {ev}")
    return ev


def f_beauty(c: Ctx, name="park-golden-a1.jpg"):
    pid = c.pid(name)
    t0 = time.time()
    st, p, _ = http("POST", f"/api/photos/{pid}/beauty/prepare")
    ok(st, p, "beauty/prepare")
    wait_event(lambda e: e.get("type") == "beauty.ready" and e.get("photo_id") == pid, t0 - 1, 600)
    prep = time.time() - t0
    people = ok(*http("GET", f"/api/photos/{pid}/people")[:2], "photo people")
    face = next(pp for pp in people["people"] if pp.get("is_subject")) if people["people"] else None
    person = face["person_id"] if face else None
    stack = {"version": 1, "ops": [
        {"type": "warp", "kind": "face", "person_id": person, "level": "standard", "slim": 40, "chin": 0, "eyes": 20, "nose": 0},
        {"type": "warp", "kind": "body", "person_id": person, "level": "standard", "arms": 30, "legs": 0, "waist": 30,
         "lengthen_legs": 0, "protect_background": True},
        {"type": "beauty", "person_id": person, "level": "standard", "smooth": 60, "whiten": 30, "blemish": True,
         "eye_brighten": 30, "teeth_whiten": 20, "dark_circles": 30}]}
    ok(*http("PUT", f"/api/edits/{pid}", {"stack": stack})[:2], "put edits")
    orig, ob, _ = render(pid, True)
    edit, eb, _ = render(pid, False)
    m = rect_mask(orig.shape, face["face_box"]) if face else np.zeros(orig.shape[:2], bool)
    only_beauty, bb, _ = render(pid, False, stack={"version": 1, "ops": [stack["ops"][2]]})
    only_warp, wb, _ = render(pid, False, stack={"version": 1, "ops": stack["ops"][:2]})
    return {"photo": name, "photo_id": pid, "prepare_seconds": round(prep, 1),
            "people": people, "stack": stack, "all_ops": diff_in_out(orig, edit, m),
            "beauty_only": diff_in_out(orig, only_beauty, m), "warp_only": diff_in_out(orig, only_warp, m),
            "face_laplacian_var(orig, beauty)": [round(lap_var(orig, m), 1), round(lap_var(only_beauty, m), 1)],
            "original": save(c.out, f"beauty_{pid}_original.jpg", ob),
            "edited": save(c.out, f"beauty_{pid}_edited.jpg", eb),
            "beauty_only_file": save(c.out, f"beauty_{pid}_beauty_only.jpg", bb),
            "warp_only_file": save(c.out, f"beauty_{pid}_warp_only.jpg", wb)}


def f_export_upscale(c: Ctx, names=("park-golden-a1.jpg", "park-golden-a2.jpg"), scale=2):
    dest = c.out / "export"
    if dest.exists():
        shutil.rmtree(dest)
    dest.mkdir(parents=True)
    s = ok(*http("GET", "/api/settings")[:2], "settings")
    roots = list(s.get("roots") or [])
    if str(c.out) not in roots:
        ok(*http("PATCH", "/api/settings", {"roots": roots + [str(c.out)]})[:2], "settings roots")
    ids = [c.pid(n) for n in names]
    t0 = time.time()
    p = ok(*http("POST", "/api/export", {"ids": ids, "dest": str(dest), "upscale": scale, "quality": 92,
                                          "name_template": f"{{name}}_x{scale}"})[:2], "export")
    e = wait_task(p["task_id"], t0 - 1, 1800)
    secs = time.time() - t0
    files = []
    for f in sorted(dest.iterdir()):
        im = Image.open(f)
        files.append({"file": str(f), "bytes": f.stat().st_size, "size": list(im.size)})
    src = [{"name": n, "size": [c.by_name[n]["width"], c.by_name[n]["height"]]} for n in names]
    if e.get("state") != "done" or e.get("error") or len(files) != len(names):
        raise RuntimeError(f"export: {e}; files {files}")
    for f, s_ in zip(files, src, strict=False):
        if f["size"] != [s_["size"][0] * scale, s_["size"][1] * scale]:
            raise RuntimeError(f"upscaled size {f['size']} != {scale}x {s_['size']}")
    return {"seconds": round(secs, 1), "task": e, "sources": src, "outputs": files}


def f_assistant(c: Ctx, message="把闭眼的照片都淘汰", engine=None, execute=True):
    body = {"session_id": c.session, "message": message,
            "context": {"filter": "", "selection": [], "current_photo_id": None,
                        "locale": "zh-CN" if any(ord(ch) > 127 for ch in message) else "en"}}
    if engine:
        body["engine"] = engine
    t0 = time.time()
    plan = ok(*http("POST", "/api/assistant/plan", body)[:2], "assistant/plan")
    plan_s = time.time() - t0
    ev = {"message": message, "engine_requested": engine, "engine_used": plan.get("engine"),
          "plan_seconds": round(plan_s, 2), "plan": plan}
    if not execute or plan.get("unsupported") or not plan.get("steps"):
        return ev
    # snapshot of the flags before
    def flags():
        out = {}
        cursor = None
        while True:
            q = f"/api/photos?session_id={c.session}&limit=2000" + (f"&cursor={cursor}" if cursor else "")
            p = ok(*http("GET", q)[:2], "photos")
            for ph in p["photos"]:
                out[ph["id"]] = (ph.get("user_rating"), ph.get("flag"), ph.get("color_label"))
            cursor = p.get("next_cursor")
            if not cursor:
                return out
    before = flags()
    t0 = time.time()
    ok(*http("POST", "/api/assistant/execute", {"plan_id": plan["plan_id"]})[:2], "assistant/execute")
    done = wait_event(lambda e: e.get("type") == "assistant.done" and e.get("plan_id") == plan["plan_id"], t0 - 1, 900)
    after = flags()
    changed = {k for k in after if after[k] != before.get(k)}
    rejected = [k for k in changed if after[k][1] == -1]
    closed = {ph["id"] for ph in c.by_name.values() if "closed_eyes" in (ph.get("issues") or [])}
    ev.update({"execute_seconds": round(time.time() - t0, 2), "done_ok": done.get("ok"),
               "results": done.get("results"), "changed": len(changed), "rejected": len(rejected),
               "rejected_have_closed_eyes": sum(1 for k in rejected if k in closed),
               "closed_eyes_photos": len(closed)})
    # undo the way the web UI does: restore the `before` values from assistant.done
    undo = done.get("undo") or {}
    restored = 0
    groups: dict[tuple, list[int]] = {}
    for ph in undo.get("photos", []):
        groups.setdefault((ph.get("user_rating"), ph.get("flag"), ph.get("color_label")), []).append(ph["id"])
    for (r, f, cl), ids in groups.items():
        ok(*http("PATCH", "/api/photos", {"ids": ids, "user_rating": r, "flag": f, "color_label": cl})[:2], "undo patch")
        restored += len(ids)
    for ed in undo.get("edits", []):
        ok(*http("PUT", f"/api/edits/{ed['photo_id']}", {"stack": ed["before"]})[:2], "undo edits")
    final = flags()
    ev.update({"undo_photos": restored, "undo_edits": len(undo.get("edits", [])),
               "after_undo_equal_before": all(final[k] == before.get(k) for k in final)})
    if not done.get("ok"):
        raise RuntimeError(f"assistant.done not ok: {json.dumps(ev, ensure_ascii=False)[:1500]}")
    return ev


def f_describe(c: Ctx, name="park-golden-a1.jpg"):
    pid = c.pid(name)
    t0 = time.time()
    st, p, _ = http("POST", "/api/assistant/describe", {"photo_id": pid})
    out = {"status": st, "seconds": round(time.time() - t0, 2), "result": p}
    if st >= 400:
        raise RuntimeError(json.dumps(out, ensure_ascii=False))
    t0 = time.time()
    st, p, _ = http("POST", "/api/assistant/suggest", {"photo_id": pid})
    out["suggest"] = {"status": st, "seconds": round(time.time() - t0, 2), "result": p}
    return out


def multipart(fields: dict, files: dict):
    boundary = uuid.uuid4().hex
    parts = []
    for k, v in fields.items():
        parts.append(f"--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n".encode())
    for k, (fname, data, ct) in files.items():
        parts.append(f"--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"; filename=\"{fname}\"\r\n"
                     f"Content-Type: {ct}\r\n\r\n".encode() + data + b"\r\n")
    parts.append(f"--{boundary}--\r\n".encode())
    return b"".join(parts), f"multipart/form-data; boundary={boundary}"


def f_face_search(c: Ctx):
    res = {}
    for actor in ("a1", "a3", "a8"):
        ref = c.library / "actors" / f"{actor}.jpg"
        data, ct = multipart({"session_id": str(c.session)}, {"image": (ref.name, ref.read_bytes(), "image/jpeg")})
        t0 = time.time()
        p = ok(*http("POST", "/api/faces/search", raw=data, ctype=ct)[:2], "faces/search")
        # the actor photo itself is part of the library: its own face's person is the reference
        own = [f["person_id"] for f in analysis(c.pid(f"{actor}.jpg"))["faces"]]
        sims = p.get("similar_faces", [])
        names = {v["id"]: n for n, v in c.by_name.items()}
        hit_names = [names.get(s["photo_id"], "?") for s in sims[:20]]
        person_of = {}
        for sfc in sims:
            for f in analysis(sfc["photo_id"])["faces"]:
                person_of[f["id"]] = f.get("person_id")
        same = sum(1 for sfc in sims if person_of.get(sfc["face_id"]) in own)
        res[actor] = {"seconds": round(time.time() - t0, 2), "faces_detected": len(p.get("faces_detected", [])),
                      "similar_faces_of_reference_person": f"{same}/{len(sims)}",
                      "similarity_range": [sims[-1]["similarity"], sims[0]["similarity"]] if sims else None,
                      "top_candidates": p.get("candidates", [])[:3], "reference_person": own,
                      "top1_is_reference_person": bool(p.get("candidates")) and p["candidates"][0]["person_id"] in own,
                      "similar_faces": len(sims),
                      "top20_photos_with_actor_in_name": sum(1 for n in hit_names if f"-{actor}" in n or n.startswith(actor + ".")),
                      "top20_photo_names": hit_names}
    fid = analysis(c.pid("beach-walk-a2.jpg"))["faces"][0]["id"]
    p = ok(*http("POST", "/api/faces/search", {"face_id": fid, "session_id": c.session})[:2], "faces/search json")
    res["by_face_id(beach-walk-a2)"] = {"top_candidates": p.get("candidates", [])[:3], "similar_faces": len(p.get("similar_faces", []))}
    if not all(res[a]["top1_is_reference_person"] for a in ("a1", "a3", "a8")):
        raise RuntimeError(f"top candidate is not the actor's person: {json.dumps(res, ensure_ascii=False)[:1500]}")
    return res


XMP_PRE = """<?xpacket begin="﻿" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:dc="http://purl.org/dc/elements/1.1/"
    xmp:Rating="3" xmp:Label="Red">
   <dc:subject><rdf:Bag><rdf:li>pre-existing</rdf:li><rdf:li>lightroom</rdf:li></rdf:Bag></dc:subject>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>
"""


def f_xmp(c: Ctx, names=("park-golden-a1.jpg", "park-golden-a2.jpg", "beach-walk-a1.jpg", "pl01-noon.jpg")):
    import re

    src = c.out / "xmp-src"
    if src.exists():
        shutil.rmtree(src)
    src.mkdir(parents=True)
    for n in names:
        sub = next(p for p in c.library.rglob(n))
        shutil.copy2(sub, src / n)
    (src / (Path(names[0]).stem + ".xmp")).write_text(XMP_PRE, encoding="utf-8")
    s = ok(*http("GET", "/api/settings")[:2], "settings")
    roots = list(s.get("roots") or [])
    patch = {"xmp_mode": "sidecar"}
    if str(c.out) not in roots:
        patch["roots"] = roots + [str(c.out)]
    ok(*http("PATCH", "/api/settings", patch)[:2], "settings xmp")
    sess = ok(*http("POST", "/api/import", {"path": str(src), "recursive": False, "title": "xmp-roundtrip"})[:2], "import")["session"]
    sid = sess["id"]
    for _ in range(300):
        s2 = ok(*http("GET", f"/api/sessions/{sid}")[:2], "session")["session"]
        if s2["import_state"] == "ready":
            break
        time.sleep(0.2)
    photos = {p["file_name"]: p for p in ok(*http("GET", f"/api/photos?session_id={sid}&limit=100")[:2], "photos")["photos"]}
    first = photos[names[0]]
    tags0 = ok(*http("GET", f"/api/photos/{first['id']}/tags")[:2], "tags")["tags"]
    ev = {"session": sid, "read_on_import": {"user_rating": first.get("user_rating"), "color_label": first.get("color_label"),
                                             "tags": tags0}}
    # write: rating / flag / label on the others, then a manual sync
    ids = [photos[n]["id"] for n in names[1:]]
    ok(*http("PATCH", "/api/photos", {"ids": ids[:1], "user_rating": 5, "color_label": "green"})[:2], "patch rating")
    ok(*http("PATCH", "/api/photos", {"ids": ids[1:2], "flag": -1})[:2], "patch reject")
    ok(*http("PATCH", "/api/photos", {"ids": ids[2:3], "user_rating": 2})[:2], "patch rating 2")
    ok(*http("POST", "/api/photos/tags", {"ids": ids[:1], "add": ["imagepicker-test"]})[:2], "tags add")
    time.sleep(3)
    w = ok(*http("POST", "/api/xmp/sync", {"session_id": sid, "direction": "write"})[:2], "xmp write")
    side = {}
    for n in names:
        cands = [src / (Path(n).stem + ".xmp"), src / (n + ".xmp")]
        f = next((p for p in cands if p.exists()), None)
        if f:
            t = f.read_text(encoding="utf-8", errors="replace")
            r = re.search(r'xmp:Rating(?:="|>)(-?\d)', t)
            lab = re.search(r'xmp:Label(?:="|>)([A-Za-z]+)', t)
            side[n] = {"file": f.name, "rating": r.group(1) if r else None, "label": lab.group(1) if lab else None,
                       "keywords": re.findall(r"<rdf:li>([^<]+)</rdf:li>", t), "bytes": f.stat().st_size}
    ev["write"] = {"result": w, "sidecars": side}
    # read: change one sidecar outside the app (rating -> 1) and sync back
    target = names[1]
    f = src / side[target]["file"] if target in side else None
    if f is None:
        raise RuntimeError(f"no sidecar written for {target}: {ev}")
    t = f.read_text(encoding="utf-8")
    t2 = re.sub(r'xmp:Rating="-?\d"', 'xmp:Rating="1"', t)
    t2 = re.sub(r"<xmp:Rating>-?\d</xmp:Rating>", "<xmp:Rating>1</xmp:Rating>", t2)
    time.sleep(1.1)
    f.write_text(t2, encoding="utf-8")
    r = ok(*http("POST", "/api/xmp/sync", {"session_id": sid, "direction": "read"})[:2], "xmp read")
    after = ok(*http("GET", f"/api/photos/{photos[target]['id']}")[:2], "photo")["photo"]
    ev["read"] = {"result": r, "edited_sidecar": f.name, "catalog_rating_after": after.get("user_rating")}
    good = (first.get("user_rating") == 3 and "pre-existing" in tags0 and side.get(names[1], {}).get("rating") == "5"
            and side.get(names[2], {}).get("rating") == "-1" and after.get("user_rating") == 1)
    ev["roundtrip_ok"] = good
    if not good:
        raise RuntimeError(f"xmp round trip mismatch: {json.dumps(ev, ensure_ascii=False)[:2000]}")
    return ev


def f_auto_adjust(c: Ctx, name="pl01-overcast.jpg"):
    pid = c.pid(name)
    t0 = time.time()
    p = ok(*http("POST", f"/api/edits/{pid}/auto", {"mode": "auto"})[:2], "auto")
    return {"photo": name, "seconds": round(time.time() - t0, 2), "adjust": p.get("adjust")}


FEATURES = {
    "hardware": f_hardware,
    "models": f_models,
    "masks": f_masks,
    "besttake": f_besttake,
    "inpaint": f_inpaint,
    "inpaint_lantern": lambda c: f_inpaint(c, "lantern-night-a8.jpg"),
    "inpaint_strokes": f_inpaint_strokes,
    "besttake_all": f_besttake_all,
    "besttake_fix": f_besttake_fix,
    "besttake_fix_gaze": lambda c: f_besttake_fix(c, "gaze-away"),
    "hardware_end": f_hardware,
    "denoise": lambda c: f_enhance(c, "denoise", "df01-noise.jpg"),
    "face_restore": lambda c: f_enhance(c, "face_restore", "park-golden-a2.jpg"),
    "beauty": f_beauty,
    "export_upscale": f_export_upscale,
    "assistant": f_assistant,
    "assistant_en": lambda c: f_assistant(c, "keep 2 per scene and reject the rest", execute=True),
    "assistant_llm": lambda c: f_assistant(c, "把闭眼的照片都淘汰", engine="llm"),
    "assistant_llm_en": lambda c: f_assistant(c, "reject every blurry photo", engine="llm"),
    "describe": f_describe,
    "face_search": f_face_search,
    "auto_adjust": f_auto_adjust,
    "xmp": f_xmp,
}


def main() -> int:
    global BASE
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default=BASE)
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--library", type=Path, required=True)
    ap.add_argument("--only", default="")
    ap.add_argument("--models", default="", help="comma separated ids for the `models` step")
    ap.add_argument("--log", type=Path, help="also append the JSON lines to this file (UTF-8)")
    a = ap.parse_args()
    BASE = a.base.rstrip("/")
    a.out = a.out.resolve()
    a.out.mkdir(parents=True, exist_ok=True)
    stop = threading.Event()
    th = threading.Thread(target=events_thread, args=(stop,), daemon=True)
    th.start()
    time.sleep(1.0)
    c = Ctx(a.out, a.library.resolve())
    c.load()
    names = [n for n in (a.only.split(",") if a.only else FEATURES) if n]
    rc = 0
    for n in names:
        t0 = time.time()
        rec = {"feature": n}
        try:
            fn = FEATURES[n]
            ev = fn(c, a.models.split(",")) if n == "models" and a.models else fn(c)
            rec.update(status="works", evidence=ev)
        except Exception as e:  # noqa: BLE001 - report every failure and continue
            rec.update(status="fails", error=f"{type(e).__name__}: {e}", trace=traceback.format_exc()[-1500:])
            rc = 1
        rec["seconds"] = round(time.time() - t0, 2)
        line = json.dumps(rec, ensure_ascii=False, default=str)
        print(line, flush=True)
        if a.log:
            with a.log.open("a", encoding="utf-8") as fh:
                fh.write(line + "\n")
    errs = [e for e in EVENTS if e.get("type") == "task.progress" and e.get("state") == "failed"]
    ws = [e for e in EVENTS if e.get("type") == "worker.status"]
    line = json.dumps({"feature": "_events", "failed_tasks": errs[-30:], "worker_status": ws[-5:],
                       "event_types": sorted({e.get("type") for e in EVENTS})}, ensure_ascii=False, default=str)
    print(line, flush=True)
    if a.log:
        with a.log.open("a", encoding="utf-8") as fh:
            fh.write(line + "\n")
    stop.set()
    return rc


if __name__ == "__main__":
    raise SystemExit(main())
