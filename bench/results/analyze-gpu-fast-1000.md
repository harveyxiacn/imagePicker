## Analysis profile:  (1000 photos from `bench\data\qwen-photo-library\jpeg`)

Machine: ROG台式 / Windows 11 / 13th Gen Intel(R) Core(TM) i7-13700K (24 logical cores), python 3.12.10. Date 2026-10-23 09:45.

### Worker `analyze.batch`, profile `fast`

Steps run: phash, quality, faces; skipped: none. Device: cuda (CUDAExecutionProvider, CPUExecutionProvider). Decode threads 8, batch 32 (GPU hand-off every 32), analysis size 1024.

**1000 photos in 53.518 s = 18.69 photos/s** (53.52 ms per photo wall; model warm-up 0.973 s not included).

| step | photos | total s | share | mean ms | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| decode | 1000 | 36.333 | 8.9% | 36.33 | 35.93 | 47.15 |
| faces | 1000 | 72.853 | 17.9% | 72.85 | 60.84 | 149.69 |
| quality | 1000 | 294.919 | 72.7% | 294.92 | 292.08 | 398.41 |
| phash | 1000 | 1.707 | 0.4% | 1.71 | 1.46 | 3.29 |

Step times are per photo on the decode threads (they run in parallel, so the sum exceeds the wall time); batched steps are the batch time divided by its photos. Decode alone on one thread: p50 22.05 ms, p95 23.14 ms (50 photos).
