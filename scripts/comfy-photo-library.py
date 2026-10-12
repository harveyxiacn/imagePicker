"""Resumable local Qwen 2.1 photo corpus. No cloud API or model downloads.

Uses the v2 scene inventory, native installed INT8 nodes, durable prompt recovery,
and incremental JPEG export. Prompt labels are intentions, not ground truth.
"""
from __future__ import annotations

import argparse
from collections import Counter
from datetime import datetime, timedelta
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import random
import shutil
import subprocess
import sys
import time
import urllib.parse
import urllib.error
import urllib.request

from PIL import Image
from PIL.TiffImagePlugin import IFDRational

SPEC = importlib.util.spec_from_file_location("v2", Path(__file__).with_name("comfy-testset-v2.py"))
v2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(v2)
ROOT = Path(__file__).resolve().parents[1] / "bench/data/qwen-photo-library"
PREFIX = "imagepicker-library-20261008"
DEVICES = [
    ("SONY", "ILCE-7M4", "FE 35mm F1.8", 35, 2.8),
    ("SONY", "ILCE-7M4", "FE 85mm F1.8", 85, 1.8),
    ("Canon", "Canon EOS R6 Mark II", "RF24-70mm F2.8 L IS USM", 50, 4),
    ("Canon", "Canon EOS R5", "RF70-200mm F2.8 L IS USM", 135, 4),
    ("NIKON CORPORATION", "NIKON Z 6_2", "NIKKOR Z 24-70mm f/4 S", 35, 5.6),
    ("NIKON CORPORATION", "NIKON Z 8", "NIKKOR Z 180-600mm f/5.6-6.3 VR", 400, 6.3),
    ("FUJIFILM", "X-T5", "XF23mmF1.4 R LM WR", 23, 2),
    ("FUJIFILM", "X-T5", "XF80mmF2.8 R LM OIS WR Macro", 80, 4),
    ("OM Digital Solutions", "OM-1", "M.ZUIKO DIGITAL ED 12-40mm F2.8 PRO", 25, 4),
    ("Panasonic", "DC-S5M2", "LUMIX S 20-60/F3.5-5.6", 28, 5.6),
    ("RICOH IMAGING COMPANY, LTD.", "RICOH GR III", "GR LENS 18.3mm F2.8", 18.3, 2.8),
    ("Apple", "iPhone 15 Pro", "iPhone 15 Pro back triple camera 6.765mm f/1.78", 6.765, 1.78),
    ("Xiaomi", "Xiaomi 14", "Xiaomi 14 back camera", 6, 1.6),
    ("samsung", "SM-S918B", "Samsung Galaxy S23 Ultra back camera", 6.3, 1.7),
]


def save_json(path, data):
    tmp = path.with_suffix(".tmp")
    tmp.write_text(json.dumps(data, ensure_ascii=False, indent=2), encoding="utf-8")
    tmp.replace(path)


def api(server, path, body=None):
    request = urllib.request.Request(server + path,
        data=None if body is None else json.dumps(body).encode(),
        headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(request, timeout=90) as response:
        return json.load(response)


def jobs():
    all_jobs = v2.build_jobs(random.Random(7))
    # Round robin independent categories, then complete bursts consecutively.
    actors = [j for j in all_jobs if j["phase"] == "actors"]
    buckets = [[j for j in all_jobs if j["phase"] == phase]
               for phase in ("people", "groups", "places", "things", "defects")]
    ordered = actors[:]
    while any(buckets):
        for bucket in buckets:
            if bucket:
                ordered.append(bucket.pop(0))
    ordered += [j for j in all_jobs if j["phase"] == "bursts"]
    scenes = {}
    frames = Counter()
    for j in ordered:
        sid = scenes.setdefault(j["scene"], len(scenes))
        j["device_index"] = sid % len(DEVICES)
        j["taken_at"] = (datetime(2026, 10, 1, 8) + timedelta(minutes=sid * 17,
            seconds=frames[j["scene"]] * (0.4 if j.get("burst") else 25))).isoformat(timespec="milliseconds")
        frames[j["scene"]] += 1
        if j["kind"] == "i2i":
            j["denoise"] = 1.0
            j["edit_mode"] = "qwen-native-reference-latent"
    return ordered


def graph(j, records, steps):
    g = v2.job_graph(j, steps)
    g["1"] = {"class_type": "UNETLoader", "inputs": {
        "unet_name": "qwen_image_2.1_int8_convrot.safetensors", "weight_dtype": "default"}}
    g["2"]["inputs"]["clip_name"] = "qwen3vl_8b_w4a8.safetensors"
    g["8"]["inputs"]["filename_prefix"] = f"{PREFIX}/{j['phase']}/{j['name']}"
    g["4"]["inputs"]["images"] = {}
    # References use saved server output names, never guessed counter suffixes.
    if j["kind"] == "ref":
        for i, actor in enumerate(j["refs"], 1):
            g[f"1{i}"]["inputs"]["image"] = records[actor]["server_image"] + " [output]"
    if j["kind"] == "i2i":
        base = records[j["name"].rsplit("-", 1)[0] + "-00"]
        g["10"]["inputs"]["image"] = base["server_image"] + " [output]"
        g["4"]["inputs"].update({"images.image_1": ["10", 0], "resolution": 0})
        g["6"]["inputs"]["latent_image"] = ["4", 2]
        g.pop("5")
        target = "the person"
        if "group" in j["tags"]:
            victim = next(t.split(":", 1)[1] for t in j["tags"] if t.startswith("victim:"))
            position = j["actors"].index(victim)
            location = "at the far LEFT" if position == 0 else f"number {position+1} from left"
            target = f"only the person {location}, {v2.ACTORS[victim]}"
        changes = {
            "eyes-closed": "has BOTH EYES FULLY CLOSED in a natural blink. Eyelids completely cover both eyes, no iris or pupil visible. Keep the smile",
            "gaze-away": "turns the head slightly and looks clearly to the side, away from the camera",
            "mid-talk": "has the mouth half open mid-sentence and eyes slightly squinted",
            "blur": "has visible local motion blur on the head and hands; keep the background sharp",
            "laugh": "laughs with mouth open and tilts the head back a little",
            "best": "has eyes open and a natural smile, looking at the camera, sharp focus",
        }
        delta = next(value for tag, value in changes.items() if tag in j["tags"])
        g["4"]["inputs"]["prompt"] = (
            f"Edit <image1> only: {target} {delta}. All other people remain unchanged. "
            "Preserve the exact faces, identities, clothes, poses, camera, framing, lighting and background. "
            "Photorealistic adjacent burst frame, 0.4 seconds later.")
    return g


def export_jpeg(png, dst, job, mode="normal", device_index=None):
    im = Image.open(png).convert("RGB")
    make, model, lens, focal, aperture = DEVICES[job["device_index"] if device_index is None else device_index]
    dt = datetime.fromisoformat(job["taken_at"])
    if mode == "clock-offset":
        dt += timedelta(seconds=120)
    stamp = dt.strftime("%Y:%m:%d %H:%M:%S")
    low = any(s in job["prompt"].lower() for s in ("night", "dim ", "candle", "neon"))
    exif = Image.Exif()
    exif.update({271: make, 272: model, 274: 1, 306: stamp,
                 305: "imagePicker synthetic Qwen 2.1 test fixture", 315: "AI-generated fictional scene"})
    sub = {36867: stamp, 36868: stamp, 37521: f"{dt.microsecond // 1000:03d}",
           36881: "+08:00", 34855: 3200 if low else 200,
           33434: IFDRational(1, 60 if low else 500), 33437: IFDRational(aperture),
           37386: IFDRational(focal), 42036: lens, 40962: im.width, 40963: im.height,
           40961: 1}
    if mode == "missing-lens":
        sub.pop(42036)
    exif[34665] = sub
    dst.parent.mkdir(parents=True, exist_ok=True)
    temp = dst.with_suffix(".tmp")
    im.save(temp, "JPEG", quality=94, exif=b"" if mode == "no-exif" else exif.tobytes())
    temp.replace(dst)
    # Read back nested EXIF, not just the file name or save result.
    with Image.open(dst) as check:
        check.load()
        if mode != "no-exif":
            assert check.getexif()[272] == model
            assert check.getexif().get_ifd(34665)[36867] == stamp
    return {"make": make, "model": model, "lens": None if mode == "missing-lens" else lens,
            "focal_mm": focal, "f_number": aperture, "metadata_mode": mode}


def boundaries(manifest):
    """48 explicit metadata/duplicate fixtures; never counted as new Qwen scenes."""
    done = {r["name"] for r in manifest["images"]}
    bases = [r for r in manifest["images"] if r["phase"] in ("people", "groups", "places", "things")][:12]
    if len(bases) < 12:
        return
    for i, base in enumerate(bases):
        for mode in ("exact-duplicate", "no-exif", "missing-lens", "clock-offset"):
            name = f"edge-{i:02d}-{mode}"
            if name in done:
                continue
            dst = ROOT / "jpeg/edge" / (name + ".jpg")
            dst.parent.mkdir(parents=True, exist_ok=True)
            if mode == "exact-duplicate":
                shutil.copyfile(ROOT / base["jpeg"], dst)
                metadata = base["metadata"]
            else:
                metadata = export_jpeg(ROOT / base["png"], dst, base, mode,
                    (base["device_index"] + 1) % len(DEVICES) if mode == "clock-offset" else None)
            manifest["images"].append({**base, "name": name, "phase": "edge", "kind": "metadata-fixture",
                "source": base["name"], "jpeg": dst.relative_to(ROOT).as_posix(), "metadata": metadata,
                "expected_clock_offset_seconds": 120 if mode == "clock-offset" else 0,
                "sha256": hashlib.sha256(dst.read_bytes()).hexdigest()})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8191")
    ap.add_argument("--limit", type=int, default=0, help="Maximum newly generated images this run")
    ap.add_argument("--steps", type=int, default=20)
    ap.add_argument("--plan", action="store_true")
    ap.add_argument("--pilot", action="store_true")
    args = ap.parse_args()
    ROOT.mkdir(parents=True, exist_ok=True)
    # Hold a process lock for this corpus to prevent two drivers from spending GPU
    # time on the same manifest. Windows releases this lock after a crash.
    lock = None
    if not args.plan and os.name == "nt":
        import msvcrt
        lock = (ROOT / "driver.lock").open("a+b")
        if lock.tell() == 0:
            lock.write(b"0")
            lock.flush()
        lock.seek(0)
        msvcrt.locking(lock.fileno(), msvcrt.LK_NBLCK, 1)
    planned = jobs()
    save_json(ROOT / "plan.json", {"synthetic": True, "qwen_images": len(planned),
        "edge_images": 48, "devices": DEVICES, "jobs": planned})
    if args.plan:
        print(Counter(j["phase"] for j in planned), flush=True)
        return
    if args.pilot:
        names = {"a1", "a2", "a3", "a4", "park-golden-a1", "family-dinner-1",
                 "pl01-sunrise", "th01-1", "bs01-a1-00", "bs01-a1-01", "bs01-a1-02",
                 "bg01-family-dinner-00", "bg01-family-dinner-01", "bg01-family-dinner-02"}
        planned = [j for j in planned if j["name"] in names]
    mp = ROOT / "manifest.json"
    manifest = json.loads(mp.read_text(encoding="utf-8")) if mp.exists() else {
        "synthetic": True, "model": "qwen_image_2.1_int8_convrot.safetensors",
        "target": 1000, "label_status": "prompt intent; visual validation required", "images": []}
    records = {r["name"]: r for r in manifest["images"]}
    state_path = ROOT / "inflight.json"
    if state_path.exists():
        pending = json.loads(state_path.read_text())
        if pending["name"] in records:
            state_path.unlink()  # crash after atomic manifest commit, before cleanup
    generated = 0
    for j in planned:
        if j["name"] in records:
            if not (ROOT / records[j["name"]]["jpeg"]).is_file():
                raise RuntimeError(f"Missing completed file: {j['name']}")
            continue
        if args.limit and generated >= args.limit:
            break
        if (ROOT / "STOP").exists():
            print("STOP requested; no new prompt submitted", flush=True)
            break
        started = time.monotonic()
        state = json.loads(state_path.read_text()) if state_path.exists() else None
        if state and state["name"] != j["name"]:
            raise RuntimeError("Different pending job; resume using the same selection")
        if not state:
            while any(api(args.server, "/queue").get(k) for k in ("queue_running", "queue_pending")):
                time.sleep(5)
            workflow = graph(j, records, args.steps)
            wp = ROOT / "workflows" / (j["name"] + ".json")
            wp.parent.mkdir(exist_ok=True)
            save_json(wp, workflow)
            try:
                response = api(args.server, "/prompt", {"prompt": workflow, "client_id": PREFIX})
            except urllib.error.HTTPError as e:
                raise RuntimeError(e.read().decode()) from e
            state = {"name": j["name"], "prompt_id": response["prompt_id"]}
            save_json(state_path, state)
            print(f"SUBMITTED {j['phase']}/{j['name']} {state['prompt_id']}", flush=True)
        while True:
            history = api(args.server, "/history/" + state["prompt_id"])
            entry = history.get(state["prompt_id"])
            if entry:
                if entry.get("status", {}).get("status_str") == "error":
                    save_json(ROOT / "last-error.json", entry)
                    raise RuntimeError("ComfyUI failed; inspect last-error.json; inflight retained")
                if entry.get("status", {}).get("completed"):
                    break
            if time.monotonic() - started > 3600:
                raise TimeoutError("Prompt timeout; inflight retained for recovery")
            time.sleep(3)
        output = entry["outputs"]["8"]["images"][0]
        png = ROOT / "png" / j["phase"] / (j["name"] + ".png")
        png.parent.mkdir(parents=True, exist_ok=True)
        url = args.server + "/view?" + urllib.parse.urlencode(output)
        with urllib.request.urlopen(url, timeout=90) as r:
            png.write_bytes(r.read())
        with Image.open(png) as im:
            im.load()
            size = im.size
        jpeg = ROOT / "jpeg" / j["phase"] / (j["name"] + ".jpg")
        metadata = export_jpeg(png, jpeg, j)
        record = {**j, "png": png.relative_to(ROOT).as_posix(), "jpeg": jpeg.relative_to(ROOT).as_posix(),
                  "server_image": "/".join(filter(None, (output.get("subfolder"), output["filename"]))),
                  "width": size[0], "height": size[1], "metadata": metadata, "steps": args.steps,
                  "prompt_id": state["prompt_id"], "visual_review": "pending",
                  "sha256": hashlib.sha256(jpeg.read_bytes()).hexdigest()}
        manifest["images"].append(record)
        records[j["name"]] = record
        boundaries(manifest)
        save_json(mp, manifest)
        state_path.unlink()
        generated += 1
        save_json(ROOT / "progress.json", {"saved": len(manifest["images"]), "target": 1000,
            "last_name": j["name"], "last_seconds": round(time.monotonic()-started, 1),
            "status": "generating", "updated_at": datetime.now().astimezone().isoformat()})
        print(f"DONE {len(manifest['images'])}/1000 {j['name']} {time.monotonic()-started:.1f}s", flush=True)
    if manifest["images"]:
        subprocess.run([sys.executable, str(Path(__file__).with_name("comfy-photo-library-audit.py"))], check=True)
    save_json(ROOT / "progress.json", {"saved": len(manifest["images"]), "target": 1000,
        "status": "complete" if len(manifest["images"]) == 1000 else "run-ended-incomplete",
        "updated_at": datetime.now().astimezone().isoformat()})
    print(f"RUN ENDED: {len(manifest['images'])}/1000 saved", flush=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as exc:
        if ROOT.exists():
            save_json(ROOT / "failure.json", {"error": str(exc),
                "updated_at": datetime.now().astimezone().isoformat()})
        raise
