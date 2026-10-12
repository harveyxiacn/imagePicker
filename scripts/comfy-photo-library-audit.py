"""Verify corpus files/EXIF and measure bursts with imagePicker's own pHash."""
import importlib.util
import json
from collections import Counter, defaultdict
from pathlib import Path
import hashlib
import numpy as np
from PIL import Image, ImageDraw, ImageOps

ROOT = Path(__file__).resolve().parents[1]
DATA = ROOT / "bench/data/qwen-photo-library"
spec = importlib.util.spec_from_file_location("phash", ROOT / "ai-worker/imagepicker_ai/steps/phash.py")
phash = importlib.util.module_from_spec(spec)
spec.loader.exec_module(phash)


def main():
    manifest = json.loads((DATA / "manifest.json").read_text(encoding="utf-8"))
    errors, bursts, samples = [], defaultdict(list), []
    categories = Counter()
    cameras, lenses = set(), set()
    for r in manifest["images"]:
        p = DATA / r["jpeg"]
        try:
            assert hashlib.sha256(p.read_bytes()).hexdigest() == r["sha256"], "hash mismatch"
            with Image.open(p) as im:
                im.load()
                e = im.getexif()
                mode = r["metadata"]["metadata_mode"]
                if mode == "no-exif":
                    assert not e, "unexpected EXIF"
                else:
                    cameras.add(e[272])
                    sub = e.get_ifd(34665)
                    assert sub[36867] and sub[37521] and sub[36881] == "+08:00"
                    if mode == "missing-lens":
                        assert 42036 not in sub
                    else:
                        lenses.add(sub[42036])
                if r["phase"] == "bursts":
                    bursts[r["burst"]].append((r["name"], phash.phash_hex(np.asarray(im.convert("RGB")))))
                if categories[r["phase"]] < (6 if r["phase"] == "bursts" else 3) and r["phase"] != "edge":
                    samples.append((r["name"], ImageOps.contain(im.convert("RGB"), (300, 220))))
            categories[r["phase"]] += 1
        except Exception as exc:
            errors.append({"name": r["name"], "error": str(exc)})
    measured = []
    for name, frames in bursts.items():
        frames.sort()
        distances = [{"a": a[0], "b": b[0], "hamming": phash.hamming(a[1], b[1])}
                     for i, a in enumerate(frames) for b in frames[i+1:]]
        measured.append({"burst": name, "frames": len(frames), "pairwise": distances,
            "max_distance": max((d["hamming"] for d in distances), default=None)})
    report = {"target": 1000, "saved": len(manifest["images"]), "verified": sum(categories.values()),
        "complete": len(manifest["images"]) == 1000 and not errors,
        "categories": categories, "cameras": sorted(cameras), "lenses": sorted(lenses),
        "errors": errors, "burst_measurements": measured,
        "visual_labels": "Unverified prompt intentions; no automatic identity/expression accuracy claim"}
    (DATA / "audit.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    if samples:
        sheet = Image.new("RGB", (900, ((len(samples)+2)//3) * 250), "#eeeeee")
        draw = ImageDraw.Draw(sheet)
        for i, (name, im) in enumerate(samples):
            x, y = i % 3 * 300, i // 3 * 250
            sheet.paste(im, (x+(300-im.width)//2, y))
            draw.text((x+5, y+225), name, fill="black")
        sheet.save(DATA / "contact-sheet.jpg", quality=88)
    print(json.dumps({k: report[k] for k in ("saved", "verified", "complete", "categories", "errors")}, ensure_ascii=False))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
