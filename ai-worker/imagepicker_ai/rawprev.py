"""Embedded JPEG previews of camera RAW files, for the analysis decode.

Pillow cannot demosaic RAW files: on TIFF-based ones (NEF, CR2, ARW, DNG, ...) it reads IFD0,
which is often a 160 x 120 thumbnail or an old-style JPEG it cannot decode, and it does not open
CR3 / RAF at all. Every camera stores a JPEG preview of the full frame next to the sensor data
(1-2 MP, often full size), which is what the Core's thumbnails, previews and the lite analysis
already use (`crates/ip-imaging/src/imp/container.rs`, `raw_preview`); this is a port of that
selection, so the AI steps see the same pixels as the rest of the app:

* TIFF-based RAW: JPEG streams referenced from IFD0..n, the Exif IFD and SubIFDs
  (JPEGInterchangeFormat / Length, old-style and new-style JPEG strips, Panasonic's 0x002E
  blob); when the best one is under 1000 px a byte scan of the first 12 MiB for JPEG streams;
* CR3 (ISO BMFF): the PRVW / THMB JPEGs inside the `moov` uuid boxes;
* RAF: the JPEG the header points to, else a scan of the first 4 MiB.

The largest decodable (non-lossless) candidate by area wins. The file is memory-mapped; only
the chosen JPEG is copied.
"""

from __future__ import annotations

import mmap
import struct
from pathlib import Path

# the extensions crates/ip-imaging treats as RAW (`ImageFormat::from_path`)
RAW_EXTENSIONS = frozenset(
    {".cr2", ".cr3", ".nef", ".nrw", ".arw", ".srf", ".sr2", ".raf", ".orf", ".rw2", ".dng"}
    | {".pef", ".srw", ".3fr", ".iiq", ".x3f"}
)

_TAG_COMPRESSION = 0x0103
_TAG_STRIP_OFFSETS = 0x0111
_TAG_ORIENTATION = 0x0112
_TAG_STRIP_BYTES = 0x0117
_TAG_SUB_IFDS = 0x014A
_TAG_JPEG_OFFSET = 0x0201
_TAG_JPEG_LEN = 0x0202
_TAG_EXIF_IFD = 0x8769
_TAG_GPS_IFD = 0x8825
_TAG_RW2_JPG = 0x002E
_TYPE_SIZE = {1: 1, 2: 1, 6: 1, 7: 1, 3: 2, 8: 2, 4: 4, 9: 4, 11: 4, 13: 4, 5: 8, 10: 8, 12: 8}
_SMALL = 1000  # px: a TIFF preview below this long edge triggers the byte scan
_SCAN_SPAN = 32 << 20  # bytes copied for a scanned JPEG (full-size previews are < 20 MB)


def is_raw(path: str | Path) -> bool:
    return Path(path).suffix.lower() in RAW_EXTENSIONS


def jpeg_dims(data: bytes | memoryview | mmap.mmap, start: int = 0, end: int | None = None):
    """(width, height) of a decodable (non-lossless) JPEG starting at `start`, else None."""
    end = len(data) if end is None else min(end, len(data))
    if end - start < 4 or data[start] != 0xFF or data[start + 1] != 0xD8:
        return None
    p = start + 2
    while p + 4 <= end:
        if data[p] != 0xFF:
            p += 1
            continue
        m = data[p + 1]
        if m == 0xFF:
            p += 1
            continue
        if m in (0x00, 0x01) or 0xD0 <= m <= 0xD8:
            p += 2
            continue
        if m in (0xDA, 0xD9):
            return None  # no SOF before the scan
        seg_len = (data[p + 2] << 8) | data[p + 3]
        if seg_len < 2:
            return None
        if 0xC0 <= m <= 0xCF and m not in (0xC4, 0xC8, 0xCC):
            if m == 0xC3 or p + 9 > end:  # lossless (raw sensor data in CR2 / DNG)
                return None
            h = (data[p + 5] << 8) | data[p + 6]
            w = (data[p + 7] << 8) | data[p + 8]
            return (w, h) if w > 0 and h > 0 else None
        p += 2 + seg_len
    return None


class _Tiff:
    def __init__(self, data: mmap.mmap | bytes, base: int = 0, end: int | None = None):
        self.d, self.base = data, base
        self.end = len(data) if end is None else end
        bo = bytes(data[base : base + 2])
        if bo not in (b"II", b"MM"):
            raise ValueError("not TIFF")
        self.e = "<" if bo == b"II" else ">"
        self.magic = self.u16(2)
        if self.magic not in (42, 0x4F52, 0x5352, 0x55):
            raise ValueError("not TIFF")

    def u16(self, off: int) -> int:
        p = self.base + off
        if off < 0 or p + 2 > self.end:
            raise ValueError("out of range")
        return struct.unpack_from(self.e + "H", self.d, p)[0]

    def u32(self, off: int) -> int:
        p = self.base + off
        if off < 0 or p + 4 > self.end:
            raise ValueError("out of range")
        return struct.unpack_from(self.e + "I", self.d, p)[0]

    def ifd(self, off: int) -> tuple[dict[int, tuple[int, int, int]], int]:
        """{tag: (type, count, value offset relative to the TIFF start)}, next IFD offset."""
        n = self.u16(off)
        if n > 4096:
            raise ValueError("bad IFD")
        out: dict[int, tuple[int, int, int]] = {}
        for i in range(n):
            p = off + 2 + 12 * i
            tag, typ, count = self.u16(p), self.u16(p + 2), self.u32(p + 4)
            size = _TYPE_SIZE.get(typ, 0) * count
            pos = p + 8 if size <= 4 else self.u32(p + 8)
            if size and self.base + pos + size > self.end:
                continue
            out.setdefault(tag, (typ, count, pos))
        try:
            nxt = self.u32(off + 2 + 12 * n)
        except ValueError:
            nxt = 0
        return out, nxt

    def uint(self, e: tuple[int, int, int], i: int = 0) -> int | None:
        typ, count, pos = e
        if i >= count:
            return None
        if typ in (1, 7):
            return self.d[self.base + pos + i]
        if typ == 3:
            return self.u16(pos + 2 * i)
        if typ in (4, 13):
            return self.u32(pos + 4 * i)
        return None

    def walk(self) -> list[dict[int, tuple[int, int, int]]]:
        """IFD0..n (at most 8), then the Exif / GPS / Sub IFDs below them (depth <= 3)."""
        seen: set[int] = set()
        chain: list[dict[int, tuple[int, int, int]]] = []
        off = self.u32(4)
        while off and off not in seen and len(chain) < 8:
            seen.add(off)
            try:
                ifd, off = self.ifd(off)
            except ValueError:
                break
            chain.append(ifd)
        out = list(chain)

        def children(ifd: dict[int, tuple[int, int, int]], depth: int) -> None:
            if depth > 3:
                return
            for tag in (_TAG_EXIF_IFD, _TAG_GPS_IFD, _TAG_SUB_IFDS):
                e = ifd.get(tag)
                if e is None:
                    continue
                for i in range(min(e[1], 8)):
                    try:
                        o = self.uint(e, i)
                        if not o or o in seen:
                            continue
                        seen.add(o)
                        child, _ = self.ifd(o)
                    except ValueError:
                        continue
                    out.append(child)
                    children(child, depth + 1)

        for ifd in chain:
            children(ifd, 0)
        return out


def _tiff_candidates(mm: mmap.mmap) -> tuple[list[tuple[int, int]], int | None]:
    """JPEG (offset, end) ranges referenced from the IFDs, and IFD0's orientation."""
    try:
        t = _Tiff(mm)
        ifds = t.walk()
    except (ValueError, struct.error):
        return [], None
    out: list[tuple[int, int]] = []
    orient = None
    for k, ifd in enumerate(ifds):
        try:
            if k == 0 and _TAG_ORIENTATION in ifd:
                orient = t.uint(ifd[_TAG_ORIENTATION])
            if _TAG_JPEG_OFFSET in ifd and _TAG_JPEG_LEN in ifd:
                o, n = t.uint(ifd[_TAG_JPEG_OFFSET]), t.uint(ifd[_TAG_JPEG_LEN])
                if o is not None and n:
                    out.append((o, o + n))
            comp = t.uint(ifd[_TAG_COMPRESSION]) if _TAG_COMPRESSION in ifd else None
            if comp in (6, 7) and _TAG_STRIP_OFFSETS in ifd and _TAG_STRIP_BYTES in ifd:
                o, n = t.uint(ifd[_TAG_STRIP_OFFSETS]), t.uint(ifd[_TAG_STRIP_BYTES])
                if o is not None and n:
                    out.append((o, o + n))
            e = ifd.get(_TAG_RW2_JPG)
            if e is not None and e[1] > 4:  # Panasonic: UNDEFINED blob, count = JPEG length
                out.append((e[2], e[2] + e[1]))
        except (ValueError, struct.error):
            continue
    return [(a, min(b, len(mm))) for a, b in out if 0 <= a < len(mm)], orient


def _scan_candidates(mm: mmap.mmap, limit: int) -> list[tuple[int, int]]:
    """JPEG starts (FF D8 FF Cx..) in the first `limit` bytes; each runs to the end of the file
    (at most _SCAN_SPAN bytes: the decoder stops at the JPEG's end marker anyway)."""
    end = min(len(mm), limit)
    out: list[tuple[int, int]] = []
    i = 0
    while len(out) < 16:
        i = mm.find(b"\xff\xd8\xff", i, end)
        if i < 0 or i + 4 > end:
            break
        if mm[i + 3] >= 0xC0:
            out.append((i, min(len(mm), i + _SCAN_SPAN)))
        i += 3
    return out


def _boxes(mm: mmap.mmap, start: int, end: int) -> list[tuple[bytes, int, int]]:
    """ISO BMFF boxes in [start, end): (type, payload start, payload end)."""
    out = []
    p = start
    while p + 8 <= end:
        size = struct.unpack_from(">I", mm, p)[0]
        typ = bytes(mm[p + 4 : p + 8])
        hdr = 8
        if size == 0:
            size = end - p
        elif size == 1:
            if p + 16 > end:
                break
            size, hdr = struct.unpack_from(">Q", mm, p + 8)[0], 16
        if size < hdr:
            break
        if p + size > end:
            out.append((typ, p + hdr, end))  # truncated trailing box: take what is there
            break
        out.append((typ, p + hdr, p + size))
        p += size
    return out


def _cr3_candidates(mm: mmap.mmap) -> tuple[list[tuple[int, int]], int | None]:
    out: list[tuple[int, int]] = []
    orient = None

    def children(a: int, b: int) -> None:
        nonlocal orient
        boxes = _boxes(mm, a, b)
        plausible = boxes and all(c == 32 or chr(c).isalnum() for c in boxes[0][0])
        if not plausible and b - a > 8:
            boxes = _boxes(mm, a + 8, b)
        for typ, s, e in boxes:
            if typ in (b"PRVW", b"THMB"):
                k = mm.find(b"\xff\xd8\xff", s, min(e, s + 64))
                if k >= 0:
                    out.append((k, e))
            elif typ == b"CMT1" and orient is None:  # IFD0 as a TIFF
                try:
                    t = _Tiff(mm, s, e)
                    ifd, _ = t.ifd(t.u32(4))
                    if _TAG_ORIENTATION in ifd:
                        orient = t.uint(ifd[_TAG_ORIENTATION])
                except (ValueError, struct.error):
                    pass

    for typ, s, e in _boxes(mm, 0, len(mm)):
        if typ == b"moov":
            for t2, s2, e2 in _boxes(mm, s, e):
                if t2 == b"uuid" and e2 - s2 > 16:
                    children(s2 + 16, e2)
                    for t3, s3, e3 in _boxes(mm, s2 + 16, e2):
                        if t3 == b"uuid" and e3 - s3 > 16:
                            children(s3 + 16, e3)
        elif typ == b"uuid" and e - s > 16:
            children(s + 16, e)
    return out, orient


def _best(mm: mmap.mmap, cands: list[tuple[int, int]]):
    best = None
    for a, b in cands:
        d = jpeg_dims(mm, a, b)
        if d and (best is None or d[0] * d[1] > best[2][0] * best[2][1]):
            best = (a, b, d)
    return best


def raw_preview(path: str | Path) -> tuple[bytes, tuple[int, int], int | None] | None:
    """(JPEG bytes, (w, h), orientation of the RAW's IFD0 or None) of the largest embedded
    preview, or None when the file has none (or is not a RAW container we know)."""
    with open(path, "rb") as f:
        try:
            mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        except ValueError:  # empty file
            return None
    try:
        orient = None
        if mm[:15] == b"FUJIFILMCCD-RAW":
            best = None
            if len(mm) >= 92:
                off, n = struct.unpack_from(">II", mm, 84)
                if off + n <= len(mm):
                    best = _best(mm, [(off, off + n)])
            best = best or _best(mm, _scan_candidates(mm, 4 << 20))
        elif len(mm) > 12 and mm[4:8] == b"ftyp":
            cands, orient = _cr3_candidates(mm)
            best = _best(mm, cands)
        else:
            cands, orient = _tiff_candidates(mm)
            best = _best(mm, cands)
            if best is None or max(best[2]) < _SMALL:
                scanned = _best(mm, _scan_candidates(mm, 12 << 20))
                if scanned and (best is None or _area(scanned) > _area(best)):
                    best = scanned
        if best is None:
            return None
        a, b, dims = best
        return bytes(mm[a:b]), dims, orient if orient in range(1, 9) else None
    finally:
        mm.close()


def _area(c: tuple[int, int, tuple[int, int]]) -> int:
    return c[2][0] * c[2][1]
