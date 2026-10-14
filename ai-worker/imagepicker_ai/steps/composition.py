"""Saliency box for the composition score (docs/03 §3.4).

The composition score itself (rule of thirds, headroom, horizon, edge cut-off) is computed by the
core from the stored boxes (`crates/ip-core/src/analysis/composition.rs`); the worker contributes
the one thing only the pixels give: where the visual subject is when no face anchors it.

`saliency_box` = centre-weighted global colour contrast on a 32 x 32 grid: each cell's Lab
distance to the image's mean colour, times a centre prior (1 at the centre, 0.5 at the corners);
the box spans the cells whose saliency is at least half of the maximum, trimmed to the 5th - 95th
percentile of the saliency mass along each axis so a lone bright pixel cannot stretch it.
Returns normalised [x, y, w, h]; None for a flat image (no contrast, nothing to anchor on).
"""

from __future__ import annotations

import cv2
import numpy as np

GRID = 32
MIN_CONTRAST = 4.0  # max Lab distance below this: flat image
KEEP = 0.5  # cells at least this share of the maximum belong to the subject


def _span(mass: np.ndarray) -> tuple[float, float]:
    """5th - 95th percentile of a 1-D mass distribution, in 0-1 grid coordinates."""
    c = np.cumsum(mass)
    if c[-1] <= 0:
        return 0.0, 1.0
    c = c / c[-1]
    n = len(mass)
    lo = int(np.searchsorted(c, 0.05))
    hi = int(np.searchsorted(c, 0.95))
    return lo / n, min(n, hi + 1) / n


def saliency_box(rgb: np.ndarray) -> list[float] | None:
    small = cv2.resize(rgb, (GRID, GRID), interpolation=cv2.INTER_AREA)
    lab = cv2.cvtColor(small, cv2.COLOR_RGB2LAB).astype(np.float32)
    dist = np.linalg.norm(lab - lab.reshape(-1, 3).mean(axis=0), axis=2)
    if float(dist.max()) < MIN_CONTRAST:
        return None
    ys, xs = (np.arange(GRID) + 0.5) / GRID - 0.5, (np.arange(GRID) + 0.5) / GRID - 0.5
    d = np.sqrt(ys[:, None] ** 2 + xs[None, :] ** 2) / np.sqrt(0.5)
    sal = dist * (1.0 - 0.5 * d)
    sal = np.where(sal >= KEEP * sal.max(), sal, 0.0)
    x0, x1 = _span(sal.sum(axis=0))
    y0, y1 = _span(sal.sum(axis=1))
    return [round(x0, 4), round(y0, 4), round(x1 - x0, 4), round(y1 - y0, 4)]
