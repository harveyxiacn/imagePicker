"""`mask.generate` (docs/api-contract-m3.md section D): AI masks as 8-bit grayscale PNGs.

Targets and models (see registry.yaml, docs/07 section 5):
  subject            BiRefNet matting of the whole frame
  person (+bbox)     BiRefNet on a body crop of the face box, connected-component / watershed
                     selection of that one person (all people when no bbox is given)
  skin / hair / clothes
                     MediaPipe selfie multiclass segmenter (skin = body-skin + face-skin); small
                     people in large frames are re-segmented on crops and composed back
  sky                U^2-Net sky model when installed, otherwise the colour/position heuristic

Every mask is produced at the output resolution (long edge = `size`, upright), then its edges are
snapped to the image with a guided filter on the luminance. Everything heavy runs in one worker
thread so the event loop (progress, other RPCs, cancellation) stays responsive.
"""

from __future__ import annotations

import asyncio
import hashlib
import logging
import os
import re
import threading
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import cv2
import numpy as np
from PIL import Image

from ..decode import DecodeError, load_image
from ..errors import DECODE_FAILED, InvalidParams, ModelUnavailable, OutOfMemory, RpcError
from ..hw import HardwareInfo
from ..models.manager import ModelManager
from ..steps.embed import _is_oom
from ..steps.faces import FaceAnalyzer
from .filters import refine_alpha, resize_mask, smoothstep, to_u8
from .nets import (
    CLS_BODY_SKIN,
    CLS_CLOTHES,
    CLS_FACE_SKIN,
    CLS_HAIR,
    BiRefNet,
    SelfieMulticlass,
    SkySegmenter,
)
from .person import Box, select_person
from .sky import sky_heuristic

log = logging.getLogger(__name__)

TARGETS = ("subject", "sky", "person", "skin", "hair", "clothes")
PART_TARGETS = ("skin", "hair", "clothes")
YUNET = "yunet"
SELFIE = "mediapipe-selfie-multiclass"
SKY_MODEL = "skyseg-u2net"
BIREFNET_GPU = ["birefnet-fp16", "birefnet-lite-fp16", "birefnet-lite", "birefnet-lite-512"]
BIREFNET_CPU_T0 = ["birefnet-lite-512", "birefnet-lite"]
BIREFNET_CPU = ["birefnet-lite", "birefnet-lite-512"]
BIREFNET_DEFAULT_GPU = "birefnet-lite-fp16"  # birefnet-fp16 is an optional quality pack

SMALL_FACE_FRAC = 0.18  # faces narrower than this fraction of the long edge get a crop pass
MAX_PART_CROPS = 12
MAX_PERSONS_ALL = 8

ProgressFn = Callable[[dict[str, Any]], None]


def _safe_name(photo_id: Any) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", str(photo_id))


def bbox_hash(bbox: list[float]) -> str:
    """Stable 8-hex id of a normalised bbox (4 decimals), used in `<photo>_person_<hash>.png`."""
    key = ",".join(f"{v:.4f}" for v in bbox)
    return hashlib.sha1(key.encode()).hexdigest()[:8]  # noqa: S324 - not security relevant


def mask_path(out_dir: Path, photo_id: Any, target: str, bbox: list[float] | None) -> Path:
    name = f"{_safe_name(photo_id)}_{target}"
    if target == "person" and bbox is not None:
        name += f"_{bbox_hash(bbox)}"
    return out_dir / f"{name}.png"


def write_png(path: Path, alpha_u8: np.ndarray) -> None:
    tmp = path.with_name(path.name + ".tmp")
    with open(tmp, "wb") as f:
        Image.fromarray(alpha_u8, mode="L").save(f, "PNG", optimize=False, compress_level=3)
    os.replace(tmp, path)


def load_upright(path: str, size: int, orientation: int | None) -> np.ndarray:
    """Decode to an upright RGB uint8 array whose long edge is exactly `size` (up- or downscaled)."""
    d = load_image(path, size, orientation)
    rgb = d.rgb
    long_edge = max(d.width, d.height)
    if long_edge != size:
        s = size / long_edge
        nw, nh = max(1, round(d.width * s)), max(1, round(d.height * s))
        rgb = cv2.resize(rgb, (nw, nh), interpolation=cv2.INTER_CUBIC if s > 1 else cv2.INTER_AREA)
    return np.ascontiguousarray(rgb)


@dataclass
class MaskRequest:
    photo_id: Any
    path: str
    orientation: int | None
    targets: list[str]
    person_bbox: list[float] | None
    size: int
    out_dir: Path
    allow_download: bool

    @classmethod
    def parse(cls, params: Any) -> MaskRequest:
        if not isinstance(params, dict):
            raise InvalidParams("params must be an object")
        photo = params.get("photo")
        if (
            not isinstance(photo, dict)
            or "photo_id" not in photo
            or not isinstance(photo.get("path"), str)
        ):
            raise InvalidParams("photo needs photo_id and path")
        orient = photo.get("orientation")
        if orient is not None and (not isinstance(orient, int) or not 1 <= orient <= 8):
            raise InvalidParams("photo.orientation must be an EXIF value 1-8")
        targets = params.get("targets")
        if (
            not isinstance(targets, list)
            or not targets
            or not all(isinstance(t, str) for t in targets)
        ):
            raise InvalidParams("targets must be a non-empty array of strings")
        uniq: list[str] = []
        for t in targets:
            if t not in uniq:
                uniq.append(t)
        bbox = params.get("person_bbox")
        if bbox is not None:
            if (
                not isinstance(bbox, list)
                or len(bbox) != 4
                or not all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in bbox)
                or bbox[2] <= 0
                or bbox[3] <= 0
            ):
                raise InvalidParams("person_bbox must be [x, y, w, h] (normalised, w/h > 0)")
            bbox = [float(v) for v in bbox]
        size = params.get("size", 1024)
        if not isinstance(size, int) or isinstance(size, bool) or not 64 <= size <= 8192:
            raise InvalidParams("size must be an integer in [64, 8192]")
        out_dir = params.get("out_dir")
        if not isinstance(out_dir, str) or not out_dir:
            raise InvalidParams("out_dir is required")
        return cls(
            photo["photo_id"],
            photo["path"],
            orient,
            uniq,
            bbox,
            size,
            Path(out_dir),
            bool(params.get("allow_download", False)),
        )


@dataclass
class _Photo:
    """Decoded image at output resolution + per-request caches shared between targets."""

    rgb: np.ndarray
    faces: list[Box] | None = None
    subject: np.ndarray | None = None
    selfie: np.ndarray | None = None
    lock: threading.Lock = field(default_factory=threading.Lock)

    @property
    def height(self) -> int:
        return self.rgb.shape[0]

    @property
    def width(self) -> int:
        return self.rgb.shape[1]


class MaskGenerator:
    def __init__(self, manager: ModelManager, hw: HardwareInfo):
        self.manager = manager
        self.hw = hw
        self.force_cpu = hw.device == "cpu"
        self.pool = ThreadPoolExecutor(1, thread_name_prefix="mask")
        self._faces: dict[str, FaceAnalyzer] = {}

    def shutdown(self) -> None:
        self.pool.shutdown(wait=False, cancel_futures=True)
        for f in self._faces.values():
            f.close()
        self._faces.clear()

    # ------------------------------------------------------------------ model selection
    def _providers(self) -> list[str]:
        return ["CPUExecutionProvider"] if self.force_cpu else self.hw.providers

    def _birefnet_order(self) -> list[str]:
        if self.force_cpu or self.hw.device in ("cpu", "mps"):
            return BIREFNET_CPU_T0 if self.hw.tier == "T0" else BIREFNET_CPU
        return BIREFNET_GPU

    def subject_model(self) -> str:
        """Installed BiRefNet variant to use, or the one to download when none is installed."""
        order = self._birefnet_order()
        for mid in order:
            if self.manager.is_installed(mid):
                return mid
        return BIREFNET_DEFAULT_GPU if order is BIREFNET_GPU else order[0]

    def required_models(self, target: str) -> list[str]:
        """Hard model requirements of `target` (empty = works without downloads)."""
        if target in ("subject", "person"):
            return [self.subject_model()]
        if target in PART_TARGETS:
            return [SELFIE]
        return []

    def optional_models(self, target: str) -> list[str]:
        if target == "sky":
            return [SKY_MODEL]
        if target in ("person",) + PART_TARGETS:
            return [YUNET]  # face boxes: other people / small-person crops
        return []

    def models_for_targets(self, targets: list[str]) -> list[str]:
        out: list[str] = []
        for t in targets:
            for m in self.required_models(t) + self.optional_models(t):
                if m not in out:
                    out.append(m)
        return out

    # ------------------------------------------------------------------ loaders
    def _acquire_ort(self, mid: str, cls: type) -> Any:
        return self.manager.acquire(mid, lambda spec, path: cls(path, self._providers(), spec.id))

    def _birefnet(self) -> BiRefNet:
        return self._acquire_ort(self.subject_model(), BiRefNet)

    def _selfie(self) -> SelfieMulticlass:
        return self.manager.acquire(SELFIE, lambda spec, path: SelfieMulticlass(path, spec.id))

    def _sky_net(self) -> SkySegmenter | None:
        if not self.manager.is_installed(SKY_MODEL):
            return None
        return self._acquire_ort(SKY_MODEL, SkySegmenter)

    def _face_analyzer(self) -> FaceAnalyzer | None:
        if not self.manager.is_installed(YUNET):
            return None
        path = str(self.manager.path(YUNET))
        fa = self._faces.get(path)
        if fa is None:
            fa = self._faces[path] = FaceAnalyzer(path, None)
        return fa

    # ------------------------------------------------------------------ per-photo helpers
    def _detect_faces(self, photo: _Photo) -> list[Box]:
        if photo.faces is None:
            fa = self._face_analyzer()
            boxes: list[Box] = []
            if fa is not None:
                w, h = photo.width, photo.height
                s = min(1.0, 1280.0 / max(w, h))
                img = photo.rgb
                if s < 1.0:
                    img = cv2.resize(
                        img, (round(w * s), round(h * s)), interpolation=cv2.INTER_AREA
                    )
                for f in fa.detect(img):
                    bx, by, bw, bh = f["bbox"]
                    boxes.append((bx * w, by * h, bw * w, bh * h))
            photo.faces = boxes
        return photo.faces

    def _subject_alpha(self, photo: _Photo) -> np.ndarray:
        """Raw (unrefined) full-frame BiRefNet matte at output resolution."""
        if photo.subject is None:
            photo.subject = self._birefnet().predict(photo.rgb)
        return photo.subject

    def _part_probs(self, photo: _Photo) -> np.ndarray:
        """HxWx6 class probabilities; small faces are re-segmented on crops and blended in."""
        if photo.selfie is not None:
            return photo.selfie
        net = self._selfie()
        h, w = photo.height, photo.width
        probs = resize_mask(net.predict(photo.rgb), w, h)
        long_edge = max(w, h)
        small = [f for f in self._detect_faces(photo) if f[2] < SMALL_FACE_FRAC * long_edge]
        small.sort(key=lambda f: -f[2] * f[3])
        for fx, fy, fw, fh in small[:MAX_PART_CROPS]:
            side = max(fw, fh) * 5.0
            cx, cy = fx + fw / 2, fy + fh / 2 + 1.0 * fw
            x0, x1 = int(max(0, cx - side / 2)), int(min(w, cx + side / 2))
            y0, y1 = int(max(0, cy - side / 2)), int(min(h, cy + side / 2))
            if x1 - x0 < 16 or y1 - y0 < 16:
                continue
            pc = resize_mask(net.predict(photo.rgb[y0:y1, x0:x1]), x1 - x0, y1 - y0)
            wy = _edge_ramp(y1 - y0, 0.2)[:, None]
            wx = _edge_ramp(x1 - x0, 0.2)[None, :]
            wgt = (wy * wx)[..., None]
            probs[y0:y1, x0:x1] = probs[y0:y1, x0:x1] * (1 - wgt) + pc * wgt
        photo.selfie = probs
        return probs

    # ------------------------------------------------------------------ targets
    def _compute(
        self, photo: _Photo, target: str, bbox: list[float] | None
    ) -> tuple[np.ndarray, str]:
        """Return (alpha float32 [0,1] at output resolution, model id)."""
        rgb = photo.rgb
        if target == "subject":
            m = self._birefnet()
            return refine_alpha(self._subject_alpha(photo), rgb, 0.004, 1e-3), m.model_id
        if target == "person":
            return self._person(photo, bbox)
        if target in PART_TARGETS:
            p = self._part_probs(photo)
            if target == "skin":
                a = p[..., CLS_BODY_SKIN] + p[..., CLS_FACE_SKIN]
            elif target == "hair":
                a = p[..., CLS_HAIR]
            else:
                a = p[..., CLS_CLOTHES]
            a = refine_alpha(np.clip(a, 0.0, 1.0), rgb, 0.010, 2e-3)
            return smoothstep(a, 0.15, 0.85), SELFIE
        if target == "sky":
            return self._sky(photo)
        raise ValueError(target)

    def _sky(self, photo: _Photo) -> tuple[np.ndarray, str]:
        net = self._sky_net()
        if net is not None:
            raw = net.predict(photo.rgb)
            model = net.model_id
        else:
            subj = None
            if photo.subject is not None:
                subj = photo.subject
            elif self.manager.is_installed(self.subject_model()):
                subj = self._subject_alpha(photo)  # exclude people / foreground objects
            raw = sky_heuristic(photo.rgb, subj)
            model = "heuristic"
        return smoothstep(refine_alpha(raw, photo.rgb, 0.010, 2e-3), 0.1, 0.9), model

    def _person(self, photo: _Photo, bbox: list[float] | None) -> tuple[np.ndarray, str]:
        net = self._birefnet()
        w, h = photo.width, photo.height
        faces = self._detect_faces(photo)
        if bbox is not None:
            face: Box = (bbox[0] * w, bbox[1] * h, bbox[2] * w, bbox[3] * h)
            raw = select_person(photo.rgb, face, faces, net.predict)
        elif faces:
            raw = np.zeros((h, w), np.float32)
            for f in sorted(faces, key=lambda f: -f[2] * f[3])[:MAX_PERSONS_ALL]:
                raw = np.maximum(raw, select_person(photo.rgb, f, faces, net.predict))
        else:
            raw = self._subject_alpha(photo)  # no face detector / no faces: whole foreground
        return refine_alpha(raw, photo.rgb, 0.004, 1e-3), net.model_id

    # ------------------------------------------------------------------ run
    def _run_target(
        self, photo: _Photo, target: str, bbox: list[float] | None
    ) -> tuple[np.ndarray, str]:
        try:
            return self._compute(photo, target, bbox)
        except Exception as e:  # noqa: BLE001
            if not _is_oom(e) or self.force_cpu:
                if _is_oom(e):
                    raise OutOfMemory(str(e)) from e
                raise
            log.warning("OOM while computing mask %s; retrying on CPU", target)
            self.manager.unload()
            self.force_cpu = True
            return self._compute(photo, target, bbox)

    def _load_photo(self, req: MaskRequest) -> _Photo:
        return _Photo(load_upright(req.path, req.size, req.orientation))

    async def generate(self, params: Any, progress: ProgressFn | None = None) -> dict[str, Any]:
        req = MaskRequest.parse(params)
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        masks: dict[str, str] = {}
        models: dict[str, str] = {}
        skipped: dict[str, str] = {}

        targets = []
        for t in req.targets:
            if t in TARGETS:
                targets.append(t)
            else:
                skipped[t] = "unsupported_target"

        # --- models: download on request, otherwise skip targets whose model is missing
        failed: set[str] = set()
        wanted = [m for m in self.models_for_targets(targets) if not self.manager.is_installed(m)]
        if wanted and req.allow_download:
            for mid in wanted:

                def on_progress(e: dict[str, Any]) -> None:
                    loop.call_soon_threadsafe(
                        notify, {"kind": "model.download", "done": e["bytes"], **e}
                    )

                try:
                    await loop.run_in_executor(
                        self.pool, lambda m=mid: self.manager.ensure(m, on_progress)
                    )
                except Exception as e:  # noqa: BLE001
                    log.warning("download of %s failed: %s", mid, e)
                    failed.add(mid)
        active: list[str] = []
        for t in targets:
            absent = [m for m in self.required_models(t) if not self.manager.is_installed(m)]
            if absent:
                skipped[t] = "download_failed" if set(absent) & failed else "model_unavailable"
            else:
                active.append(t)

        if active:
            try:
                photo = await loop.run_in_executor(self.pool, self._load_photo, req)
            except DecodeError as e:
                raise RpcError(DECODE_FAILED, str(e), "decode_failed") from e
            req.out_dir.mkdir(parents=True, exist_ok=True)
        total = len(active)
        for i, t in enumerate(active):
            try:
                alpha, model = await loop.run_in_executor(
                    self.pool, self._run_target, photo, t, req.person_bbox
                )
            except ModelUnavailable:
                skipped[t] = "model_unavailable"
            except OutOfMemory:
                skipped[t] = "out_of_memory"
            except Exception:  # noqa: BLE001
                log.exception("mask %s failed", t)
                skipped[t] = "failed"
            else:
                path = mask_path(req.out_dir, req.photo_id, t, req.person_bbox)
                await loop.run_in_executor(self.pool, write_png, path, to_u8(alpha))
                masks[t] = str(path)
                models[t] = model
            if total > 1:
                notify({"kind": "mask", "done": i + 1, "total": total})
        return {"masks": masks, "models": models, "skipped": skipped}


def _edge_ramp(n: int, frac: float) -> np.ndarray:
    """1 in the middle, smooth 0 -> 1 ramp over `frac` of the length at both ends."""
    k = max(1, round(n * frac))
    x = np.ones(n, np.float32)
    ramp = smoothstep(np.arange(k, dtype=np.float32) + 0.5, 0.0, float(k))
    x[:k] = np.minimum(x[:k], ramp)
    x[n - k :] = np.minimum(x[n - k :], ramp[::-1])
    return x
