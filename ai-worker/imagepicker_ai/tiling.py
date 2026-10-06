"""Tiled inference over large images for fixed-scale restoration networks (denoise, upscale).

Tiles are always padded to the same size (edge-replicated / reflected) so GPU kernels are reused
and the network sees no zero borders. Two stitching modes:

  blend=True   feathered overlap: every tile contributes with a linear ramp across the overlap,
               accumulated in float32 (SCUNet: window attention shows seams without it);
  blend=False  `overlap` pixels of context are computed around each tile and discarded
               (Real-ESRGAN style); no float accumulators, writes uint8 directly.
"""

from __future__ import annotations

import math
from collections.abc import Callable

import cv2
import numpy as np

# fn(batch NCHW float32 in [0, 1]) -> NCHW float32 (scale x larger), values nominally in [0, 1]
TileFn = Callable[[np.ndarray], np.ndarray]


def _ramp(n: int, o: int, first: bool, last: bool) -> np.ndarray:
    w = np.ones(n, np.float32)
    if o > 0:
        r = (np.arange(o, dtype=np.float32) + 0.5) / o
        if not first:
            w[:o] = r
        if not last:
            w[n - o :] = np.minimum(w[n - o :], r[::-1])
    return w


def _starts(total: int, tile: int, step: int) -> list[int]:
    if total <= tile:
        return [0]
    xs = list(range(0, total - tile, step))
    xs.append(total - tile)
    return xs


def run_tiled(
    img: np.ndarray,
    fn: TileFn,
    tile: int,
    overlap: int,
    scale: int = 1,
    multiple: int = 1,
    blend: bool = True,
    progress: Callable[[int, int], None] | None = None,
    should_stop: Callable[[], bool] | None = None,
) -> np.ndarray:
    """`img` HxWx3 uint8 -> (H*scale)x(W*scale)x3 uint8."""
    h, w = img.shape[:2]
    th = min(tile, int(math.ceil(h / multiple)) * multiple)
    tw = min(tile, int(math.ceil(w / multiple)) * multiple)
    th, tw = max(th, multiple), max(tw, multiple)
    ov = max(0, min(overlap, th // 2 - 1, tw // 2 - 1))
    ys = _starts(h, th, th - 2 * ov)
    xs = _starts(w, tw, tw - 2 * ov)
    total = len(ys) * len(xs)
    done = 0

    if blend:
        acc = np.zeros((h * scale, w * scale, 3), np.float32)
        wsum = np.zeros((h * scale, w * scale), np.float32)
    else:
        out = np.empty((h * scale, w * scale, 3), np.uint8)

    def pad_tile(y0: int, x0: int) -> tuple[np.ndarray, int, int]:
        """Crop (th x tw), padded to the full tile size; returns (tile, valid_h, valid_w)."""
        crop = img[y0 : y0 + th, x0 : x0 + tw]
        ch, cw = crop.shape[:2]
        if ch < th or cw < tw:
            crop = cv2.copyMakeBorder(
                crop,
                0,
                th - ch,
                0,
                tw - cw,
                cv2.BORDER_REFLECT_101 if min(ch, cw) > 1 else cv2.BORDER_REPLICATE,
            )
        return crop, ch, cw

    for iy, y0 in enumerate(ys):
        for ix, x0 in enumerate(xs):
            if should_stop and should_stop():
                raise InterruptedError
            crop, ch, cw = pad_tile(y0, x0)
            x = np.ascontiguousarray(crop.transpose(2, 0, 1)[None].astype(np.float32) * (1 / 255.0))
            y = np.asarray(fn(x), dtype=np.float32)[0].transpose(1, 2, 0)  # (th*s, tw*s, 3)
            y = y[: ch * scale, : cw * scale]
            if blend:
                wy = _ramp(ch * scale, ov * scale, iy == 0, iy == len(ys) - 1)
                wx = _ramp(cw * scale, ov * scale, ix == 0, ix == len(xs) - 1)
                wgt = wy[:, None] * wx[None, :]
                sl = (
                    slice(y0 * scale, y0 * scale + ch * scale),
                    slice(x0 * scale, x0 * scale + cw * scale),
                )
                acc[sl] += y * wgt[..., None]
                wsum[sl] += wgt
            else:
                top = 0 if iy == 0 else ov
                left = 0 if ix == 0 else ov
                bottom = ch if iy == len(ys) - 1 else ch - ov
                right = cw if ix == len(xs) - 1 else cw - ov
                # the last tiles are shifted back to fit; trim what earlier tiles already wrote
                gy0 = y0 + top
                gx0 = x0 + left
                if iy == len(ys) - 1 and iy > 0:
                    prev_end = ys[iy - 1] + th - ov
                    gy0 = max(gy0, prev_end)
                    top = gy0 - y0
                if ix == len(xs) - 1 and ix > 0:
                    prev_end = xs[ix - 1] + tw - ov
                    gx0 = max(gx0, prev_end)
                    left = gx0 - x0
                blk = y[top * scale : bottom * scale, left * scale : right * scale]
                out[
                    gy0 * scale : gy0 * scale + blk.shape[0],
                    gx0 * scale : gx0 * scale + blk.shape[1],
                ] = np.clip(np.rint(blk * 255.0), 0, 255).astype(np.uint8)
            done += 1
            if progress:
                progress(done, total)

    if blend:
        res = acc / np.maximum(wsum, 1e-6)[..., None]
        return np.clip(np.rint(res * 255.0), 0, 255).astype(np.uint8)
    return out
