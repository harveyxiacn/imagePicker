"""MediaPipe Face Mesh (478 points) index sets and polygon rasterisation.

All sets are ordered loops (closed polygons) in the canonical 468-point topology; "left"/"right"
follow the *subject* (MediaPipe naming), so the subject's right eye is on the image left.

  RIGHT_EYE      subject's right eye (image left): lower lid corner->corner, then upper lid back
  LEFT_EYE       subject's left eye (image right)
  RIGHT_BROW     subject's right eyebrow (image left): lower edge then upper edge
  LEFT_BROW      subject's left eyebrow (image right)
  LIPS_OUTER     outer lip contour (includes the mouth opening and the teeth)
  LIPS_INNER     inner lip contour (the mouth opening)
  FACE_OVAL      jaw / forehead outline
  NOSTRILS       hull of the alar-base / nostril landmarks (rejection zone for blemishes)
  iris           points 468-477 (centres 468 / 473)
"""

from __future__ import annotations

import cv2
import numpy as np

RIGHT_EYE = [33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246]
LEFT_EYE = [362, 382, 381, 380, 374, 373, 390, 249, 263, 466, 388, 387, 386, 385, 384, 398]
RIGHT_BROW = [46, 53, 52, 65, 55, 107, 66, 105, 63, 70]
LEFT_BROW = [276, 283, 282, 295, 285, 336, 296, 334, 293, 300]
LIPS_OUTER = [
    61,
    146,
    91,
    181,
    84,
    17,
    314,
    405,
    321,
    375,
    291,
    409,
    270,
    269,
    267,
    0,
    37,
    39,
    40,
    185,
]
LIPS_INNER = [
    78,
    95,
    88,
    178,
    87,
    14,
    317,
    402,
    318,
    324,
    308,
    415,
    310,
    311,
    312,
    13,
    82,
    81,
    80,
    191,
]
FACE_OVAL = [
    10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377,
    152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234, 127, 162, 21, 54, 103, 67, 109,
]  # fmt: skip
NOSTRILS = [48, 64, 98, 97, 2, 326, 327, 294, 278, 331, 279, 360, 438, 457, 237, 218, 129, 358]
NOSE_TIP = 1
N_FACE = 468
N_FULL = 478

# polygons removed from the skin mask: (index loop, dilation in face widths)
SKIN_EXCLUDE = (
    (RIGHT_EYE, 0.012),
    (LEFT_EYE, 0.012),
    (RIGHT_BROW, 0.010),
    (LEFT_BROW, 0.010),
    (LIPS_OUTER, 0.010),
)


def polygon_mask(
    pts_px: np.ndarray, idx: list[int], shape: tuple[int, int], dilate_px: float = 0.0
) -> np.ndarray:
    """Rasterise the polygon `idx` of landmarks `pts_px` (N, 2 pixels) -> uint8 {0, 1} mask HxW."""
    m = np.zeros(shape, np.uint8)
    poly = np.round(pts_px[idx] * 16).astype(np.int32)  # 4 fractional bits for sub-pixel edges
    cv2.fillPoly(m, [poly], 1, lineType=cv2.LINE_AA, shift=4)
    if dilate_px >= 0.5:
        r = int(round(dilate_px))
        k = cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * r + 1, 2 * r + 1))
        m = cv2.dilate(m, k)
    return m


def hull_mask(pts_px: np.ndarray, idx: list[int], shape: tuple[int, int]) -> np.ndarray:
    m = np.zeros(shape, np.uint8)
    hull = cv2.convexHull(np.round(pts_px[idx]).astype(np.int32))
    cv2.fillConvexPoly(m, hull, 1)
    return m


def feature_mask(pts_px: np.ndarray, shape: tuple[int, int], face_width_px: float) -> np.ndarray:
    """Union of the eye, eyebrow and (outer) lip polygons, dilated; uint8 {0, 1}."""
    out = np.zeros(shape, np.uint8)
    for idx, d in SKIN_EXCLUDE:
        out |= polygon_mask(pts_px, idx, shape, d * face_width_px)
    return out


def oval_mask(pts_px: np.ndarray, shape: tuple[int, int]) -> np.ndarray:
    return polygon_mask(pts_px, FACE_OVAL, shape)
