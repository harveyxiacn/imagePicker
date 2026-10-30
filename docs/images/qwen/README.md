# README screenshots — Qwen library

Captured on 2026-10-30 from source revision `6cbd3902` (version 0.1.0), using the
production WebUI build and the Rust HTTP server, not the mock backend.

The source library contains 1,000 synthetic images generated with ComfyUI and
Qwen-Image 2.1. Images include portraits, group shots, landscapes, travel, pets,
and deliberately varied burst frames. They are not real camera photographs.
Ratings, groups and people come from the existing analysed verification catalog;
the screenshots do not establish accuracy on real-world photos.

## Reproduce

1. Build the current source with `pnpm build` in `web/` and
   `cargo build --locked --release -p ip-cli` at the repository root.
2. Make a disposable copy of an analysed Qwen catalog. Use SQLite's backup API
   for `catalog.db` (a running catalog may have a WAL); copy its `cache/`,
   `edits/`, `settings.json`, and `onboarding.done`. Keep the original photo
   paths accessible. Set the copied settings' model directory to your installed
   models. This capture used `bench/data/verification-2026-10-29/catalog-final2`
   as the source and `bench/data/readme-2026-10-30` as the disposable copy.
3. Give the 1,000-photo session the display title `Qwen · 1000 张示例图库`.
   Set language to Chinese and theme to dark. The capture changes the theme
   while taking the light screenshot; use only the disposable catalog.
4. Start the built executable with `serve --port 17830 --data-dir
   bench/data/readme-2026-10-30 --web-dir web/dist`. Configure the installed AI
   worker through `IMAGEPICKER_WORKER_CMD` for semantic search.
5. From `web/`, run `node scripts/readme-screenshots.mjs`. Override `BASE_URL`
   and `OUT_DIR` if necessary. The script requires a session with exactly 1,000
   images, checks visible images have loaded, and records page errors and HTTP
   5xx responses in `capture.json`.

Images are direct Chromium screenshots at 1600 × 1000, without compositing or
fabricated UI content. The original generated library, model weights, catalog,
and caches are local-only and are not included in this directory.
