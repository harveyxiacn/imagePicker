# Analysis throughput: baseline and optimisations (2026-10-16)

Where the wait after an import goes, measured with `bench/profile-analysis.py` (see
[README](README.md#analysis-stage-profile-profile-analysispy-run-this-on-your-gpu-pc)), and what
was changed because of it. CPU only: the measuring session runs as an ordinary Windows account
without a GPU adapter, so the GPU numbers are still to be recorded on the desktop session (see
"Run this on the GPU PC" at the end).

## Method

* Machine: the Windows desktop (Intel Core i7-13700K, 16 cores / 24 threads, Windows 11), CPU
  only (`--device cpu`: ONNX Runtime `CPUExecutionProvider`, NIMA on LiteRT/XNNPACK, YuNet on
  OpenCV, MediaPipe face landmarker). Worker venv `uv sync --extra cpu --extra mediapipe`
  (Python 3.12, onnxruntime 1.30, OpenCV 5.0, Pillow 12.3). Release build of `imagepicker`.
* Photos, first 200 by path of each set:
  * `qwen`: the synthetic photo library (`bench/data/qwen-photo-library/jpeg`, 1216 x 1216 and
    similar, ~0.3 MB JPEG), people-heavy: 3.8 faces per photo on average, up to 9;
  * `gen12`: `imagepicker bench gen --count 200`, 12 MP JPEGs (4000 x 3000, 1.8 MB) without faces,
    the decode cost of a camera JPEG.
* Worker part: in-process `Analyzer.analyze_batch`, requests of 32 photos like the Core, analysis
  size 1024, 8 decode threads (the default on this CPU), embedding batch 32; a warm-up request
  (one photo per decode thread) loads the models first and is not counted. Step times are per
  photo, measured on the decode threads, which run in parallel: they show where CPU time goes
  (and how much contention inflates it), the wall time is what the user waits for.
* End to end: `imagepicker analyze DIR --profile lite --export OUT` in a scratch data dir
  (import with thumbnails, the on-device lite analysis, grouping / clustering / scoring, export of
  every photo with the default settings). `fast` / `standard` end to end need the AI worker over
  loopback WebSocket, which the measuring account cannot open: for those the worker part is the
  analysis stage.
* One run each, back to back, through the shared job queue (it serialises this repository's
  builds; other load on the machine was not controlled). Two runs of the same configuration on
  96 and 200 photos differed by 1-5% per photo.

## Baseline (before)

### `standard` profile (all eight steps)

| set | wall s (200 photos) | photos/s | ms / photo |
|---|---:|---:|---:|
| qwen | 144.4 | 1.39 | 722 |
| gen12 | 64.6 | 3.10 | 323 |

Per step and photo (mean / p95 ms on the decode threads; share of all step time):

| step | qwen mean | qwen p95 | qwen share | gen12 mean | gen12 p95 | gen12 share |
|---|---:|---:|---:|---:|---:|---:|
| decode | 182 | 202 | 4.7% | 174 | 329 | 18.6% |
| faces (YuNet + landmarker) | 493 | 781 | 12.6% | 178 | 390 | 19.0% |
| identity (AuraFace) | 1990 | 3407 | 51.0% | 1 | 5 | 0.1% |
| quality (incl. tilt, noise) | 762 | 1148 | 19.5% | 255 | 429 | 27.2% |
| phash | 9 | 21 | 0.2% | 2 | 7 | 0.2% |
| prep224 | 16 | 31 | 0.4% | 8 | 14 | 0.9% |
| aesthetic (NIMA) | 106 | 142 | 2.7% | 45 | 59 | 4.9% |
| iqa (NIMA technical) | 106 | 139 | 2.7% | 44 | 55 | 4.7% |
| embed, batched (SigLIP2, per photo) | 228 | 245 | 5.8% | 220 | 227 | 23.5% |
| zero-shot heads + `.emb.npy` | 13 | 28 | 0.3% | 8 | 26 | 0.9% |

Decode alone on one thread: 42 ms p50 for `qwen`, 79 ms p50 for a 12 MP JPEG (DCT-domain
downscale to 2000 px, then Lanczos to 1024); inside the pipeline it took 2-4x as long.

### `fast` profile (phash, quality, faces)

| set | wall s | photos/s | decode | faces | quality | phash |
|---|---:|---:|---:|---:|---:|---:|
| qwen | 17.0 | 11.75 | 58 | 222 | 359 | 3 |
| gen12 | 11.8 | 16.94 | 121 | 135 | 179 | 2 |

(mean ms per photo on the decode threads.) 16 instead of 8 decode threads was slower on both sets
(18.1 s / 12.9 s): the steps are bound by the GIL and memory bandwidth, not by thread count.

### `lite` end to end (no Python)

| set | import | analysing | grouping | clustering | scoring | export | total |
|---|---:|---:|---:|---:|---:|---:|---:|
| qwen | 0.26 s | 2.00 s | 0.003 s | 0.000 s | 0.004 s | 0.06 s | 2.49 s |
| gen12 | 0.32 s | 3.29 s | 0.005 s | 0.000 s | 0.010 s | 0.22 s | 3.92 s |

The lite path (Rust, all cores) is 50-100 photos/s; grouping, scoring and export are negligible
next to the AI analysis. The export copies unchanged originals, so it is I/O only.

### What the profile says

1. Every ONNX Runtime session (SigLIP2 on the embedding thread, AuraFace called from all eight
   decode threads at once) keeps its intra-op threads spinning between operators. On the CPU those
   spinning threads compete with the decode threads: decode inside the pipeline took 2-4x its
   single-thread time, quality 2-5x.
2. The Core sends 32 photos per `analyze.batch` and the worker handed them to the embedding thread
   as one batch of 32: embedding only started once all 32 were decoded and analysed, and the next
   request's decoding only once the embedding was done. Decode and inference never overlapped,
   although the worker was built as a pipeline. On the CPU the SigLIP2 pass (about 220 ms per
   photo) is the longest single stage of `gen12`, on a GPU it is short but still not hidden.
3. `quality` is the largest CPU step without faces (27% `standard`, 41-56% `fast`), and 85% of it
   is the horizon / tilt estimate (`steps/tilt.py`, 129 of 152 ms single-threaded on `qwen`).
4. `identity` dominates people photos on the CPU (2 s per photo with ~4 faces): AuraFace is a
   ResNet-100 per face. On a GPU it runs on CUDA; on the CPU it is the price of the model.
5. RAW files: the worker decoded them with Pillow, which reads IFD0 of TIFF-based RAWs (a 160 px
   thumbnail on NEF / DNG, an old-style JPEG it cannot decode on CR2) and does not open CR3 / RAF
   at all, so RAW photos without a JPEG twin were analysed from a thumbnail or failed with
   `decode_failed`. Not in the sets above (no RAW samples in the repository).
