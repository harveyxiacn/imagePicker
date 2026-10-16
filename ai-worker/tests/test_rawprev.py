"""RAW files are analysed from their embedded JPEG preview (`rawprev`), on synthetic containers
shaped like NEF / CR2 (TIFF with SubIFDs), CR3 (ISO BMFF) and RAF."""

import io
import struct

import numpy as np
from PIL import Image

from imagepicker_ai.decode import load_image
from imagepicker_ai.rawprev import is_raw, jpeg_dims, raw_preview

from .conftest import synth_image


def _jpeg(w: int, h: int, seed: int = 1) -> bytes:
    buf = io.BytesIO()
    Image.fromarray(synth_image(seed, (w, h))).save(buf, "JPEG", quality=90)
    return buf.getvalue()


# a lossless (SOF3) stream like the raw sensor data of CR2 / DNG: large, but never a preview
LOSSLESS = b"\xff\xd8\xff\xc3\x00\x0b\x08" + struct.pack(">HH", 4000, 6000) + b"\x01\x01\x11\x00"


def _tiff_raw(preview: bytes, orientation: int = 1) -> bytes:
    """Little-endian TIFF: IFD0 = 160x120 RGB thumbnail (+ orientation), SubIFD 1 = the JPEG
    preview (JPEGInterchangeFormat), SubIFD 2 = a lossless 'sensor' strip."""
    thumb = synth_image(9, (160, 120)).tobytes()
    ifd0_n, sub_n = 11, 3
    ifd0_off = 8
    sub1_off = ifd0_off + 2 + 12 * ifd0_n + 4
    sub2_off = sub1_off + 2 + 12 * sub_n + 4
    subs_arr = sub2_off + 2 + 12 * sub_n + 4  # two LONG offsets (count 2 > 4 bytes)
    bps_arr = subs_arr + 8
    thumb_off = bps_arr + 6
    prev_off = thumb_off + len(thumb)
    raw_off = prev_off + len(preview)

    def ifd(entries, nxt=0):
        out = struct.pack("<H", len(entries))
        for tag, typ, count, val in sorted(entries):
            out += struct.pack("<HHI", tag, typ, count)
            out += struct.pack("<HH", val, 0) if typ == 3 and count == 1 else struct.pack("<I", val)
        return out + struct.pack("<I", nxt)

    ifd0 = ifd(
        [
            (0x00FE, 4, 1, 1),
            (0x0100, 3, 1, 160),
            (0x0101, 3, 1, 120),
            (0x0102, 3, 3, bps_arr),
            (0x0103, 3, 1, 1),
            (0x0106, 3, 1, 2),
            (0x0111, 4, 1, thumb_off),
            (0x0112, 3, 1, orientation),
            (0x0115, 3, 1, 3),
            (0x0117, 4, 1, len(thumb)),
            (0x014A, 4, 2, subs_arr),
        ]
    )
    sub1 = ifd([(0x0103, 3, 1, 6), (0x0201, 4, 1, prev_off), (0x0202, 4, 1, len(preview))])
    sub2 = ifd([(0x0103, 3, 1, 7), (0x0111, 4, 1, raw_off), (0x0117, 4, 1, len(LOSSLESS))])
    data = b"II*\x00" + struct.pack("<I", ifd0_off) + ifd0 + sub1 + sub2
    assert len(data) == subs_arr
    data += struct.pack("<II", sub1_off, sub2_off) + struct.pack("<HHH", 8, 8, 8)
    assert len(data) == thumb_off
    return data + thumb + preview + LOSSLESS


def _box(typ: bytes, payload: bytes) -> bytes:
    return struct.pack(">I", 8 + len(payload)) + typ + payload


def _cr3(preview: bytes, thumb: bytes, orientation: int) -> bytes:
    cmt1 = b"II*\x00" + struct.pack("<I", 8) + struct.pack("<H", 1)
    cmt1 += struct.pack("<HHIHH", 0x0112, 3, 1, orientation, 0) + struct.pack("<I", 0)
    uuid_body = b"\x85\xc0\xb6\x87\x82\x0f\x11\xe0\x81\x11\xf4\xce\x46\x2b\x6a\x48"
    uuid_body += _box(b"CMT1", cmt1) + _box(b"THMB", b"\x00" * 16 + thumb)
    moov = _box(b"uuid", uuid_body)
    prvw_uuid = b"\xea\xf4\x2b\x5e\x1c\x98\x4b\x88\xb9\xfb\xb7\xdc\x40\x6e\x4d\x16"
    prvw_uuid += b"\x00" * 8 + _box(b"PRVW", b"\x00" * 14 + preview)
    return _box(b"ftyp", b"crx " + b"\x00" * 4) + _box(b"moov", moov) + _box(b"uuid", prvw_uuid)


def test_tiff_raw_uses_the_largest_preview_and_the_raw_orientation(tmp_path):
    prev = _jpeg(2000, 1500)
    p = tmp_path / "DSC_0001.NEF"
    p.write_bytes(_tiff_raw(prev, orientation=6))
    with Image.open(p) as im:  # what Pillow alone sees: the IFD0 thumbnail
        assert max(im.size) == 160
    data, dims, orient = raw_preview(p)
    assert data == prev and dims == (2000, 1500) and orient == 6
    d = load_image(str(p), 1024)
    assert (d.orig_width, d.orig_height) == (1500, 2000)  # rotated by the RAW's own tag
    assert d.rgb.shape == (1024, 768, 3)
    # same pixels as decoding the preview JPEG itself
    ref = tmp_path / "prev.jpg"
    ref.write_bytes(prev)
    np.testing.assert_array_equal(d.rgb, load_image(str(ref), 1024, orientation_hint=6).rgb)


def test_cr3_preview_and_orientation(tmp_path):
    prev, thumb = _jpeg(1620, 1080, 2), _jpeg(160, 120, 3)
    p = tmp_path / "IMG_0001.CR3"
    p.write_bytes(_cr3(prev, thumb, 8))
    data, dims, orient = raw_preview(p)
    assert data == prev and dims == (1620, 1080) and orient == 8
    d = load_image(str(p), 1024)
    assert d.rgb.shape == (1024, 683, 3)


def test_raf_header_preview(tmp_path):
    prev = _jpeg(1920, 1280, 4)
    head = b"FUJIFILMCCD-RAW 0201FF383501".ljust(84, b"\x00")
    head += struct.pack(">II", 100, len(prev))
    p = tmp_path / "DSCF0001.RAF"
    p.write_bytes(head.ljust(100, b"\x00") + prev + b"\x00" * 1000)
    data, dims, orient = raw_preview(p)
    assert data == prev and dims == (1920, 1280) and orient is None
    assert load_image(str(p), 1024, orientation_hint=6).rgb.shape == (1024, 683, 3)


def test_scan_finds_a_preview_the_ifds_do_not_list(tmp_path):
    prev = _jpeg(1600, 1200, 5)
    tiff = _tiff_raw(_jpeg(320, 240, 6))  # IFDs only point at a small JPEG
    p = tmp_path / "x.orf"
    p.write_bytes(tiff + b"\x00" * 64 + prev)
    data, dims, _ = raw_preview(p)
    assert dims == (1600, 1200) and data.startswith(prev)


def test_raw_without_preview_falls_back_to_pillow(tmp_path):
    p = tmp_path / "plain.dng"
    Image.fromarray(synth_image(7, (300, 200))).save(p, "TIFF")
    assert raw_preview(p) is None
    assert load_image(str(p), 1024).rgb.shape == (200, 300, 3)


def test_only_raw_extensions_take_the_preview_path():
    assert is_raw("a.CR3") and is_raw("b.nef") and is_raw("c.dng")
    assert not is_raw("a.jpg") and not is_raw("b.heic") and not is_raw("c.tif")
    assert jpeg_dims(LOSSLESS) is None
    assert jpeg_dims(_jpeg(64, 48)) == (64, 48)
