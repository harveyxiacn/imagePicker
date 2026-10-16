# imagePicker User Guide (English)

> [简体中文](zh-CN.md) · [Guide index](README.md) · [Project home](../../README.md)
>
> This guide describes what is **implemented today**; names match the English UI strings. Version: v0.1 (pre-release).
> Screenshots come from the mock backend with generated sample images (gradient landscapes and cartoon faces), shown with the Chinese UI; no real photos. On macOS `Ctrl` means `⌘`.

## Contents

1. [Overview & interface](#1-overview--interface)
2. [Installation](#2-installation)
3. [First run & AI components](#3-first-run--ai-components)
4. [Importing photos](#4-importing-photos)
5. [Culling workflow & shortcuts](#5-culling-workflow--shortcuts)
6. [Smart analysis & stars](#6-smart-analysis--stars)
7. [Groups & stacks](#7-groups--stacks)
8. [People & face filters](#8-people--face-filters)
9. [Best take](#9-best-take)
10. [Editing](#10-editing)
11. [Portrait retouching](#11-portrait-retouching)
12. [Repair](#12-repair)
13. [Export & presets](#13-export--presets)
14. [AI assistant](#14-ai-assistant)
15. [LAN WebUI & guest access](#15-lan-webui--guest-access)
16. [Privacy & data locations](#16-privacy--data-locations)
17. [Lightroom / darktable interop (XMP)](#17-lightroom--darktable-interop-xmp)
18. [Settings](#18-settings)
19. [Troubleshooting & FAQ](#19-troubleshooting--faq)
20. [Mobile & remote AI (experimental)](#20-mobile--remote-ai-experimental)
21. [Uninstall](#21-uninstall)

---

## 1. Overview & interface

Typical flow: **import → one-click analysis → group & cull (keyboard-driven) → best take → edit & retouch → export**. Your original files are never modified: ratings and edits live in a local catalog (edits are an "edit stack" you can undo or reset at any time).

![Home](../images/home.png)

Home: drop a folder (desktop) or click *Choose folder* to import; *Recent* lists imported sessions. Top right: *My taste*, *Settings*, language and theme.

![Library grid](../images/grid.png)

| Area | What it does |
|---|---|
| Top bar | View switch: **Grid / Loupe / Compare / Group**; *Analyze*, *AI Assistant*, *People*, *Export*; AI engine status (tier and GPU) |
| Filter bar | Rating, AI rating, flag, colour label, people, issues, scene, sort, thumbnail size, *Grouped* toggle |
| Left "Smart collections" | Built-in: Best per group, Has closed eyes, Undecided, Edited, Duplicates; save the current filter as your own collection; the *Events* list of the open session sits below |
| Grid | Split by scene; bursts collapse into stacks (badge shows the count); thumbnails show stars, flag and issue icons |
| Right inspector | Score breakdown, AI suggestion (with *Accept*), flag, colour, faces, EXIF (`Tab` hides / shows it) |
| Status bar | Thumbnail progress, selection count, picked / rejected / rated totals, connection state |

> The search box in the top bar finds photos by description (see section 6, *Search by description*). Narrow screens (width < 768 px, e.g. phones) switch to a mobile layout automatically, see [section 20](#20-mobile--remote-ai-experimental).

## 2. Installation

> Installers will be published on [GitHub Releases](https://github.com/harveyxiacn/imagePicker/releases). Until then (or for the latest code) build from source as described below.

### Requirements

| | Minimum | Recommended |
|---|---|---|
| OS | Windows 10/11 (WebView2, built into Win11), macOS, Linux (incl. CachyOS / Arch) | — |
| RAM | 8 GB | 16 GB+ |
| GPU | Not required (CPU works) | NVIDIA 8 GB+ VRAM (T2+); see [hardware tiers](../../README.md#hardware-tiers) |
| Disk | ~0.5 GB (base models); ~2 GB for the standard profile; all add-on packs can reach tens of GB | SSD |

### Windows

1. Download `imagePicker_<version>_x64-setup.exe` (NSIS installer; `.msi` for managed deployment) and run it.
2. The installers are **not code-signed yet**, so Windows SmartScreen may say "Windows protected your PC": click *More info → Run anyway*. You can verify the download against `SHA256SUMS.txt` on the Releases page (PowerShell: `Get-FileHash .\imagePicker_*_x64-setup.exe`).
3. If WebView2 is missing, install the [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) first.

### Windows portable (USB drive, no installer)

1. Unpack a portable zip (`imagePicker_<version>_x64-portable.zip`, assembled as described in `docs/release.md`) anywhere, for example onto an external drive. It contains `imagePicker.exe` and an empty file `imagepicker.portable` next to it.
2. Start `imagePicker.exe`. Everything the app stores (catalog, thumbnails, settings, logs, crash records, models, AI components) goes to the `data\` folder beside the exe, not to your Windows profile. *Settings → Cache* shows **Portable mode: on (data next to the app)** and the data folder. Your photos are never copied.
3. To make an existing copy portable, close the app, create an empty file called `imagepicker.portable` next to `imagePicker.exe` (there is deliberately no button for it) and, to keep your catalog, move the contents of `%APPDATA%\imagePicker` into a new `data\` folder there. Delete the marker (and move the data back) to return to the normal mode.
4. If the drive shows up under another letter on the next computer (`E:` → `F:`), photo folders that are on **the same drive as the app** follow automatically. Folders on other drives keep their path; if they are missing, use *Relocate folder* (section 4). The AI components may have to be reinstalled in *Settings → AI components*; downloaded models are kept.

Order in which the data folder is chosen: `--data-dir` → `IMAGEPICKER_DATA_DIR` → portable folder → `%APPDATA%\imagePicker`. Window size and position are still remembered in your Windows profile.

### macOS

Download the `.dmg` (universal: Intel and Apple silicon) and drag the app to *Applications*. The app is **not signed or notarised yet**, so Gatekeeper blocks the first launch: in Finder **right-click → Open → Open**, or click *Open Anyway* in *System Settings → Privacy & Security*, or run `xattr -dr com.apple.quarantine /Applications/imagePicker.app` in Terminal.

### Linux (incl. CachyOS / Arch)

Download the `.AppImage` (make it executable, then run) or the `.deb`. Building from source needs WebKitGTK and friends:

```sh
# Debian / Ubuntu
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libgtk-3-dev patchelf
# Arch / CachyOS (check package names against your repositories)
sudo pacman -S webkit2gtk-4.1 gtk3 libayatana-appindicator librsvg patchelf
```

### Build from source

Needs Rust, Node 24 and pnpm 10:

```sh
cd web && pnpm install
cd ../apps/desktop && pnpm install
pnpm tauri build                 # installer for the host OS
```

More options in [apps/desktop/README.md](../../apps/desktop/README.md).

### Browser only (WebUI)

```sh
cd web && pnpm install && pnpm build && cd ..
cargo run -p ip-cli -- serve --web-dir web/dist        # default http://127.0.0.1:7878
```

A browser cannot receive OS folder drops, so use *Choose folder*; everything else matches the desktop app.

## 3. First run & AI components

On first start a welcome card shows the detected GPU, the recommended tier and the download size:

- **Use basic features first**: downloads nothing; import, rate and cull right away;
- **Download AI components**: downloads the recommended models in the background without blocking you; you can also do it later in *Settings → Hardware & models*.

Three coach-mark bubbles follow (Analyze, stacks, number-key rating).

### AI components (Python worker)

Analysis, AI masks, portraits, repair and the local assistant models run in a separate Python process (`ai-worker`). **The installers include a built-in setup, so you don't need to install Python yourself:**

1. Click *Download AI components* on the welcome card, or open *Settings → Hardware & models → AI components* and click *Install*.
2. The app installs a private Python 3.12 and the AI components into `runtime/` in the data directory (your system Python is untouched) and picks the build for your hardware: CUDA on NVIDIA GPUs with driver ≥ 580, CPU otherwise.
3. It downloads about 1–2.5 GB (2–5 minutes depending on your connection), can be cancelled at any time, and takes about 2.3 GB on disk. Models are then downloaded on demand.
4. After an app update you are asked to reinstall the matching version; *Remove* on the same page frees the space.

Nothing is installed while *Settings → Faces & privacy → Allow network* is off. Any AI action offers *Install AI components* when they are missing.

> The *AI engine* dot in the top bar shows Ready / Starting / Busy / Crashed / Unavailable.

**Running from source (developers):** the desktop dev mode uses the repo's `ai-worker/` directly (needs [uv](https://docs.astral.sh/uv/)):

```sh
cd ai-worker
uv sync --extra cuda --extra mediapipe     # NVIDIA GPU (driver >= 580)
uv sync --extra cpu  --extra mediapipe     # CPU only
# optional: local AI assistant models  --extra llm-cuda (with cuda) / --extra llm (with cpu)
```

Environment variables: `IMAGEPICKER_WORKER_DIR` (worker directory), `IMAGEPICKER_WORKER_CMD` (custom start command), `IMAGEPICKER_MODELS_DIR` (models directory).

### Model downloads

The first time an AI feature needs models, the *AI models required* dialog lists each model, its purpose, size and licence:

![Model consent](../images/model-consent.png)

- *Download and continue* starts the download; *Move to background* keeps working meanwhile;
- Models marked "non-commercial" or "research only" are not for commercial use, and models marked "licence unknown" have no established commercial-use standing yet (for example undisclosed training data): the badge next to the licence says so, and hovering it shows the training data and notes. Personal use is fine; check the terms before any commercial use;
- Sources: Auto (falls back to a mirror) / Hugging Face / HF mirror / ModelScope, chosen in *Settings → Hardware & models*. The source you pick is tried first (with the HF mirror selected, the first request already goes to the mirror), the others are fallbacks. **ModelScope is a placeholder for now**: no model is hosted there yet, so choosing it behaves like *Auto*;
- Models are stored locally; afterwards no network is needed. You can delete models in Settings to free space.

## 4. Importing photos

- **Desktop**: drag a folder onto Home, or click *Choose folder* (native picker).
- **Browser / phone**: click *Choose folder* and use the in-app folder browser (limited to the *allowed folders*, see [Settings](#18-settings)).
- Supported: JPEG, PNG, WebP, AVIF, TIFF, camera RAW (CR2/CR3, NEF, ARW, RAF, ORF, RW2, DNG and more) and HEIC/HEIF files. RAW thumbnails use the embedded preview.
- Import scans sub-folders recursively: *Scanning…* → *Generating thumbnails…* → *Done*. You can browse and rate while thumbnails appear.
- *Remove session* only removes the catalog record and caches; **originals are untouched**.
- **Memory cards (SD card / camera storage)**: when a card with a `DCIM` folder is plugged in (a removable drive on Windows, a volume under `/Volumes` on macOS, a mount under `/media` or `/run/media` on Linux), Home shows a *Memory cards* box with its name, free space and an estimated photo count. Nothing happens until you click *Import from card*: choose the destination folder (inside an allowed folder), a folder structure (default `year/year-month-day`, e.g. `2026/2026-10-03`, from each photo's capture date; a RAW and its JPEG always land together), and optionally a session name. The app then **copies** the new photos (the card is only read, never changed), checks every copy against the original, never overwrites anything (a clash gets `_1`, `_2`…), skips files that are already in your catalog, and imports the copies as a session; RAW+JPEG pairs and the exclusions from *Import options* work as usual. You can close the window while it runs and cancel from the dialog or the task history. If a file cannot be read or the destination fills up, the files already copied and checked are still imported and the summary lists what is left; run the import again later and only the missing files are copied. Ejecting the card is left to the operating system. Android is not supported yet.
- **Import options** (the link under the drop area): list globs or sub-folders to leave out, one per line (`*.dng`, `backup/**`, `trip/rejects`; matching is case-insensitive, `*` stays inside a folder, `**` crosses folders). The same exclusions apply when the folder is rescanned.
- **RAW + JPEG pairs**: a RAW file and a JPEG (or HEIC) with the same name in the same folder (`IMG_0001.CR2` + `IMG_0001.JPG`) become **one** grid item with an *RAW+JPEG* badge. The JPEG is shown and analysed, the RAW rides along: rating, flag and colour label apply to both files (and to both XMP sidecars when XMP sync is on: `IMG_0001.xmp` and `IMG_0001.CR2.xmp`). Groups and analysis count the pair once. A RAW without a JPEG next to it stays a normal item. When you export, choose *JPEG*, *RAW* or *Both* for such pairs.
- **Changes on disk**: the desktop app watches imported folders. Files you add or remove show up within a few seconds; photos whose file has vanished stay in the catalog (with their ratings and edits) with a broken-link mark. If you moved or renamed the whole folder, use *Relocate folder…* in the banner above the grid and pick the new location: files are re-linked by relative path and size. *Rescan* does the same check by hand (useful on network drives).
- **Move rejected files**: right-click a photo → *Move rejected files…* moves every photo marked as rejected (with its RAW and sidecars) to the **recycle bin** or into a **`_rejected` folder** next to the originals, after you confirm. Nothing is ever deleted permanently by the app; photos in the recycle bin show as missing until you restore them and rescan.

HEIC/HEIF (phone photos) need a decoder, tried in this order: a JPEG preview inside the file → the system decoder (Windows: install *HEIF Image Extensions* and *HEVC Video Extensions* from the Microsoft Store; built into macOS) → libheif (Linux: install `libheif` with its HEVC plugin; with the AI components installed, the libheif they ship is used automatically). Without any of them these photos have no thumbnails, see [Troubleshooting](#19-troubleshooting--faq).

Known limitations: RAW previews were only verified with synthetic samples, so reports on real camera files are welcome.

## 5. Culling workflow & shortcuts

Suggested keyboard flow: `←/→` browse → `1`–`5` rate (`Shift+digit` rates and moves on) → `P` pick / `X` reject → `S` expand a stack → `B` group view to choose the best → `D` edit.

Click selects one photo, `Ctrl+click` multi-selects, `Ctrl+A` selects all; rating, flags and colours apply to **all selected photos**. Press `?` any time for the shortcut overlay (generated from the keymap, same as below).

![Inspector](../images/inspector.png)

### Views & navigation

| Key | Action | Scope |
|---|---|---|
| `←` `→` | Previous / next | Grid, Loupe, Compare, Group |
| `↑` `↓` | Row up / down | Grid |
| `Home` `End` | First / last | Grid, Loupe, Compare, Group |
| `Enter` / `Space` | Open Loupe | Grid |
| `Esc` | Back to grid / clear selection | Grid, Loupe, Compare, Group |
| `G` `E` `C` `B` | Grid / Loupe / Compare / Group view | all but the edit page |
| `F` | Fullscreen | all but the edit page |
| `Tab` | Hide / show inspector (Grid, Loupe); swap A/B (Compare, Group) | |
| `S` (Compare, Group) | Toggle synced zoom | Compare, Group |
| `Space` | Fit / 100% | Loupe, Compare, Group |
| Hold `Z` | 100% magnifier | Loupe, Compare, Group |
| `Shift+F` | Show / hide face boxes | Loupe, Compare, Group |

**Zoom.** 100% shows the photo's own pixels: one image pixel per screen pixel, so focus and noise can be judged as they are (on a high-DPI screen with Windows scaling above 100% the photo is therefore smaller at 100% than the page text suggests). Browsing at *Fit* uses the preview only, so flipping through photos stays instant. Zooming in past what the preview holds (photos above about 16 MP) loads the full-resolution image in the background; the preview stays on screen until it arrives, the first time about half a second for a 24 MP JPEG, afterwards from the cache. RAW files show their embedded JPEG, so their 100% is that JPEG's resolution.

**Compare.** `C` opens Compare with 2-up, 3-up or 4-up cells (buttons at the top right). If two or more photos are selected in the grid, those photos are the cells, in grid order; with more selected than cells, `←` `→` step through the selection and the view shows the page holding the cursor. With one or no photo selected, A is the photo you came from and the cells fill with the photos that follow the cursor.

### Rating & marking

| Key | Action |
|---|---|
| `0`–`5` | Set rating (`0` clears) |
| `Shift+0`–`Shift+5` | Rate and jump to the next photo |
| `P` / `X` / `U` | Pick / reject / clear flag |
| `6` `7` `8` `9` | Colour label: red / yellow / green / blue |
| `A` | Accept the AI rating (current or selected) |
| `Ctrl+Shift+A` | Accept all AI ratings (asks to confirm; one `Ctrl+Z` undoes it; overwrites existing ratings) |

> Stars and flags are independent: **pick / reject is a flag**, the rating is 0–5. Reject only marks a photo; no file is deleted. **Your rating always wins over the AI rating**; both are stored separately.

### Stacks, groups & filters

| Key | Action | Scope |
|---|---|---|
| `S` / `Shift+S` | Expand / collapse current stack / all stacks | Grid |
| `,` `.` | Previous / next group | Grid, Group |
| `Enter` / `Shift+Enter` | Pick A / pick A and go to next group | Group |
| `Shift+P` | Person filter | all but the edit page |
| `Ctrl+K` | Command palette: fuzzy-search the actions available on the current screen (generated from the keymap) and page shortcuts; `↑` `↓` select, `Enter` runs | everywhere (incl. edit page) |
| `Ctrl+J` | Open AI assistant | everywhere (incl. edit page) |
| `Ctrl+E` | Export | everywhere (incl. edit page) |
| `?` | Shortcut help | everywhere (incl. edit page) |

#### Customising shortcuts

*Settings → Shortcuts* lists every action with its current key. **Click a key, then press the new combination** (`Esc` cancels). The change is saved at once in the server settings, so the desktop app and the LAN web UI (owner login) use the same keys; the `?` overlay and the `Ctrl+K` palette show the new keys. A key can be reused in different places (for example `S` is "sync zoom" in compare and "expand stack" in the grid) but never twice in the same place. If the combination is already taken you see which action holds it and can **swap** the two actions or **cancel**. The arrow button next to a custom key resets it; *Reset all to defaults* clears every change. On macOS `Ctrl` is stored as `⌘`/`Ctrl` ("mod") and shown as `⌘`.

Fixed shortcuts (marked with a lock) cannot be changed, and no other action can take their keys: `Esc` (back / close / cancel), `?` (help), `Ctrl+K` (command palette), holding `Z` (100% loupe), `Space` to open a photo from the grid, `D` and `Esc` to leave the edit page, and `Shift+arrows` (extend the selection in the grid). Guests on the LAN web UI cannot open the settings and always use the default keys.

**More filters.** The filter bar also offers (each appears only when the session has something to choose from): a *Lens* button to tick the lenses found in the session; a *Keywords* button to tick keywords (any of them by default, or tick *Photos must have all selected keywords*); a *Date* button with *From* / *To* dates (inclusive, in the local time of the place where the photos were taken, limited to the span of the session); and a *Sharpness* chip group: Sharp / Blurry / Not analysed (click again to clear; *Blurry* includes photos carrying the blurry issue, *Not analysed* means no sharpness result yet). Active conditions show as removable tags under the bar and are saved with the rest of the filter as a smart collection. Right-click a face and choose *Photos with similar faces*: a *Similar to chosen faces* tag appears; drag its slider to tune how alike a face must be (higher is stricter, default 38%, applied on release). In the Search-by-face dialog the *Minimum similarity* slider decides which faces count as similar (default 25%), and *Show these photos in the library* turns the face into the same filter.

**Device filter.** When a session mixes several cameras or phones (or has photos without camera EXIF), the filter bar shows a *Device* button. Tick one or more devices (each shows a kind icon: phone, camera, drone, action camera) or *No camera info* to narrow the library; the button shows how many are active, and "Clear filters" resets it. The Inspector shows the same kind icon next to the camera name.

**Time alignment.** Cameras and phones rarely agree on the time; photos taken at the same moment can look hours apart, and bursts and events are split wrongly. Once a session has two or more devices and has been analysed, the *Device* popover shows *Time alignment* below the list: each device with its current correction (*no correction* or for example `-1h 02m 05s`) and the program's estimate, found from the GPS time or from pairs of photos in which different devices shot the same scene, with a confidence (high / medium / low) and the number of matches. *Accept* uses the estimate, *Accept N estimates* takes every estimate with at least 50% confidence at once, *Edit* types an offset (`-1h 02m 05s`, `+90s`, `1:02:05`, at most +-7 days; it is added to that device's capture times), *Reset* removes a correction, and the refresh button in the corner estimates again. A device that does not overlap with the others gets no estimate; type its offset instead. After a change bursts, scenes and events are grouped again (groups you split or merged by hand stay). A correction only changes the time used for grouping: your photo files and their EXIF capture times are never touched, and the grid still sorts by the original capture time.

### Edit page shortcuts

| Key | Action |
|---|---|
| `D` / `Esc` | Back to library |
| `\` | Before / after (show original) |
| `O` | Toggle mask overlay |
| `R` | Crop mode |
| `Shift+E` | Brush erase tool |
| `Ctrl+Shift+C` / `Ctrl+Shift+V` | Copy / paste settings |
| `Ctrl+Z` / `Ctrl+Shift+Z` (or `Ctrl+Y`) | Undo / redo |

### Touch (WebUI)

Loupe: swipe left/right for previous/next; double-tap toggles Fit ↔ 100%; wheel zooms around the cursor, drag pans. The phone-specific gestures (swipe up to pick, down to reject, quick cull) are in [section 20](#20-mobile--remote-ai-experimental).

## 6. Smart analysis & stars

Click **Analyze** in the top bar, choose a profile and press *Start analysis*:

![Analysis profiles](../images/analyze-menu.png)

| Profile | Covers | Models |
|---|---|---|
| **Fast** | Blur / exposure / face detection | Almost nothing to download |
| **Standard** (default) | Adds people recognition, aesthetic and image-quality scores, scene classification | About 2 GB depending on tier |
| **Quick (on device)** (Lite) | Blur / exposure / noise scores, tilt detection and burst grouping; no closed-eye, people, aesthetic or scene scores | None: runs on this device with built-in algorithms and needs neither the AI components nor any model |
| **Standard (remote AI, pairing required)** | Same as Standard, but downscaled copies of your photos are analysed by a paired computer at home | Models live on the host; pair first, see [section 20](#20-mobile--remote-ai-experimental) |

- Analyse only the selection with *Analyse only the N selected*;
- **Before the first analysis** the app explains what face recognition is used for (closed-eye and expression checks, grouping and filtering by person, best take, portrait retouching) and asks whether to use it. Until you agree no face is detected and no face feature is extracted; *Don't use face recognition* switches it off and the analysis runs without face-based results (closed eyes, people, best take). Change it any time in *Settings → Faces & privacy*;
- Progress runs: analysing photos → burst grouping → scoring → people clustering. **Pause** in the status bar stops after the batch in flight (up to 32 photos) and keeps what is done; **Continue** analyses only the photos that are left, **Cancel** groups and scores what was analysed so far. A paused analysis stays paused after you restart the app;
- **If the app is closed or crashes during an analysis**, the next start continues it automatically with the photos that are left (finished batches are not analysed again; the status bar says it was continued). An analysis that keeps taking the app down is not restarted more than three times: start it again by hand and it only handles the photos without analysis;
- **Without the AI components** (not installed, or a model download failed) the analysis does not just fail: the model download dialog, the install hint and the status bar offer *Use quick analysis* / *Finish with quick analysis*, which groups bursts and scores sharpness, exposure and noise on this device for the same photos. Install the AI components later and run the Standard analysis again: the quick results are upgraded;
- **After you install a new model** (say the aesthetic or scene model) the *Analyze* menu tells you how many analysed photos can get the new scores; starting the analysis then computes only those steps for them, nothing else is recalculated;
- In *Settings → Analysis* set the default profile, **analyse after import**, and burst-grouping strictness (loose / normal / strict).

### AI stars & score breakdown

After analysis every photo gets an AI suggested rating (0–5, half stars possible) and a 0–1 overall score. The inspector's *Score breakdown* lists each factor and its contribution:

| Factor | Meaning |
|---|---|
| Sharpness / Exposure / Low noise | Technical metrics |
| Quality / Aesthetic | Model estimates |
| Face | Eyes open, smile, looking at camera |
| Composition | Rule of thirds, headroom, level horizon, subject not cut off by the frame edge (a small weight in the score; shown only when the photo has a face or a clear subject) |

Readable reasons are shown, e.g. "Alice has closed eyes", "Sharp focus", "Best of 4 burst frames". Issue tags: closed eyes, blurry, overexposed, underexposed, noisy, tilted, bystander, expression not great. *Bystander* means a sharp, reasonably large face that is not one of the photo's subjects; faces on statues, posters, screens and printed matter are recognised by their flat, colourless look and are not counted (a heavily retouched or black-and-white photo, or a colourful smooth poster, can still fool this), and "Remove bystanders" skips them (you can still remove such a face by hand). *Expression not great* marks a subject caught mid-word (mouth open without a smile) or half-blinking; smiling or laughing is never tagged, and it needs the face landmarks (not in the Lite profile). Neither tag changes the rating. *Tilted* means the horizon or the building verticals are 2–10° off level (the reason shows the angle, e.g. "Horizon is tilted by 3.5°"); it is only judged when the photo has clear straight lines, so many tilted photos without them are not tagged, and it never lowers the rating: fix it with *Crop & straighten* (`R`) in the editor.

- Click *Accept* in the inspector, or press `A`, to write the AI rating into **your** rating; `Ctrl+Shift+A` accepts in bulk.
- The *AI rating* filter filters by suggested stars.

### My taste

*My taste* learns which photos **you** keep. Every pick (`P`) or reject (`X`), and every 4–5 star (keep) or 1–2 star (reject) rating you make by hand, is a training example; 3 stars count as neutral. Decisions the app makes (assistant actions, *Accept AI rating*) are never used. It learns across all your sessions and everything stays on this computer.

- **Getting started:** it needs at least 30 kept and 30 rejected analysed photos. Until then *Settings → My taste* (also reachable from Home) shows what is missing, e.g. "12 more picks and 5 more rejects".
- **Only when it helps:** each training checks itself with 5-fold cross-validation on your own decisions and compares against the generic AI score (AUC). It only switches on when it is clearly better; the card shows both numbers.
- **Using it:** choose *Sort → My taste* in the grid to put the photos you're most likely to keep first. The inspector shows the photo's taste score; click *Why?* to see what speaks for and against it (e.g. "Open eyes ↑", "Blur ↑"). While it is on, the AI score is also blended with your taste (weight α up to 0.6).
- **Training:** automatic after 50 new decisions, or after 10 new decisions once you pause culling for two minutes; *Retrain* trains right away. Training runs in the background and doesn't block the app or analysis.
- **Older decisions:** decisions recorded by an earlier version may include the assistant's "keep the best, reject the rest" flags (that version could not tell them apart); the card shows how many, and *Forget my taste* starts fresh. A decision only counts while the photo still has that rating or flag, so changes from the assistant, *Accept AI rating*, XMP sync or Lightroom silence it, and undoing them brings it back.
- **Privacy:** switch *Learn from my decisions* off to stop recording (picks made while it is off are not recorded later either) and remove the personalisation at once. *Forget my taste* deletes the recorded decisions, the model and its scores (tick *Also stop learning* to opt out at the same time). Your stars and flags are never changed.

> **Honest note:** the issue thresholds were tuned on a 1000-image synthetic test library, not yet on a large set of real photos. Bright or dark frames that keep their detail (white backdrops, paper, neon streets at night) are no longer tagged over- or underexposed; inside a burst, a frame clearly softer than the sharpest one is tagged "blurry". Soft-background portraits can still be tagged blurry. Treat AI stars as suggestions.

### Search by description

Type what is in the photo into the top-bar search box, **in Chinese or English**: "sunset at the beach", "a cat on the sofa", "海边日落". The search runs about 0.4 s after you stop typing; press Enter to run it at once, `Esc` or the × in the box to clear it.

- Results are ranked **best match first**. A "Search: …" pill appears in the filter bar; click its × to remove it. Search combines with the other filters (for example "3 stars and up + search: sunset").
- At most the 300 best matches are shown; when nothing reaches the match threshold you see "No photos match …".
- **Requirements**: (1) the photos went through the **standard analysis** (search uses the image features that analysis stores; photos that were only analysed with the fast profile, or not at all, cannot be found, and the app tells you how many); (2) the **search model** (`siglip2-base-text`, about 550 MB, one-time download, offline afterwards) is installed. Without it the box is greyed out and an *Install search model* button sits next to it.
- Guests on the LAN can search too.

> **Honest note:** search understands the overall content of a picture (scene, objects, colours, mood). It is not text recognition and does not count things; use the people filter for people. The model is multilingual but trained mostly on English, so **Chinese is usually less accurate than English**: if a Chinese query misses, try English or a simpler description. Recall has not been measured on a large set of real photos yet (the evaluation script is `bench/eval-search.py`).

## 7. Groups & stacks

Analysis splits photos into **scenes** (minutes apart, same place/theme) and **bursts** (seconds apart, near-identical frames). In the grid:

- every scene has a header ("Scene N · X groups · Y photos");
- bursts collapse into **stacks** showing only the best frame, with a count badge. Click the badge or press `S` to expand, `Shift+S` for all;
- the *Grouped / Flat* toggle in the filter bar switches scene/stack display on or off.

![Expanded stack](../images/stack-expanded.png)

### Group view

Select a photo inside a stack and press `B`:

![Group view](../images/group-view.png)

- **A / B compare**: A is the left base frame (marked *Recommended*), B the right comparison; `Tab` swaps, `S` syncs zoom and pan; the strip below chooses B;
- **Pick A** (`Enter`) and **Keep best, reject the rest** (pick A, mark the other frames rejected; undoable);
- **Split here** (B and later become a new group), **Merge with previous / next group**: fix the AI grouping by hand; both can be undone with `Ctrl+Z` (undoing a merge splits again at the old boundary);
- `,` / `.` previous / next group, `Shift+Enter` picks A and moves to the next group;
- **Expression matrix**: rows are people, columns frames, colours show the expression (green: eyes open and smiling, yellow: so-so, red: closed eyes / blurry); the *Best* column lists each person's best frames. A hint names frames "everyone is happy with"; if there is none you can *Compose best expressions for everyone*.

### Events

Analysis also gathers scenes into **events**: a new event starts when two neighbouring scenes are more than 2 hours apart, or (when the photos carry GPS) more than 5 km apart. The *Events* list in the left sidebar shows the open session's events (only when it has at least two): the first line is the date, the second the time span of that day and the position (coordinates when there is GPS), and the count sits on the right. Click an event to browse only its photos; click it again (or *All photos*) to clear. Events are recomputed after you split or merge groups. The `{event}` token of the export name template is this event name (for example `2024-05-03 1012-1440`).

### Timeline

*Timeline* in the left sidebar opens a strip above the grid with the photos of the library by **capture date**, newest tiles on the right. Each tile shows a thumbnail of the best photo of that period (not rejected, highest rating, highest AI score), a bar for how many photos it holds and the count.

- The strip starts with years. Click a tile to filter the grid to that period (the same capture-date range as in the filter bar); click it again to clear. The small magnifier on a tile (or double-click, or `+`) zooms in: year → months → days. The breadcrumb and the *Zoom out* button (or `-`) go back up.
- *All sessions / This session* switches between counting every session and only the open one. The strip takes the other filters of the grid into account (rating, flag, colour, people, …), so you can see, for example, where your 4-star photos were taken.
- Dates are the **local** capture dates (the UTC offset stored with the photo is applied); a photo near midnight in another time zone lands on the day it was taken. Photos without a capture time are listed as a count under the strip and cannot be filtered by date.
- When a month or a day is shown, the **events** of that period appear as buttons under the tiles; click one to browse its photos (an event of another session opens that session).
- Keyboard: `Tab` into the strip, arrow keys / `Home` / `End` move between tiles, `Enter` or `Space` filters, `+` / `↓` zooms in, `-` / `↑` / `Backspace` zooms out.
- Photos taken at a different UTC offset than the one used for the filter can differ by a few photos from the tile count near the edge of a period, because the grid's date filter compares the stored capture time. The map half of F1.9 is not part of this release.

### Duplicates

Duplicate photos anywhere in the library (including other sessions) carry a ⧉ badge in the top-left corner of the grid cell:

- **Exact**: a copy with identical file content (copied to another folder, imported twice). Found right after import;
- **Near**: the same picture saved again without its capture metadata (downloaded from a chat app, saved from the web, a screenshot of the file). Needs an analysis first; neighbouring frames of a burst are not treated as duplicates.

*Smart collections → Duplicates* shows just these photos. imagePicker only marks them; it never deletes or moves a file.

## 8. People & face filters

After analysis, faces are clustered into **people** on your machine (AuraFace identity features).

- **People page** (top bar *People*): a card per person with photo counts. *Rename*, *Hide person*, tick several and *Merge selected*, or *View photos together* (two or more people).

  ![People page](../images/people.png)

- **Undoable**: renaming, hiding / unhiding and merging people, and moving faces with *Not this person* / *This is a new person*, can be undone with `Ctrl+Z` and redone with `Ctrl+Shift+Z`. Undoing a merge re-creates the merged people from their original faces (names and hidden state are kept; person ids and saved beauty profiles are not restored).
- **Best per person**: switch to the *Best per person* view to see N best photos per person (1 / 3 / 5…).
- **Export by person**: exports the selected people's photos into one folder per person.
- **Search by face**: drop or paste (`Ctrl+V`) an external photo (JPG/PNG/WebP) to find the same person in your library; the image is processed locally only. The results list likely people (click one to filter) and similar faces.
- **This is a new person**: when the person is not among the matches, or the AI mixed two people up, tick their faces among the similar faces of the search results (close matches come pre-ticked; one face per photo) and click *This is a new person*, optionally with a name. The faces leave their old person and are locked to the new one, so re-analysing never moves them back.
- **On photos**: with face boxes on (`Shift+F`), click or right-click a face: filter photos with this person / exclude this person / name / "Not this person" (splits into a new person) / "Find similar faces" (search with this face, then make the look-alikes a new person together).

### Person filter (`Shift+P`)

![Person filter](../images/person-filter.png)

Click a person to cycle **off → include → exclude**. Match mode: *All together (AND)* or *Any (OR)*. You can also require the selected people to be eyes open / smiling / looking at camera / the subject, limit head count (single, 2–3, many, none), and choose whether photos where they appear only as background bystanders count. The result count updates live; *Save as smart collection* keeps the filter.

> Face recognition only starts once you agree (asked before the first analysis, see section 6). Turn it off or withdraw consent in *Settings → Faces & privacy*. Then faces are neither detected nor clustered, and people, best take and beauty profiles are unavailable; delete what was already extracted with *Clear all face data*.

## 9. Best take

Someone blinked in the group shot? In group view click **Compose best expressions for everyone** under the matrix (or *Best expression (this burst)…* in the Repair panel):

![Best take editor](../images/besttake.png)

1. Pick the **base** frame at the top (the most suitable frame is chosen automatically: fewest and smallest faces to replace, nobody missing, not blurry or badly exposed; the group's best photo only gives way when another frame clearly needs less compositing. Hover the base button to see why; the frame list shows how many faces each frame would need replaced);
2. **Best for everyone** uses that base and chooses the best composable expression for everyone who can clearly improve (gains under 4 points are skipped to avoid compositing artefacts); *Best for everyone on this base* keeps your chosen base frame;
3. Or go manual: click a face in the photo and the right panel lists that person's **candidate expressions (score)**; click one to paste it onto the base; the *base* entry restores the original;
4. Tune *Blend strength* and *Edge feather*; *Compare original* (`\`); *Restore this person*;
5. *Done* returns to the library. The result is a generated layer in the edit stack; undoable, original untouched.

Candidates that cannot be composed are greyed out with a reason (head turned too much, face occluded, face too small, face not found); camera movement, occlusion or seams may produce warnings.

> Known limitation: the automatic base only looks at expression scores, face sizes, missing people and blur/exposure issues; camera movement is detected only while compositing. It needs the AI components and their models (face landmarks, portrait segmentation, BiRefNet).

## 10. Editing

Press `D` in Grid/Loupe (or right-click → *Edit*) to open the edit page. Every adjustment is **non-destructive** and saved automatically ("Saved" at the top). The filmstrip at the bottom switches photos; `D` / `Esc` returns.

![Edit page: basic](../images/edit-basic.png)

### AI one-click

*AI one-click* offers **Auto / Portrait / Landscape** (rule-based; the result is slider values you can keep tuning). **Edit suggestions** (needs the local VLM, T2+) proposes adjustments with a reason; *Apply* applies it (undoable).

> *Match reference…* is a greyed-out placeholder for now (the tooltip says "Coming in a later release"); it cannot be used yet.

### Basic & colour

| Panel | Contents |
|---|---|
| **Basic** | Exposure, contrast, highlights, shadows, whites, blacks, temperature, tint, vibrance, saturation, clarity, dehaze |
| **Curves** | Click to add a point, drag to adjust, double-click or right-click to delete |
| **HSL** | Hue, saturation, luminance for 8 bands (red, orange, yellow, green, aqua, blue, purple, magenta) |
| **Colour grading** | Shadows / midtones / highlights wheels + balance |
| **Crop & straighten** (`R`) | Aspect (original / free / …), angle; *Auto straighten* and *Auto crop* (see below) |
| **Presets** | Built-in presets (Natural, Japanese clean, Cinematic, Classic B&W, Soft portrait, Landscape pop, Warm / Cool film, Vivid, Matte, High-contrast B&W, Teal & orange, Golden hour…); *Save as preset* |
| **LUT** | Built-in LUTs; import your own `.cube` (type the file path, must be inside an allowed folder), with an amount slider |
| **Output sharpen** | Sharpening applied on output |

Each section has *Reset section*; *Reset all* is at the top; *History* lists every step.

#### Auto straighten and Auto crop

In *Crop & straighten*, pick an aspect, then:

- **Auto straighten** levels the horizon (or the building verticals) in one click. It uses the tilt measured when the photo was analysed (or measures it on the spot if the photo has not been analysed), sets the angle, and takes the largest crop of the chosen aspect that stays inside the rotated photo. Photos that are already level are left alone ("Already level"); if there is no clear horizon or vertical line you are told so and can use the angle slider.
- **Auto crop** suggests a rule-of-thirds crop for the chosen aspect (*Free* keeps the photo's own shape): it keeps every detected face inside the frame and places them on a third line, or, without faces, the most eye-catching region. A subject that is large in the frame is centred instead.

Both only change the crop in the edit stack (one undo step, the original is never touched); fine-tune with the crop tool (`R`) or the angle slider afterwards.

### Local adjustments & masks

Under *Local adjustments* choose an **AI target**: Subject, Sky, Background, Person (a specific person or any), Skin, Hair, Clothes; or add a **Radial / Linear gradient** (drag the handles on the photo). Each local adjustment has its own exposure, contrast, highlights, shadows, temperature, saturation, clarity, dehaze, plus amount, invert and feather. `O` toggles the mask overlay.

![Local masks](../images/edit-masks.png)

The first AI mask asks to download a segmentation model. Without a sky model the sky mask falls back to a colour / brightness heuristic.

### Before/after, copy & paste, sync

- `\` temporarily shows the original; *Split* at the top does a split comparison;
- **Copy settings** (`Ctrl+Shift+C`, choose categories: crop, global, local, LUT, output sharpen) → **Paste settings** (`Ctrl+Shift+V`; multi-select in the grid to paste onto all);
- **Sync to group** copies the current adjustments to the burst's other photos and adapts them to their exposure / white-balance differences.

## 11. Portrait retouching

Open **Portrait retouch** on the edit page. The first time click *Prepare portrait data* (downloads landmark, pose and skin/body-mask models; computed once per photo).

![Portrait retouch](../images/portrait.png)

1. *Applies to*: **Everyone** or one specific person (settings are per person);
2. **Level**: Natural / Standard / Refined;
3. Sliders:
   - Skin: Smooth, Brighten, Eyes, Teeth, Dark circles, Blemish (on/off)
   - Face shape: Slim face, Chin, Big eyes, Slim nose
   - Body: Slim arms, Slim legs, Slim waist, Longer legs (needs a detected body pose; disabled automatically when only the head is visible or the body is occluded)
   - Protect bg: keep the background intact during body reshaping (on by default);
4. *Refined* with high values warns that the result may look over-retouched;
5. **Save as this person's profile**, then *Apply profile* on any photo containing that person, or apply profiles in bulk to a multi-selection.

> Known limitations: reshaping / beauty strengths were calibrated on synthetic portraits, so judge real faces yourself; generative background fill after body reshaping is not available.

## 12. Repair

The **Repair** panel on the edit page:

![Repair panel](../images/repair.png)

| Tool | How |
|---|---|
| **Remove bystanders (N detected)** | Hover to outline faces judged to be bystanders; click to erase (runs only after you confirm) |
| **Brush erase** (`Shift+E`) | Paint over what to remove; hold Space or middle mouse to pan, `Enter` applies; undo last stroke / clear strokes; adjustable brush size |
| **Restore faces** | Set *Face restore strength* and run (GFPGAN); faces under 48 px are skipped |
| **Denoise** | Set *Denoise strength* and run |
| **Generated layers** | Each result is a layer you can hide / show / delete; stored in the edit stack |

Generative tasks show progress and a *Cancel* button (in the Repair panel, the library status bar and the best-take editor's footer); a cancelled task leaves the photo untouched and stops the AI work in progress. Failures offer a retry. Every generative task is listed under *Settings → Task history*. Upscaling is chosen at [export](#13-export--presets).

> Known limitations: large removals (holes over 512 px) come out blurry or with ghosting; faces on statues, posters or screens are mostly skipped when finding bystanders (a flat, colourless look gives them away), but a colourful smooth poster or a black-and-white photo can still fool this, which is why you confirm before erasing.

## 13. Export & presets

Press `Ctrl+E` (or *Export* in the top bar):

![Export dialog](../images/export.png)

| Option | Description |
|---|---|
| **Preset** | Original (full size, original format, quality 95), WeChat (long edge 2048, quality 85), Xiaohongshu (3:4 crop to 1440×1920, quality 90), Instagram (4:5 crop to 1080×1350, quality 90); the three sharing presets remove the GPS position. The preset table comes from the app's backend; the AI assistant ("export the picks for Instagram") uses the same table |
| Scope | The N selected / everything in the current filter |
| Destination folder | Type it or *Browse…* (must be inside an allowed folder) |
| Long edge | Original size / 2048 / 4096 / custom pixels |
| Crop | None / 3:4 / 4:5 / 1:1 / 2:3 / 16:9: a centred crop, then scaled down to the long edge (never up) |
| Format | Original format (unedited photos at original size are copied as they are, RAW stays RAW; everything else is JPEG) / JPEG / PNG / TIFF (8-bit, uncompressed) / WebP (lossless) |
| Remove GPS location / Remove all metadata | See below |
| JPEG quality | Default 90; PNG, TIFF and WebP are lossless and ignore it |
| Naming template | `{name}` original file name, `{seq}` sequence number, `{date}` capture date, `{event}` event name (date and time span, e.g. `2024-05-03 1012-1440`; the import name for photos not analysed yet), `{scene}` scene (e.g. portrait), `{camera}` camera, `{rating}` stars; a live example is shown. Name clashes get `_1`, `_2`…: existing files are never overwritten |
| Watermark | Tick it to add a **text** or **logo image** watermark: position (3×3 grid), margin and opacity; text also has colour, size and shadow and may contain `{name}` `{date}` `{year}` `{camera}` `{event}` `{scene}` `{rating}` (e.g. `© {year} {camera}`); for a logo pick a PNG (transparency is respected) and set its width as a percentage of the long edge. The settings are remembered for next time |
| Upscale | Off / ×2 / ×4 (on CPU tiers each photo can take tens of seconds) |
| Export into one folder per person | Used from the People page |
| RAW+JPEG pairs | Export the JPEG (as shown, with edits; default), the RAW, or both |

While an export runs, its progress shows at the bottom right with a *Cancel* button: files already written stay, the remaining photos are skipped.

### Collage and contact sheet

Switch the export dialog from *Photos* to **Collage** to combine 2 to 30 photos (the selected ones, or everything in the filter) into a single image. The saved edits are rendered, colours are converted to sRGB, and the result is written in the format you pick (JPEG / PNG / TIFF / WebP) with the usual naming template (default `collage`; an existing file is never overwritten, the new one becomes `collage_1`…).

| Option | Description |
|---|---|
| Layout | **Grid** (rows and columns chosen automatically from the photos' shapes), **Story** (the first photo large, the others small beside or below it) or **Contact sheet** (many thumbnails with the file name and star rating under each; both can be switched off) |
| Canvas size | 4:5 (1080×1350), 1:1 (1080×1080), 16:9 (3840×2160), A4 at 300 dpi (2480×3508) or custom pixels (64 to 12000 per side, at most 80 megapixels) |
| Gap / outer margin | Pixels between photos and between the photos and the edge |
| Background | Colour of the gaps, the margin and the bars |
| Fit | **Fill**: each photo fills its cell and the overflow is cropped, centred on the faces when the photo has detected faces (otherwise centred); **Whole photo**: nothing is cropped, bars show the background |
| Format, quality, watermark | As above; the watermark is drawn once on the finished collage |

The preview in the dialog shows the real layout for the photos' shapes and updates as you change the options; the photo order is the selection order. A collage runs as a task with progress and a *Cancel* button; cancelling writes no file.

Export includes all your edits and carries the original's metadata over: EXIF (for RAW and TIFF files too; maker notes are not copied), XMP (rating, keywords, title, creator…; entries that only describe the original, such as its orientation or Lightroom/darktable develop settings, are left out) and IPTC. What each format can carry:

| Format | EXIF | XMP | IPTC | Colour profile |
|---|---|---|---|---|
| JPEG | ✓ | ✓ | ✓ | sRGB |
| TIFF | ✓ (no maker notes) | ✓ | ✓ | sRGB |
| PNG | ✓ | ✓ | — (title, keywords etc. are in the XMP) | sRGB |
| WebP | ✓ | ✓ | — (likewise) | sRGB |

Photos with a wide-gamut colour profile (Display P3, Adobe RGB) are converted to sRGB, as are their thumbnails and previews, and every re-encoded export embeds an sRGB ICC profile.

**Remove GPS location** deletes the GPS position from EXIF and XMP; **Remove all metadata** writes no EXIF, XMP or IPTC at all (the colour profile stays). Both also apply to photos that would be copied unchanged: JPEG, PNG and WebP originals are only cleaned, their image data stays byte for byte the same (a photo stored rotated keeps its orientation tag, otherwise it would show sideways); RAW, HEIF and TIFF originals cannot be cleaned in place and are written as a full-size JPEG (or the chosen format) instead. With neither option, exporting at original size without edits copies the file unchanged.

A **watermark** is drawn into the pixels after resizing and colour conversion, identically for every format. So with a watermark, photos that would otherwise be copied as they are (original format, original size, unedited) are **re-encoded** instead (as JPEG with the original format option), and the dialog says so; the original files are never modified. The built-in font (DejaVu Sans) covers Latin, Greek and Cyrillic but **not Chinese, Japanese or Korean**: for those, press *Choose…* next to *Font* and pick a TTF / OTF font file (collections such as `.ttc` are not supported), otherwise the export is refused with a message about the missing characters. Logo and font files must lie inside the allowed folders.

> Known limitations: WebP is lossless only (larger files) and at most 16383 px per side; TIFF is 8-bit uncompressed, and there is no 16-bit or Display P3 output; HEIC/HEIF photos can only be re-encoded on export where a HEIF decoder is available (as for thumbnails, see [Importing photos](#4-importing-photos)), and HDR (PQ/HLG) content is not tone-mapped; XMP stored inside DNG/CR3 files and extended XMP (over 64 KB, e.g. depth maps) is not copied (`.xmp` sidecars are); thumbnails made by an earlier version keep their colours until the cache is cleared (*Settings → Cache*).

## 14. AI assistant

Press `Ctrl+J` or click *AI Assistant*. Give commands in natural language (Chinese or English). The assistant **shows a plan first**: *Preview impact* shows which photos would change, then *Run*. Steps that reject, bulk-rate or export are marked *Needs confirmation* and ask once more; no original is ever deleted; afterwards *Undo the whole run* (or `Ctrl+Z`) restores everything in one step.

Every step that changes photos has a defined scope: when a command filters first, only the filtered photos are changed. If the local model proposes a step with no condition of its own that would act on every photo, although your command names a condition, the plan is rejected and the rules engine is used instead. A step that changes more than 500 photos, or more than half the session (and at least 20 photos), is a large change and needs confirmation; if a step would change more photos than the plan showed when it runs, it fails and you should ask again.

![AI assistant](../images/assistant.png)

Example commands (these work as written; the rules engine understands both languages):

| Goal | Examples |
|---|---|
| Filter (applied immediately, changes no data) | "only show 4 stars and above", "only show Alice's photos", "show only picked" |
| Rating / flags | "rate all photos 3 stars", "reject all photos with closed eyes", "accept AI ratings for all photos" |
| Keep per group / scene | "keep 2 per scene and reject the rest", "rate the 2 best 5 stars", "pick the best of each group" |
| Editing | "auto adjust", "unify the tone", "apply warm film to all photos" |
| Portraits & best take | "apply beauty profiles", "best take for everyone" |
| Repair | "remove bystanders" |
| Export | "export the picks for Instagram" |
| Describe / suggestions (needs local VLM, T2+) | "give me some edit suggestions" |

The Chinese equivalents (for example 「只看 4 星以上」「每个场景只留 2 张，其他淘汰」「一键修图」) work as well.

Quick buttons under the input: Pick best, Unify tone, Picked only, Remove bystanders.

**Engines** (badge next to the title):

| Engine | Notes |
|---|---|
| Rules | Chinese / English intent templates; works offline on every tier |
| Local model | Needs the assistant models (T2+, the `llm` extra of the AI components); the model only drafts the plan; tools and arguments are validated before anything runs |

Choose Auto / Rules only / Local model in *Settings → Hardware & models → Assistant engine*. Unrecognised commands get an explanation and examples.

From the command line: `imagepicker ask --session <id> "only show 4 stars and above"` (add `--execute` to run it).

## 15. LAN WebUI & guest access

> **Trusted LAN only:** the LAN WebUI and remote AI currently use **plain HTTP** (no HTTPS/TLS; TLS is planned for 1.x). Passwords, session cookies and photo content are not encrypted on the network and could be sniffed by other devices on it. Enable them only on your own home or office network; never expose the port to the internet, forward it through your router, or use it on untrusted Wi-Fi.

Let phones and tablets on the same network use your library in a browser (e.g. a 4090 desktop serves, a laptop or tablet browses).

1. *Settings → LAN / WebUI*: set the **owner password** first (first-time setup only from this computer; at least 8 characters). Optionally set a **guest password** (must differ from the owner's) and tick *Allow read-only guests*;
2. Turn on *Enable LAN access*. Default port 7878 (1024–65535; a port change needs a restart);
3. The page lists the access URLs and a QR code; open the link or scan the code on another device and sign in.

![LAN settings](../images/settings-lan.png)

| Role | Permissions |
|---|---|
| Owner | Full access |
| Read-only guest | Browse only: cannot rate, edit or export; cannot see people or face data |

Security notes: LAN access currently uses **plain HTTP** (no HTTPS), so passwords and photos are not encrypted on the network: enable it only on a trusted LAN and use a strong password; failed logins are limited to 5 per minute; remote devices can only reach the *allowed folders*; request origins are restricted. CLI: `imagepicker serve --lan` (the password may come from the `IMAGEPICKER_PASSWORD` environment variable so it stays off the command line).

## 16. Privacy & data locations

- Photos, face features, ratings and edits are **stored only on your machine** and never uploaded.
- **Originals are read-only**: importing, analysing, rating, XMP sidecar sync, editing and exporting never change a single byte of an original file (an automated test compares SHA-256 hashes before and after). The only exception is the *Modify originals (write embedded XMP)* mode that you choose and confirm yourself in *Settings → Interop (XMP)*, see section 17.
- The only network use is downloading models on demand (and the mirror you pick). With *Settings → Faces & privacy → Allow network* off, nothing is downloaded and the app runs fully offline.
- **Face recognition needs your consent**: its purpose is explained before the first analysis, and until you agree no face is detected and no face feature is extracted. Turn it off or withdraw consent in *Settings → Faces & privacy*; *Clear all face data* deletes every face, person and beauty profile (photos and ratings stay) after you type the confirmation word.
- LAN access is off by default; passwords are stored as Argon2id hashes.

### Catalog backups & restore

- At every start the catalog's integrity is checked in the background (`PRAGMA quick_check`) without slowing the start down.
- The catalog is backed up once a day to `backups/` in the data directory (`catalog-<date>-<time>.db`, UTC), keeping the latest 7; *Settings → Catalog backups → Back up now* makes one at once. Backups contain the catalog only (ratings, flags, edits, people, analysis results), **not your originals** — back those up yourself.
- If the catalog is damaged so badly that it cannot be opened, the app moves it to `backups/corrupt-….db`, starts with an empty catalog and shows *The catalog cannot be opened* with the available backups; damage found by the start-up check is reported the same way. Pick a backup and click *Restore*; originals are not affected.
- Restoring replaces the current catalog with the backup (changes made after it are lost); the replaced catalog is kept as `backups/replaced-….db` and the page reloads. You can also restore any backup from *Settings → Catalog backups*.

### Logs, crash records & diagnostics

- The app writes log files to `logs/` in the data directory (`imagepicker-<date>.<n>.log`, UTC). A new file starts every day and after 5 MiB; files older than 14 days are deleted and the folder is kept under 40 MiB.
- If the app crashes (a Rust panic) or the AI worker dies with an unhandled error or a native fault, a crash record is written to `crashes/` first (time, version, system, thread, message, backtrace; for the worker the last lines of its output). The latest 30 are kept.
- *Settings → Diagnostics → Export diagnostics* saves a zip for bug reports: versions, an OS and hardware summary, your settings (passwords and tokens removed), the latest logs and crash records (file paths and secrets masked), and catalog statistics (counts, schema version, integrity check, root folder names). It **never** contains photos, thumbnails, face crops or face features, the catalog itself, or the path or name of any photo. Nothing is uploaded; the owner only (guests cannot export). Masking is automatic but best effort: open the zip and look before you share it.

### Data locations

Default data directory (override with `IMAGEPICKER_DATA_DIR` or `--data-dir`):

| OS | Path |
|---|---|
| Windows | `%APPDATA%\imagePicker` |
| macOS | `~/Library/Application Support/imagePicker` |
| Linux | `$XDG_DATA_HOME/imagePicker` (usually `~/.local/share/imagePicker`) |

| What | Where (inside the data directory) |
|---|---|
| Catalog (ratings, flags, edits, people) | `catalog.db` |
| Catalog backups (one a day, 7 kept) | `backups/` |
| Thumbnail / preview / full-resolution (`cache/full`, at most 1 GB) / AI mask / edit render / generated-result caches | `cache/` (clear in *Settings → Cache*; rebuilt on demand) |
| Generated patch layers of edits | `edits/` |
| Imported LUTs | `luts/` |
| Logs (rotating, 14 days / 40 MiB) | `logs/` |
| Crash records | `crashes/` |
| AI models | default `<platform data dir>/imagePicker/models`, shown in Settings (`IMAGEPICKER_MODELS_DIR` overrides) |

Backup tip: the app backs `catalog.db` up to `backups/` every day (see above); for a full backup copy `backups/` (or `catalog.db` while the app is closed) and `edits/`; caches and models can be rebuilt.

## 17. Lightroom / darktable interop (XMP)

Choose a mode in *Settings → Interop (XMP)*:

| Mode | Behaviour |
|---|---|
| **Off** (default) | XMP is neither read nor written |
| **Sidecar** | Reads `<name>.xmp` on import (the XMP embedded in a JPEG when there is no sidecar); after a rating change writes the sidecar (debounced, atomic, keeping other content). **Originals are never modified** |
| **Modify originals (write embedded XMP)** (advanced) | Like *Sidecar*, and also writes ratings, flags, colour labels and keywords into the XMP embedded in original JPEGs. **This rewrites the original files**: pixel data and other metadata stay the same and the modification time is kept, but the file contents and checksums change and backup / sync tools see them as modified. Choosing it asks for a second confirmation; use it only if your other software reads embedded XMP only, and back up your photos first |

> Older versions had a *Sidecar + embedded* mode that was described as "also reads embedded XMP" but rewrote originals. After the upgrade it becomes *Sidecar*; if you really want embedded XMP written, choose *Modify originals* again and confirm.

Conventions: rating → `xmp:Rating`, **reject → −1** (darktable convention, also understood by Lightroom); colour label → `xmp:Label`; keywords → `dc:subject` / `lr:hierarchicalSubject`.

**Conflicts:** when a sidecar is newer than the catalog the sidecar wins and you are told (choose *Use sidecar / Use catalog* to override). **Sync now:** pick a session and use *Read sidecar / Write sidecar* manually.

## 18. Settings

Open with the gear icon on Home or in the top bar.

| Section | Contents |
|---|---|
| **Hardware & models** | AI worker state, tier (with a manual override: Auto or T0–T3; the AI worker restarts when idle to apply it), compute device, assistant engine; model download source, models directory, model table (size / licence / status; download and delete) |
| **Analysis** | Default profile, analyse after import, burst-grouping strictness |
| **Faces & privacy** | Face recognition switch, face recognition consent (show explanation / withdraw), clear all face data, allow network |
| **Cache** | Usage per cache kind, clear, cache limit (default 20 GB; least recently used is evicted) |
| **Catalog backups** | Integrity state, latest backup, *Back up now*, the backups with *Restore* (see section 16) |
| **Diagnostics** | *Export diagnostics*: a zip for bug reports without photos, face data or photo paths (see section 16) |
| **Task history** | Recent best take, removal, enhancement, export and analysis tasks with their outcome (the last 200 are kept); running ones can be cancelled; tasks still running when the app exited are marked *Interrupted*. *Tasks* in the library status bar opens it too |
| **Rendering** | Render backend: Auto / GPU / CPU |
| **LAN / WebUI** | See section 15; also *Allowed folders* (import, export and folder browsing are limited to these roots) |
| **Interop (XMP)** | Off / Sidecar / Modify originals (confirmed twice), see section 17 |
| **Shortcuts** | Editable table, see *Customising shortcuts* in section 5 |
| **Remote AI** | Phone side: connect to a host at home (host address and 6-digit pairing code), see its state, disconnect; see section 20 |
| **Remote AI devices** | Host side: *Start pairing* shows a code and QR code, list of paired devices with *Revoke*; see section 20 |
| **Language & appearance** | UI language (简体中文 / English), theme (dark / light / follow system), **High contrast** (text, borders and focus rings meet WCAG AA; works with dark and light) and **Interface font size** (90%–150%; scales interface text and controls, not photos). The last two are stored on the current device / browser only |

![Settings: hardware & models](../images/settings-hardware.png)

![Light theme](../images/grid-light.png)

## 19. Troubleshooting & FAQ

| Symptom | What to do |
|---|---|
| Analyze says the AI engine is unavailable / the status dot is red | Install (or reinstall) the AI components in *Settings → Hardware & models*; the install log is `logs/runtime-install.log` in the data directory, worker logs are in `logs/` too |
| NVIDIA GPU but the tier shows T0 / CPU only | Install with `--extra cuda` (driver >= 580); otherwise it is treated as CPU |
| Model download fails / is slow | Switch to the HF mirror in *Settings → Hardware & models → Download source* (ModelScope currently behaves like *Auto*); check that *Allow network* is on |
| AI stars feel "off" | Thresholds are not calibrated yet; treat them as suggestions, let *My taste* adapt, adjust strictness in Settings |
| An analysis stopped with "AI analysis unavailable" / the model download failed | Click *Finish with quick analysis* (or choose *Quick (on device)* in the Analyze menu) to get grouping and sharpness / exposure scores now; install the AI components or retry the download later and run Standard again |
| Burst grouping too loose / tight | *Settings → Analysis → Burst grouping strictness*, or *Split / Merge* by hand in group view |
| HEIC photos have no thumbnails | No HEIF decoder is available (the log says what is missing): on Windows install *HEIF Image Extensions* + *HEVC Video Extensions* from the Microsoft Store, or the AI components; on Linux install `libheif` with its HEVC plugin (e.g. `libheif-plugin-libde265`). Restart the app and browse again; blurry small thumbnails can be rebuilt by clearing thumbnails in *Settings → Cache* |
| Cannot drop folders in the browser | Browser mode does not receive OS drops; use *Choose folder* or the desktop app |
| "This folder is not in an allowed directory" | Add it under *Settings → LAN → Allowed folders* |
| LAN devices cannot connect | LAN access enabled and owner password set, firewall allows the port (default 7878), same network; restart after changing the port |
| Login says "too many attempts" | Failed logins are limited to 5 per minute; wait as indicated |
| A generative task times out | Retry; upscaling and face restore are slow on CPU tiers; reduce load or use a higher tier |
| Cache too large | Clear kinds one by one or lower the limit in *Settings → Cache*; originals and edits are unaffected |
| Reset what *My taste* learned | *Settings → My taste → Forget my taste* |
| At start the app says *The catalog cannot be opened* or *The catalog failed its integrity check* | Pick a backup and *Restore* (section 16); the damaged file is kept in `backups/` in the data directory. Without a backup start over with the new empty catalog and import again (originals are not affected; ratings kept in XMP sidecars are read back on import) |
| No people / closed-eye tags after analysis | Face recognition is off or not agreed to yet: *Settings → Faces & privacy*, show the explanation and agree, then analyse again |

**FAQ**

- **Does it modify or delete my originals?** No. Ratings, flags and edits live in the catalog; *Reject* is only a mark; export writes to a new folder you choose (with XMP sidecars enabled only `.xmp` sidecar files are written next to photos). The only thing that rewrites originals is the *Modify originals (write embedded XMP)* mode, which you have to choose and confirm (section 17).
- **Does it need the internet?** Only to download models. Afterwards it can run fully offline.
- **Can I use it without a GPU?** Yes: fast analysis, basic editing and culling work; standard analysis, upscaling etc. are slower. Without the AI components you can still group and score basically with *Quick (on device)*.
- **Which camera RAW formats?** Common brands (see section 4); RAW is shown from its embedded preview.
- **Android?** Experimental: there is an app shell, a mobile UI, on-device quick analysis and remote-AI pairing, and CI builds a debug APK, but it has **not been verified on a real device** and there is no released installer. See [section 20](#20-mobile--remote-ai-experimental) and the [design document](../08-Android版本设计.md) (Chinese).
- **Natural-language search?** The top-bar search box works: describe the photo in Chinese or English. It needs the standard analysis and the search model (section 6, *Search by description*). A photo that cannot be found has most likely not been through the standard analysis.

More known issues: [backlog](../backlog.md) (Chinese). Please report problems via GitHub Issues and **never attach personal photos**.

## 20. Mobile & remote AI (experimental)

> **Trusted LAN only:** the LAN WebUI and remote AI currently use **plain HTTP** (no HTTPS/TLS; TLS is planned for 1.x). Passwords, session cookies and photo content are not encrypted on the network and could be sniffed by other devices on it. Enable them only on your own home or office network; never expose the port to the internet, forward it through your router, or use it on untrusted Wi-Fi.

> **Status:** the mobile UI already works in a narrow browser window (a phone using the WebUI of your computer at home, see section 15); an Android app shell exists too, but it is **not verified on real devices and there is no released installer**. What follows describes what the code does; how the Android app behaves in practice is to be confirmed in a later release.

### Mobile layout

Below 768 px window width (a phone in portrait) the app switches automatically: a bottom bar appears (**Library / Cull / People / Settings**) and the top bar collapses to *More* and *Filter*. Tablets (768–1024 px) and desktops use the wider layouts.

- **Culling:** one photo full screen; **swipe left/right** to browse, **swipe up to pick, down to reject** (an *Undo* shows afterwards), **double-tap to zoom**, **long-press to multi-select** (pick/reject then apply to all selected photos). While zoomed in a drag only pans. Picking/rejecting tries a light haptic tick (it may have no effect in some WebViews).
- **Quick cull:** the *Quick cull* button in the cull screen shows the AI's recommended cards of one burst group at a time: **swipe right = keep, left = reject, up = skip group**; the top shows "Group N/total" and the end gives "X kept, Y rejected". Run an analysis first so that burst groups exist.
- **Editing:** a bottom drawer with *Adjust* / *Portrait* tabs, parameters chosen horizontally and one slider to adjust, *Hold for original* to compare; *Expand / collapse panel* changes the drawer height.
- **Analysis:** the analyze button opens a bottom sheet to pick the profile. On a phone use **Quick (on device)** (no downloads; grouping and sharpness / exposure / noise scores only, no closed-eye or people) or, after pairing a computer at home, **Standard (remote AI)**.
- **Import from phone albums** (Android app): *Import from phone albums* on Home asks for photo permission and lists the albums; if you only granted selected photos it says *Only some photos are accessible* and offers *Allow all*; *Browse folders instead* is the alternative. Only albums under DCIM / Pictures / Download can be imported for now; others are listed but cannot be imported.

Known limitations: exporting to `Pictures/imagePicker`, sharing, moving to the trash and the foreground service (background analysis) exist in the code but **the UI is not wired to them yet**, so there are no such entry points on Android; HEIC on Android can only use the JPEG embedded in the file; there is no closed-eye detection, people clustering or portrait landmarks on the phone (use remote AI for those).

### Remote AI: let your computer at home analyse for the phone

When the phone is too weak, hand AI work to a computer at home that has the AI components installed (the "host"). The phone uploads **downscaled copies** of the photos; the host deletes them when done; originals never leave the phone.

1. **Host:** enable LAN access and set the owner password in *Settings → LAN / WebUI* (section 15); then open *Settings → Remote AI devices* and press **Start pairing**: a **6-digit code** and a QR code appear, valid for 5 minutes (press *Generate again* when it expires).
2. **Phone:** open *Settings → Remote AI*, enter the **host address** (e.g. `192.168.1.23:7878`) and the **6-digit pairing code**, press *Connect*. The Android app currently has **no camera QR scanner wired in**, so type them in (the *Scan QR code* button only shows when the platform provides a scanner).
3. Once paired the page shows *Connected / Host offline / Not paired* and the host tier, and the analyze menu offers **Standard (remote AI)**. *Disconnect* returns to *Quick (on device)*.
4. **Revoke:** *Settings → Remote AI devices* on the host lists the paired devices; *Revoke* disables a phone. Every phone has its own token and **can only use remote AI, not your library or settings**.

Limits and cautions:

- The transport is **plain HTTP**: use it on a trusted LAN only;
- The host exposes only a limited set of AI methods (analysis, AI masks, portrait preparation, best-take composing, removal, denoise / face restore / upscale, search by face); a phone cannot make the host download models, and the **local AI assistant (LLM/VLM) is not included**;
- While the host is offline the UI says "analysis jobs will wait in the queue"; the retry behaviour after a dropped connection has not been verified on a device;
- A phone outside the host's LAN cannot connect (there is no built-in tunnelling).

## 21. Uninstall

1. Uninstall the app the usual way (Windows: Settings → Apps; macOS: delete the app; Linux: delete the AppImage or `sudo apt remove` / `pacman -R`).
2. To remove all data as well, delete the [data directory](#data-locations) manually (catalog, caches and models). **This deletes your ratings, edits and people data**, so back up what you need first. Your original photos live in your own folders and are not touched.
3. If you used XMP sidecars, the `.xmp` files next to your photos remain (Lightroom / darktable can still use them); delete them yourself if you want.
