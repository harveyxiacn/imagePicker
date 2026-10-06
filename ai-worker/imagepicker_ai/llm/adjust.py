"""`Adjust` fields the VLM may suggest (ranges from crates/ip-render/src/lib.rs `struct Adjust`).

Only the global scalar sliders are suggested; `curve`, `hsl`, `grading` stay at their defaults
(absent) and `source` is set to `ai_vlm@1`.
"""

from __future__ import annotations

import math
from typing import Any

# name -> (min, max, decimals)
ADJUST_RANGES: dict[str, tuple[float, float, int]] = {
    "exposure": (-5.0, 5.0, 2),  # EV stops
    "contrast": (-100.0, 100.0, 0),
    "highlights": (-100.0, 100.0, 0),
    "shadows": (-100.0, 100.0, 0),
    "whites": (-100.0, 100.0, 0),
    "blacks": (-100.0, 100.0, 0),
    "temp": (-3000.0, 3000.0, 0),  # Kelvin relative to as-shot, + = warmer
    "tint": (-100.0, 100.0, 0),  # green(-) .. magenta(+)
    "vibrance": (-100.0, 100.0, 0),
    "saturation": (-100.0, 100.0, 0),
    "clarity": (-100.0, 100.0, 0),
    "dehaze": (-100.0, 100.0, 0),
}
# Range the model is asked to stay in (well inside the hard clamp: a suggestion, not a rewrite).
SUGGEST_LIMITS: dict[str, tuple[float, float]] = {
    "exposure": (-3.0, 3.0),
    "temp": (-1500.0, 1500.0),
}
SOURCE = "ai_vlm@1"


def _num(v: Any) -> float:
    if isinstance(v, bool):
        return 0.0
    if isinstance(v, (int, float)):
        f = float(v)
    elif isinstance(v, str):
        try:
            f = float(v.strip().replace("EV", "").replace("ev", "").replace("K", ""))
        except ValueError:
            return 0.0
    else:
        return 0.0
    return f if math.isfinite(f) else 0.0


def clamp_adjust(raw: Any) -> dict[str, Any]:
    """Every field of ADJUST_RANGES, clamped and rounded; unknown keys dropped, bad values -> 0."""
    src = raw if isinstance(raw, dict) else {}
    out: dict[str, Any] = {}
    for name, (lo, hi, nd) in ADJUST_RANGES.items():
        v = min(hi, max(lo, _num(src.get(name))))
        v = round(v, nd)
        out[name] = int(v) if nd == 0 else float(v)
    out["source"] = SOURCE
    return out


def adjust_schema() -> dict[str, Any]:
    """Grammar schema of the `adjust` object (all fields required, numeric ranges)."""
    props: dict[str, Any] = {}
    for name, (lo, hi, nd) in ADJUST_RANGES.items():
        lo, hi = SUGGEST_LIMITS.get(name, (lo, hi))
        if nd == 0:
            props[name] = {"type": "integer", "minimum": int(lo), "maximum": int(hi)}
        else:
            props[name] = {"type": "number", "minimum": lo, "maximum": hi}
    return {
        "type": "object",
        "properties": props,
        "required": list(props),
        "additionalProperties": False,
    }
