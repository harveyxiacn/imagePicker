# imagePicker desktop (Tauri 2)

Thin native shell around the same SPA + Rust server as the WebUI (docs/02-架构设计.md §3, ADR-1/4).

- On start, `ip_server::spawn` binds `127.0.0.1:0` (default data dir, or `$IMAGEPICKER_DATA_DIR`) and serves the
  bundled SPA (`web/dist` -> resource `web/`). The main window opens at that URL, so the SPA uses relative `/api`.
- Tauri only adds: window (min 1024x700, size/position remembered, dark background before first paint), native
  folder picker, OS folder drag-and-drop, "Reveal in Explorer/Finder", app menu, single-instance, graceful
  server shutdown on exit.
- SPA <-> shell: Tauri commands `pick_folder` / `reveal_in_folder` (via `window.__TAURI_INTERNALS__.invoke`, see
  `web/src/platform/index.ts`) and DOM events `ip-native-import` / `ip-native-drag` dispatched by the shell.
- A random per-launch token is exposed as `window.__IP_SESSION_TOKEN__`. TODO: the server ignores it for now.

## Prerequisites

Rust (MSVC on Windows), Node 24, pnpm 10. Windows needs WebView2 (present on Win11). Linux needs
`libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libgtk-3-dev patchelf`.

## Develop

```sh
cd web && pnpm install && cd ../apps/desktop && pnpm install
pnpm tauri dev
```

Runs the Vite dev server (`pnpm --dir ../../web dev`, :5173) and the shell. In dev the embedded API listens on
`127.0.0.1:7878` (Vite proxies `/api` there) and the window opens the Vite URL (HMR works).

## Build

```sh
cd apps/desktop
pnpm tauri build                      # all configured bundles for the host OS
pnpm tauri build --bundles nsis       # Windows installer only
pnpm tauri build --no-bundle          # compile only (what CI does)
```

Output: `<target>/release/bundle/nsis/imagePicker_0.1.0_x64-setup.exe` (Windows), `.dmg` (macOS),
`.AppImage`/`.deb` (Linux). Use `CARGO_TARGET_DIR=target-desktop` to keep it apart from the other crates.

Icons: `pnpm icons` regenerates `src-tauri/icons` from `app-icon.svg` (placeholder amber aperture).
