"""Sweat cleanup (doc 03 section 7.5): `deshine` and `flyaway` on the synthetic heads of test_hair."""

from __future__ import annotations

import cv2
import numpy as np
import pytest

from imagepicker_ai.beauty import landmarks as lmk
from imagepicker_ai.beauty import sweat
from imagepicker_ai.besttake.compose import Components
from imagepicker_ai.masks.nets import CLS_FACE_SKIN, CLS_HAIR

from .synth_face import face_box, landmarks_for, observer
from .test_hair import T_C, T_FW, H, W, colour_parts, make_head, oval_px, paste


def parts(crop: np.ndarray) -> np.ndarray:
    """colour_parts, but bright shine on the skin still counts as skin (as a real segmenter sees it)."""
    out = colour_parts(crop)
    f = crop.astype(np.float32)
    r, b = f[..., 0], f[..., 2]
    skin = ((r > 120) & (r >= b - 5)).astype(np.float32) * (1 - out[..., CLS_HAIR])
    out[..., CLS_FACE_SKIN] = cv2.GaussianBlur(skin, (0, 0), 1.0)
    out[..., 0] = np.clip(1 - out[..., CLS_HAIR] - out[..., CLS_FACE_SKIN], 0, 1)
    return out


def comps() -> Components:
    return Components(observe=observer(), parts=parts)


def sweaty_face() -> np.ndarray:
    img, _ = make_head(3, T_C, T_FW, messy=True)
    # sweat shine: bright, nearly white blobs on the forehead, nose and one cheek
    shine = np.zeros((H, W), np.float32)
    cx, cy = T_C
    for dx, dy, r in (
        (-0.12, -0.33, 0.07),
        (0.1, -0.3, 0.06),
        (0.0, 0.02, 0.05),
        (0.22, 0.12, 0.05),
    ):
        cv2.circle(shine, (int(cx + dx * T_FW), int(cy + dy * T_FW)), int(r * T_FW), 1.0, -1)
    shine = cv2.GaussianBlur(shine, (0, 0), 0.03 * T_FW) * (oval_px(cx, cy, T_FW) > 0)
    shine = np.clip(shine * 1.6, 0, 1)[..., None]
    out = (
        img.astype(np.float32) * (1 - 0.85 * shine) + np.array([252.0, 244.0, 238.0]) * 0.85 * shine
    )
    return np.clip(out, 0, 255).astype(np.uint8)


@pytest.fixture(scope="module")
def deshined():
    img = sweaty_face()
    out = sweat.deshine_arrays(img, face_box(*T_C, T_FW), comps())
    assert out.patch is not None, out.reason
    return img, out, paste(img, out)


def test_deshine_reduces_the_highlight_area(deshined):
    _, out, _ = deshined
    skin = out.stats["skin"]
    assert skin["highlight_before"] > 0.03
    assert skin["highlight_after"] < 0.4 * skin["highlight_before"]
    hair = out.stats["hair"]
    assert hair["highlight_after"] <= hair["highlight_before"]


def test_deshine_keeps_the_face_detail(deshined):
    img, _, comp = deshined
    pts = landmarks_for(face_box(*T_C, T_FW))
    feat = lmk.feature_mask(pts, (H, W), T_FW) > 0  # eyes, brows, mouth: untouched
    assert np.abs(comp.astype(int) - img)[feat].max() == 0
    # skin away from the shine keeps its colour; fine texture survives
    cheek = (
        slice(int(T_C[1] + 0.05 * T_FW), int(T_C[1] + 0.15 * T_FW)),
        slice(int(T_C[0] - 0.3 * T_FW), int(T_C[0] - 0.18 * T_FW)),
    )
    assert (
        np.abs(comp[cheek].astype(float).mean((0, 1)) - img[cheek].astype(float).mean((0, 1))).max()
        < 2.0
    )
    g0 = cv2.Laplacian(cv2.cvtColor(img[cheek], cv2.COLOR_RGB2GRAY), cv2.CV_32F).std()
    g1 = cv2.Laplacian(cv2.cvtColor(comp[cheek], cv2.COLOR_RGB2GRAY), cv2.CV_32F).std()
    assert g1 > 0.85 * g0
    # the brightest shine is now much darker
    bright = (img.astype(int).sum(2) > 690) & ~feat
    assert bright.sum() > 200
    assert (comp.astype(int).sum(2) > 690)[bright].mean() < 0.2
    # nothing changes far from the head
    assert np.abs(comp.astype(int) - img)[:, :40].max() == 0


def test_flyaway_tidies_strands_and_keeps_the_face():
    img, t_hair = make_head(3, T_C, T_FW, messy=True)
    out = sweat.flyaway_arrays(img, face_box(*T_C, T_FW), comps())
    assert out.patch is not None, out.reason
    assert out.stats["stray_px_before"] > 300
    assert out.stats["stray_px_after"] < 0.2 * out.stats["stray_px_before"]
    comp = paste(img, out)
    assert np.abs(comp.astype(int) - img)[oval_px(*T_C, T_FW) > 0].max() == 0
    # the hair body itself stays (only thin strands / loose pieces go)
    opened = cv2.morphologyEx(t_hair, cv2.MORPH_OPEN, np.ones((15, 15), np.uint8))
    _, lab, st, _ = cv2.connectedComponentsWithStats(opened)
    main = lab == 1 + int(np.argmax(st[1:, cv2.CC_STAT_AREA]))  # the clump off to the side goes too
    body = cv2.erode(main.astype(np.uint8), np.ones((9, 9), np.uint8)) > 0
    assert body.sum() > 5000
    assert (np.abs(comp.astype(int) - img).max(2)[body] > 0).mean() < 0.02


def test_clean_hair_has_nothing_to_tidy():
    img, _ = make_head(4, T_C, T_FW, messy=False)
    out = sweat.flyaway_arrays(img, face_box(*T_C, T_FW), comps())
    assert out.patch is None and out.reason == "no_flyaways"


def test_sweat_needs_segmentation_and_a_face():
    img, _ = make_head(4, T_C, T_FW, messy=False)
    no_parts = Components(observe=observer())
    assert sweat.deshine_arrays(img, face_box(*T_C, T_FW), no_parts).reason == "model_unavailable"
    blind = Components(observe=lambda rgb, box: None, parts=parts)
    assert sweat.flyaway_arrays(img, face_box(*T_C, T_FW), blind).reason == "face_not_found"


def test_enhance_rpc_accepts_the_sweat_ops(tmp_path):
    import asyncio

    from imagepicker_ai.errors import ModelUnavailable
    from imagepicker_ai.service import WorkerService

    from .test_hair import backdrop

    s = WorkerService(models_dir=str(tmp_path / "models"), idle_unload_s=0)
    try:
        listing = asyncio.run(s.models_list(None, None))
        assert set(listing["enhance_models"]["deshine"]) == set(
            s.enhance.required_models("deshine")
        )
        p = tmp_path / "a.png"
        cv2.imwrite(str(p), backdrop(1)[..., ::-1])
        for op in ("deshine", "flyaway"):
            with pytest.raises(ModelUnavailable) as e:
                asyncio.run(
                    s.enhance.run(
                        {
                            "photo": {"photo_id": 1, "path": str(p)},
                            "op": op,
                            "out_dir": str(tmp_path),
                        }
                    )
                )
            assert "mediapipe-selfie-multiclass" in e.value.extra["models"]
    finally:
        s.besttake.shutdown()
        s.enhance.shutdown()
        s.gen.shutdown()
        s.beauty.shutdown()
        s.masks.shutdown()
        s.analyzer.shutdown()
