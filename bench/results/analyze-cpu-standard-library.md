## Analysis profile:  (1962 photos from `bench\data\qwen-photo-library`)

Machine: ROG台式 / Windows 11 / 13th Gen Intel(R) Core(TM) i7-13700K (24 logical cores), python 3.12.10. Date 2026-10-23 09:20.

### Worker `analyze.batch`, profile `standard`

Steps run: phash, quality, faces, identity, embed, aesthetic, iqa, scene; skipped: none. Device: cpu (CPUExecutionProvider). Decode threads 8, batch 32 (GPU hand-off every 32), analysis size 1024.

**1962 photos in 1148.155 s = 1.71 photos/s** (585.2 ms per photo wall; model warm-up 4.298 s not included).

| step | photos | total s | share | mean ms | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| decode | 1962 | 431.953 | 7.8% | 220.16 | 163.13 | 601.21 |
| faces | 1962 | 612.022 | 11.1% | 311.94 | 238.32 | 807.98 |
| identity | 1962 | 2002.95 | 36.2% | 1020.87 | 501.87 | 3962.41 |
| quality | 1962 | 1606.195 | 29.1% | 818.65 | 770.36 | 1742.85 |
| phash | 1962 | 12.113 | 0.2% | 6.17 | 1.64 | 24.9 |
| prep224 | 1962 | 36.179 | 0.7% | 18.44 | 6.19 | 101.52 |
| aesthetic | 1962 | 180.748 | 3.3% | 92.12 | 47.72 | 259.9 |
| iqa | 1962 | 175.863 | 3.2% | 89.63 | 47.56 | 256.45 |
| embed (batched) | 1962 | 458.003 | 8.3% | 233.44 | 291.07 | 462.86 |
| heads (batched) | 1962 | 9.245 | 0.2% | 4.71 | 4.13 | 10.74 |

Step times are per photo on the decode threads (they run in parallel, so the sum exceeds the wall time); batched steps are the batch time divided by its photos. Decode alone on one thread: p50 22.31 ms, p95 24.23 ms (50 photos).
