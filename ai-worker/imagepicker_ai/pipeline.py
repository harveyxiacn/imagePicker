"""`analyze.batch`: decode thread pool -> per-image CPU steps -> batched GPU embedding.

  items --> [decode pool threads: decode + faces + identity + quality + phash + NIMA + 224px prep]
        --> ready queue
        --> [single GPU thread: SigLIP2 batches (OOM backoff) + zero-shot scene / IQA heads]
        --> artifacts + result JSON

The SigLIP2 image embedding is computed once per photo and shared by `embed` (written to
`.emb.npy`), `scene` and the zero-shot half of `iqa`.

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
from .steps import DEFAULT_PROFILE, PROFILES, STEP_ALIASES, STEPS
from .steps.embed import EMBED_DIM, TEXT_MODEL, TORCH_MODEL, Embedder, prepare_image
from .steps.faces import FaceAnalyzer
from .steps.identity import MODEL_ID as AURAFACE
from .steps.identity import IdentityEmbedder, kps_from_bbox
from .steps.nima import AESTHETIC_ID as NIMA_AESTHETIC
from .steps.nima import TECHNICAL_ID as NIMA_TECHNICAL
from .steps.nima import NimaModel, calibrate_aesthetic, calibrate_technical, litert_available
from .steps.phash import phash_hex
from .steps.quality import analyze_quality
from .steps.zeroshot import (
    SCENE_CLASSES,
    PromptBank,
    combine_iqa,
    iqa_zero_shot,
    scene_scores,
)

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
    nima_tech: float | None = (
        None  # calibrated 0-1, combined with the zero-shot IQA on the GPU side
    )


@dataclass
class BatchRequest:
    items: list[dict[str, Any]]
    steps: list[str]
    skipped_steps: list[str]
    profile: str | None
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
        profile = params.get("profile")
        if profile is not None and profile not in PROFILES:
            raise InvalidParams(f"profile must be one of {sorted(PROFILES)}")
        if "steps" in params:
            raw_steps = params["steps"]  # explicit steps win over the profile
        else:
            profile = profile or DEFAULT_PROFILE
            raw_steps = list(PROFILES[profile])
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
        if "identity" in steps and "faces" not in steps:
            steps.insert(0, "faces")  # identity needs the detections / landmarks
        size = params.get("analysis_size", 1024)
        if not isinstance(size, int) or not 128 <= size <= 8192:
            raise InvalidParams("analysis_size must be an integer in [128, 8192]")
        out_dir = params.get("out_dir")
        if ("embed" in steps or "identity" in steps) and not out_dir:
            raise InvalidParams("out_dir is required for the embed and identity steps")
        return cls(
            items,
            steps,
            skipped,
            profile if "steps" not in params else None,
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
        self.identity = IdentityEmbedder(manager, hw)
        self.bank = PromptBank(Path(manager.models_dir) / "_cache")
        self._faces: dict[tuple[str, str | None], FaceAnalyzer] = {}

    def shutdown(self) -> None:
        self.decode_pool.shutdown(wait=False, cancel_futures=True)
        self.gpu_pool.shutdown(wait=False, cancel_futures=True)
        for f in self._faces.values():
            f.close()
        self._faces.clear()

    # ------------------------------------------------------------------ models
    def missing_models(self, steps: list[str]) -> list[str]:
        """Hard-required model ids not installed for `steps` (faces / embed: the batch fails)."""
        need: list[str] = []
        if "faces" in steps:
            need.append(YUNET)
        if "embed" in steps:
            need += self.embedder.required_models()
        return [m for m in need if not self.manager.is_installed(m)]

    def models_for_steps(self, steps: list[str]) -> list[str]:
        """All model ids `steps` would use on this machine (what `models.ensure` should fetch)."""
        out: list[str] = []

        def add(*ids: str) -> None:
            out.extend(i for i in ids if i not in out)

        if "faces" in steps or "identity" in steps:
            add(YUNET, LANDMARKER)
        if "identity" in steps:
            add(AURAFACE)
        if "embed" in steps or "scene" in steps:
            add(*self.embedder.required_models())
        if "scene" in steps and not self.bank.cached():
            add(TEXT_MODEL)
        if "aesthetic" in steps:
            add(NIMA_AESTHETIC)
        if "iqa" in steps:
            add(NIMA_TECHNICAL)
        return out

    def _text_available(self) -> bool:
        return self.manager.is_installed(TEXT_MODEL) or (
            self.embedder.backend != "onnx" and self.manager.is_installed(TORCH_MODEL)
        )

    def _prompts_available(self) -> bool:
        return self.bank.cached() or self._text_available()

    def _image_model_ready(self) -> bool:
        return all(self.manager.is_installed(m) for m in self.embedder.required_models())

    def soft_models(self, steps: list[str]) -> dict[str, list[str]]:
        """Models of the optional steps (missing -> the step is reported in `skipped_steps`)."""
        need: dict[str, list[str]] = {}
        if "identity" in steps:
            need["identity"] = [AURAFACE, YUNET]
        if "aesthetic" in steps:
            need["aesthetic"] = [NIMA_AESTHETIC]
        if "iqa" in steps:
            need["iqa"] = [NIMA_TECHNICAL]
        if "scene" in steps:
            need["scene"] = self.embedder.required_models()
        return need

    def _face_analyzer(self) -> FaceAnalyzer:
        yunet = str(self.manager.path(YUNET))
        lm = str(self.manager.path(LANDMARKER)) if self.manager.is_installed(LANDMARKER) else None
        key = (yunet, lm)
        fa = self._faces.get(key)
        if fa is None:
            fa = self._faces[key] = FaceAnalyzer(yunet, lm)
        return fa

    def _nima(self, model_id: str) -> NimaModel:
        return self.manager.acquire(model_id, lambda spec, path: NimaModel(path))

    # ------------------------------------------------------------------ main entry
    async def analyze_batch(
        self, params: Any, progress: ProgressFn | None = None
    ) -> dict[str, Any]:
        req = BatchRequest.parse(params)
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        warnings: list[str] = []
        steps = list(req.steps)
        skipped = list(req.skipped_steps)
        t_wall = time.perf_counter()

        # --- models: download on request, otherwise skip optional steps whose models are missing
        wanted = set(self.missing_models(steps))
        if "faces" in steps and not self.manager.is_installed(LANDMARKER):
            wanted.add(LANDMARKER)
        soft = self.soft_models(steps)
        for ids in soft.values():
            wanted.update(m for m in ids if not self.manager.is_installed(m))
        if "scene" in steps and not self._prompts_available():
            wanted.add(TEXT_MODEL)
        missing = [m for m in wanted if not self.manager.is_installed(m)]
        if missing:
            if not req.allow_download:
                hard = [m for m in self.missing_models(steps) if m in missing]
                if hard:
                    raise ModelUnavailable(
                        f"models not installed: {', '.join(hard)} (call models.ensure)", hard
                    )
            else:
                for mid in sorted(missing):
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
        for step, ids in soft.items():
            absent = [m for m in ids if not self.manager.is_installed(m)]
            if absent:
                steps.remove(step)
                skipped.append(step)
                warnings.append(
                    f"step {step!r} skipped: models not installed ({', '.join(absent)})"
                )
        if "scene" in steps and not self._prompts_available():
            steps.remove("scene")
            skipped.append("scene")
            warnings.append(
                "step 'scene' skipped: siglip2-base-text not installed and no prompt cache"
            )
        if ("iqa" in steps or "aesthetic" in steps) and not litert_available():
            for s_ in ("iqa", "aesthetic"):
                if s_ in steps:
                    steps.remove(s_)
                    skipped.append(s_)
            warnings.append("steps iqa/aesthetic skipped: ai-edge-litert is not available")

        embed = "embed" in steps
        scene = "scene" in steps
        # zero-shot half of iqa: free when embeddings are computed anyway; on GPU tiers worth one pass
        zs_iqa = (
            "iqa" in steps
            and self._prompts_available()
            and self._image_model_ready()
            and (embed or scene or (self.hw.device != "cpu" and not self.embedder.force_cpu))
        )
        need_emb = embed or scene or zs_iqa
        bank_vecs: dict[str, np.ndarray] | None = None
        if scene or zs_iqa:
            try:
                bank_vecs = await loop.run_in_executor(
                    self.gpu_pool, self.bank.get, self.embedder.embed_text
                )
            except Exception as e:  # noqa: BLE001
                log.warning("zero-shot prompt embeddings unavailable: %s", e)
                warnings.append(f"zero-shot heads unavailable: {e}")
                if scene:
                    steps.remove("scene")
                    skipped.append("scene")
                scene = zs_iqa = False
                need_emb = embed
        plan = _Plan(
            steps=steps,
            embed=embed,
            scene=scene,
            zs_iqa=zs_iqa,
            need_emb=need_emb,
            bank=bank_vecs,
            out_dir=req.out_dir,
        )

        faces_an = self._face_analyzer() if "faces" in steps else None
        if faces_an is not None and not faces_an.has_landmarks:
            warnings.append(
                "faces step is YuNet-only: landmark fields (incl. gaze) are null "
                "(mediapipe or the face-landmarker model is unavailable)"
            )
        if "aesthetic" in steps:
            self._nima(NIMA_AESTHETIC)
        if "iqa" in steps:
            self._nima(NIMA_TECHNICAL)
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

        batch = max(1, self.embedder.batch_size)
        window = max(self.decode_workers * 2, batch * 2)
        pending: list[tuple[int, asyncio.Future[_Work]]] = []
        buf: list[_Work] = []
        gpu_task: asyncio.Task[None] | None = None
        next_i = 0

        async def run_gpu(works: list[_Work]) -> None:
            t0 = time.perf_counter()
            try:
                timings["scene"] += await loop.run_in_executor(
                    self.gpu_pool, self._embed_and_save, plan, works
                )
            except RpcError as e:
                for w in works:
                    w.result["error"] = {"code": e.code, "message": e.message, "kind": e.kind}
            except Exception as e:  # noqa: BLE001
                log.exception("embedding batch failed")
                for w in works:
                    w.result["error"] = {
                        "code": errors.STEP_FAILED,
                        "message": f"embed failed: {e}",
                        "kind": "step_failed",
                    }
            timings["embed"] += time.perf_counter() - t0
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
                                plan,
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
                if need_emb and w.prepared is not None:
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
        if "identity" in steps:
            models_used["identity"] = AURAFACE
        if "aesthetic" in steps:
            models_used["aesthetic"] = AESTHETIC_MODEL
        if "iqa" in steps:
            models_used["iqa"] = _iqa_model_name(zs_iqa)
        if scene:
            models_used["scene"] = SCENE_MODEL
        return {
            "items": results,
            "steps": steps,
            "skipped_steps": skipped,
            "profile": req.profile,
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
        self,
        idx: int,
        item: dict[str, Any],
        req: BatchRequest,
        plan: _Plan,
        faces_an: FaceAnalyzer | None,
    ) -> _Work:
        steps = plan.steps
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
        nima_tech: float | None = None
        try:
            boxes = None
            if "faces" in steps and faces_an is not None:
                t0 = time.perf_counter()
                faces = faces_an.analyze(rgb)
                tm["faces"] = time.perf_counter() - t0
                kps_list = [f.pop("_kps", None) for f in faces]  # private; identity consumes it
                res["faces"] = faces
                boxes = [tuple(f["bbox"]) for f in faces]
                if "identity" in steps:
                    t0 = time.perf_counter()
                    self._identity(res, rgb, faces, kps_list, plan, pid)
                    tm["identity"] = time.perf_counter() - t0
            if "quality" in steps:
                t0 = time.perf_counter()
                q = analyze_quality(rgb, boxes)  # type: ignore[arg-type]
                tm["quality"] = time.perf_counter() - t0
                res["quality"] = q
                res["sharpness"], res["exposure"], res["noise"] = (
                    q["sharpness"],
                    q["exposure"],
                    q["noise"],
                )
            if "phash" in steps:
                t0 = time.perf_counter()
                res["phash"] = phash_hex(rgb)
                tm["phash"] = time.perf_counter() - t0
            if plan.need_emb or "aesthetic" in steps or "iqa" in steps:
                t0 = time.perf_counter()
                prepared = prepare_image(rgb)
                tm["prep224"] = time.perf_counter() - t0
            if "aesthetic" in steps:
                t0 = time.perf_counter()
                raw = self._nima(NIMA_AESTHETIC).score(prepared)
                res["aesthetic"] = round(calibrate_aesthetic(raw), 4)
                res["aesthetic_model"] = AESTHETIC_MODEL
                tm["aesthetic"] = time.perf_counter() - t0
            if "iqa" in steps:
                t0 = time.perf_counter()
                nima_tech = calibrate_technical(self._nima(NIMA_TECHNICAL).score(prepared))
                tm["iqa"] = time.perf_counter() - t0
                if not plan.zs_iqa:
                    res["iqa"] = round(nima_tech, 4)
                    res["iqa_model"] = _iqa_model_name(False)
        except Exception as e:  # noqa: BLE001
            log.exception("step failed for %s", item["path"])
            res["error"] = {
                "code": errors.STEP_FAILED,
                "message": f"{type(e).__name__}: {e}",
                "kind": "step_failed",
            }
            return _Work(idx, res, None, tm, True)
        w = _Work(idx, res, prepared if plan.need_emb else None, tm)
        w.nima_tech = nima_tech
        return w

    def _identity(
        self,
        res: dict[str, Any],
        rgb: np.ndarray,
        faces: list[dict[str, Any]],
        kps_list: list[Any],
        plan: _Plan,
        pid: Any,
    ) -> None:
        """AuraFace embeddings of all faces -> `<photo_id>.faces.npy` + `identity_index` per face."""
        assert plan.out_dir is not None
        path = plan.out_dir / f"{_safe_name(pid)}.faces.npy"
        res["identity_file"] = None
        if not faces:
            path.unlink(missing_ok=True)  # drop a stale file from an earlier run
            return
        try:
            kps = [
                k if k is not None else kps_from_bbox(f["bbox"], rgb.shape[1], rgb.shape[0])
                for k, f in zip(kps_list, faces, strict=True)
            ]
            emb = self.identity.embed_faces(rgb, kps).astype(np.float16)
            _atomic_write(path, lambda f: np.save(f, emb))
        except Exception as e:  # noqa: BLE001
            log.warning("identity failed for photo %s: %s", pid, e)
            res["identity_error"] = f"{type(e).__name__}: {e}"
            return
        for i, f in enumerate(faces):
            f["identity_index"] = i
        res["identity_file"] = str(path)

    def _embed_and_save(self, plan: _Plan, works: list[_Work]) -> float:
        """Embed, write `.emb.npy`, run the zero-shot heads. Returns seconds spent in the heads."""
        vecs = self.embedder.embed_prepared([w.prepared for w in works if w.prepared is not None])
        t0 = time.perf_counter()
        sc = scene_scores(vecs, plan.bank["scene"]) if plan.scene and plan.bank else None
        zs = (
            iqa_zero_shot(vecs, plan.bank["iqa_good"], plan.bank["iqa_bad"])
            if plan.zs_iqa and plan.bank
            else None
        )
        for i, (w, v) in enumerate(zip(works, vecs, strict=True)):
            if plan.embed:
                assert plan.out_dir is not None
                p = plan.out_dir / f"{_safe_name(w.result['photo_id'])}.emb.npy"
                arr = v.astype(np.float16)
                _atomic_write(p, lambda f, arr=arr: np.save(f, arr))
                w.result["embedding_file"] = str(p)
                w.result["embedding_dim"] = EMBED_DIM
                w.result["embedding_model"] = self.embedder.active_model
            if sc is not None:
                probs = sc[i]
                w.result["scene_type"] = SCENE_CLASSES[int(np.argmax(probs))]
                w.result["scene_scores"] = {
                    c: round(float(p), 4) for c, p in zip(SCENE_CLASSES, probs, strict=True)
                }
            if zs is not None:
                v_iqa = combine_iqa(w.nima_tech, float(zs[i]))
                w.result["iqa"] = round(float(v_iqa), 4)  # type: ignore[arg-type]
                w.result["iqa_model"] = _iqa_model_name(True)
            w.prepared = None
        return time.perf_counter() - t0 if (sc is not None or zs is not None) else 0.0


@dataclass
class _Plan:
    """Resolved per-batch execution plan (after model availability / skipping)."""

    steps: list[str]
    embed: bool
    scene: bool
    zs_iqa: bool
    need_emb: bool
    bank: dict[str, np.ndarray] | None
    out_dir: Path | None


AESTHETIC_MODEL = "nima-aesthetic"
SCENE_MODEL = "siglip2-zeroshot"


def _iqa_model_name(zero_shot: bool) -> str:
    return "nima-technical+siglip2-zeroshot" if zero_shot else "nima-technical"


__all__ = ["Analyzer", "BatchRequest", "Cancelled"]
