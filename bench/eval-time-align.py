"""Score the multi-device time alignment (docs/03 section 2.4, docs/09 P1-2) on an analysed library.

Usage:
    python bench/eval-time-align.py DATA_DIR [--session ID] [--trials 60] [--min-photos 30]
                                    [--offsets 4s,45s,7m13s,1h2m5s,-3h11m,11h59m] [--seed 1]
                                    [--json OUT.json]

DATA_DIR is the --data-dir of a catalog that holds an analysed session with photos of two or more
devices (cameras / phones) that shot the SAME scenes, e.g. the owner's own library after
`imagepicker analyze <dir> --profile standard --data-dir DATA_DIR`. The catalog is opened read-only
and no file is written.

The acceptance metric is "error <= 5 s on a whole-device-offset sample". The real clocks of the
devices are not known, so the sample is built by shifting: one device's capture times are moved by
a known offset s (in memory only) and the estimator has to find s on top of whatever offset the
devices already had. For every trial

    error = (estimate_shifted - estimate_unshifted) + s

and the report gives, per offset size and overall, the share of trials with |error| <= 5 s, the
median / p95 / max error and the number of trials without an estimate. The unshifted estimates
(the real clock differences of the library) and the GPS-time evidence are listed first.

The estimator below is a line-for-line port of `crates/ip-core/src/analysis/time_align.rs`
(`estimate`, `pair_deltas`, `find_peak`, `gps_anchor`; same thresholds). Needs numpy; standard
library otherwise. Run it on the GPU PC next to the analysed catalog; it is not part of CI.
"""

import argparse
import json
import random
import re
import sqlite3
import statistics
import sys
from pathlib import Path

import numpy as np

# AlignParams::default()
MAX_OFFSET_MS = 24 * 3_600_000
COS_MIN = 0.90
HAMMING_MAX = 8
COARSE_MS = 60_000
FINE_MS = 10_000
MIN_SUPPORT = 3
MAX_PER_DEVICE = 1_500
GPS_MIN = 3
GPS_SPREAD_MS = 5_000
ACCEPT_MIN_CONFIDENCE = 0.5
TOLERANCE_MS = 5_000

DEFAULT_OFFSETS = "4s,-9s,45s,-90s,7m13s,-26m,1h2m5s,-3h11m,11h59m,-23h"


def parse_offset(text: str) -> int:
    """`1h2m5s`, `-90s`, `+7m` -> ms."""
    m = re.fullmatch(r"([+-]?)(?:(\d+)h)?(?:(\d+)m)?(?:(\d+(?:\.\d+)?)s)?", text.strip().lower())
    if not m or not any(m.groups()[1:]):
        raise ValueError(f"bad offset {text!r}")
    sec = int(m[2] or 0) * 3600 + int(m[3] or 0) * 60 + float(m[4] or 0)
    return int(round(sec * 1000)) * (-1 if m[1] == "-" else 1)


def fmt_offset(ms: int) -> str:
    total = int(round(ms / 1000))
    sign = "-" if total < 0 else "+"
    total = abs(total)
    h, rest = divmod(total, 3600)
    m, s = divmod(rest, 60)
    return f"{sign}{h}h{m:02d}m{s:02d}s" if h else f"{sign}{m}m{s:02d}s" if m else f"{sign}{s}s"


# ---------------------------------------------------------------- data


class Photo:
    __slots__ = ("device", "t", "gps_delta", "phash", "emb")

    def __init__(self, device, t, gps_delta, phash, emb):
        self.device, self.t, self.gps_delta, self.phash, self.emb = device, t, gps_delta, phash, emb


def decode_f16(blob):
    return np.frombuffer(blob, dtype="<f2").astype(np.float32)


def load(data_dir: Path, session: int | None) -> tuple[list[Photo], dict[int, str]]:
    db = sqlite3.connect(f"file:{data_dir / 'catalog.db'}?mode=ro", uri=True)
    db.row_factory = sqlite3.Row
    cols = {r["name"] for r in db.execute("PRAGMA table_info(photo)")}
    gps = (
        "CASE WHEN p.gps_time_ms IS NOT NULL AND p.taken_at_offset_min IS NOT NULL"
        " THEN p.gps_time_ms - p.taken_at END"
        if "gps_time_ms" in cols
        else "NULL"
    )
    where = "WHERE p.device_id IS NOT NULL AND p.taken_at IS NOT NULL AND COALESCE(p.missing,0)=0"
    join = ""
    args: tuple = ()
    if session is not None:
        join = "JOIN session_photo sp ON sp.photo_id=p.id AND sp.session_id=?"
        args = (session,)
    rows = db.execute(
        f"SELECT p.device_id, p.taken_at, {gps} AS gps_delta, a.phash, a.embedding"
        f" FROM photo p {join} LEFT JOIN analysis a ON a.photo_id=p.id {where}",
        args,
    ).fetchall()
    photos = []
    for r in rows:
        emb = decode_f16(r["embedding"]) if r["embedding"] else None
        ph = int.from_bytes(r["phash"], "big") if r["phash"] and len(r["phash"]) == 8 else None
        photos.append(Photo(r["device_id"], r["taken_at"], r["gps_delta"], ph, emb))
    names = {
        r["id"]: (r["name"] or f"{r['make']} {r['model']}".strip())
        for r in db.execute("SELECT id, name, make, model FROM device")
    }
    return photos, names


# ---------------------------------------------------------------- estimator (port)


def median(v: list[int]) -> int:
    v = sorted(v)
    n = len(v)
    return v[n // 2] if n % 2 else v[n // 2 - 1] + (v[n // 2] - v[n // 2 - 1]) // 2


def thin(ph: list[Photo]) -> list[Photo]:
    ph = sorted(ph, key=lambda p: p.t)
    n = len(ph)
    if n <= MAX_PER_DEVICE:
        return ph
    return [ph[i * n // MAX_PER_DEVICE] for i in range(MAX_PER_DEVICE)]


def popcount64(x: np.ndarray) -> np.ndarray:
    return np.unpackbits(x.view(np.uint8).reshape(-1, 8), axis=1).sum(axis=1)


def sim_matrix(a: list[Photo], b: list[Photo]) -> np.ndarray:
    """Similarity of every a x b pair, -1 where the pair is not a visual match."""
    n, m = len(a), len(b)
    sim = np.full((n, m), -1.0, dtype=np.float32)
    ea = [i for i, p in enumerate(a) if p.emb is not None]
    eb = [j for j, p in enumerate(b) if p.emb is not None]
    if ea and eb and len({a[i].emb.shape[0] for i in ea} | {b[j].emb.shape[0] for j in eb}) == 1:
        A = np.stack([a[i].emb for i in ea])
        B = np.stack([b[j].emb for j in eb])
        A /= np.maximum(np.linalg.norm(A, axis=1, keepdims=True), 1e-9)
        B /= np.maximum(np.linalg.norm(B, axis=1, keepdims=True), 1e-9)
        c = A @ B.T
        c[c < COS_MIN] = -1.0
        sim[np.ix_(ea, eb)] = c
    # pairs without two embeddings: pHash
    ha = {i: p.phash for i, p in enumerate(a) if p.phash is not None and p.emb is None}
    hb = {j: p.phash for j, p in enumerate(b) if p.phash is not None}
    if ha and hb:
        ia = np.array(list(ha), dtype=np.int64)
        ib = np.array(list(hb), dtype=np.int64)
        xa = np.array([ha[i] for i in ia], dtype=np.uint64)
        xb = np.array([hb[j] for j in ib], dtype=np.uint64)
        for k, i in enumerate(ia):
            d = popcount64(xb ^ xa[k])
            ok = d <= HAMMING_MAX
            sim[i, ib[ok]] = 1.0 - d[ok] / 64.0
    for j, p in enumerate(b):  # embedding on b only, phash on a only: phash path
        if p.emb is not None and p.phash is not None:
            for i, q in enumerate(a):
                if q.emb is None and q.phash is not None and sim[i, j] < 0:
                    d = bin(q.phash ^ p.phash).count("1")
                    if d <= HAMMING_MAX:
                        sim[i, j] = 1.0 - d / 64.0
    return sim


def pair_deltas(a: list[Photo], b: list[Photo]) -> list[int]:
    ta = np.array([p.t for p in a], dtype=np.int64)
    tb = np.array([p.t for p in b], dtype=np.int64)
    sim = sim_matrix(a, b)
    sim[np.abs(ta[:, None] - tb[None, :]) > MAX_OFFSET_MS] = -1.0
    ok = sim.max(axis=1) >= 0
    ba = sim.argmax(axis=1)  # first maximum wins, like the Rust loop
    bb = sim.argmax(axis=0)
    out = []
    for i in np.nonzero(ok)[0]:
        j = ba[i]
        if bb[j] == i and sim[i, j] >= 0:
            out.append(int(ta[i] - tb[j]))
    return out


def find_peak(deltas: list[int]):
    """(delta, support, confidence) of the densest cluster, or None."""
    n = len(deltas)
    if n < MIN_SUPPORT:
        return None
    d = sorted(deltas)
    best, best_lo, hi = 0, 0, 0
    for lo in range(n):
        while hi < n and d[hi] - d[lo] <= 2 * COARSE_MS:
            hi += 1
        if hi - lo > best:
            best, best_lo = hi - lo, lo
    centre = median(d[best_lo : best_lo + best])
    for _ in range(4):
        near = [x for x in d if abs(x - centre) <= FINE_MS]
        if not near:
            return None
        c = median(near)
        if c == centre:
            break
        centre = c
    support = sum(1 for x in d if abs(x - centre) <= FINE_MS)
    if support < MIN_SUPPORT:
        return None
    chance = n * (2 * FINE_MS) / (2 * MAX_OFFSET_MS)
    significance = min(1.0, max(0.0, 1.0 - chance / support))
    if significance < 0.5:
        return None
    frac = support / n
    conf = significance * (support / (support + 4.0)) * (0.4 + 0.6 * frac)
    return centre, support, conf


def gps_anchor(ph: list[Photo]):
    deltas = [p.gps_delta for p in ph if p.gps_delta is not None]
    if len(deltas) < GPS_MIN:
        return None
    m = median(deltas)
    near = [x for x in deltas if abs(x - m) <= GPS_SPREAD_MS]
    if len(near) < GPS_MIN or len(near) * 4 < len(deltas) * 3:
        return None
    frac = len(near) / len(deltas)
    return median(near), frac * (len(near) / (len(near) + 2.0)), len(near)


def estimate(photos: list[Photo]) -> dict[int, dict]:
    by_dev: dict[int, list[Photo]] = {}
    for p in photos:
        by_dev.setdefault(p.device, []).append(p)
    est: dict[int, dict] = {}
    for dev, ph in by_dev.items():
        g = gps_anchor(ph)
        if g:
            est[dev] = dict(offset=g[0], confidence=g[1], evidence=g[2], source="gps")
    if not est and by_dev:
        dev = max(by_dev, key=lambda d: (len(by_dev[d]), -d))
        est[dev] = dict(offset=0, confidence=1.0, evidence=0, source="reference")
    thinned = {d: thin(v) for d, v in by_dev.items()}
    cache: dict[tuple[int, int], tuple | None] = {}

    def pair(x: int, y: int):
        lo, hi = min(x, y), max(x, y)
        if (lo, hi) not in cache:
            cache[(lo, hi)] = find_peak(pair_deltas(thinned[lo], thinned[hi]))
        e = cache[(lo, hi)]
        if e is None:
            return None
        return e if x == lo else (-e[0], e[1], e[2])

    while True:
        best = None
        for y in by_dev:
            if y in est:
                continue
            for x, ex in est.items():
                e = pair(x, y)
                if e is None:
                    continue
                conf = min(e[2], ex["confidence"])
                if best is None or conf > best[0]:
                    best = (conf, x, y, e)
        if best is None:
            break
        conf, x, y, e = best
        est[y] = dict(offset=est[x]["offset"] + e[0], confidence=conf, evidence=e[1], source="visual")
    return est


# ---------------------------------------------------------------- evaluation


def percentile(xs: list[float], q: float) -> float:
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(round(q * (len(xs) - 1))))] if xs else float("nan")


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("data_dir", type=Path)
    ap.add_argument("--session", type=int)
    ap.add_argument("--trials", type=int, default=60)
    ap.add_argument("--min-photos", type=int, default=30, help="devices with fewer photos are left out")
    ap.add_argument("--offsets", default=DEFAULT_OFFSETS)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--json", type=Path)
    args = ap.parse_args()

    photos, names = load(args.data_dir, args.session)
    counts: dict[int, int] = {}
    for p in photos:
        counts[p.device] = counts.get(p.device, 0) + 1
    keep = {d for d, n in counts.items() if n >= args.min_photos}
    photos = [p for p in photos if p.device in keep]
    print(f"{len(photos)} photos, {len(keep)} devices with >= {args.min_photos} photos")
    for d in sorted(keep, key=lambda d: -counts[d]):
        g = sum(1 for p in photos if p.device == d and p.gps_delta is not None)
        e = sum(1 for p in photos if p.device == d and p.emb is not None)
        print(f"  device {d:>4} {names.get(d, '?'):<32} {counts[d]:>6} photos, {e} embeddings, {g} with GPS time")
    if len(keep) < 2:
        print("need photos of two or more devices", file=sys.stderr)
        return 2

    base = estimate(photos)
    print("\nestimate on the library as it is (real clock differences):")
    for d in sorted(keep, key=lambda d: -counts[d]):
        e = base.get(d)
        print(
            f"  device {d:>4}: "
            + (f"{fmt_offset(e['offset']):>14}  conf {e['confidence']:.2f}  evidence {e['evidence']:>4}  {e['source']}" if e else "no estimate")
        )

    rng = random.Random(args.seed)
    offsets = [parse_offset(s) for s in args.offsets.split(",")]
    movable = [d for d in keep if d in base]
    if not movable:
        print("no device has an estimate to shift", file=sys.stderr)
        return 2
    trials = []
    for k in range(args.trials):
        dev = rng.choice(movable)
        s = offsets[k % len(offsets)]
        # the device's clock moves by s, so its GPS evidence (gps - exif) moves by -s
        shifted = [
            Photo(p.device, p.t + s, None if p.gps_delta is None else p.gps_delta - s, p.phash, p.emb) if p.device == dev else p
            for p in photos
        ]
        e = estimate(shifted).get(dev)
        err = None if e is None else (e["offset"] - base[dev]["offset"]) + s
        trials.append(dict(device=dev, shift_ms=s, error_ms=err, confidence=None if e is None else e["confidence"], source=None if e is None else e["source"]))
        print(f"  trial {k + 1:>3}/{args.trials} device {dev:>4} shift {fmt_offset(s):>14} -> " + ("no estimate" if err is None else f"error {err / 1000:+.1f}s conf {e['confidence']:.2f} {e['source']}"))

    done = [t for t in trials if t["error_ms"] is not None]
    errs = [abs(t["error_ms"]) for t in done]
    ok = sum(1 for x in errs if x <= TOLERANCE_MS)
    print(f"\n{len(trials)} trials: {len(done)} with an estimate, {len(trials) - len(done)} without")
    print(f"  |error| <= 5 s: {ok}/{len(trials)} = {ok / len(trials):.1%} of all trials, {ok / max(1, len(done)):.1%} of the estimated ones")
    if errs:
        print(f"  |error| median {statistics.median(errs) / 1000:.2f}s  p95 {percentile(errs, 0.95) / 1000:.2f}s  max {max(errs) / 1000:.2f}s")
    confident = [t for t in done if t["confidence"] >= ACCEPT_MIN_CONFIDENCE]
    bad = [t for t in confident if abs(t["error_ms"]) > TOLERANCE_MS]
    print(f"  confidence >= {ACCEPT_MIN_CONFIDENCE} (what 'accept all' takes): {len(confident)} trials, {len(bad)} wrong by more than 5 s")
    print("  by shift size:")
    for lo, hi, label in [(0, 60_000, "< 1 min"), (60_000, 3_600_000, "1 min - 1 h"), (3_600_000, 10**12, ">= 1 h")]:
        sel = [t for t in trials if lo <= abs(t["shift_ms"]) < hi]
        good = [t for t in sel if t["error_ms"] is not None and abs(t["error_ms"]) <= TOLERANCE_MS]
        print(f"    {label:<12} {len(good)}/{len(sel)} within 5 s")
    if args.json:
        args.json.write_text(json.dumps(dict(baseline=base, trials=trials), indent=2, default=str))
    return 0 if ok == len(trials) else 1


if __name__ == "__main__":
    sys.exit(main())
