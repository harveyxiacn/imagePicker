"""Pick the person instance that belongs to a face box (group photos).

BiRefNet is a salient-object matting network: it returns *everybody* as one foreground blob. To
isolate one person we
  1. crop a body-sized window below / around the face (5.2 face widths wide, from 0.8 face heights
     above the face to 7 below) so that other people are mostly outside the network input,
  2. threshold the matte and keep the connected component(s) that cover the face box,
  3. when the component still contains other detected faces (people touching / overlapping),
     split it with a marker-based watershed: one marker per face (head + torso strip), the
     confident background as the extra marker, and keep the pixels won by the target face.
The returned map is a soft alpha: matte * smoothed selection, so edges keep BiRefNet's softness.
"""

from __future__ import annotations

from collections.abc import Callable

import cv2
import numpy as np

# face box in pixels: (x, y, w, h)
Box = tuple[float, float, float, float]

BODY_HALF_WIDTH = 2.6  # in face widths
BODY_ABOVE = 0.8  # in face heights
BODY_BELOW = 7.0
FG_THRESHOLD = 0.5


def body_crop(face: Box, img_w: int, img_h: int) -> tuple[int, int, int, int]:
    """(x0, y0, x1, y1) integer crop window for a face, clipped to the image."""
    x, y, w, h = face
    cx = x + w / 2
    x0 = max(0, int(np.floor(cx - BODY_HALF_WIDTH * w)))
    x1 = min(img_w, int(np.ceil(cx + BODY_HALF_WIDTH * w)))
    y0 = max(0, int(np.floor(y - BODY_ABOVE * h)))
    y1 = min(img_h, int(np.ceil(y + (1.0 + BODY_BELOW) * h)))
    return x0, y0, max(x0 + 1, x1), max(y0 + 1, y1)


def _box_mask(shape: tuple[int, int], box: Box, shrink: float = 0.0) -> np.ndarray:
    x, y, w, h = box
    m = np.zeros(shape, np.uint8)
    dx, dy = w * shrink, h * shrink
    x0, y0 = max(0, int(round(x + dx))), max(0, int(round(y + dy)))
    x1, y1 = min(shape[1], int(round(x + w - dx))), min(shape[0], int(round(y + h - dy)))
    if x1 > x0 and y1 > y0:
        m[y0:y1, x0:x1] = 1
    return m


def _torso_marker(shape: tuple[int, int], face: Box) -> np.ndarray:
    """Head + a narrow strip below it: where the person's own pixels certainly are."""
    x, y, w, h = face
    m = _box_mask(shape, face, shrink=0.2)
    cx = x + w / 2
    strip: Box = (cx - 0.45 * w, y + 1.2 * h, 0.9 * w, 1.8 * h)
    return m | _box_mask(shape, strip)


def _same_person(a: Box, b: Box) -> bool:
    ax, ay = a[0] + a[2] / 2, a[1] + a[3] / 2
    bx, by = b[0] + b[2] / 2, b[1] + b[3] / 2
    return abs(ax - bx) < 0.5 * max(a[2], b[2]) and abs(ay - by) < 0.5 * max(a[3], b[3])


def select_person(
    rgb: np.ndarray,
    face: Box,
    others: list[Box],
    matte: Callable[[np.ndarray], np.ndarray],
) -> np.ndarray:
    """Soft alpha (float32 HxW, full image size) of the person owning `face`.

    `matte(crop_rgb)` returns the foreground alpha for a crop (BiRefNet). `others` are the other
    detected face boxes of the image (pixels); the target face is dropped from them.
    """
    ih, iw = rgb.shape[:2]
    x0, y0, x1, y1 = body_crop(face, iw, ih)
    crop = np.ascontiguousarray(rgb[y0:y1, x0:x1])
    ch, cw = crop.shape[:2]
    alpha = np.clip(matte(crop), 0.0, 1.0).astype(np.float32)

    local: Box = (face[0] - x0, face[1] - y0, face[2], face[3])
    fg = (alpha > FG_THRESHOLD).astype(np.uint8)
    n, labels = cv2.connectedComponents(fg, connectivity=8)
    out = np.zeros((ih, iw), np.float32)
    if n <= 1:
        return out

    core = _box_mask((ch, cw), local, shrink=0.2).astype(bool)
    area = max(int(core.sum()), 1)
    overlap = np.bincount(labels[core].ravel(), minlength=n).astype(np.float64)
    overlap[0] = 0.0
    best = int(overlap.argmax())
    if overlap[best] < 0.1 * area:
        return out  # the matte does not see this face as foreground
    keep = np.zeros(n, bool)
    keep[overlap >= max(0.25 * overlap[best], 0.1 * area)] = True
    keep[best] = True
    selected = keep[labels]

    rivals = [
        (o[0] - x0, o[1] - y0, o[2], o[3])
        for o in others
        if not _same_person(face, o)
        and _box_mask((ch, cw), (o[0] - x0, o[1] - y0, o[2], o[3])).any()
    ]
    if rivals:
        selected = _split_with_watershed(crop, alpha, selected, local, rivals)

    sel = cv2.GaussianBlur(selected.astype(np.float32), (0, 0), 1.5)
    out[y0:y1, x0:x1] = alpha * np.clip(sel * 1.5, 0.0, 1.0)
    return out


def _split_with_watershed(
    crop: np.ndarray, alpha: np.ndarray, selected: np.ndarray, face: Box, rivals: list[Box]
) -> np.ndarray:
    ch, cw = crop.shape[:2]
    s = min(1.0, 512.0 / max(ch, cw))
    if s < 1.0:
        sw, sh = max(1, round(cw * s)), max(1, round(ch * s))
        img = cv2.resize(crop, (sw, sh), interpolation=cv2.INTER_AREA)
        sel = cv2.resize(selected.astype(np.uint8), (sw, sh), interpolation=cv2.INTER_NEAREST)
        al = cv2.resize(alpha, (sw, sh), interpolation=cv2.INTER_AREA)
    else:
        img, sel, al = crop, selected.astype(np.uint8), alpha
        sw, sh = cw, ch

    def sc(b: Box) -> Box:
        return (b[0] * s, b[1] * s, b[2] * s, b[3] * s)

    markers = np.zeros((sh, sw), np.int32)
    bg_label = len(rivals) + 2
    markers[al < 0.1] = bg_label
    markers[(sel > 0) & (markers == bg_label)] = 0
    markers[_torso_marker((sh, sw), sc(face)).astype(bool) & (sel > 0)] = 1
    for i, r in enumerate(rivals):
        m = _torso_marker((sh, sw), sc(r)).astype(bool) & (sel > 0) & (markers != 1)
        markers[m] = i + 2
    if not (markers == 1).any():
        return selected
    bgr = cv2.cvtColor(img, cv2.COLOR_RGB2BGR)
    cv2.watershed(cv2.GaussianBlur(bgr, (0, 0), 1.0), markers)
    won = (markers == 1) & (sel > 0)
    # unresolved pixels (-1, watershed lines) next to our region stay with us
    lines = (markers == -1) & (sel > 0)
    won |= (
        cv2.dilate((markers == 1).astype(np.uint8), np.ones((3, 3), np.uint8)).astype(bool) & lines
    )
    if s < 1.0:
        won = cv2.resize(won.astype(np.uint8), (cw, ch), interpolation=cv2.INTER_NEAREST) > 0
    return won & selected
