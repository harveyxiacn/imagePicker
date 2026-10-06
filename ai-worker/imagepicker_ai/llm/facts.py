"""Measurements fed to the VLM next to the picture (a 3-4B model reads numbers better than pixels)."""

from __future__ import annotations

import math
from typing import Any

import cv2
import numpy as np

SUGGEST_SIDE = 768  # long edge of the image handed to the VLM


def resize_long_edge(rgb: np.ndarray, side: int = SUGGEST_SIDE) -> np.ndarray:
    h, w = rgb.shape[:2]
    s = side / max(h, w)
    if s >= 1.0:
        return np.ascontiguousarray(rgb)
    size = (max(1, round(w * s)), max(1, round(h * s)))
    return np.ascontiguousarray(cv2.resize(rgb, size, interpolation=cv2.INTER_AREA))


def image_facts(rgb: np.ndarray) -> dict[str, float]:
    """Luma / clipping / colour statistics of an RGB uint8 image (all in 0..1 unless noted)."""
    small = resize_long_edge(rgb, 256)
    f = small.astype(np.float32) / 255.0
    luma = 0.2126 * f[..., 0] + 0.7152 * f[..., 1] + 0.0722 * f[..., 2]
    p2, p50, p98 = (float(x) for x in np.percentile(luma, [2, 50, 98]))
    mx = f.max(axis=2)
    mn = f.min(axis=2)
    sat = np.where(mx > 1e-4, (mx - mn) / np.maximum(mx, 1e-4), 0.0)
    r, g, b = (float(f[..., i].mean()) for i in range(3))
    return {
        "luma_mean": round(float(luma.mean()), 3),
        "luma_p2": round(p2, 3),
        "luma_median": round(p50, 3),
        "luma_p98": round(p98, 3),
        "clipped_shadows": round(float((luma < 0.02).mean()), 3),
        "clipped_highlights": round(float((luma > 0.98).mean()), 3),
        "saturation_mean": round(float(sat.mean()), 3),
        "red_mean": round(r, 3),
        "green_mean": round(g, 3),
        "blue_mean": round(b, 3),
    }


def _brightness_word(m: float) -> str:
    if m < 0.15:
        return "very dark"
    if m < 0.30:
        return "dark"
    if m < 0.42:
        return "slightly dark"
    if m <= 0.62:
        return "well exposed"
    if m <= 0.75:
        return "slightly bright"
    if m <= 0.85:
        return "bright"
    return "very bright"


def _hist_summary(hist: Any) -> str | None:
    """Mean / 5th / 95th percentile of a luma histogram given as a list of bin counts."""
    if not isinstance(hist, list) or len(hist) < 8:
        return None
    try:
        h = np.asarray(hist, dtype=np.float64).reshape(-1)
    except (TypeError, ValueError):
        return None
    tot = h.sum()
    if not np.isfinite(tot) or tot <= 0:
        return None
    x = (np.arange(h.size) + 0.5) / h.size
    c = np.cumsum(h) / tot
    p5 = float(x[np.searchsorted(c, 0.05)])
    p95 = float(x[min(h.size - 1, np.searchsorted(c, 0.95))])
    return f"mean {float((x * h).sum() / tot):.2f}, p5 {p5:.2f}, p95 {p95:.2f}"


def _scalars(d: Any, limit: int = 14) -> str:
    if not isinstance(d, dict):
        return ""
    parts = []
    for k, v in d.items():
        if isinstance(v, bool) or not isinstance(v, (int, float, str)):
            continue
        parts.append(f"{k}={round(v, 3) if isinstance(v, float) else v}")
        if len(parts) >= limit:
            break
    return ", ".join(parts)


def measurements_text(facts: dict[str, float], context: dict[str, Any] | None) -> str:
    """Human-readable block for the prompt (image statistics + the caller's context)."""
    ctx = context if isinstance(context, dict) else {}
    lines = [
        f"- brightness: mean luma {facts['luma_mean']:.2f} ({_brightness_word(facts['luma_mean'])}), "
        f"median {facts['luma_median']:.2f}, darkest 2% below {facts['luma_p2']:.2f}, "
        f"brightest 2% above {facts['luma_p98']:.2f}",
        f"- clipping: {facts['clipped_shadows'] * 100:.1f}% of pixels crushed to black, "
        f"{facts['clipped_highlights'] * 100:.1f}% blown to white",
        f"- colour: mean saturation {facts['saturation_mean']:.2f}; average R/G/B "
        f"{facts['red_mean']:.2f}/{facts['green_mean']:.2f}/{facts['blue_mean']:.2f}",
    ]
    hs = _hist_summary(ctx.get("histogram"))
    if hs:
        lines.append(f"- analysis histogram (luma): {hs}")
    elif isinstance(ctx.get("histogram"), dict):
        s = _scalars(ctx["histogram"])
        if s:
            lines.append(f"- analysis histogram: {s}")
    s = _scalars(ctx.get("scores"))
    if s:
        lines.append(f"- quality scores (0-1): {s}")
    scene = ctx.get("scene_type")
    if isinstance(scene, str) and scene:
        lines.append(f"- scene type: {scene}")
    return "\n".join(lines)


def baseline_adjust(f: dict[str, float]) -> dict[str, float]:
    """Rule-based starting point (non-zero sliders only) computed from the measurements.

    A 3-4B VLM is poor at turning numbers into slider values (it fills every slider with the same
    small constant and misses colour casts), so the prompt hands it this baseline to confirm or
    refine; it still decides which problems exist and writes the reason.
    """
    out: dict[str, float] = {}
    med = max(f["luma_median"], 0.02)
    ev = 2.2 * math.log2(0.5 / med) * 0.7  # sRGB-encoded median -> EV towards a mid-grey target
    ev = max(-2.5, min(2.5, ev))
    if abs(ev) >= 0.5:
        out["exposure"] = round(ev, 1)
    if f["clipped_highlights"] > 0.02 or f["luma_p98"] > 0.97 or ev < -1.0:
        out["highlights"] = -int(min(70, 25 + 1500 * f["clipped_highlights"]))
        if f["clipped_highlights"] > 0.05:
            out["whites"] = -int(min(50, 600 * f["clipped_highlights"]))
    if f["clipped_shadows"] > 0.05 or f["luma_p2"] < 0.04 or ev > 1.0:
        out["shadows"] = int(min(70, 20 + 500 * f["clipped_shadows"] + 10 * max(ev, 0)))
    spread = f["luma_p98"] - f["luma_p2"]
    if spread < 0.55:
        out["contrast"] = int(min(40, (0.65 - spread) * 100))
        if spread < 0.4:
            out["dehaze"] = 15
    r, g, b = (max(f[k], 0.01) for k in ("red_mean", "green_mean", "blue_mean"))
    k = math.log2(b / r)  # > 0: bluish, < 0: reddish (gray-world; only strong casts)
    if abs(k) > 0.4:
        out["temp"] = int(max(-1500, min(1500, (k - math.copysign(0.4, k)) * 2500)))
    return out


# problems decided by the measurements, never by the VLM (it contradicts itself on these)
MEASURED_PROBLEMS = {
    "underexposed",
    "overexposed",
    "blown_highlights",
    "crushed_shadows",
    "low_contrast",
    "color_cast_warm",
    "color_cast_cool",
}


def measured_problems(f: dict[str, float]) -> list[str]:
    out = []
    if f["luma_median"] < 0.3:
        out.append("underexposed")
    elif f["luma_median"] > 0.7:
        out.append("overexposed")
    if f["clipped_highlights"] > 0.03:
        out.append("blown_highlights")
    if f["clipped_shadows"] > 0.08:
        out.append("crushed_shadows")
    if f["luma_p98"] - f["luma_p2"] < 0.45:
        out.append("low_contrast")
    k = math.log2(max(f["blue_mean"], 0.01) / max(f["red_mean"], 0.01))
    if k > 0.4:
        out.append("color_cast_cool")
    elif k < -0.4:
        out.append("color_cast_warm")
    return out


def merge_adjust(
    baseline: dict[str, float], vlm: dict[str, Any], deadband: float = 15, cap: float = 30
) -> dict[str, Any]:
    """Measured sliders win; the VLM may add a slider the measurements left at 0 when it asks for a
    clearly visible change (|v| >= deadband, capped), which drops its habit of filler values."""
    out: dict[str, Any] = dict(vlm)
    for name in list(out):
        if name == "source":
            continue
        if name in baseline:
            out[name] = baseline[name]
            continue
        v = out[name]
        lim = cap * (50 if name == "temp" else 1) if name != "exposure" else 0.0
        small = abs(v) < (deadband * (20 if name == "temp" else 1))
        if name == "exposure" or small:
            out[name] = 0.0 if name == "exposure" else 0
        else:
            out[name] = max(-lim, min(lim, v))
    return out
