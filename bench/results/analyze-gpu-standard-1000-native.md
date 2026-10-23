## Analysis profile:  (1000 photos from `bench\data\qwen-photo-library\jpeg`)

Machine: ROG台式 / Windows 11 / 13th Gen Intel(R) Core(TM) i7-13700K (24 logical cores), python 3.12.10. Date 2026-10-23 10:29.

### Worker `analyze.batch`, profile `standard`

Steps run: phash, quality, faces, identity, embed, aesthetic, iqa, scene; skipped: none. Device: cuda (CUDAExecutionProvider, CPUExecutionProvider). Decode threads 8, batch 32 (GPU hand-off every 32), analysis size 1024.

**1000 photos in 89.369 s = 11.19 photos/s** (89.37 ms per photo wall; model warm-up 2.487 s not included).

| step | photos | total s | share | mean ms | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| decode | 1000 | 63.122 | 10.4% | 63.12 | 60.49 | 88.27 |
| faces | 1000 | 213.489 | 35.2% | 213.49 | 204.87 | 382.96 |
| identity | 1000 | 94.534 | 15.6% | 94.53 | 38.44 | 292.55 |
| quality | 1000 | 118.215 | 19.5% | 118.22 | 112.98 | 179.49 |
| phash | 1000 | 3.29 | 0.5% | 3.29 | 2.94 | 6.11 |
| prep224 | 1000 | 6.953 | 1.1% | 6.95 | 6.25 | 12.09 |
| aesthetic | 1000 | 50.658 | 8.3% | 50.66 | 49.56 | 73.71 |
| iqa | 1000 | 50.067 | 8.2% | 50.07 | 48.83 | 75.43 |
| embed (batched) | 1000 | 4.526 | 0.8% | 4.53 | 2.71 | 9.41 |
| heads (batched) | 1000 | 2.021 | 0.3% | 2.02 | 2.01 | 2.35 |

Step times are per photo on the decode threads (they run in parallel, so the sum exceeds the wall time); batched steps are the batch time divided by its photos. Decode alone on one thread: p50 36.41 ms, p95 39.63 ms (50 photos).
