# Large sessions: sort-key indexes and session counters (2026-10-13)

Migration v13 (docs/05 section 1.3). Synthetic data from `imagepicker bench seed`; the server is a
real `imagepicker serve` driven by `bench/run.mjs --only query` (HTTP, loopback, includes JSON
encoding of the 500 rows of a page). Windows desktop, release build, one run each, back to back,
same seeded catalogs (the "after" run opened the v12 catalogs of the "before" run, so it also
exercised the v13 backfill on 100k rows). Raw numbers: `results/perf-before.json`,
`results/perf-after.json` (p50 / p95 of 30 samples, ms).

## What changed

* `GET /api/photos` read every member of the session, filtered, and kept the top `limit + 1` by
  the sort key: O(session size) per page. The sort keys (`taken_at`, `file_name`, `user_rating`,
  `ai_score`) are now copied to `session_photo.sk_*` (triggers keep them in step), with one index
  per sort (`idx_sp_*`, expressions identical to the query's `ORDER BY`). A page is an index range
  scan that stops after `limit + 1` rows; the cursor stays `(key, photo_id)` and is turned into a
  seekable range (`key >= ?` plus a residual tie check), so deep pages cost the same as the first.
  Filters (rating, flag, people...) are applied to the rows of the scan, which stops as soon as
  the page is full.
* `GET /api/sessions` summed every member row of every session. `session_stat` holds
  `photo_count / picked / rejected / rated` per session (triggers on `session_photo` and on
  `photo.flag` / `photo.user_rating`); an unfiltered `total` reads it too.
* Offset paging: the API never had an `offset` parameter, only the opaque cursor; its format and
  order (ties by `photo_id`, missing capture time last) are unchanged, so no client changes.
* Repair: `catalog_meta.session_derived_epoch` != `SESSION_DERIVED_EPOCH` makes the next open
  recompute counters and sort keys from the source tables.

## Results (single 100k-row session, `limit=500`, p50 ms)

| Case | Before | After |
|---|---|---|
| default (`taken_at`) page | 9.6 | 2.8 |
| `-taken_at` | 28.8 | 2.8 |
| `name` | 9.8 | 2.9 |
| `rating` | 11.0 | 3.2 |
| `ai` | 23.4 | 3.1 |
| `rating_gte=4` | 16.9 | 3.3 |
| `flag=picked` | 17.5 | 3.8 |
| `flag=rejected` + `sort=ai` | 19.1 | 6.5 |
| `persons=2` | 13.8 | 7.0 |
| `limit=1` collection count | 14.9 | 0.52 |
| `GET /api/sessions` (100k-row session) | 26.8 | 0.23 |
| `GET /api/sessions` (5 x 20k sessions) | 24.8 | 0.33 |
| `GET /api/sessions` (1 x 20k session) | 4.8 | 0.28 |

Deep pages (single samples, ms, 100k session, `taken_at`): page 0 / 20 / 59 were 21.2 / 23.2 / 19.1,
now 15.4 / 6.1 / 4.9 (first request after start includes cold caches); `name` 18.8 / 26.9 / 19.0
now 4.5 / 5.3 / 4.3; `ai` 23.6 / 28.5 / 26.0 now 9.4 / 5.8 / 6.2. The 20k and 5 x 20k catalogs
improve by 1.3-2.5x (default page 4.9 -> 3.5 ms, `-taken_at` 7.9 -> 3.1 ms, p50). A whole-session
pull at 5000 per page is dominated by row serialisation and did not change (1319 -> 1100 ms at
100k, 188 -> 217 ms at 20k; run-to-run noise on this machine is 10-30%).

Most of the remaining ~2.8 ms of a page is the 500-row JSON response and the wide-row read, not
the session scan.

## Cost: writes

`bench seed --rows 100000 --session-size 100000` (inserts the photos, links them to the session,
faces, bursts): 1.3 s before, 3.0 s after (+17 us per photo row for the triggers and the five
extra indexes). Import of real files is dominated by decoding, so this is far below the
per-file work; metadata/rating/analysis updates add one indexed row update each.

## Tests

`crates/ip-core/src/tests_scale.rs`: counters equal a fresh `COUNT` after import (with RAW
companions), re-import, rating/flag patches, metadata and analysis writes, shared photos,
pairing, photo deletes and session deletes; repair when the epoch marker differs; the v12 -> v13
backfill; paging through every sort key (ties on `taken_at`, NULL times/ratings/scores,
mixed-case names, several page sizes, with and without a filter) equals the order computed
independently in Rust.
