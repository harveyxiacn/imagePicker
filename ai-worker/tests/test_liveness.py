"""Liveness / flatness heuristic, expression mapping and saliency box (no models)."""

import cv2
import numpy as np

from imagepicker_ai.steps import faces as faces_mod
from imagepicker_ai.steps.composition import saliency_box
from imagepicker_ai.steps.faces import expression_flag
from imagepicker_ai.steps.liveness import (
    ARTIFICIAL_AT,
    MONO_SAT,
    artificial_confidence,
    face_artificial,
    face_features,
    image_saturation,
    plane_r2,
)

W, H = 640, 480
BOX = (0.35, 0.2, 0.3, 0.45)  # normalised face box on the 640x480 canvas


def _scene(face_rgb, seed=0, bg=(90, 120, 140)):
    """A canvas with an oval face region painted by `face_rgb(h, w) -> (h, w, 3) uint8`."""
    img = np.full((H, W, 3), bg, np.uint8)
    x, y, bw, bh = BOX
    x0, y0, x1, y1 = int(x * W), int(y * H), int((x + bw) * W), int((y + bh) * H)
    patch = face_rgb(y1 - y0, x1 - x0)
    mask = np.zeros((H, W), np.uint8)
    cv2.ellipse(
        mask, ((x0 + x1) // 2, (y0 + y1) // 2), ((x1 - x0) // 2, (y1 - y0) // 2), 0, 0, 360, 255, -1
    )
    canvas = img.copy()
    canvas[y0:y1, x0:x1] = patch
    return np.where(mask[..., None] > 0, canvas, img)


def _real_skin(h, w, seed=0):
    """Skin-coloured, shaded like a rounded face, with pore / sensor texture and dark features."""
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    r2 = ((xx - w / 2) / (w / 2)) ** 2 + ((yy - h / 2) / (h / 2)) ** 2
    shade = 1.0 - 0.35 * np.clip(r2, 0, 1)
    base = np.array([226, 172, 140], np.float32)
    img = base[None, None, :] * shade[..., None]
    img += rng.normal(0, 5.0, img.shape)  # texture
    for ex in (0.3, 0.7):  # eyes
        cv2.circle(img, (int(w * ex), int(h * 0.38)), max(2, w // 18), (60, 40, 35), -1)
    cv2.ellipse(
        img, (w // 2, int(h * 0.72)), (w // 6, max(2, h // 25)), 0, 0, 180, (120, 50, 50), -1
    )
    return np.clip(img, 0, 255).astype(np.uint8)


def _flat_print(h, w):
    """A printed / rendered face: pale, nearly grey, perfectly smooth, one soft gradient."""
    gx = np.linspace(0, 6, w, dtype=np.float32)[None, :, None]
    img = np.empty((h, w, 3), np.float32)
    img[...] = np.array([200, 196, 192], np.float32) + gx
    return np.clip(img, 0, 255).astype(np.uint8)


def _stone(h, w, seed=1):
    """A grey statue: shaded like a face, no colour, only faint grain."""
    rng = np.random.default_rng(seed)
    yy, xx = np.mgrid[0:h, 0:w].astype(np.float32)
    r2 = ((xx - w / 2) / (w / 2)) ** 2 + ((yy - h / 2) / (h / 2)) ** 2
    g = 170 * (1.0 - 0.35 * np.clip(r2, 0, 1)) + rng.normal(0, 0.8, (h, w))
    img = np.repeat(g[..., None], 3, axis=2)
    img[..., 2] += 3  # a hint of cool tone
    return np.clip(img, 0, 255).astype(np.uint8)


def test_flat_printed_face_is_artificial():
    img = _scene(_flat_print)
    conf = face_artificial(img, BOX, image_saturation(img))
    assert conf is not None and conf >= ARTIFICIAL_AT, conf


def test_textured_skin_is_a_real_face():
    img = _scene(_real_skin)
    conf = face_artificial(img, BOX, image_saturation(img))
    assert conf is not None and conf < 0.3, conf


def test_grey_statue_is_artificial_even_with_grain():
    img = _scene(_stone)
    conf = face_artificial(img, BOX, image_saturation(img))
    assert conf is not None and conf >= ARTIFICIAL_AT, conf


def test_scores_order_print_above_skin():
    flat = face_features(cv2.resize(_flat_print(100, 100), (64, 64)))
    skin = face_features(cv2.resize(_real_skin(100, 100), (64, 64)))
    assert flat["tex"] < skin["tex"]
    assert flat["plane"] > skin["plane"]
    assert flat["sat"] < skin["sat"]
    assert flat["skin"] < skin["skin"]
    assert artificial_confidence(flat) > artificial_confidence(skin) + 0.4


def test_smooth_but_colourful_face_is_not_flagged_by_flatness_alone():
    # retouched / low-ISO real skin: perfectly smooth but skin-coloured and saturated
    def smooth_skin(h, w):
        img = np.empty((h, w, 3), np.float32)
        img[...] = (226, 172, 140)
        return img.astype(np.uint8)

    img = _scene(smooth_skin)
    conf = face_artificial(img, BOX, image_saturation(img))
    assert conf is not None and conf < ARTIFICIAL_AT, conf


def test_monochrome_photo_never_flags():
    img = _scene(_flat_print)
    grey = cv2.cvtColor(cv2.cvtColor(img, cv2.COLOR_RGB2GRAY), cv2.COLOR_GRAY2RGB)
    sat = image_saturation(grey)
    assert sat < MONO_SAT
    conf = face_artificial(grey, BOX, sat)
    assert conf is not None and conf < ARTIFICIAL_AT


def test_tiny_face_has_no_verdict():
    img = _scene(_flat_print)
    assert face_artificial(img, (0.5, 0.5, 0.02, 0.03), 0.3) is None


def test_plane_fit_flat_and_curved():
    flat = np.full((64, 64), 120, np.uint8)
    ramp = np.tile(np.linspace(0, 255, 64, dtype=np.float32), (64, 1))
    bowl = ((np.mgrid[0:64, 0:64][0] - 32) ** 2 + (np.mgrid[0:64, 0:64][1] - 32) ** 2).astype(
        np.float32
    )
    assert plane_r2(flat) == 1.0
    assert plane_r2(ramp) > 0.999
    assert plane_r2(bowl) < 0.1


def test_expression_flag_mapping():
    # talking: mouth open without a smile
    assert expression_flag(0.9, 0.05, 0.40) == "talking"
    assert expression_flag(0.9, 0.05, 0.25) == "talking"
    assert expression_flag(0.9, 0.05, 0.24) is None
    # a laugh / smile with an open mouth is fine
    assert expression_flag(0.9, 0.80, 0.50) is None
    assert expression_flag(0.9, 0.35, 0.50) is None
    # half-blink; fully closed eyes belong to the closed_eyes tag
    assert expression_flag(0.55, 0.1, 0.0) == "half_blink"
    assert expression_flag(0.45, 0.1, 0.0) == "half_blink"
    assert expression_flag(0.44, 0.1, 0.0) is None
    assert expression_flag(0.65, 0.1, 0.0) is None
    # no landmarks: no verdict
    assert expression_flag(None, None, None) is None
    assert faces_mod.MOUTH_OPEN_TALKING == 0.25


def test_saliency_box_finds_the_bright_object():
    img = np.full((480, 640, 3), 100, np.uint8)
    cv2.rectangle(img, (400, 120), (520, 300), (240, 60, 40), -1)
    x, y, w, h = saliency_box(img)  # type: ignore[misc]
    cx, cy = x + w / 2, y + h / 2
    assert abs(cx - 460 / 640) < 0.08 and abs(cy - 210 / 480) < 0.1
    assert 0.1 < w < 0.4 and 0.2 < h < 0.6


def test_saliency_box_flat_image_is_none():
    assert saliency_box(np.full((240, 320, 3), 128, np.uint8)) is None


def test_analyzer_reports_artificial_and_expression_fields(monkeypatch):
    """`FaceAnalyzer.analyze` adds the P1-8 fields to every detected face (no models needed)."""
    an = faces_mod.FaceAnalyzer("unused.onnx")
    monkeypatch.setattr(an, "detect", lambda rgb: [{"bbox": list(BOX), "det_score": 0.9}])
    (flat,) = an.analyze(_scene(_flat_print))
    assert flat["likely_artificial"] is True and flat["artificial"] >= ARTIFICIAL_AT
    assert flat["mouth_open"] is None and flat["bad_expression"] is None  # no landmarker
    (real,) = an.analyze(_scene(_real_skin))
    assert real["likely_artificial"] is False and real["artificial"] < ARTIFICIAL_AT
