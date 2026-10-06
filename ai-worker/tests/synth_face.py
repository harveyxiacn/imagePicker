"""Synthetic faces with exact 478-point landmarks for model-free Best Take tests.

`template(...)` places a canonical landmark layout (unit = face width) at a given centre, so a drawn
face and its "detected" landmarks agree by construction; `observer(...)` is a drop-in for the
MediaPipe-based `Components.observe`.
"""

from __future__ import annotations

import math

import cv2
import numpy as np

from imagepicker_ai.beauty import landmarks as lmk
from imagepicker_ai.facemesh import FaceObs

OVAL_HALF = (0.5, 0.65)  # oval semi axes in face widths
EYE_C = 0.22  # eye centre offset from the midline
EYE_Y = -0.10
BROW_Y = -0.22
MOUTH_Y = 0.30


def _loop(idx: list[int], centre: tuple[float, float], rx: float, ry: float, start: float = 0.0):
    n = len(idx)
    out = {}
    for k, i in enumerate(idx):
        a = start + 2 * math.pi * k / n
        out[i] = (centre[0] + rx * math.cos(a), centre[1] + ry * math.sin(a))
    return out


def template() -> np.ndarray:
    """(478, 2) landmarks in face-width units around the oval centre (0, 0)."""
    pts: dict[int, tuple[float, float]] = {}
    pts.update(_loop(lmk.FACE_OVAL, (0, 0), *OVAL_HALF, start=-math.pi / 2))
    pts.update(_loop(lmk.RIGHT_EYE, (-EYE_C, EYE_Y), 0.09, 0.045))
    pts.update(_loop(lmk.LEFT_EYE, (EYE_C, EYE_Y), 0.09, 0.045))
    pts.update(_loop(lmk.RIGHT_BROW, (-EYE_C, BROW_Y), 0.11, 0.025))
    pts.update(_loop(lmk.LEFT_BROW, (EYE_C, BROW_Y), 0.11, 0.025))
    pts.update(_loop(lmk.LIPS_OUTER, (0, MOUTH_Y), 0.16, 0.06))
    pts.update(_loop(lmk.LIPS_INNER, (0, MOUTH_Y), 0.12, 0.02))
    nose = {1: 0.05, 2: 0.10, 4: 0.0, 5: -0.02, 6: -0.08, 8: -0.12, 9: -0.16, 19: 0.11, 94: 0.12,
            168: -0.10, 195: 0.02, 197: -0.04}  # fmt: skip
    for i, y in nose.items():
        pts[i] = (0.0, y)
    pts.update({97: (-0.05, 0.10), 98: (-0.07, 0.09), 129: (-0.09, 0.07), 358: (0.09, 0.07)})
    pts.update({13: (0, MOUTH_Y - 0.02), 14: (0, MOUTH_Y + 0.02)})
    rng = np.random.default_rng(1)
    arr = np.zeros((lmk.N_FULL, 2))
    for i in range(lmk.N_FULL):
        if i in pts:
            arr[i] = pts[i]
        else:  # anything unused: inside the oval
            r = math.sqrt(rng.random()) * 0.4
            a = rng.random() * 2 * math.pi
            arr[i] = (r * math.cos(a) * 0.9, r * math.sin(a) * 1.1)
    return arr


TEMPLATE = template()


def landmarks_for(box: tuple[float, float, float, float]) -> np.ndarray:
    x, y, w, h = box
    return TEMPLATE * w + (x + w / 2, y + h / 2)


def face_box(cx: float, cy: float, fw: float) -> tuple[float, float, float, float]:
    return (cx - fw / 2, cy - OVAL_HALF[1] * fw, fw, 2 * OVAL_HALF[1] * fw)


def observer(pose: tuple[float, float, float] | None = (0.0, 0.0, 0.0), poses=None):
    """`Components.observe` stand-in. `poses`: optional list consumed one entry per call."""
    calls = {"n": 0}

    def observe(rgb, face_px):
        p = pose
        if poses is not None:
            p = poses[min(calls["n"], len(poses) - 1)]
        calls["n"] += 1
        return FaceObs(landmarks_for(tuple(face_px)), p)

    return observe


def draw_face(
    img: np.ndarray,
    cx: float,
    cy: float,
    fw: float,
    eyes_open: bool,
    skin=(224, 172, 140),
    seed: int = 0,
) -> None:
    """Draw the face on `img` (RGB uint8, in place) consistent with `landmarks_for(face_box(..))`."""
    h, w = img.shape[:2]
    layer = np.zeros_like(img)
    ax, ay = OVAL_HALF[0] * fw, OVAL_HALF[1] * fw
    c = (int(round(cx)), int(round(cy)))
    mask = np.zeros((h, w), np.uint8)
    cv2.ellipse(mask, c, (int(ax), int(ay)), 0, 0, 360, 255, -1, cv2.LINE_AA)
    # hair cap above the oval
    hair = np.zeros((h, w), np.uint8)
    cv2.ellipse(
        hair,
        (c[0], c[1] - int(0.12 * fw)),
        (int(ax * 1.12), int(ay * 1.08)),
        0,
        180,
        360,
        255,
        -1,
        cv2.LINE_AA,
    )
    # skin: gentle shading + fine texture (so alignment / flow have something to lock on)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    shade = 1.0 - 0.12 * ((xx - cx) / ax) ** 2 - 0.10 * ((yy - cy) / ay)
    rng = np.random.default_rng(seed)
    tex = cv2.GaussianBlur(rng.normal(0, 7, (h, w)).astype(np.float32), (0, 0), 1.2) * 1.5
    face_rgb = np.clip(
        np.array(skin, np.float32)[None, None] * shade[..., None] + tex[..., None], 0, 255
    )
    layer = face_rgb.astype(np.uint8)
    out = img.astype(np.float32)
    ha = (hair / 255.0)[..., None]
    out = out * (1 - ha) + np.array((60, 40, 30), np.float32) * ha
    ma = (mask / 255.0)[..., None]
    out = out * (1 - ma) + layer.astype(np.float32) * ma
    img[:] = np.clip(out, 0, 255).astype(np.uint8)
    # features
    for sgn in (-1, 1):
        ex, ey = int(round(cx + sgn * EYE_C * fw)), int(round(cy + EYE_Y * fw))
        erx, ery = int(0.09 * fw), int(0.045 * fw)
        if eyes_open:
            cv2.ellipse(img, (ex, ey), (erx, ery), 0, 0, 360, (245, 245, 245), -1, cv2.LINE_AA)
            cv2.circle(img, (ex, ey), max(2, int(0.035 * fw)), (40, 70, 120), -1, cv2.LINE_AA)
            cv2.circle(img, (ex, ey), max(1, int(0.015 * fw)), (5, 5, 5), -1, cv2.LINE_AA)
        else:
            cv2.ellipse(
                img, (ex, ey), (erx, max(1, ery // 4)), 0, 0, 360, (150, 100, 80), -1, cv2.LINE_AA
            )
        by = int(round(cy + BROW_Y * fw))
        cv2.ellipse(
            img,
            (ex, by),
            (int(0.11 * fw), max(1, int(0.02 * fw))),
            0,
            0,
            360,
            (70, 45, 30),
            -1,
            cv2.LINE_AA,
        )
    cv2.ellipse(
        img,
        (c[0], int(cy + MOUTH_Y * fw)),
        (int(0.16 * fw), int(0.05 * fw)),
        0,
        0,
        360,
        (170, 70, 80),
        -1,
        cv2.LINE_AA,
    )
    cv2.line(
        img,
        (c[0], int(cy - 0.05 * fw)),
        (c[0], int(cy + 0.10 * fw)),
        (190, 140, 115),
        max(1, int(0.015 * fw)),
        cv2.LINE_AA,
    )
