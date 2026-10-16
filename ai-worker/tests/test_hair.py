"""Hair transfer (doc 03 section 5.6): model-free tests on synthetic heads with known hair masks.

The fixture draws the same person twice on a green backdrop: a target with flat, stringy, shiny
("sweaty") hair and a donor with fuller, clean hair, shifted and slightly larger (a small pose
offset). Hair is bluish-black and the backdrop has almost no blue, so a colour rule stands in for the
selfie-multiclass segmenter and the "true" hair masks are known exactly.
"""

from __future__ import annotations

import cv2
import numpy as np
import pytest

from imagepicker_ai.beauty import landmarks as lmk
from imagepicker_ai.besttake import hair as hm
from imagepicker_ai.besttake.compose import Components
from imagepicker_ai.masks.nets import CLS_FACE_SKIN, CLS_HAIR

from .synth_face import BROW_Y, OVAL_HALF, draw_face, face_box, landmarks_for, observer

W, H = 1000, 900
T_C, T_FW = (500.0, 470.0), 220.0  # target face centre / width
D_C, D_FW = (530.0, 455.0), 240.0  # donor: shifted and 9 % larger


def backdrop(seed: int) -> np.ndarray:
    rng = np.random.default_rng(seed)
    low = rng.random((4, 5, 3)).astype(np.float32)
    img = cv2.resize(low, (W, H), interpolation=cv2.INTER_CUBIC)
    base = np.array([80.0, 140.0, 50.0], np.float32)
    span = np.array([40.0, 40.0, 20.0], np.float32)
    out = base + (img - 0.5) * span
    out += cv2.GaussianBlur(rng.normal(0, 3, (H, W)).astype(np.float32), (0, 0), 0.8)[..., None]
    return np.clip(out, 0, 255).astype(np.uint8)


def hair_shape(cx: float, cy: float, fw: float, messy: bool) -> np.ndarray:
    """Hair mask (uint8 0/1) of a head with face centre (cx, cy) and face width fw."""
    m = np.zeros((H, W), np.uint8)
    if messy:  # flat on top, a few long strands and a flyaway clump on the right
        cv2.ellipse(
            m, (int(cx), int(cy - 0.12 * fw)), (int(0.62 * fw), int(0.76 * fw)), 0, 180, 360, 1, -1
        )
        cv2.rectangle(
            m,
            (int(cx - 0.62 * fw), int(cy - 0.12 * fw)),
            (int(cx + 0.62 * fw), int(cy + 0.15 * fw)),
            1,
            -1,
        )
        for k, dx in enumerate((-0.6, -0.55, 0.52, 0.58)):
            x = int(cx + dx * fw)
            cv2.line(m, (x, int(cy + 0.1 * fw)), (x + (-1) ** k * 10, int(cy + 0.62 * fw)), 1, 3)
        cv2.ellipse(
            m,
            (int(cx + 0.78 * fw), int(cy - 0.5 * fw)),
            (int(0.14 * fw), int(0.06 * fw)),
            30,
            0,
            360,
            1,
            -1,
        )
    else:  # full volume, down to the jaw
        cv2.ellipse(
            m, (int(cx), int(cy - 0.15 * fw)), (int(0.72 * fw), int(0.9 * fw)), 0, 180, 360, 1, -1
        )
        cv2.rectangle(
            m,
            (int(cx - 0.72 * fw), int(cy - 0.15 * fw)),
            (int(cx + 0.72 * fw), int(cy + 0.35 * fw)),
            1,
            -1,
        )
    return m


def paint_hair(img: np.ndarray, mask: np.ndarray, messy: bool, seed: int) -> None:
    rng = np.random.default_rng(seed)
    base = np.array([42.0, 48.0, 100.0], np.float32)
    if messy:
        # stringy: strong vertical stripes; sweaty: bright, nearly colourless specular streaks
        noise = rng.normal(0, 1, (H, W)).astype(np.float32)
        stripes = cv2.GaussianBlur(noise, (0, 0), sigmaX=0.6, sigmaY=9.0) * 40.0
        tex = np.repeat(stripes[..., None], 3, axis=2)
        spec = cv2.GaussianBlur((rng.random((H, W)) > 0.985).astype(np.float32), (0, 0), 2.5)
        spec = np.clip(spec * 18.0, 0, 1)[..., None]
        col = (base + tex) * (1 - spec) + np.array([200.0, 204.0, 228.0]) * spec
    else:
        tex = cv2.GaussianBlur(rng.normal(0, 5, (H, W)).astype(np.float32), (0, 0), 0.7)[..., None]
        col = base + tex
    sel = mask > 0
    img[sel] = np.clip(col, 0, 255).astype(np.uint8)[sel]


def oval_px(cx: float, cy: float, fw: float) -> np.ndarray:
    return lmk.oval_mask(landmarks_for(face_box(cx, cy, fw)), (H, W))


def make_head(seed: int, c: tuple[float, float], fw: float, messy: bool):
    img = backdrop(seed)
    draw_face(img, c[0], c[1], fw, True, seed=1)
    hair = hair_shape(c[0], c[1], fw, messy) & (oval_px(c[0], c[1], fw) == 0)
    paint_hair(img, hair, messy, seed + 100)
    return img, hair


def colour_parts(crop: np.ndarray) -> np.ndarray:
    """Stand-in for selfie-multiclass: hair = bluish, face skin = skin-coloured, rest = background."""
    f = crop.astype(np.float32)
    r, g, b = f[..., 0], f[..., 1], f[..., 2]
    hair = np.clip((b - r - 15.0) / 20.0, 0, 1)
    skin = ((r > 120) & (r - b > 40) & (r > g)).astype(np.float32)
    skin = cv2.GaussianBlur(skin, (0, 0), 1.0)
    out = np.zeros(crop.shape[:2] + (6,), np.float32)
    out[..., CLS_HAIR] = hair
    out[..., CLS_FACE_SKIN] = skin * (1 - hair)
    out[..., 0] = np.clip(1 - hair - out[..., CLS_FACE_SKIN], 0, 1)
    return out


def comps(poses=None) -> Components:
    return Components(observe=observer(poses=poses), parts=colour_parts)


@pytest.fixture(scope="module")
def pair():
    target, t_hair = make_head(3, T_C, T_FW, messy=True)
    donor, d_hair = make_head(4, D_C, D_FW, messy=False)
    return target, donor, t_hair, d_hair


def paste(base: np.ndarray, out: hm.HairOut) -> np.ndarray:
    x, y, w, h = out.rect
    comp = base.copy()
    a = out.patch[..., 3:4].astype(np.float32) / 255
    comp[y : y + h, x : x + w] = np.rint(
        comp[y : y + h, x : x + w] * (1 - a) + out.patch[..., :3] * a
    ).astype(np.uint8)
    return comp


def donor_hair_in_target(d_hair: np.ndarray) -> np.ndarray:
    """The donor's hair mask mapped into the target frame (the fixture's similarity is exact)."""
    s = T_FW / D_FW
    M = np.array([[s, 0, T_C[0] - s * D_C[0]], [0, s, T_C[1] - s * D_C[1]]], np.float32)
    return cv2.warpAffine(d_hair, M, (W, H), flags=cv2.INTER_NEAREST)


@pytest.fixture(scope="module")
def composed(pair):
    target, donor, _, _ = pair
    out = hm.hair_compose_arrays(target, donor, face_box(*T_C, T_FW), face_box(*D_C, D_FW), comps())
    assert out.patch is not None, out.reason
    return out, paste(target, out)


def test_hair_transfer_keeps_the_target_face(pair, composed):
    target, _, _, _ = pair
    _, comp = composed
    # the target's face below the brow line is untouched, bit for bit
    oval = cv2.erode(oval_px(*T_C, T_FW), np.ones((5, 5), np.uint8))
    rows = np.arange(H)[:, None] > T_C[1] + (BROW_Y + 0.05) * T_FW
    face = (oval > 0) & rows
    assert face.sum() > 20_000
    assert np.abs(comp.astype(int) - target)[face].max() == 0
    # far from the head nothing changes either
    assert np.abs(comp.astype(int) - target)[H - 60 :].max() == 0
    assert np.abs(comp.astype(int) - target)[:, :40].max() == 0


def test_hair_region_takes_the_donor_hair(pair, composed):
    target, donor, t_hair, d_hair = pair
    out, comp = composed
    hd = donor_hair_in_target(d_hair)
    inner = cv2.erode(hd, np.ones((9, 9), np.uint8)) > 0
    inner &= oval_px(*T_C, T_FW) == 0
    assert inner.sum() > 10_000
    d_inner = cv2.erode(d_hair, np.ones((9, 9), np.uint8)) > 0

    def stats(img, sel):
        g = img.astype(np.float32)[sel]
        return g.mean(0), g.std(0)

    m_c, s_c = stats(comp, inner)
    m_d, s_d = stats(donor, d_inner)
    m_t, s_t = stats(target, inner & (t_hair > 0))
    assert np.abs(m_c - m_d).max() < 6.0, (m_c, m_d)
    assert np.abs(s_c - s_d).max() < 4.0, (s_c, s_d)  # the clean texture, not the stringy one
    assert s_t.max() > 2 * s_d.max()  # (the fixture really differs)
    # shine is gone from the hair
    bright = lambda img: img.astype(int).sum(2) > 450  # noqa: E731
    assert bright(target)[inner & (t_hair > 0)].mean() > 0.01
    assert bright(comp)[inner].mean() < 0.002
    assert out.align["scale"] == pytest.approx(T_FW / D_FW, rel=0.02)
    assert out.align["landmark_rms"] < 0.01


def test_flyaway_target_hair_outside_the_donor_hair_is_filled(pair, composed):
    target, _, t_hair, d_hair = pair
    _, comp = composed
    hd = donor_hair_in_target(d_hair)
    outside = (t_hair > 0) & (cv2.dilate(hd, np.ones((15, 15), np.uint8)) == 0)
    assert outside.sum() > 300  # strands and the flyaway clump
    hair_after = colour_parts(comp)[..., CLS_HAIR] > 0.5
    assert hair_after[outside].mean() < 0.1  # replaced by backdrop
    # and the fill looks like the backdrop around it (green, not hair-blue)
    fill = comp.astype(np.float32)[outside]
    assert fill[:, 1].mean() > fill[:, 2].mean() + 40


def test_hair_transfer_seam_is_soft(pair, composed):
    target, _, _, _ = pair
    out, comp = composed
    a = out.patch[..., 3].astype(np.float32) / 255
    # alpha changes gradually (no 0 -> 1 step between neighbours)
    step = max(np.abs(np.diff(a, axis=0)).max(), np.abs(np.diff(a, axis=1)).max())
    assert step < 0.5
    assert out.debug["seam_grad"] < hm.WARN_SEAM_GRAD
    assert "seam" not in out.quality["warnings"]
    assert out.quality["score"] > 0.6


def test_big_pose_difference_is_refused(pair):
    target, donor, _, _ = pair
    for pose in ((30.0, 0.0, 0.0), (0.0, 22.0, 0.0), (0.0, 0.0, 28.0)):
        out = hm.hair_compose_arrays(
            target,
            donor,
            face_box(*T_C, T_FW),
            face_box(*D_C, D_FW),
            comps(poses=[(0.0, 0.0, 0.0), pose]),
        )
        assert out.patch is None and out.reason == "pose_too_different", pose
    # just inside the gates: composed, with a warning
    out = hm.hair_compose_arrays(
        target,
        donor,
        face_box(*T_C, T_FW),
        face_box(*D_C, D_FW),
        comps(poses=[(0.0, 0.0, 0.0), (15.0, 0.0, 0.0)]),
    )
    assert out.patch is not None and "pose_differs" in out.quality["warnings"]


def test_small_donor_and_missing_models_are_refused(pair):
    target, _, _, _ = pair
    small, _ = make_head(5, (500, 450), 90.0, messy=False)
    out = hm.hair_compose_arrays(
        target, small, face_box(*T_C, T_FW), face_box(500, 450, 90.0), comps()
    )
    assert out.patch is None and out.reason == "donor_too_small"
    out = hm.hair_compose_arrays(
        target, target, face_box(*T_C, T_FW), face_box(*T_C, T_FW), Components(observe=observer())
    )
    assert out.patch is None and out.reason == "hair_not_found"
    bald = backdrop(9)
    draw_face(bald, *D_C, D_FW, True, seed=1)
    bald[..., 2] = np.minimum(bald[..., 2], bald[..., 0])  # no blue anywhere: no hair
    out = hm.hair_compose_arrays(target, bald, face_box(*T_C, T_FW), face_box(*D_C, D_FW), comps())
    assert out.patch is None and out.reason == "donor_no_hair"


def test_hair_metrics_rank_the_clean_hair_higher(pair):
    target, donor, _, _ = pair
    mt = hm.hair_metrics(target, face_box(*T_C, T_FW), comps())
    md = hm.hair_metrics(donor, face_box(*D_C, D_FW), comps())
    assert mt["ok"] and md["ok"]
    assert md["hair_ratio"] > mt["hair_ratio"] * 1.1  # fuller
    assert mt["shine"] > md["shine"] + 0.005  # shinier
    assert mt["roughness"] > md["roughness"]  # stringier silhouette
    assert md["face_px"] == pytest.approx(D_FW, rel=0.05)
    # the cues are scale-normalised: the same head at half size measures about the same
    half = cv2.resize(donor, (W // 2, H // 2), interpolation=cv2.INTER_AREA)
    mh = hm.hair_metrics(half, tuple(v / 2 for v in face_box(*D_C, D_FW)), comps())
    assert mh["hair_ratio"] == pytest.approx(md["hair_ratio"], rel=0.08)


def test_pose_delta_wraps():
    d = hm.pose_delta((10.0, 85.0, 175.0), (-5.0, -85.0, -175.0))
    assert d["yaw"] == pytest.approx(15.0)
    assert d["pitch"] == pytest.approx(10.0)
    assert d["roll"] == pytest.approx(10.0)
    assert hm.pose_gate(d)
    assert not hm.pose_gate({"yaw": 0.0, "pitch": 0.0, "roll": 25.0})


def test_hair_face_too_small():
    img = backdrop(1)
    out = hm.hair_compose_arrays(img, img, (10, 10, 30, 40), (10, 10, 30, 40), comps())
    assert out.reason == "face_too_small"
    assert OVAL_HALF[0] == 0.5  # (fixture geometry the assertions above rely on)


# ------------------------------------------------------------------------------ rpc contract
def test_hair_rpc_registered_validated_and_needs_models(tmp_path):
    import asyncio

    from imagepicker_ai.errors import InvalidParams, ModelUnavailable
    from imagepicker_ai.service import WorkerService

    s = WorkerService(models_dir=str(tmp_path / "models"), idle_unload_s=0)
    try:
        assert {"hair.rank", "hair.compose"} <= set(s._handlers())
        listing = asyncio.run(s.models_list(None, None))
        assert listing["hair_models"] == s.hair.required_models()
        p = tmp_path / "a.png"
        cv2.imwrite(str(p), backdrop(1)[..., ::-1])
        ph = {"photo_id": 1, "path": str(p)}
        req = {
            "target": ph,
            "donor": ph,
            "target_face": [0.4, 0.4, 0.2, 0.3],
            "donor_face": [0.4, 0.4, 0.2, 0.3],
            "out_dir": str(tmp_path / "out"),
        }
        with pytest.raises(ModelUnavailable) as e:
            asyncio.run(s.hair.compose(req))
        assert set(s.hair.required_models()) <= set(e.value.extra["models"])
        with pytest.raises(InvalidParams):
            asyncio.run(s.hair.compose({**req, "donor_face": [0, 0, 0, 1]}))
        with pytest.raises(InvalidParams):
            asyncio.run(s.hair.rank({"faces": "x"}))
        with pytest.raises(InvalidParams):
            asyncio.run(s.hair.rank({"faces": [{"photo": ph, "face": [0, 0, 1, 1]}] * 25}))
        with pytest.raises(ModelUnavailable):
            asyncio.run(s.hair.rank({"faces": [{"photo": ph, "face": [0.4, 0.4, 0.2, 0.3]}]}))
    finally:
        s.besttake.shutdown()
        s.enhance.shutdown()
        s.gen.shutdown()
        s.beauty.shutdown()
        s.masks.shutdown()
        s.analyzer.shutdown()
