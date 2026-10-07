"""M5: Best Take, inpaint, enhance. Fast tests are model-free (injected landmarks / networks)."""

from __future__ import annotations

import asyncio

import cv2
import numpy as np
import pytest

from imagepicker_ai.besttake.compose import Components, compose_arrays
from imagepicker_ai.enhance.ops import (
    FaceObs,
    denoise_tiled,
    estimate_noise,
    restore_face,
    upscale_tiled,
)
from imagepicker_ai.errors import InvalidParams, ModelUnavailable
from imagepicker_ai.inpaint.core import FillEngine, inpaint_arrays
from imagepicker_ai.service import WorkerService
from imagepicker_ai.tiling import run_tiled

from .conftest import synth_image
from .synth_face import draw_face, face_box, landmarks_for, observer

W, H = 1200, 900
CX, CY, FW = 600, 430, 240


def make_pair(shift=(7.4, -5.3), rot=0.3):
    bg = synth_image(3, (W, H))
    truth = bg.copy()
    draw_face(truth, CX, CY, FW, True, seed=1)
    base = bg.copy()
    draw_face(base, CX, CY, FW, False, seed=1)
    M = cv2.getRotationMatrix2D((W / 2, H / 2), rot, 1.0)
    M[:, 2] += shift
    src = cv2.warpAffine(truth, M, (W, H), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101)
    c2 = M @ np.array([CX, CY, 1.0])
    return base, src, truth, face_box(CX, CY, FW), face_box(c2[0], c2[1], FW)


def paste(base, out):
    x, y, w, h = out.rect
    comp = base.copy()
    a = out.patch[..., 3:4].astype(np.float32) / 255
    comp[y : y + h, x : x + w] = (
        comp[y : y + h, x : x + w] * (1 - a) + out.patch[..., :3] * a
    ).astype(np.uint8)
    return comp


# ------------------------------------------------------------------------------ best take
def test_besttake_synthetic_pair_places_patch_and_matches_source():
    base, src, truth, bf, sf = make_pair()
    out = compose_arrays(base, src, bf, sf, Components(observe=observer()))
    assert out.patch is not None, out.reason
    x, y, w, h = out.rect
    assert out.patch.shape == (h, w, 4)
    # the patch covers the whole face, leaves the image corners alone
    assert x <= bf[0] and y <= bf[1] and x + w >= bf[0] + bf[2] and y + h >= bf[1] + bf[3]
    assert x > 0 and y > 0 and x + w < W and y + h < H
    a = out.patch[..., 3]
    assert a[h // 2, w // 2] == 255 and a[0, 0] == 0 and a[-1, -1] == 0
    comp = paste(base, out)
    eyes = (slice(CY - 40, CY + 20), slice(CX - 120, CX + 120))
    err_base = np.abs(base.astype(int) - truth)[eyes].mean()
    err_comp = np.abs(comp.astype(int) - truth)[eyes].mean()
    assert err_comp < 2.0 < err_base  # composite == source face after alignment
    face = (slice(CY - 150, CY + 150), slice(CX - 150, CX + 150))
    assert np.abs(comp.astype(int) - truth)[face].mean() < 2.0
    outside = np.abs(comp.astype(int) - base)[: CY - 300].max()
    assert outside == 0  # nothing changes far from the face
    q = out.quality
    assert q["aligned"] and q["score"] > 0.8 and q["warnings"] == []


def test_besttake_global_alignment_recovers_shift():
    base, src, _, bf, sf = make_pair(shift=(14.0, 9.0), rot=0.8)
    out = compose_arrays(base, src, bf, sf, Components(observe=observer()))
    g = out.debug["global"]
    assert g.ok and g.inliers > 30 and g.residual_px < 2.0


def test_besttake_large_pose_change_not_composable():
    base, src, _, bf, sf = make_pair()
    obs = observer(poses=[(0.0, 0.0, 0.0), (32.0, 5.0, 0.0)])
    out = compose_arrays(base, src, bf, sf, Components(observe=obs))
    assert out.patch is None and out.reason == "large_pose_change"


def test_besttake_moderate_pose_change_warns():
    base, src, _, bf, sf = make_pair()
    obs = observer(poses=[(0.0, 0.0, 0.0), (18.0, 0.0, 0.0), (18.0, 0.0, 0.0)])
    out = compose_arrays(base, src, bf, sf, Components(observe=obs))
    assert out.patch is not None
    assert "large_pose_change" in out.quality["warnings"]


def test_besttake_camera_moved_not_composable():
    base, _, _, bf, _ = make_pair()
    other = synth_image(99, (W, H))  # a different scene: no common background
    draw_face(other, CX + 30, CY, FW, True, seed=1)
    out = compose_arrays(base, other, bf, face_box(CX + 30, CY, FW), Components(observe=observer()))
    assert out.patch is None and out.reason == "camera_moved"


def test_besttake_big_translation_is_camera_moved():
    base, src, _, bf, sf = make_pair(shift=(300.0, 40.0))
    out = compose_arrays(base, src, bf, sf, Components(observe=observer()))
    assert out.patch is None and out.reason == "camera_moved"


def test_besttake_no_landmarks():
    base, src, _, bf, sf = make_pair()
    out = compose_arrays(base, src, bf, sf, Components(observe=lambda rgb, box: None))
    assert out.patch is None and out.reason == "face_not_found"


def test_besttake_other_face_is_cut_out():
    base, src, truth, bf, sf = make_pair()
    # a second person right next to the target, drawn in both frames
    for img in (base, src):
        draw_face(img, CX + 330, CY, 200, True, seed=5)
    other = face_box(CX + 330, CY, 200)
    comp_ = Components(observe=observer(), detect_faces=lambda rgb: [bf, other])
    out = compose_arrays(base, src, bf, sf, comp_)
    assert out.patch is not None
    comp = paste(base, out)
    ox, oy, ow, oh = (int(v) for v in other)
    assert np.abs(comp.astype(int) - base)[oy : oy + oh, ox : ox + ow].max() == 0


# ------------------------------------------------------------------------------ tiling / ops
@pytest.mark.parametrize("shape", [(100, 90), (517, 389), (64, 64), (1, 40)])
def test_tiling_identity_both_modes(shape):
    img = synth_image(1, (max(shape[1], 8), max(shape[0], 8)))[: shape[0], : shape[1]]
    for blend in (True, False):
        out = run_tiled(img, lambda x: x, tile=128, overlap=16, scale=1, multiple=8, blend=blend)
        assert out.shape == img.shape
        assert np.abs(out.astype(int) - img).max() <= 1


def test_tiling_upscale_shape_and_content():
    img = synth_image(2, (203, 151))
    nn = lambda x: np.repeat(np.repeat(x, 2, axis=2), 2, axis=3)  # noqa: E731
    out = upscale_tiled(img, nn, 2, tile=64)
    assert out.shape == (img.shape[0] * 2, img.shape[1] * 2, 3)
    ref = cv2.resize(img, None, fx=2, fy=2, interpolation=cv2.INTER_NEAREST)
    assert np.abs(out.astype(int) - ref).max() <= 1


def test_denoise_tiled_strength_blends():
    img = synth_image(3, (150, 120))
    flat = lambda x: np.zeros_like(x) + x.mean(axis=(2, 3), keepdims=True)  # noqa: E731
    half = denoise_tiled(img, flat, 64, 0.5)
    full = denoise_tiled(img, flat, 64, 1.0)
    mid = (img.astype(float) + full.astype(float)) / 2
    assert np.abs(half - mid).max() <= 1.5
    assert estimate_noise(full) < estimate_noise(img)


def test_restore_face_patch_rect_and_alpha():
    img = synth_image(4, (900, 700))
    box = face_box(450, 350, 260)
    obs = FaceObs(landmarks_for(box), None)
    res = restore_face(img, obs, lambda x: x, 1.0)  # identity "network"
    assert res is not None
    x, y, w, h = res.rect
    assert res.rgba.shape == (h, w, 4)
    assert 0 <= x < 450 < x + w <= 900 and 0 <= y < 350 < y + h <= 700
    assert res.rgba[h // 2, w // 2, 3] == 255 and res.rgba[0, 0, 3] == 0
    tiny = FaceObs(landmarks_for(face_box(100, 100, 30)), None)
    assert restore_face(img, tiny, lambda x: x, 1.0) is None


def test_inpaint_core_fills_with_engine_and_keeps_outside():
    img = synth_image(6, (800, 600))
    obj = img.copy()
    cv2.rectangle(obj, (300, 200), (380, 300), (255, 0, 0), -1)
    mask = np.zeros(img.shape[:2], np.float32)
    mask[200:301, 300:381] = 1

    def engine_fn(crop, m):  # "network": return the clean background for the hole
        return crop

    # an engine that returns the *clean* image requires the crop of `img`; emulate by mean fill
    def mean_fill(crop, m):
        out = crop.copy()
        out[m > 0.5] = crop[m < 0.5].mean(0).astype(np.uint8)
        return out

    res = inpaint_arrays(obj, mask, FillEngine(512, mean_fill))
    assert res is not None
    x, y, w, h = res.rect
    assert x <= 300 and y <= 200 and x + w >= 381 and y + h >= 301
    a = res.patch[..., 3]
    assert a[200 - y + 50, 300 - x + 40] == 255 and a[0, 0] == 0
    comp = obj.copy()
    al = a[..., None].astype(np.float32) / 255
    comp[y : y + h, x : x + w] = (
        comp[y : y + h, x : x + w] * (1 - al) + res.patch[..., :3] * al
    ).astype(np.uint8)
    assert (comp[250, 340] != [255, 0, 0]).any()
    assert np.abs(comp.astype(int) - obj)[:100].max() == 0
    assert inpaint_arrays(obj, np.zeros_like(mask), FillEngine(512, mean_fill)) is None


# ------------------------------------------------------------------------------ rpc contract
@pytest.fixture
def svc(tmp_path):
    s = WorkerService(models_dir=str(tmp_path / "models"), idle_unload_s=0)
    yield s
    s.besttake.shutdown()
    s.enhance.shutdown()
    s.gen.shutdown()
    s.beauty.shutdown()
    s.masks.shutdown()
    s.analyzer.shutdown()


def run(coro):
    return asyncio.run(coro)


def photo(tmp_path, name="a.png", seed=1):
    p = tmp_path / name
    cv2.imwrite(str(p), cv2.cvtColor(synth_image(seed, (320, 240)), cv2.COLOR_RGB2BGR))
    return {"photo_id": seed, "path": str(p)}


def test_handlers_registered(svc):
    h = svc._handlers()
    assert {"besttake.compose", "inpaint.run", "enhance.run"} <= set(h)


def test_models_missing_is_32010(svc, tmp_path):
    ph = photo(tmp_path)
    box = [0.3, 0.3, 0.2, 0.3]
    with pytest.raises(ModelUnavailable) as e:
        run(
            svc.besttake.compose(
                {
                    "base": ph,
                    "source": ph,
                    "base_face": box,
                    "source_face": box,
                    "out_dir": str(tmp_path),
                }
            )
        )
    assert e.value.code == -32010 and "mediapipe-face-landmarker" in e.value.extra["models"]
    mask = tmp_path / "m.png"
    cv2.imwrite(str(mask), np.full((240, 320), 255, np.uint8))
    with pytest.raises(ModelUnavailable) as e:
        run(svc.inpaint.run({"photo": ph, "mask": str(mask), "out_dir": str(tmp_path)}))
    assert e.value.extra["models"] == ["lama-big-fp32"]
    for op, ids in (
        ("denoise", ["scunet-color-real-psnr"]),
        ("face_restore", ["gfpgan-v1.4", "mediapipe-face-landmarker"]),
    ):
        with pytest.raises(ModelUnavailable) as e:
            run(svc.enhance.run({"photo": ph, "op": op, "out_dir": str(tmp_path)}))
        assert set(ids) <= set(e.value.extra["models"])
    with pytest.raises(ModelUnavailable) as e:
        run(svc.enhance.run({"photo": ph, "op": "upscale", "scale": 4, "out_dir": str(tmp_path)}))
    assert e.value.extra["models"][0].startswith("realesrgan-x4")


def test_sdxl_without_pro_extra_is_32010_with_model_id(svc, tmp_path):
    from imagepicker_ai.inpaint import sdxl

    if sdxl.available():
        pytest.skip("pro extra installed")
    mask = tmp_path / "m.png"
    cv2.imwrite(str(mask), np.full((240, 320), 255, np.uint8))
    with pytest.raises(ModelUnavailable) as e:
        run(
            svc.inpaint.run(
                {
                    "photo": photo(tmp_path),
                    "mask": str(mask),
                    "model": "sdxl",
                    "out_dir": str(tmp_path),
                }
            )
        )
    assert e.value.code == -32010 and e.value.extra["models"] == ["sdxl-inpaint"]


def test_invalid_params(svc, tmp_path):
    ph = photo(tmp_path)
    with pytest.raises(InvalidParams):
        run(svc.enhance.run({"photo": ph, "op": "sharpen", "out_dir": str(tmp_path)}))
    with pytest.raises(InvalidParams):
        run(
            svc.enhance.run({"photo": ph, "op": "denoise", "strength": 2, "out_dir": str(tmp_path)})
        )
    with pytest.raises(InvalidParams):
        run(svc.enhance.run({"photo": ph, "op": "upscale", "scale": 3, "out_dir": str(tmp_path)}))
    with pytest.raises(InvalidParams):
        run(svc.inpaint.run({"photo": ph, "out_dir": str(tmp_path)}))
    with pytest.raises(InvalidParams):
        run(
            svc.inpaint.run(
                {"photo": ph, "mask": "x.png", "model": "flux", "out_dir": str(tmp_path)}
            )
        )
    with pytest.raises(InvalidParams):
        run(
            svc.besttake.compose(
                {"base": ph, "source": ph, "base_face": [0, 0, 0, 1], "out_dir": "x"}
            )
        )


def test_models_list_reports_m5_models(svc):
    res = run(svc.models_list(None, None))
    assert res["besttake_models"][0] == "mediapipe-face-landmarker"
    assert res["inpaint_models"] == {"lama": ["lama-big-fp32"], "sdxl": ["sdxl-inpaint"]}
    assert res["enhance_models"]["denoise"] == ["scunet-color-real-psnr"]
    by_id = {m["id"]: m for m in res["models"]}
    for mid, op in (
        ("lama-big-fp32", "inpaint.run"),
        ("scunet-color-real-psnr", "enhance.denoise"),
        ("gfpgan-v1.4", "enhance.face_restore"),
        ("realesrgan-x2", "enhance.upscale"),
    ):
        assert op in by_id[mid]["required_for"] and by_id[mid]["license"]
    assert by_id["sdxl-inpaint"]["license"] == "OpenRAIL++"
    # coarse tags Core uses for its up-front 409 check
    tag = lambda t: {m["id"] for m in res["models"] if t in m["required_for"]}  # noqa: E731
    assert (
        tag("inpaint") == {"lama-big-fp32", "sdxl-inpaint"}
        and "pro" in by_id["sdxl-inpaint"]["required_for"]
    )
    assert by_id["sdxl-inpaint"]["optional"]
    assert tag("enhance") >= {
        "scunet-color-real-psnr",
        "gfpgan-v1.4",
        "realesrgan-x2",
        "realesrgan-x4",
    }
    assert {"mediapipe-face-landmarker", "mediapipe-selfie-multiclass"} <= tag("besttake")


def test_registry_m5_entries_are_complete(svc):
    for mid in ("lama-big-fp32", "scunet-color-real-psnr", "gfpgan-v1.4", "realesrgan-x2",
                "realesrgan-x4", "realesrgan-x2-fp16", "realesrgan-x4-fp16", "sdxl-inpaint"):  # fmt: skip
        s = svc.registry.get(mid)
        assert s.license and s.size_mb > 0 and s.vram_mb > 0
        assert all(f.sha256 for f in s.files if f.path.endswith((".onnx", ".safetensors", ".data")))
        assert not s.noncommercial
    assert svc.registry.get("sdxl-inpaint").exclusive_group == "diffusion"


def test_manager_lease_blocks_exclusive_eviction(svc):
    from imagepicker_ai.errors import OutOfMemory

    m = svc.manager
    spec_a, spec_b = svc.registry.get("sdxl-inpaint"), svc.registry.get("birefnet-lite")
    # explicit budget: the default derives from host RAM, which must not influence this test.
    # Holds sdxl-inpaint (12000 MB) once, but not twice, so eviction is what makes room.
    m.budget_mb = 16000
    assert spec_a.exclusive_group != spec_b.exclusive_group
    # fake install: loaders are not called through acquire() without files, so go through _loaded
    from imagepicker_ai.models.manager import _Loaded

    m._loaded["sdxl-inpaint"] = _Loaded(spec_a, object(), 100, in_use=1)
    other = svc.registry.get("sdxl-inpaint")
    with pytest.raises(OutOfMemory), m._lock:
        m._make_room(other)
    m._loaded["sdxl-inpaint"].in_use = 0
    with m._lock:
        m._make_room(other)
    assert "sdxl-inpaint" not in m._loaded
    assert not any(d["in_use"] for d in m.loaded())


# ------------------------------------------------------------------------------ with models
@pytest.fixture(scope="module")
def msvc(models_dir):
    from imagepicker_ai.models import Downloader

    s = WorkerService(models_dir=str(models_dir), idle_unload_s=0)
    d = Downloader(models_dir)
    need = {*s.besttake.all_models(), "lama-big-fp32", "scunet-color-real-psnr", "gfpgan-v1.4",
            s.enhance.upscale_model(2)}  # fmt: skip
    for mid in need:
        try:
            d.ensure(s.registry.get(mid))
        except Exception as e:  # noqa: BLE001
            pytest.skip(f"cannot download {mid}: {e}")
    yield s
    s.besttake.shutdown()
    s.enhance.shutdown()
    s.gen.shutdown()
    s.beauty.shutdown()
    s.masks.shutdown()
    s.analyzer.shutdown()
    s.manager.unload()


def read_rgb(path):
    return cv2.cvtColor(cv2.imread(str(path), cv2.IMREAD_COLOR), cv2.COLOR_BGR2RGB)


@pytest.mark.models
def test_inpaint_lama_removes_synthetic_object(msvc, tmp_path):
    bg = synth_image(5, (1200, 900))
    img = bg.copy()
    cv2.rectangle(img, (520, 330), (640, 510), (230, 30, 30), -1)
    cv2.circle(img, (610, 320), 45, (240, 200, 30), -1)
    mask = np.zeros(img.shape[:2], np.uint8)
    cv2.rectangle(mask, (518, 328), (642, 512), 255, -1)
    cv2.circle(mask, (610, 320), 47, 255, -1)
    ip, mp = tmp_path / "i.png", tmp_path / "m.png"
    cv2.imwrite(str(ip), cv2.cvtColor(img, cv2.COLOR_RGB2BGR))
    cv2.imwrite(str(mp), mask)
    r = run(
        msvc.inpaint.run(
            {"photo": {"photo_id": 1, "path": str(ip)}, "mask": str(mp), "out_dir": str(tmp_path)}
        )
    )
    assert r["model"] == "lama-big-fp32"
    p = cv2.imread(r["patch"], cv2.IMREAD_UNCHANGED)
    x, y = round(r["rect"][0] * 1200), round(r["rect"][1] * 900)
    a = p[..., 3:4].astype(np.float32) / 255
    out = img.copy()
    out[y : y + p.shape[0], x : x + p.shape[1]] = (
        out[y : y + p.shape[0], x : x + p.shape[1]] * (1 - a)
        + cv2.cvtColor(p[..., :3], cv2.COLOR_BGR2RGB) * a
    ).astype(np.uint8)
    m = mask > 0
    assert np.abs(img.astype(int) - bg)[m].mean() > 60
    assert np.abs(out.astype(int) - bg)[m].mean() < 15
    # outside the patch nothing changed
    assert np.abs(out.astype(int) - img)[:200].max() == 0


@pytest.mark.models
def test_enhance_ops_with_models(msvc, commons, tmp_path):
    path = commons("Official_portrait_of_Barack_Obama.jpg")
    img = read_rgb(path)
    h, w = img.shape[:2]
    noisy = np.clip(
        img.astype(np.float32) + np.random.default_rng(0).normal(0, 12, img.shape), 0, 255
    )
    np_ = tmp_path / "noisy.png"
    cv2.imwrite(str(np_), cv2.cvtColor(noisy.astype(np.uint8), cv2.COLOR_RGB2BGR))
    ph = {"photo_id": 1, "path": str(np_)}
    r = run(
        msvc.enhance.run({"photo": ph, "op": "denoise", "strength": 1.0, "out_dir": str(tmp_path)})
    )
    assert r["rect"] == [0, 0, 1, 1]
    den = read_rgb(r["patch"])
    assert estimate_noise(den) < 0.3 * estimate_noise(noisy.astype(np.uint8))
    assert np.abs(den.astype(int) - img).mean() < np.abs(noisy - img).mean() * 0.5

    ph = {"photo_id": 2, "path": str(path)}
    r = run(msvc.enhance.run({"photo": ph, "op": "upscale", "scale": 2, "out_dir": str(tmp_path)}))
    assert read_rgb(r["image"]).shape == (h * 2, w * 2, 3)

    r = run(
        msvc.enhance.run(
            {"photo": ph, "op": "face_restore", "strength": 0.8, "out_dir": str(tmp_path)}
        )
    )
    assert len(r["patches"]) == 1
    rect = r["patches"][0]["rect"]
    assert 0.2 < rect[0] < 0.5 and rect[2] > 0.3
    p = cv2.imread(r["patches"][0]["patch"], cv2.IMREAD_UNCHANGED)
    assert p.shape[1] == round(rect[2] * w) and p.shape[2] == 4


@pytest.mark.models
def test_besttake_real_portrait_closed_eyes(msvc, commons, tmp_path):
    from imagepicker_ai.beauty import landmarks as lmk
    from imagepicker_ai.steps.faces import FaceAnalyzer

    truth = read_rgb(commons("Official_portrait_of_Barack_Obama.jpg"))
    h, w = truth.shape[:2]
    fa = FaceAnalyzer(str(msvc.manager.path("yunet")), None)
    face = sorted(fa.detect(truth), key=lambda f: -f["bbox"][2])[0]["bbox"]
    fpx = (face[0] * w, face[1] * h, face[2] * w, face[3] * h)
    obs = msvc.besttake._mesh_for().observe(truth, fpx)
    base = truth.copy()
    for ids in (lmk.RIGHT_EYE, lmk.LEFT_EYE):
        m = cv2.GaussianBlur(
            lmk.polygon_mask(obs.pts, ids, (h, w), 0.012 * obs.width).astype(np.float32), (0, 0), 2
        )
        c = obs.pts[ids].mean(0)
        col = truth[int(c[1] - 0.08 * obs.width), int(c[0])].astype(np.float32)
        base = (base * (1 - m[..., None]) + col * m[..., None]).astype(np.uint8)
    M = cv2.getRotationMatrix2D((w / 2, h / 2), 0.6, 1.0)
    M[:, 2] += (9.3, -6.1)
    src = cv2.warpAffine(truth, M, (w, h), flags=cv2.INTER_CUBIC, borderMode=cv2.BORDER_REFLECT_101)
    bp, sp = tmp_path / "b.png", tmp_path / "s.png"
    cv2.imwrite(str(bp), cv2.cvtColor(base, cv2.COLOR_RGB2BGR))
    cv2.imwrite(str(sp), cv2.cvtColor(src, cv2.COLOR_RGB2BGR))
    c2 = M @ np.array([fpx[0] + fpx[2] / 2, fpx[1] + fpx[3] / 2, 1.0])
    sbox = [c2[0] / w - face[2] / 2, c2[1] / h - face[3] / 2, face[2], face[3]]
    r = run(
        msvc.besttake.compose(
            {
                "base": {"photo_id": 1, "path": str(bp)},
                "source": {"photo_id": 2, "path": str(sp)},
                "base_face": face,
                "source_face": sbox,
                "out_dir": str(tmp_path),
            }  # fmt: skip
        )
    )
    assert r["patch"], r
    assert r["quality"]["aligned"] and r["quality"]["score"] > 0.7
    p = cv2.imread(r["patch"], cv2.IMREAD_UNCHANGED)
    x, y = round(r["rect"][0] * w), round(r["rect"][1] * h)
    a = p[..., 3:4].astype(np.float32) / 255
    comp = base.copy()
    comp[y : y + p.shape[0], x : x + p.shape[1]] = (
        comp[y : y + p.shape[0], x : x + p.shape[1]] * (1 - a)
        + cv2.cvtColor(p[..., :3], cv2.COLOR_BGR2RGB) * a
    ).astype(np.uint8)
    eyes = np.zeros((h, w), bool)
    for ids in (lmk.RIGHT_EYE, lmk.LEFT_EYE):
        eyes |= lmk.polygon_mask(obs.pts, ids, (h, w), 0.01 * obs.width) > 0
    assert np.abs(base.astype(int) - truth)[eyes].mean() > 25
    assert np.abs(comp.astype(int) - truth)[eyes].mean() < 10
    # a different photo has no common scene with this one: not composable
    other = commons("Michelle_Obama_2013_official_portrait.jpg")
    r2 = run(
        msvc.besttake.compose(
            {
                "base": {"photo_id": 1, "path": str(bp)},
                "source": {"photo_id": 3, "path": str(other)},
                "base_face": face,
                "source_face": face,
                "out_dir": str(tmp_path),
            }  # fmt: skip
        )
    )
    assert r2["patch"] is None and r2["reason"] in ("camera_moved", "face_not_found")
