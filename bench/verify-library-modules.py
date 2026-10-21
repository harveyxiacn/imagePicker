"""Additional real-library API checks. Run only against a disposable verification catalog.

Unlike unit tests, these use an imported, analysed library and verify returned files/pixels.
The companion api-features.py covers the model-dependent AI operations.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import io
import json
import shutil
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

from PIL import Image


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--base", default="http://127.0.0.1:17914")
    ap.add_argument("--out", type=Path, required=True)
    ap.add_argument("--session", type=int, default=1)
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    out = a.out.resolve()
    if "verification-" not in str(out):
        raise SystemExit("Use a dedicated verification-* output directory")
    out.mkdir(parents=True, exist_ok=True)

    def api(method, path, body=None, expected=None):
        req = urllib.request.Request(
            a.base + "/api" + path,
            method=method,
            data=None if body is None else json.dumps(body).encode(),
            headers={"Content-Type": "application/json"},
        )
        try:
            r = urllib.request.urlopen(req, timeout=300)
        except urllib.error.HTTPError as e:
            r = e
        raw = r.read()
        data = (
            json.loads(raw)
            if "json" in r.headers.get("Content-Type", "") and raw
            else raw
        )
        if expected is not None:
            assert r.status == expected, (path, r.status, data)
        else:
            assert r.status < 400, (path, r.status, str(data)[:500])
        return data

    def wait(label, fn, seconds=120):
        end = time.monotonic() + seconds
        while time.monotonic() < end:
            val = fn()
            if val:
                return val
            time.sleep(0.25)
        raise AssertionError("timeout: " + label)

    def task(tid):
        def done():
            t = next((x for x in api("GET", "/tasks")["tasks"] if x["id"] == tid), None)
            if t and t["status"] in ("done", "failed", "cancelled"):
                assert t["status"] == "done", t
                return t

        return wait(tid, done, 1800)

    def photos(sid=a.session, **params):
        result, cursor = [], None
        while True:
            q = dict(session_id=sid, limit=127, **params)
            if cursor:
                q["cursor"] = cursor
            p = api("GET", "/photos?" + urllib.parse.urlencode(q))
            result.extend(p["photos"])
            cursor = p.get("next_cursor")
            if not cursor:
                return result

    all_photos = photos()
    assert len(all_photos) == 1000, len(all_photos)
    ids = [p["id"] for p in all_photos]
    assert len(set(ids)) == 1000
    samples = list({Path(p["path"]).parent.name: p for p in all_photos}.values())
    pid = ids[0]

    def catalog():
        assert all(p["analyzed"] for p in all_photos)
        assert len(photos(sort="name")) == 1000
        assert len(photos(sort="-taken_at")) == 1000
        return {
            "photos": 1000,
            "unique_ids": 1000,
            "all_analyzed": True,
            "groups": len(api("GET", f"/groups?session_id={a.session}")["scenes"]),
        }

    def media():
        def thumb(p):
            b = api("GET", f"/thumb/{p['id']}?s=256")
            im = Image.open(io.BytesIO(b))
            im.load()
            assert max(im.size) <= 256 and min(im.size) > 0

        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            list(pool.map(thumb, all_photos))
        dimensions = []
        for p in samples:
            for endpoint in ("preview", "full", "original"):
                b = api("GET", f"/{endpoint}/{p['id']}")
                im = Image.open(io.BytesIO(b))
                im.load()
                assert min(im.size) > 0
                dimensions.append({"id": p["id"], "kind": endpoint, "size": im.size})
        return {"decoded_thumbnails": 1000, "sample_media": dimensions}

    def rating_tags():
        old = {p["id"]: p for p in all_photos[:60]}
        try:
            api(
                "PATCH",
                "/photos",
                {"ids": list(old), "user_rating": 5, "flag": 1, "color_label": "red"},
            )
            filtered = photos(rating_gte=5, flag="picked", color_label="red")
            assert set(old) <= {p["id"] for p in filtered}
            api("POST", "/photos/tags", {"ids": [pid], "add": ["verification-1000"]})
            tags = api("GET", f"/photos/{pid}/tags")
            assert "verification-1000" in json.dumps(tags)
            taste = api("GET", "/taste")
            return {
                "rated": len(old),
                "filtered": len(filtered),
                "tags": tags,
                "taste": taste,
            }
        finally:
            for p in old.values():
                api(
                    "PATCH",
                    "/photos",
                    {
                        "ids": [p["id"]],
                        **{k: p[k] for k in ("user_rating", "flag", "color_label")},
                    },
                )
            api("POST", "/photos/tags", {"ids": [pid], "remove": ["verification-1000"]})

    def collections_presets():
        c = api(
            "POST", "/collections", {"name": "Verification", "query": "rating_gte=4"}
        )["collection"]
        api("PATCH", f"/collections/{c['id']}", {"name": "Verification renamed"})
        api("DELETE", f"/collections/{c['id']}")
        api("DELETE", "/collections/edited", expected=400)
        stack = {"version": 1, "ops": [{"type": "global", "exposure": 0.4}]}
        p = api("POST", "/presets", {"name": "Verification", "stack": stack})["preset"]
        assert any(x["id"] == p["id"] for x in api("GET", "/presets")["presets"])
        api("DELETE", f"/presets/{p['id']}")
        return {"collection_crud": True, "builtin_protected": True, "preset_crud": True}

    def edits():
        stacks = {
            "global": [
                {"type": "global", "exposure": 0.6, "contrast": 12, "saturation": 8}
            ],
            "crop": [{"type": "crop", "rect": [0.1, 0.1, 0.8, 0.8], "angle": 5}],
            "local": [
                {
                    "type": "local",
                    "mask": {
                        "kind": "radial",
                        "center": [0.5, 0.5],
                        "radius": [0.3, 0.4],
                        "feather": 0.5,
                    },
                    "adjust": {"exposure": 1},
                }
            ],
            "sharpen": [{"type": "output_sharpen", "amount": 30}],
        }
        results = []
        for p in samples:
            original = api(
                "POST",
                "/render/preview",
                {"photo_id": p["id"], "original": True, "long_edge": 512},
            )
            for name, ops in stacks.items():
                b = api(
                    "POST",
                    "/render/preview",
                    {
                        "photo_id": p["id"],
                        "stack": {"version": 1, "ops": ops},
                        "long_edge": 512,
                    },
                )
                im = Image.open(io.BytesIO(b))
                im.load()
                assert b != original, (p["id"], name, "no pixel change")
                (out / f"render-{p['id']}-{name}.jpg").write_bytes(b)
                results.append({"photo": p["id"], "operation": name, "size": im.size})
        old = api("GET", f"/edits/{pid}")["stack"]
        try:
            stack = {"version": 1, "ops": stacks["global"]}
            api("PUT", f"/edits/{pid}", {"stack": stack})
            assert api("GET", f"/photos/{pid}")["photo"]["has_edits"]
            sync = api(
                "POST",
                "/edits/sync",
                {
                    "from_id": pid,
                    "to_ids": ids[1:4],
                    "include": ["global"],
                    "adaptive": True,
                },
            )
            assert sync["updated"] == 3
            return {"renders": results, "adaptive_sync": sync}
        finally:
            api("PUT", f"/edits/{pid}", {"stack": old})
            for x in ids[1:4]:
                api("DELETE", f"/edits/{x}")

    def exports():
        results = []
        for fmt in ("jpeg", "png", "webp", "tiff"):
            dest = out / ("export-" + fmt)
            dest.mkdir(exist_ok=True)
            selected = ids if fmt == "jpeg" else [p["id"] for p in samples]
            t = api(
                "POST",
                "/export",
                {
                    "ids": selected,
                    "dest": str(dest),
                    "format": fmt,
                    "long_edge": 512,
                    "name_template": "{name}",
                },
            )
            done = task(t["task_id"])
            files = [p for p in dest.iterdir() if p.is_file() and p.suffix != ".xmp"]
            assert len(files) == len(selected), (fmt, len(files), len(selected), done)
            for f in files:
                im = Image.open(f)
                im.load()
                assert max(im.size) <= 512
            results.append({"format": fmt, "files": len(files), "task": done})
        return results

    def files():
        src = out / "file-operations"
        src.mkdir(exist_ok=True)
        for i, p in enumerate(all_photos[:6]):
            shutil.copy2(p["path"], src / f"{i}.jpg")
        sid = api(
            "POST",
            "/import",
            {"path": str(src), "recursive": True, "exclude": ["5.jpg"]},
        )["session"]["id"]
        wait(
            "fixture import",
            lambda: (
                api("GET", f"/sessions/{sid}")["session"]["import_state"] == "ready"
            ),
        )
        pp = photos(sid)
        assert len(pp) == 5
        shutil.copy2(all_photos[6]["path"], src / "new.jpg")
        wait("file watcher add", lambda: len(photos(sid)) == 6, 30)
        moved = out / "file-operations-renamed"
        assert src.resolve().is_relative_to(out) and moved.resolve().is_relative_to(out)
        src.rename(moved)
        api("POST", f"/sessions/{sid}/rescan")
        wait(
            "missing marker",
            lambda: api("GET", f"/photos/{pp[0]['id']}")["photo"]["missing"],
        )
        preview = api(
            "POST", f"/sessions/{sid}/relocate", {"path": str(moved), "dry_run": True}
        )
        assert not preview["relocate"]["applied"]
        api("POST", f"/sessions/{sid}/relocate", {"path": str(moved)})
        wait(
            "relocated",
            lambda: not api("GET", f"/photos/{pp[0]['id']}")["photo"]["missing"],
        )
        api("PATCH", "/photos", {"ids": [pp[0]["id"]], "flag": -1})
        api(
            "POST",
            "/photos/reject-files",
            {"session_id": sid, "mode": "folder", "confirm": False},
            expected=400,
        )
        result = api(
            "POST",
            "/photos/reject-files",
            {"session_id": sid, "mode": "folder", "confirm": True},
        )
        assert list((moved / "_rejected").rglob("*.jpg"))
        return {
            "excluded": True,
            "watcher": True,
            "relocation": preview,
            "safe_reject": result,
        }

    def backup():
        saved = api("POST", "/catalog/backup")
        before = api("GET", f"/photos/{pid}")["photo"]["user_rating"]
        new = 1 if before != 1 else 2
        api("PATCH", "/photos", {"ids": [pid], "user_rating": new})
        state = api("POST", "/catalog/restore", {"name": saved["name"]})
        assert api("GET", f"/photos/{pid}")["photo"]["user_rating"] == before
        return {"backup": saved, "restored": state["state"], "rating_restored": True}

    checks = dict(
        catalog=catalog,
        media=media,
        rating_tags=rating_tags,
        collections_presets=collections_presets,
        edits=edits,
        exports=exports,
        files=files,
        backup=backup,
    )
    failed = 0
    for name in a.only.split(",") if a.only else checks:
        start = time.monotonic()
        try:
            evidence = checks[name]()
            row = dict(module=name, status="passed", evidence=evidence)
        except Exception as e:
            failed += 1
            row = dict(module=name, status="failed", error=f"{type(e).__name__}: {e}")
        row["seconds"] = round(time.monotonic() - start, 3)
        print(json.dumps(row, ensure_ascii=False), flush=True)
        with (out / "modules.jsonl").open("a", encoding="utf-8") as f:
            f.write(json.dumps(row, ensure_ascii=False) + "\n")
    return int(bool(failed))


if __name__ == "__main__":
    raise SystemExit(main())
