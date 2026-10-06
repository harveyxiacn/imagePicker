"""Shared image I/O and request parsing for the M5 generative methods.

`besttake.compose`, `inpaint.run` and `enhance.run` all work on the full-resolution upright photo
and return RGBA PNG patches; this module holds the decode / encode / parameter helpers they share.
"""

from __future__ import annotations

import os
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from .decode import DecodeError, load_image
from .errors import DECODE_FAILED, InvalidParams, RpcError

MAX_SIDE = 8192  # decode cap (long edge); normalised rects are unaffected by it

Box = tuple[float, float, float, float]  # x, y, w, h


def safe_name(v: Any) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", str(v))


@dataclass(frozen=True)
class PhotoRef:
    photo_id: Any
    path: str
    orientation: int | None


def parse_photo(obj: Any, what: str = "photo") -> PhotoRef:
    if not isinstance(obj, dict) or "photo_id" not in obj or not isinstance(obj.get("path"), str):
        raise InvalidParams(f"{what} needs photo_id and path")
    orient = obj.get("orientation")
    if orient is not None and (
        not isinstance(orient, int) or isinstance(orient, bool) or not 1 <= orient <= 8
    ):
        raise InvalidParams(f"{what}.orientation must be an EXIF value 1-8")
    return PhotoRef(obj["photo_id"], obj["path"], orient)


def parse_box(v: Any, what: str) -> list[float]:
    if (
        not isinstance(v, list)
        or len(v) != 4
        or not all(isinstance(x, (int, float)) and not isinstance(x, bool) for x in v)
        or v[2] <= 0
        or v[3] <= 0
    ):
        raise InvalidParams(f"{what} must be [x, y, w, h] (normalised, w/h > 0)")
    return [float(x) for x in v]


def parse_out_dir(params: dict[str, Any]) -> Path:
    out_dir = params.get("out_dir")
    if not isinstance(out_dir, str) or not out_dir:
        raise InvalidParams("out_dir is required")
    return Path(out_dir)


def load_rgb(ref: PhotoRef, max_side: int = MAX_SIDE) -> np.ndarray:
    """Upright RGB uint8 at native resolution (long edge capped at `max_side`)."""
    try:
        d = load_image(ref.path, max_side, ref.orientation)
    except DecodeError as e:
        raise RpcError(DECODE_FAILED, str(e), "decode_failed") from e
    return np.ascontiguousarray(d.rgb)


def box_px(box: list[float], w: int, h: int) -> Box:
    return (box[0] * w, box[1] * h, box[2] * w, box[3] * h)


def norm_rect(rect: tuple[int, int, int, int], w: int, h: int) -> list[float]:
    """Pixel rect (x, y, w, h) -> normalised [x, y, w, h] as plain Python floats (5 decimals)."""
    x, y, rw, rh = (float(v) for v in rect)
    return [round(x / w, 5), round(y / h, 5), round(rw / w, 5), round(rh / h, 5)]


def _write_atomic(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.write_bytes(data)
    os.replace(tmp, path)


def encode_png(img: np.ndarray, level: int = 1) -> bytes:
    """PNG bytes of an RGB / RGBA / gray uint8 array (cv2 encoder: much faster than Pillow)."""
    if img.ndim == 3 and img.shape[2] == 3:
        img = cv2.cvtColor(img, cv2.COLOR_RGB2BGR)
    elif img.ndim == 3 and img.shape[2] == 4:
        img = cv2.cvtColor(img, cv2.COLOR_RGBA2BGRA)
    ok, buf = cv2.imencode(".png", img, [cv2.IMWRITE_PNG_COMPRESSION, level])
    if not ok:
        raise RuntimeError("png encode failed")
    return buf.tobytes()


def write_png(path: Path, img: np.ndarray, level: int = 1) -> None:
    _write_atomic(path, encode_png(img, level))


def write_rgba_png(path: Path, rgb: np.ndarray, alpha: np.ndarray, level: int = 1) -> None:
    """`alpha`: uint8 HxW or float [0, 1]."""
    if alpha.dtype != np.uint8:
        alpha = np.clip(np.rint(alpha * 255.0), 0, 255).astype(np.uint8)
    write_png(path, np.dstack([rgb, alpha]), level)


def read_mask(path: str, width: int, height: int) -> np.ndarray:
    """8-bit mask PNG -> float32 [0, 1] resized to (width, height) (area / bilinear)."""
    try:
        data = np.fromfile(path, dtype=np.uint8)
        m = cv2.imdecode(data, cv2.IMREAD_UNCHANGED)
    except Exception as e:  # noqa: BLE001
        raise InvalidParams(f"cannot read mask {path}: {e}") from e
    if m is None:
        raise InvalidParams(f"cannot decode mask {path}")
    if m.ndim == 3:
        if m.shape[2] == 4:  # RGBA: alpha if it carries information, else luminance
            a = m[..., 3]
            m = a if a.min() < 255 else cv2.cvtColor(m[..., :3], cv2.COLOR_BGR2GRAY)
        else:
            m = cv2.cvtColor(m, cv2.COLOR_BGR2GRAY)
    if m.dtype == np.uint16:
        m = (m >> 8).astype(np.uint8)
    f = m.astype(np.float32) * (1.0 / 255.0)
    if f.shape[1] != width or f.shape[0] != height:
        shrink = width < f.shape[1] or height < f.shape[0]
        f = cv2.resize(
            f, (width, height), interpolation=cv2.INTER_AREA if shrink else cv2.INTER_LINEAR
        )
    return f
