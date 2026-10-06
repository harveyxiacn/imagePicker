"""Benchmark of the M5 generative methods (ms, median of --runs after one warm-up).

  uv run python scripts/bench_m5.py [--device auto|cpu] [--runs 3] [--portrait face.jpg] [--json out.json]

Rows: besttake (one face, `--portrait` or a Commons portrait from .models/_testimg), inpaint (512 px
mask on 12 MP), denoise 12 MP, face_restore (per face), upscale x2 12 MP. Models are downloaded
into the models dir when missing. The CPU run is meant for `--device cpu` (use --megapixels to
shrink the big jobs there).
"""

from __future__ import annotations

import argparse
import asyncio
import json
import statistics
import sys
import tempfile
import time
from pathlib import Path

import cv2
import numpy as np

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))

from imagepicker_ai.models import Downloader  # noqa: E402
from imagepicker_ai.service import WorkerService  # noqa: E402
from imagepicker_ai.steps.faces import FaceAnalyzer  # noqa: E402


def big_image(mp: float) -> np.ndarray:
    w = int((mp * 1e6 * 4 / 3) ** 0.5)
    h = int(w * 3 / 4)
    rng = np.random.default_rng(0)
    low = cv2.resize(
        rng.random((12, 16, 3)).astype(np.float32), (w, h), interpolation=cv2.INTER_CUBIC
    )
    img = (low * 200 + 25).clip(0, 255).astype(np.uint8)
    for _ in range(60):
        c = tuple(int(v) for v in rng.integers(0, 255, 3))
        cv2.circle(
            img,
            (int(rng.integers(0, w)), int(rng.integers(0, h))),
            int(rng.integers(20, 200)),
            c,
            -1,
            cv2.LINE_AA,
        )
    noise = rng.normal(0, 6, img.shape).astype(np.float32)
    return (img.astype(np.float32) + noise).clip(0, 255).astype(np.uint8)


def timed(fn, runs: int) -> float:
    fn()  # warm-up (model load, cuDNN autotune)
    ts = []
    for _ in range(runs):
        t = time.perf_counter()
        fn()
        ts.append((time.perf_counter() - t) * 1000)
    return statistics.median(ts)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--device", default="auto")
    ap.add_argument("--runs", type=int, default=3)
    ap.add_argument("--megapixels", type=float, default=12.0)
    ap.add_argument(
        "--portrait", default=str(ROOT / ".models/_testimg/Official_portrait_of_Barack_Obama.jpg")
    )
    ap.add_argument("--models-dir", default=str(ROOT / ".models"))
    ap.add_argument("--json")
    a = ap.parse_args()

    svc = WorkerService(models_dir=a.models_dir, device=a.device, idle_unload_s=0)
    d = Downloader(Path(a.models_dir))
    need = {*svc.besttake.all_models(), "lama-big-fp32", "scunet-color-real-psnr", "gfpgan-v1.4",
            svc.enhance.upscale_model(2)}  # fmt: skip
    for mid in need:
        d.ensure(svc.registry.get(mid))
    tmp = Path(tempfile.mkdtemp(prefix="bench_m5_"))
    res: dict[str, float] = {}

    def run(coro_fn):
        return lambda: asyncio.run(coro_fn())

    # --- besttake: base with eyes painted shut + shifted source of a real portrait
    portrait = cv2.cvtColor(cv2.imread(a.portrait), cv2.COLOR_BGR2RGB)
    h, w = portrait.shape[:2]
    fa = FaceAnalyzer(str(svc.manager.path("yunet")), None)
    face = sorted(fa.detect(portrait), key=lambda f: -f["bbox"][2])[0]["bbox"]
    M = cv2.getRotationMatrix2D((w / 2, h / 2), 0.6, 1.0)
    M[:, 2] += (9.3, -6.1)
    src = cv2.warpAffine(
        portrait, M, (w, h), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101
    )
    bp, sp, pp = tmp / "base.png", tmp / "src.png", tmp / "portrait.png"
    for p, im in ((bp, portrait), (sp, src), (pp, portrait)):
        cv2.imwrite(str(p), cv2.cvtColor(im, cv2.COLOR_RGB2BGR))
    c2 = M @ np.array([(face[0] + face[2] / 2) * w, (face[1] + face[3] / 2) * h, 1.0])
    sbox = [c2[0] / w - face[2] / 2, c2[1] / h - face[3] / 2, face[2], face[3]]
    bt = {"base": {"photo_id": 1, "path": str(bp)}, "source": {"photo_id": 2, "path": str(sp)},
          "base_face": face, "source_face": sbox, "out_dir": str(tmp)}  # fmt: skip
    r = asyncio.run(svc.besttake.compose(bt))
    assert r["patch"], r
    res["besttake_one_face"] = timed(run(lambda: svc.besttake.compose(bt)), a.runs)

    # --- 12 MP jobs
    img = big_image(a.megapixels)
    H, W = img.shape[:2]
    ip, mp = tmp / "big.jpg", tmp / "mask.png"
    cv2.imwrite(str(ip), cv2.cvtColor(img, cv2.COLOR_RGB2BGR), [cv2.IMWRITE_JPEG_QUALITY, 92])
    mask = np.zeros((H, W), np.uint8)
    cv2.rectangle(mask, (W // 2 - 256, H // 2 - 256), (W // 2 + 256, H // 2 + 256), 255, -1)
    cv2.imwrite(str(mp), mask)
    ph = {"photo_id": 3, "path": str(ip)}
    res["inpaint_512_mask"] = timed(
        run(lambda: svc.inpaint.run({"photo": ph, "mask": str(mp), "out_dir": str(tmp)})), a.runs
    )
    res["denoise"] = timed(
        run(
            lambda: svc.enhance.run(
                {"photo": ph, "op": "denoise", "strength": 1.0, "out_dir": str(tmp)}
            )
        ),
        a.runs,
    )
    res["upscale_x2"] = timed(
        run(
            lambda: svc.enhance.run({"photo": ph, "op": "upscale", "scale": 2, "out_dir": str(tmp)})
        ),
        a.runs,
    )
    fr = {"photo": {"photo_id": 4, "path": str(pp)}, "op": "face_restore", "strength": 0.8,
          "faces": [face], "out_dir": str(tmp)}  # fmt: skip
    res["face_restore_per_face"] = timed(run(lambda: svc.enhance.run(fr)), a.runs)

    prov = svc.hw.providers[0] if a.device != "cpu" else "CPUExecutionProvider"
    print(f"device={a.device} provider={prov} image={W}x{H} ({W * H / 1e6:.1f} MP) runs={a.runs}")
    for k, v in res.items():
        print(f"{k:24s} {v:10.0f} ms")
    if a.json:
        Path(a.json).write_text(
            json.dumps({"device": a.device, "provider": prov, "ms": res}, indent=2)
        )


if __name__ == "__main__":
    main()
