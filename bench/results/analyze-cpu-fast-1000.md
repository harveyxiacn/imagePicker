## Analysis profile:  (1000 photos from `bench\data\qwen-photo-library\jpeg`)

Machine: ROG台式 / Windows 11 / 13th Gen Intel(R) Core(TM) i7-13700K (24 logical cores), python 3.12.10. Date 2026-10-23 09:44.

### Worker `analyze.batch`, profile `fast`

Steps run: phash, quality, faces; skipped: none. Device: cpu (CPUExecutionProvider). Decode threads 8, batch 32 (GPU hand-off every 32), analysis size 1024.

**1000 photos in 54.185 s = 18.46 photos/s** (54.19 ms per photo wall; model warm-up 1.024 s not included).

| step | photos | total s | share | mean ms | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| decode | 1000 | 36.106 | 8.8% | 36.11 | 35.81 | 45.43 |
| faces | 1000 | 74.283 | 18.1% | 74.28 | 63.92 | 158.6 |
| quality | 1000 | 298.988 | 72.7% | 298.99 | 296.82 | 399.77 |
| phash | 1000 | 1.76 | 0.4% | 1.76 | 1.5 | 3.44 |

Step times are per photo on the decode threads (they run in parallel, so the sum exceeds the wall time); batched steps are the batch time divided by its photos. Decode alone on one thread: p50 22.0 ms, p95 23.55 ms (50 photos).
