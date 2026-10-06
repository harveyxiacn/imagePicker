"""`beauty.prepare` (docs/api-contract-m4.md section B): per-person geometry for portrait ops.

For every face of a photo:
  face_landmarks  478 MediaPipe Face Mesh points on a face crop, normalised to the upright image
  pose            33 MediaPipe Pose keypoints of the body that owns the face ([] if none)
  body_mask       this person's matte: BiRefNet on a body crop + connected component / watershed
                  (same code as `mask.generate` target `person` with `person_bbox`), guided filter
  skin_mask       selfie-multiclass (face-skin + body-skin) x this person's matte, guided filter,
                  minus the eye / eyebrow / outer-lip polygons of the landmarks (landmarks.py)
  blemishes       small dark / red spots on the skin of the face oval (blemish.py)
All images are 8-bit PNG, long edge = `size`, upright (EXIF applied), written to `out_dir`.
"""

from __future__ import annotations

import asyncio
import logging
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import cv2
import numpy as np

from ..decode import DecodeError
from ..errors import DECODE_FAILED, InvalidParams, OutOfMemory, RpcError
from ..masks.filters import refine_alpha, smoothstep, to_u8
from ..masks.generator import (
    SELFIE,
    YUNET,
    MaskGenerator,
    ProgressFn,
    _Photo,
    _safe_name,
    load_upright,
    write_png,
)
from ..masks.nets import CLS_BODY_SKIN, CLS_FACE_SKIN, CLS_HAIR
from ..masks.person import _torso_marker, select_person
from ..steps.embed import _is_oom
from ..steps.faces import mediapipe_available
from . import landmarks as lmk
from .blemish import detect_blemishes
from .pose import PoseEstimator, skeleton_marker

log = logging.getLogger(__name__)

FACE_LANDMARKER = "mediapipe-face-landmarker"
POSE_LANDMARKER = "mediapipe-pose-landmarker-full"
MAX_FACES = 12
CROP_MARGIN = 0.35
MIN_CROP_SIDE = 256
EXCLUDE_ZONES = (  # blemish rejection zones: (index loop, dilation in face widths)
    (lmk.RIGHT_EYE, 0.035),
    (lmk.LEFT_EYE, 0.035),
    (lmk.RIGHT_BROW, 0.025),
    (lmk.LEFT_BROW, 0.025),
    (lmk.LIPS_OUTER, 0.025),
)
NOSTRIL_DILATE = 0.02


class FaceMesh:
    """MediaPipe Face Landmarker on a crop around a known face box (478 points)."""

    def __init__(self, model_path: str | Path):
        from mediapipe.tasks.python import BaseOptions, vision

        opts = vision.FaceLandmarkerOptions(
            base_options=BaseOptions(model_asset_path=str(model_path)),
            running_mode=vision.RunningMode.IMAGE,
            num_faces=4,
            min_face_detection_confidence=0.3,
            min_face_presence_confidence=0.3,
        )
        self._lm: Any = vision.FaceLandmarker.create_from_options(opts)

    def landmarks(
        self, rgb: np.ndarray, face_px: tuple[float, float, float, float]
    ) -> np.ndarray | None:
        """(478, 2) pixel landmarks in `rgb` coordinates, or None when no mesh was found."""
        import mediapipe as mp

        h, w = rgb.shape[:2]
        x, y, bw, bh = face_px
        cx, cy = x + bw / 2, y + bh / 2
        side = max(bw, bh) * (1 + 2 * CROP_MARGIN)
        x0, y0 = int(max(0, cx - side / 2)), int(max(0, cy - side / 2))
        x1, y1 = int(min(w, cx + side / 2)), int(min(h, cy + side / 2))
        crop = rgb[y0:y1, x0:x1]
        if crop.size == 0:
            return None
        cw, ch = crop.shape[1], crop.shape[0]
        if min(cw, ch) < MIN_CROP_SIDE:
            s = MIN_CROP_SIDE / min(cw, ch)
            crop = cv2.resize(crop, None, fx=s, fy=s, interpolation=cv2.INTER_CUBIC)
        img = mp.Image(image_format=mp.ImageFormat.SRGB, data=np.ascontiguousarray(crop))
        res = self._lm.detect(img)
        best, best_d = None, 0.6 * max(bw, bh)
        for face in res.face_landmarks:
            if len(face) < lmk.N_FULL:
                continue
            pts = np.array([[x0 + p.x * cw, y0 + p.y * ch] for p in face], dtype=np.float64)
            d = float(np.linalg.norm(pts[: lmk.N_FACE].mean(0) - (cx, cy)))
            if d < best_d:
                best, best_d = pts, d
        return best

    def close(self) -> None:
        lm, self._lm = self._lm, None
        if lm is not None:
            lm.close()


def parse_request(params: Any) -> dict[str, Any]:
    if not isinstance(params, dict):
        raise InvalidParams("params must be an object")
    photo = params.get("photo")
    if (
        not isinstance(photo, dict)
        or "photo_id" not in photo
        or not isinstance(photo.get("path"), str)
    ):
        raise InvalidParams("photo needs photo_id and path")
    orient = photo.get("orientation")
    if orient is not None and (not isinstance(orient, int) or not 1 <= orient <= 8):
        raise InvalidParams("photo.orientation must be an EXIF value 1-8")
    faces = params.get("faces") or []
    if not isinstance(faces, list):
        raise InvalidParams("faces must be an array")
    parsed: list[dict[str, Any]] = []
    for i, f in enumerate(faces):
        bbox = f.get("bbox") if isinstance(f, dict) else None
        if (
            not isinstance(bbox, list)
            or len(bbox) != 4
            or not all(isinstance(v, (int, float)) and not isinstance(v, bool) for v in bbox)
            or bbox[2] <= 0
            or bbox[3] <= 0
        ):
            raise InvalidParams(f"faces[{i}].bbox must be [x, y, w, h] (normalised, w/h > 0)")
        parsed.append({"face_id": f.get("face_id"), "bbox": [float(v) for v in bbox]})
    size = params.get("size", 1536)
    if not isinstance(size, int) or isinstance(size, bool) or not 128 <= size <= 8192:
        raise InvalidParams("size must be an integer in [128, 8192]")
    out_dir = params.get("out_dir")
    if not isinstance(out_dir, str) or not out_dir:
        raise InvalidParams("out_dir is required")
    return {
        "photo_id": photo["photo_id"],
        "path": photo["path"],
        "orientation": orient,
        "faces": parsed,
        "size": size,
        "out_dir": Path(out_dir),
        "allow_download": bool(params.get("allow_download", False)),
    }


def _body_window(alpha: np.ndarray, margin: float = 0.06) -> tuple[slice, slice] | None:
    ys, xs = np.nonzero(alpha > 0.02)
    if len(ys) == 0:
        return None
    h, w = alpha.shape
    my, mx = int((ys.max() - ys.min()) * margin) + 4, int((xs.max() - xs.min()) * margin) + 4
    return (
        slice(max(0, ys.min() - my), min(h, ys.max() + my + 1)),
        slice(max(0, xs.min() - mx), min(w, xs.max() + mx + 1)),
    )


def skin_from_parts(
    probs: np.ndarray,
    rgb: np.ndarray,
    body: np.ndarray,
    pts_px: np.ndarray,
    face_w_px: float,
) -> np.ndarray:
    """Float32 [0, 1] skin of one person: parts x matte, guided filter, minus eyes/brows/lips."""
    h, w = body.shape
    out = np.zeros((h, w), np.float32)
    gate_full = smoothstep(body, 0.2, 0.8)
    raw_full = np.clip(probs[..., CLS_BODY_SKIN] + probs[..., CLS_FACE_SKIN], 0.0, 1.0) * gate_full
    win = _body_window(raw_full, 0.04)  # only where there can be skin (face, neck, hands)
    if win is None:
        return out
    gate = gate_full[win]
    a = refine_alpha(raw_full[win], rgb[win], 0.006, 2e-3)
    a = smoothstep(a, 0.15, 0.85) * gate
    feat = lmk.feature_mask(pts_px, (h, w), face_w_px)[win].astype(np.float32)
    feat = cv2.GaussianBlur(feat, (0, 0), max(0.7, 0.004 * face_w_px))
    out[win] = a * (1.0 - np.clip(feat * 1.6, 0.0, 1.0))
    return out


def exclusion_zones(pts_px: np.ndarray, shape: tuple[int, int], face_w_px: float) -> np.ndarray:
    z = np.zeros(shape, np.uint8)
    for idx, d in EXCLUDE_ZONES:
        z |= lmk.polygon_mask(pts_px, idx, shape, d * face_w_px)
    nos = lmk.hull_mask(pts_px, lmk.NOSTRILS, shape)
    r = max(1, int(round(NOSTRIL_DILATE * face_w_px)))
    z |= cv2.dilate(nos, cv2.getStructuringElement(cv2.MORPH_ELLIPSE, (2 * r + 1, 2 * r + 1)))
    return z


def resolve_overlaps(
    bodies: list[np.ndarray],
    boxes: list[tuple[float, float, float, float]],
    skeletons: list[np.ndarray | None],
) -> None:
    """In place: pixels claimed by two or more mattes go to the person whose seed is nearest.

    Seed = face box + torso strip (as in the watershed) + pose skeleton. The hand-over is a soft
    ramp of width ~ one face width over the claimed region only; unshared pixels are untouched.
    """
    n = len(bodies)
    if n < 2:
        return
    h, w = bodies[0].shape
    claimed = np.stack([b > 0.05 for b in bodies])
    if (claimed.sum(0) < 2).all():
        return
    dist = []
    for box, sk in zip(boxes, skeletons, strict=True):
        seed = _torso_marker((h, w), box).astype(bool)
        if sk is not None:
            seed |= sk > 0
        dist.append(cv2.distanceTransform((~seed).astype(np.uint8), cv2.DIST_L2, 3))
    for i in range(n):
        rest = np.full((h, w), np.inf, np.float32)
        for j in range(n):
            if j != i:
                rest = np.where(claimed[j], np.minimum(rest, dist[j]), rest)
        tau = max(2.0, boxes[i][2])  # one face width
        ramp = np.clip(0.5 + (rest - dist[i]) / (2 * tau), 0.0, 1.0)
        keep = np.where(np.isfinite(rest), ramp, 1.0).astype(np.float32)
        bodies[i] *= keep


class BeautyPreparer:
    def __init__(self, masks: MaskGenerator):
        self.masks = masks
        self.manager = masks.manager
        self._mesh: dict[str, FaceMesh] = {}
        self._pose: dict[str, PoseEstimator] = {}
        self.timings: dict[str, float] = {}

    def shutdown(self) -> None:
        for d in (self._mesh, self._pose):
            for v in d.values():
                v.close()
            d.clear()

    # ------------------------------------------------------------------ models
    def required_models(self) -> list[str]:
        """Everything `beauty.prepare` may use (what `models.ensure` should fetch)."""
        out = [YUNET, FACE_LANDMARKER, POSE_LANDMARKER, SELFIE, self.masks.subject_model()]
        return list(dict.fromkeys(out))

    def _face_mesh(self) -> FaceMesh | None:
        if not (self.manager.is_installed(FACE_LANDMARKER) and mediapipe_available()):
            return None
        path = str(self.manager.path(FACE_LANDMARKER))
        if path not in self._mesh:
            self._mesh[path] = FaceMesh(path)
        return self._mesh[path]

    def _pose_est(self) -> PoseEstimator | None:
        if not (self.manager.is_installed(POSE_LANDMARKER) and mediapipe_available()):
            return None
        path = str(self.manager.path(POSE_LANDMARKER))
        if path not in self._pose:
            self._pose[path] = PoseEstimator(path)
        return self._pose[path]

    @contextmanager
    def _t(self, name: str):
        t0 = time.perf_counter()
        try:
            yield
        finally:
            self.timings[name] = self.timings.get(name, 0.0) + time.perf_counter() - t0

    # ------------------------------------------------------------------ per person
    def _geometry(
        self,
        photo: _Photo,
        face_px: tuple[float, float, float, float],
        use: dict[str, bool],
        skipped: dict[str, str],
    ) -> tuple[np.ndarray | None, list[list[float]]]:
        """Face mesh (478, 2 px) and the pose (33 x [x, y, vis], normalised) of one face."""
        rgb = photo.rgb
        pts: np.ndarray | None = None
        mesh = self._face_mesh() if use["face_landmarks"] else None
        if mesh is not None:
            try:
                with self._t("face_mesh"):
                    pts = mesh.landmarks(rgb, face_px)
            except Exception:  # noqa: BLE001
                log.exception("face mesh failed")
            if pts is None:
                skipped.setdefault("face_landmarks", "failed")
        pose: list[list[float]] = []
        est = self._pose_est() if use["pose"] else None
        if est is not None:
            try:
                with self._t("pose"):
                    pose = est.pose_for_face(rgb, face_px)
            except Exception:  # noqa: BLE001
                log.exception("pose failed")
                skipped.setdefault("pose", "failed")
        return pts, pose

    def _body(
        self,
        photo: _Photo,
        face_px: tuple[float, float, float, float],
        others: list[tuple[float, float, float, float]],
        seeds: tuple[np.ndarray | None, list[np.ndarray | None]],
    ) -> np.ndarray:
        """Matte of the person owning `face_px` (pose skeletons seed the split from neighbours)."""
        rgb = photo.rgb
        h, w = rgb.shape[:2]
        net = self.masks._birefnet()
        with self._t("body_matte"):
            raw = select_person(rgb, face_px, others, net.predict, seeds[0], seeds[1])
            win = _body_window(raw)
            body = np.zeros((h, w), np.float32)
            if win is not None:
                body[win] = refine_alpha(raw[win], rgb[win], 0.004, 1e-3)
        return body

    def _skin(
        self, photo: _Photo, body: np.ndarray, pts: np.ndarray
    ) -> tuple[np.ndarray, list[list[float]]]:
        """Skin mask of one person (parts x matte - features) and the blemishes on it."""
        rgb = photo.rgb
        h, w = rgb.shape[:2]
        with self._t("selfie_parts"):
            probs = self.masks._part_probs(photo)
        fw = float(np.ptp(pts[:, 0]))
        with self._t("skin"):
            skin = skin_from_parts(probs, rgb, body, pts, fw)
        with self._t("blemishes"):
            oval = lmk.oval_mask(pts, (h, w))
            exc = exclusion_zones(pts, (h, w), fw)
            fbox = (float(pts[:, 0].min()), float(pts[:, 1].min()), fw, float(np.ptp(pts[:, 1])))
            blobs = detect_blemishes(
                rgb, skin, fbox, region=oval, exclude=exc, hair=probs[..., CLS_HAIR]
            )
        le = float(max(w, h))
        return skin, [[round(x / w, 5), round(y / h, 5), round(r / le, 5)] for x, y, r in blobs]

    def _with_cpu_retry(self, fn: Any, *args: Any) -> Any:
        try:
            return fn(*args)
        except Exception as e:  # noqa: BLE001
            if not _is_oom(e):
                raise
            if self.masks.force_cpu:
                raise OutOfMemory(str(e)) from e
            log.warning("OOM in beauty.prepare; retrying on CPU")
            self.manager.unload()
            self.masks.force_cpu = True
            return fn(*args)

    def _run(
        self,
        req: dict[str, Any],
        photo: _Photo,
        faces: list[dict[str, Any]],
        use: dict[str, bool],
        skipped: dict[str, str],
        notify: ProgressFn,
    ) -> list[dict[str, Any]]:
        w, h = photo.width, photo.height
        boxes = [
            (f["bbox"][0] * w, f["bbox"][1] * h, f["bbox"][2] * w, f["bbox"][3] * h) for f in faces
        ]
        out_dir: Path = req["out_dir"]
        out_dir.mkdir(parents=True, exist_ok=True)
        people: list[dict[str, Any]] = []
        n = len(boxes)
        # phase 1: geometry of everybody (the pose skeletons seed the person split of phase 2)
        geo = [self._geometry(photo, box, use, skipped) for box in boxes]
        skel = [
            skeleton_marker(pose, (h, w), 0.22 * box[2]) if pose else None
            for (_, pose), box in zip(geo, boxes, strict=True)
        ]
        # phase 2: every person's matte, then resolve regions claimed by two people
        bodies: list[np.ndarray | None] = [None] * n
        if use["body"]:
            for i, box in enumerate(boxes):
                others = [b for j, b in enumerate(boxes) if j != i]
                seeds = (skel[i], [m for j, m in enumerate(skel) if j != i])
                bodies[i] = self._with_cpu_retry(self._body, photo, box, others, seeds)
            with self._t("resolve_overlap"):
                resolve_overlaps(bodies, boxes, skel)  # type: ignore[arg-type]
        # phase 3: skin + blemishes per person, write PNGs
        for i, f in enumerate(faces):
            pts, pose = geo[i]
            body = bodies[i]
            r: dict[str, Any] = {"skin": None, "body": body, "blemishes": []}
            if use["skin"]:
                if body is None or pts is None:
                    skipped.setdefault("skin_mask", "requires_body_and_landmarks")
                else:
                    r["skin"], r["blemishes"] = self._with_cpu_retry(self._skin, photo, body, pts)
            r["face_landmarks"] = (
                [] if pts is None else [[round(x / w, 5), round(y / h, 5)] for x, y in pts]
            )
            r["pose"] = [[round(x, 5), round(y, 5), round(v, 4)] for x, y, v in pose]
            fid = f["face_id"] if f["face_id"] is not None else f"f{i}"
            tag = f"{_safe_name(req['photo_id'])}_{_safe_name(fid)}"
            skin_path = body_path = None
            with self._t("write_png"):
                if r["skin"] is not None:
                    skin_path = out_dir / f"{tag}_skin.png"
                    write_png(skin_path, to_u8(r["skin"]))
                if r["body"] is not None:
                    body_path = out_dir / f"{tag}_body.png"
                    write_png(body_path, to_u8(r["body"]))
            people.append(
                {
                    "face_id": f["face_id"],
                    "face_box": [round(v, 5) for v in f["bbox"]],
                    "face_landmarks": r["face_landmarks"],
                    "pose": r["pose"],
                    "skin_mask": str(skin_path) if skin_path else None,
                    "body_mask": str(body_path) if body_path else None,
                    "blemishes": r["blemishes"],
                }
            )
            notify({"kind": "beauty", "done": i + 1, "total": n})
        return people

    # ------------------------------------------------------------------ entry
    async def prepare(self, params: Any, progress: ProgressFn | None = None) -> dict[str, Any]:
        req = parse_request(params)
        loop = asyncio.get_running_loop()
        notify = progress or (lambda _p: None)
        skipped: dict[str, str] = {}
        models: dict[str, str] = {}
        self.timings = {}
        t_start = time.perf_counter()

        wanted = [m for m in self.required_models() if not self.manager.is_installed(m)]
        failed: set[str] = set()
        if wanted and req["allow_download"]:
            for mid in wanted:

                def on_progress(e: dict[str, Any]) -> None:
                    loop.call_soon_threadsafe(
                        notify, {"kind": "model.download", "done": e["bytes"], **e}
                    )

                try:
                    await loop.run_in_executor(
                        self.masks.pool, lambda m=mid: self.manager.ensure(m, on_progress)
                    )
                except Exception as e:  # noqa: BLE001
                    log.warning("download of %s failed: %s", mid, e)
                    failed.add(mid)

        def why(*ids: str) -> str | None:
            absent = [m for m in ids if not self.manager.is_installed(m)]
            if not absent:
                return None
            return "download_failed" if set(absent) & failed else "model_unavailable"

        birefnet = self.masks.subject_model()
        mp_ok = mediapipe_available()
        use = {
            "face_landmarks": False,
            "pose": False,
            "body": False,
            "skin": False,
        }
        for key, ids in (
            ("face_landmarks", [FACE_LANDMARKER]),
            ("pose", [POSE_LANDMARKER]),
            ("body", [birefnet]),
        ):
            reason = why(*ids) or (None if mp_ok or key == "body" else "model_unavailable")
            if reason:
                skipped["body_mask" if key == "body" else key] = reason
            else:
                use[key] = True
        reason = why(SELFIE)
        if reason:
            skipped["skin_mask"] = reason
        elif not (use["body"] and use["face_landmarks"]):
            skipped["skin_mask"] = "requires_body_and_landmarks"
        else:
            use["skin"] = True
        if not use["skin"]:
            skipped["blemishes"] = skipped["skin_mask"]
        if use["face_landmarks"]:
            models["face_landmarks"] = FACE_LANDMARKER
        if use["pose"]:
            models["pose"] = POSE_LANDMARKER
        if use["body"]:
            models["body_mask"] = birefnet
        if use["skin"]:
            models["skin_mask"] = f"{SELFIE}+{birefnet}"
            models["blemishes"] = "dog-lab"

        try:
            rgb = await loop.run_in_executor(
                self.masks.pool, load_upright, req["path"], req["size"], req["orientation"]
            )
        except DecodeError as e:
            raise RpcError(DECODE_FAILED, str(e), "decode_failed") from e
        photo = _Photo(rgb)

        faces = req["faces"]
        if not faces:
            faces = await loop.run_in_executor(self.masks.pool, self._detect, photo, skipped)
            if faces:
                models["faces"] = YUNET
        faces = faces[:MAX_FACES]

        def work() -> list[dict[str, Any]]:
            return self._run(req, photo, faces, use, skipped, notify)

        people: list[dict[str, Any]] = []
        if faces:
            try:
                people = await loop.run_in_executor(self.masks.pool, work)
            except OutOfMemory:
                skipped["body_mask"] = skipped["skin_mask"] = "out_of_memory"
        timings = {f"{k}_s": round(v, 4) for k, v in self.timings.items()}
        timings["wall_s"] = round(time.perf_counter() - t_start, 4)
        return {"people": people, "models": models, "skipped": skipped, "timings": timings}

    def _detect(self, photo: _Photo, skipped: dict[str, str]) -> list[dict[str, Any]]:
        if not self.manager.is_installed(YUNET):
            skipped["faces"] = "model_unavailable"
            return []
        found = self.masks._detect_faces(photo)
        w, h = photo.width, photo.height
        boxes = sorted(found, key=lambda b: -b[2] * b[3])
        return [{"face_id": None, "bbox": [b[0] / w, b[1] / h, b[2] / w, b[3] / h]} for b in boxes]
