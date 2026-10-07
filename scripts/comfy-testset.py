#!/usr/bin/env python3
"""Generate the synthetic photo evaluation set with a local ComfyUI (Qwen-Image 2.1 GGUF).

The set feeds the scoring / grouping calibration items in docs/backlog.md (no real photos in the
repo). Categories: portraits, landscapes, groups, bursts (same seed, small prompt deltas so the
frames look like one burst with eyes closed / blur / exposure variants) and defects.

Usage:
  scripts/comfy-testset.py [--server http://127.0.0.1:8188] [--out imagepicker-testset]
                           [--only portraits,bursts] [--dry-run] [--limit N]

Images land in <ComfyUI output dir>/<out>/<category>/<name>.png (ComfyUI's SaveImage adds a
counter suffix). A manifest.json with the prompt, seed and size of every image is written next to
this script's log in --manifest (default: <out>/manifest.json under the ComfyUI output dir when
--comfy-output is given).
"""
from __future__ import annotations

import argparse
import json
import sys
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
PROMPTS = HERE / "comfy-testset-prompts.json"

UNET = "qwen-image-2.1-Q8_0.gguf"
CLIP = "qwen3vl_8b_int8_convrot.safetensors"
VAE = "qwen_image_2.1_vae_bf16.safetensors"

SIZES = {
    "3:2": (1536, 1024),
    "2:3": (1024, 1536),
    "16:9": (1600, 896),
    "1:1": (1216, 1216),
    "4:3": (1408, 1056),
}


def graph(prompt: str, seed: int, width: int, height: int, prefix: str, steps: int) -> dict:
    """ComfyUI API prompt mirroring user/default/workflows/Qwen-Image-2.1-GGUF-Q8.json."""
    return {
        "1": {"class_type": "UnetLoaderGGUF", "inputs": {"unet_name": UNET}},
        "2": {
            "class_type": "CLIPLoader",
            "inputs": {"clip_name": CLIP, "type": "qwen_image", "device": "default"},
        },
        "3": {"class_type": "VAELoader", "inputs": {"vae_name": VAE}},
        "4": {
            "class_type": "TextEncodeQwenImage21",
            "inputs": {
                "clip": ["2", 0],
                "prompt": prompt,
                "negative_prompt": "",
                "resolution": 1024,
                "vae": ["3", 0],
            },
        },
        "5": {
            "class_type": "EmptyLatentImage",
            "inputs": {"width": width, "height": height, "batch_size": 1},
        },
        "6": {
            "class_type": "KSampler",
            "inputs": {
                "model": ["1", 0],
                "positive": ["4", 0],
                "negative": ["4", 1],
                "latent_image": ["5", 0],
                "seed": seed,
                "steps": steps,
                "cfg": 1.0,
                "sampler_name": "euler",
                "scheduler": "simple",
                "denoise": 1.0,
            },
        },
        "7": {"class_type": "VAEDecode", "inputs": {"samples": ["6", 0], "vae": ["3", 0]}},
        "8": {"class_type": "SaveImage", "inputs": {"images": ["7", 0], "filename_prefix": prefix}},
    }


def api(server: str, path: str, body: dict | None = None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(server + path, data=data, headers={"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=30) as r:
        return json.load(r)


def wait_done(server: str, prompt_id: str, timeout_s: float = 900) -> dict:
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        hist = api(server, f"/history/{prompt_id}")
        if prompt_id in hist:
            entry = hist[prompt_id]
            status = entry.get("status", {})
            if status.get("status_str") == "error":
                raise RuntimeError(f"ComfyUI error: {json.dumps(status.get('messages'))[:800]}")
            if status.get("completed", True):
                return entry
        time.sleep(1.5)
    raise TimeoutError(prompt_id)


def expand(spec: dict) -> list[dict]:
    """Flatten the prompt file into one job per image."""
    jobs: list[dict] = []
    style = spec["style"]
    for cat in spec["categories"]:
        name = cat["name"]
        base_seed = cat["seed"]
        for i, item in enumerate(cat["items"]):
            if "variants" in item:
                # a burst: same seed, prompt deltas appended
                for j, delta in enumerate(item["variants"]):
                    jobs.append(
                        {
                            "category": name,
                            "name": f"{item['id']}-{j + 1:02d}",
                            "prompt": f"{style} {item['prompt']} {delta['delta']}".strip(),
                            "tags": [item.get("tag", "burst"), *delta.get("tags", [])],
                            "seed": base_seed + i,
                            "size": item.get("size", cat.get("size", "3:2")),
                        }
                    )
            else:
                jobs.append(
                    {
                        "category": name,
                        "name": item["id"],
                        "prompt": f"{style} {item['prompt']}".strip(),
                        "tags": item.get("tags", []),
                        "seed": base_seed + i,
                        "size": item.get("size", cat.get("size", "3:2")),
                    }
                )
    return jobs


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--server", default="http://127.0.0.1:8188")
    ap.add_argument("--out", default="imagepicker-testset", help="filename_prefix root (under ComfyUI output/)")
    ap.add_argument("--comfy-output", default=str(Path.home() / "ai/ComfyUI/output"))
    ap.add_argument("--only", default="", help="comma-separated category names")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--steps", type=int, default=25)
    ap.add_argument("--dry-run", action="store_true")
    args = ap.parse_args()

    spec = json.loads(PROMPTS.read_text(encoding="utf-8"))
    jobs = expand(spec)
    if args.only:
        keep = set(args.only.split(","))
        jobs = [j for j in jobs if j["category"] in keep]
    if args.limit:
        jobs = jobs[: args.limit]
    print(f"{len(jobs)} images", flush=True)
    if args.dry_run:
        for j in jobs:
            print(f"{j['category']}/{j['name']}  seed={j['seed']} {j['size']}  {j['prompt'][:90]}…")
        return 0

    try:
        api(args.server, "/queue")
    except (urllib.error.URLError, OSError) as e:
        print(f"ComfyUI not reachable at {args.server}: {e}", file=sys.stderr)
        return 1

    out_root = Path(args.comfy_output) / args.out
    out_root.mkdir(parents=True, exist_ok=True)
    manifest_path = out_root / "manifest.json"
    manifest = json.loads(manifest_path.read_text()) if manifest_path.exists() else {"images": []}
    done = {(m["category"], m["name"]) for m in manifest["images"]}
    client = uuid.uuid4().hex
    t0 = time.monotonic()
    for n, j in enumerate(jobs, 1):
        if (j["category"], j["name"]) in done:
            print(f"[{n}/{len(jobs)}] skip {j['category']}/{j['name']} (done)", flush=True)
            continue
        w, h = SIZES[j["size"]]
        prefix = f"{args.out}/{j['category']}/{j['name']}"
        t = time.monotonic()
        resp = api(args.server, "/prompt", {"prompt": graph(j["prompt"], j["seed"], w, h, prefix, args.steps), "client_id": client})
        entry = wait_done(args.server, resp["prompt_id"])
        files = [
            f"{o['subfolder']}/{o['filename']}" if o.get("subfolder") else o["filename"]
            for node in entry.get("outputs", {}).values()
            for o in node.get("images", [])
        ]
        manifest["images"].append({**j, "width": w, "height": h, "steps": args.steps, "files": files})
        manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=1))
        print(f"[{n}/{len(jobs)}] {prefix} {w}x{h} {time.monotonic() - t:.0f}s -> {files}", flush=True)
    print(f"finished in {(time.monotonic() - t0) / 60:.1f} min; manifest {manifest_path}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
