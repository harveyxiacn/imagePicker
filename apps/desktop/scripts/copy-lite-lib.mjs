#!/usr/bin/env node
// Copies the C ABI library of crates/ip-lite-ffi into the worker source that the desktop bundle
// ships (`ai-worker/imagepicker_ai/steps/_native/`), where `lite_native.py` finds it without any
// environment variable. Run before `tauri build`:
//
//   cargo build --release -p ip-lite-ffi
//   node apps/desktop/scripts/copy-lite-lib.mjs        (or CARGO_PROFILE=debug ...)
//
// The worker keeps its Python `quality` step, so a bundle without the file still works.
import { execFileSync } from 'node:child_process'
import { copyFileSync, existsSync, mkdirSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const here = dirname(fileURLToPath(import.meta.url))
const repo = join(here, '..', '..', '..')
const name =
  process.platform === 'win32'
    ? 'ip_lite_ffi.dll'
    : process.platform === 'darwin'
      ? 'libip_lite_ffi.dylib'
      : 'libip_lite_ffi.so'
const profile = process.env.CARGO_PROFILE || 'release'
const src = join(repo, 'target', profile, name)
if (!existsSync(src)) {
  // `tauri build` builds only the app package, so the library is built here (it is small and
  // shares the same target dir).
  console.log(`[lite] ${src} is missing; building ip-lite-ffi`)
  const args = ['build', ...(profile === 'release' ? ['--release'] : []), '-p', 'ip-lite-ffi']
  execFileSync('cargo', args, { cwd: repo, stdio: 'inherit' })
}
if (!existsSync(src)) {
  console.error(`[lite] ${src} still missing after cargo build -p ip-lite-ffi`)
  process.exit(1)
}
const out = join(repo, 'ai-worker', 'imagepicker_ai', 'steps', '_native', name)
mkdirSync(dirname(out), { recursive: true })
copyFileSync(src, out)
console.log(`[lite] ${name} -> ${out}`)
