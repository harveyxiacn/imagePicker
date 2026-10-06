"""Image decoding: Pillow (+ pillow-heif), EXIF orientation applied, resized to `analysis_size`."""

from __future__ import annotations

from dataclasses import dataclass

import numpy as np
from PIL import Image, ImageOps

try:  # HEIC/HEIF/AVIF support is optional at runtime
    import pillow_heif

    pillow_heif.register_heif_opener()
except Exception:  # noqa: BLE001  # pragma: no cover
    pillow_heif = None

Image.MAX_IMAGE_PIXELS = None  # photos from 100 MP bodies are legitimate


class DecodeError(Exception):
    pass


@dataclass
class Decoded:
    rgb: np.ndarray  # HxWx3 uint8, orientation applied, long edge <= analysis_size
    orig_width: int  # after orientation
    orig_height: int
    scale: float  # analysis / original (<= 1)

    @property
    def height(self) -> int:
        return self.rgb.shape[0]

    @property
    def width(self) -> int:
        return self.rgb.shape[1]


_ORIENT_OPS = {
    2: Image.Transpose.FLIP_LEFT_RIGHT,
    3: Image.Transpose.ROTATE_180,
    4: Image.Transpose.FLIP_TOP_BOTTOM,
    5: Image.Transpose.TRANSPOSE,
    6: Image.Transpose.ROTATE_270,
    7: Image.Transpose.TRANSVERSE,
    8: Image.Transpose.ROTATE_90,
}


def load_image(
    path: str, analysis_size: int = 1024, orientation_hint: int | None = None
) -> Decoded:
    """Decode `path` to an RGB uint8 array.

    The EXIF orientation embedded in the file wins; `orientation_hint` (EXIF value 1-8 from the
    Core's own metadata pass) is only applied when the file itself carries no orientation tag
    (e.g. a HEIC/RAW preview extracted without EXIF).
    """
    try:
        im = Image.open(path)
        file_orient = im.getexif().get(0x0112)
        w0, h0 = im.size
        if im.format == "JPEG" and max(w0, h0) > analysis_size * 2:
            # DCT-domain downscale: much faster decode for large JPEGs; draft() never goes
            # below the requested size, so the long edge stays >= analysis_size.
            s = analysis_size / max(w0, h0)
            im.draft("RGB", (max(1, round(w0 * s)), max(1, round(h0 * s))))
        if file_orient in (None, 0, 1) and orientation_hint in _ORIENT_OPS:
            im = im.transpose(_ORIENT_OPS[orientation_hint])
            swapped = orientation_hint >= 5
        else:
            im = ImageOps.exif_transpose(im)
            swapped = (file_orient or 1) >= 5
        if im.mode != "RGB":
            im = im.convert("RGB")
    except Exception as e:  # noqa: BLE001
        raise DecodeError(f"cannot decode {path}: {e}") from e

    ow, oh = (h0, w0) if swapped else (w0, h0)
    w, h = im.size
    long_edge = max(w, h)
    if long_edge > analysis_size:
        s = analysis_size / long_edge
        nw, nh = max(1, round(w * s)), max(1, round(h * s))
        # reducing_gap makes Pillow use a cheap box pre-reduce, then LANCZOS: fast and alias-free
        im = im.resize((nw, nh), Image.Resampling.LANCZOS, reducing_gap=2.0)
    arr = np.asarray(im, dtype=np.uint8)
    arr = np.ascontiguousarray(arr)
    return Decoded(arr, ow, oh, scale=max(arr.shape[:2]) / max(ow, oh))
