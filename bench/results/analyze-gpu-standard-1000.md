## Analysis profile:  (1000 photos from `bench\data\qwen-photo-library\jpeg`)

Machine: ROG台式 / Windows 11 / 13th Gen Intel(R) Core(TM) i7-13700K (24 logical cores), python 3.12.10. Date 2026-10-23 09:40.

### Worker `analyze.batch`, profile `standard`

Steps run: phash, quality, faces, identity, embed, aesthetic, iqa, scene; skipped: none. Device: cuda (CUDAExecutionProvider, CPUExecutionProvider). Decode threads 8, batch 32 (GPU hand-off every 32), analysis size 1024.

**1000 photos in 73.124 s = 13.68 photos/s** (73.12 ms per photo wall; model warm-up 4.83 s not included).

| step | photos | total s | share | mean ms | p50 ms | p95 ms |
|---|---:|---:|---:|---:|---:|---:|
| decode | 1000 | 35.707 | 7.0% | 35.71 | 35.35 | 45.38 |
| faces | 1000 | 72.743 | 14.1% | 72.74 | 64.52 | 154.21 |
| identity | 1000 | 59.03 | 11.5% | 59.03 | 22.0 | 166.3 |
| quality | 1000 | 294.758 | 57.3% | 294.76 | 295.58 | 401.4 |
| phash | 1000 | 1.568 | 0.3% | 1.57 | 1.4 | 2.87 |
| prep224 | 1000 | 3.944 | 0.8% | 3.94 | 3.6 | 6.51 |
| aesthetic | 1000 | 20.23 | 3.9% | 20.23 | 19.41 | 26.52 |
| iqa | 1000 | 20.758 | 4.0% | 20.76 | 19.9 | 27.85 |
| embed (batched) | 1000 | 4.276 | 0.8% | 4.28 | 2.31 | 8.41 |
| heads (batched) | 1000 | 1.044 | 0.2% | 1.04 | 1.0 | 1.42 |

Step times are per photo on the decode threads (they run in parallel, so the sum exceeds the wall time); batched steps are the batch time divided by its photos. Decode alone on one thread: p50 22.14 ms, p95 23.3 ms (50 photos).
