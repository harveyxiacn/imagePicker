## Analysis profile:  (1000 photos from `bench\data\qwen-photo-library\jpeg`)

Machine: ROG台式 / Windows 11 / 13th Gen Intel(R) Core(TM) i7-13700K (24 logical cores), python 3.12.10. Date 2026-10-23 09:42.

### Worker `analyze.batch`, profile `standard`

Steps run: phash, quality, faces, identity, embed, aesthetic, iqa, scene; skipped: none. Device: cpu (CPUExecutionProvider). Decode threads 8, batch 32 (GPU hand-off every 32), analysis size 1024.

**1000 photos in 133.064 s = 7.52 photos/s** (133.06 ms per photo wall; model warm-up 3.189 s not included).

| step | photos | total s | share | mean ms | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| decode | 1000 | 39.101 | 6.0% | 39.1 | 38.72 | 52.37 |
| faces | 1000 | 91.013 | 14.0% | 91.01 | 77.92 | 194.65 |
| identity | 1000 | 119.004 | 18.3% | 119.0 | 59.11 | 435.85 |
| quality | 1000 | 290.615 | 44.8% | 290.61 | 292.2 | 403.16 |
| phash | 1000 | 1.769 | 0.3% | 1.77 | 1.53 | 3.45 |
| prep224 | 1000 | 4.708 | 0.7% | 4.71 | 4.41 | 7.73 |
| aesthetic | 1000 | 24.787 | 3.8% | 24.79 | 23.27 | 39.8 |
| iqa | 1000 | 25.557 | 3.9% | 25.56 | 23.45 | 44.16 |
| embed (batched) | 1000 | 51.62 | 8.0% | 51.62 | 51.79 | 53.29 |
| heads (batched) | 1000 | 1.01 | 0.2% | 1.01 | 1.01 | 1.16 |

Step times are per photo on the decode threads (they run in parallel, so the sum exceeds the wall time); batched steps are the batch time divided by its photos. Decode alone on one thread: p50 22.07 ms, p95 25.61 ms (50 photos).
