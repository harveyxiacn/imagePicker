"""`analyze.batch`: decode thread pool -> per-image CPU steps -> batched GPU embedding.

  items --> [decode pool threads: decode + faces + quality + phash + 224px prep] --> ready queue
        --> [single GPU thread: SigLIP2 batches, OOM backoff] --> artifacts + result JSON

The event loop never runs heavy work: everything executes in thread pools, so progress
notifications and other RPCs stay responsive while a batch is running.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import re
import time
from collections import defaultdict
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np

from . import errors
from .decode import DecodeError, load_image
from .errors import InvalidParams, ModelUnavailable, RpcError
from .hw import HardwareInfo
from .models.download import Cancelled
from .models.manager import ModelManager
from .steps import STEP_ALIASES, STEPS
from .steps.embed import EMBED_DIM, Embedder, prepare_image
from .steps.faces import FaceAnalyzer
from .steps.phash import phash_hex
from .steps.quality import analyze_quality

log = logging.getLogger(__name__)

YUNET = "yunet"
LANDMARKER = "mediapipe-face-landmarker"

ProgressFn = Callable[[dict[str, Any]], None]


def _safe_name(photo_id: Any) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", str(photo_id))


def _atomic_write(path: Path, writer: Callable[[Any], None], mode: str = "wb") -> None:
    tmp = path.with_name(path.name + ".tmp")
    with open(tmp, mode) as f:
        writer(f)
    os.replace(tmp, path)


@dataclass
class _Work:
    idx: int
    result: dict[str, Any]
    prepared: np.ndarray | None = None
    timings: dict[str, float] = field(default_factory=dict)
    error: bool = False


@dataclass
class BatchRequest:
    items: list[dict[str, Any]]
    steps: list[str]
    skipped_steps: list[str]
    analysis_size: int
    out_dir: Path | None
    allow_download: bool

    @classmethod
    def parse(cls, params: Any) -> BatchRequest:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        items = params.get("items")
        if not isinstance(items, list) or not items:
            raise InvalidParams("items must be a non-empty array")
        for i, it in enumerate(items):
            if (
                not isinstance(it, dict)
                or "photo_id" not in it
                or not isinstance(it.get("path"), str)
            ):
                raise InvalidParams(f"items[{i}] needs photo_id and path")
        raw_steps = params.get("steps", list(STEPS))
        if not isinstance(raw_steps, list) or not all(isinstance(s, str) for s in raw_steps):
            raise InvalidParams("steps must be an array of strings")
        steps: list[str] = []
        skipped: list[str] = []
        for s in raw_steps:
            s2 = STEP_ALIASES.get(s, s)
            if s2 in STEPS:
                if s2 not in steps:
                    steps.append(s2)
            else:
                skipped.append(s)  # e.g. "segment": not implemented in this worker version
        size = params.get("analysis_size", 1024)
        if not isinstance(size, int) or not 128 <= size <= 8192:
            raise InvalidParams("analysis_size must be an integer in [128, 8192]")
        out_dir = params.get("out_dir")
        if "embed" in steps and not out_dir:
            raise InvalidParams("out_dir is required for the embed step")
        return cls(
            items,
            steps,
            skipped,
            size,
            Path(out_dir) if out_dir else None,
            bool(params.get("allow_download", False)),
        )


class Analyzer:
    def __init__(
        self,
        manager: ModelManager,
        hw: HardwareInfo,
        decode_workers: int | None = None,
        batch_size: int = 32,
        embed_backend: str = "auto",
    ):
        self.manager = manager
        self.hw = hw
        self.decode_workers = decode_workers or max(2, min(8, hw.cpu_cores_logical - 1 or 1))
        self.decode_pool = ThreadPoolExecutor(self.decode_workers, thread_name_prefix="decode")
        self.gpu_pool = ThreadPoolExecutor(1, thread_name_prefix="gpu")
        self.embedder = Embedder(manager, hw, batch_size=batch_size, backend=embed_backend)
        self._faces: dict[tuple[str, str | None], FaceAnalyzer] = {}

    def shutdown(self) -> None:
        self.decode_pool.shutdown(wait=False, cancel_futures=True)
        self.gpu_pool.shutdown(wait=False, cancel_futures=True)
        for f in self._faces.values():
            f.close()
        self._faces.clear()

    # ------------------------------------------------------------------ models
    def missing_models(self, steps: list[str]) -> list[str]:
        """Hard-required model ids not installed for `steps`."""
        need: list[str] = []
        if "faces" in steps:
            need.append(YUNET)
        if "embed" in steps:
            need += self.embedder.required_models()
        return [m for m in need if not self.manager.is_installed(m)]

    def _face_analyzer(self) -> FaceAnalyzer:
        yunet = str(self.manager.path(YUNET))
        lm = str(self.manager.path(LANDMARKER)) if self.manager.is_installed(LANDMARKER) else None
        key = (yunet, lm)
        fa = self._faces.get(key)
        if fa is None:
            fa = self._faces[key] = FaceAnalyzer(yunet, lm)
        return fa

    # ------------------------------------------------------------------ main entry
    async def analyze_batch(
        self, params: Any, progress: ProgressFn | None = None
    ) -> dict[str, Any]:
        req = BatchRequest.parse(params)
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        warnings: list[str] = []
        t_wall = time.perf_counter()

        # --- models
        wanted = set(self.missing_models(req.steps))
        if "faces" in req.steps and not self.manager.is_installed(LANDMARKER):
            wanted.add(LANDMARKER)
        missing = [m for m in wanted if not self.manager.is_installed(m)]
        if missing:
            if not req.allow_download:
                hard = [m for m in missing if m != LANDMARKER]
                if hard:
                    raise ModelUnavailable(
                        f"models not installed: {', '.join(hard)} (call models.ensure)", hard
                    )
            else:
                for mid in missing:
                    await loop.run_in_executor(
                        self.decode_pool,
                        lambda m=mid: self.manager.ensure(
                            m,
                            lambda e: loop.call_soon_threadsafe(
                                notify,
                                {"kind": "model.download", "done": e["bytes"], **e},
                            ),
                        ),
                    )
        faces_an = self._face_analyzer() if "faces" in req.steps else None
        if faces_an is not None and not faces_an.has_landmarks:
            warnings.append(
                "faces step is YuNet-only: landmark fields are null "
                "(mediapipe or the face-landmarker model is unavailable)"
            )
        if req.out_dir:
            req.out_dir.mkdir(parents=True, exist_ok=True)

        n = len(req.items)
        results: list[dict[str, Any] | None] = [None] * n
        timings: dict[str, float] = defaultdict(float)
        state = {"done": 0, "last_emit": 0.0}

        def emit(force: bool = False) -> None:
            now = time.monotonic()
            if force or now - state["last_emit"] >= 0.1:
                state["last_emit"] = now
                notify({"kind": "analyze", "done": state["done"], "total": n})

        def finish_one() -> None:
            state["done"] += 1
            emit(force=state["done"] == n)

        embed = "embed" in req.steps
        batch = max(1, self.embedder.batch_size)
        window = max(self.decode_workers * 2, batch * 2)
        pending: list[tuple[int, asyncio.Future[_Work]]] = []
        buf: list[_Work] = []
        gpu_task: asyncio.Task[None] | None = None
        next_i = 0

        async def run_gpu(works: list[_Work]) -> None:
            t0 = time.perf_counter()
            try:
                vecs = await loop.run_in_executor(self.gpu_pool, self._embed_and_save, req, works)
            except RpcError as e:
                for w in works:
                    w.result["error"] = {"code": e.code, "message": e.message, "kind": e.kind}
                vecs = None
            except Exception as e:  # noqa: BLE001
                log.exception("embedding batch failed")
                for w in works:
                    w.result["error"] = {
                        "code": errors.STEP_FAILED,
                        "message": f"embed failed: {e}",
                        "kind": "step_failed",
                    }
                vecs = None
            timings["embed"] += time.perf_counter() - t0
            if vecs is not None:
                for w in works:
                    w.result["embedding_model"] = self.embedder.active_model
            for _ in works:
                finish_one()

        try:
            while next_i < n or pending:
                while next_i < n and len(pending) < window:
                    pending.append(
                        (
                            next_i,
                            loop.run_in_executor(
                                self.decode_pool,
                                self._process_item,
                                next_i,
                                req.items[next_i],
                                req,
                                faces_an,
                            ),
                        )
                    )
                    next_i += 1
                idx, fut = pending.pop(0)
                w = await fut
                results[idx] = w.result
                for k, v in w.timings.items():
                    timings[k] += v
                if embed and w.prepared is not None:
                    buf.append(w)
                    if len(buf) >= batch:
                        if gpu_task is not None:
                            await gpu_task
                        gpu_task, buf = asyncio.create_task(run_gpu(buf)), []
                else:
                    finish_one()
            if buf:
                if gpu_task is not None:
                    await gpu_task
                gpu_task = asyncio.create_task(run_gpu(buf))
            if gpu_task is not None:
                await gpu_task
        finally:
            for _, f in pending:
                f.cancel()
            if gpu_task is not None and not gpu_task.done():
                gpu_task.cancel()

        # write per-photo json artifacts (small; after embedding so it is complete)
        if req.out_dir:
            for r in results:
                if r is not None and "error" not in r:
                    p = req.out_dir / f"{_safe_name(r['photo_id'])}.analysis.json"
                    try:
                        _atomic_write(p, lambda f, r=r: f.write(json.dumps(r)), "w")
                        r["analysis_file"] = str(p)
                    except OSError as e:
                        r["warning"] = f"could not write analysis json: {e}"

        wall = time.perf_counter() - t_wall
        ok = sum(1 for r in results if r is not None and "error" not in r)
        models_used: dict[str, str | None] = {}
        if embed:
            models_used["embed"] = self.embedder.active_model
        if faces_an is not None:
            models_used["faces"] = YUNET + ("+" + LANDMARKER if faces_an.has_landmarks else "")
        return {
            "items": results,
            "steps": req.steps,
            "skipped_steps": req.skipped_steps,
            "models": models_used,
            "device": {
                "device": "cpu" if self.embedder.force_cpu else self.hw.device,
                "providers": self.embedder.active_providers or self.hw.providers,
            },
            "timings": {
                **{f"{k}_s": round(v, 4) for k, v in timings.items()},
                "wall_s": round(wall, 4),
                "images_per_s": round(ok / wall, 2) if wall > 0 else None,
            },
            "warnings": warnings,
        }

    # ------------------------------------------------------------------ worker functions
    def _process_item(
        self, idx: int, item: dict[str, Any], req: BatchRequest, faces_an: FaceAnalyzer | None
    ) -> _Work:
        pid = item["photo_id"]
        res: dict[str, Any] = {"photo_id": pid}
        tm: dict[str, float] = {}
        try:
            t0 = time.perf_counter()
            dec = load_image(item["path"], req.analysis_size, item.get("orientation"))
            tm["decode"] = time.perf_counter() - t0
        except DecodeError as e:
            res["error"] = {
                "code": errors.DECODE_FAILED,
                "message": str(e),
                "kind": "decode_failed",
            }
            return _Work(idx, res, None, tm, True)
        res.update(
            width=dec.orig_width,
            height=dec.orig_height,
            analysis_width=dec.width,
            analysis_height=dec.height,
        )
        rgb = dec.rgb
        prepared = None
        try:
            boxes = None
            if "faces" in req.steps and faces_an is not None:
                t0 = time.perf_counter()
                faces = faces_an.analyze(rgb)
                tm["faces"] = time.perf_counter() - t0
                res["faces"] = faces
                boxes = [tuple(f["bbox"]) for f in faces]
            if "quality" in req.steps:
                t0 = time.perf_counter()
                q = analyze_quality(rgb, boxes)  # type: ignore[arg-type]
                tm["quality"] = time.perf_counter() - t0
                res["quality"] = q
                res["sharpness"], res["exposure"], res["noise"] = (
                    q["sharpness"],
                    q["exposure"],
                    q["noise"],
                )
            if "phash" in req.steps:
                t0 = time.perf_counter()
                res["phash"] = phash_hex(rgb)
                tm["phash"] = time.perf_counter() - t0
            if "embed" in req.steps:
                t0 = time.perf_counter()
                prepared = prepare_image(rgb)
                tm["embed_prep"] = time.perf_counter() - t0
        except Exception as e:  # noqa: BLE001
            log.exception("step failed for %s", item["path"])
            res["error"] = {
                "code": errors.STEP_FAILED,
                "message": f"{type(e).__name__}: {e}",
                "kind": "step_failed",
            }
            return _Work(idx, res, None, tm, True)
        return _Work(idx, res, prepared, tm)

    def _embed_and_save(self, req: BatchRequest, works: list[_Work]) -> bool:
        vecs = self.embedder.embed_prepared([w.prepared for w in works if w.prepared is not None])
        assert req.out_dir is not None
        for w, v in zip(works, vecs, strict=True):
            p = req.out_dir / f"{_safe_name(w.result['photo_id'])}.emb.npy"
            arr = v.astype(np.float16)
            _atomic_write(p, lambda f, arr=arr: np.save(f, arr))
            w.result["embedding_file"] = str(p)
            w.result["embedding_dim"] = EMBED_DIM
            w.prepared = None
        return True


__all__ = ["Analyzer", "BatchRequest", "Cancelled"]
