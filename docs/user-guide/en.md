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
20. [Uninstall](#20-uninstall)

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
| Left "Smart collections" | Built-in: Best per group, Has closed eyes, Undecided, Edited; save the current filter as your own collection |
| Grid | Split by scene; bursts collapse into stacks (badge shows the count); thumbnails show stars, flag and issue icons |
| Right inspector | Score breakdown, AI suggestion (with *Accept*), flag, colour, faces, EXIF (`Tab` hides / shows it) |
| Status bar | Thumbnail progress, selection count, picked / rejected / rated totals, connection state |

> The search box in the top bar (natural-language search) is a greyed-out placeholder for now.

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
- Models marked "non-commercial" are for personal use only;
- Sources: Auto (falls back to a mirror) / Hugging Face / HF mirror / ModelScope, chosen in *Settings → Hardware & models*;
- Models are stored locally; afterwards no network is needed. You can delete models in Settings to free space.

## 4. Importing photos

- **Desktop**: drag a folder onto Home, or click *Choose folder* (native picker).
- **Browser / phone**: click *Choose folder* and use the in-app folder browser (limited to the *allowed folders*, see [Settings](#18-settings)).
- Supported: JPEG, PNG, WebP, AVIF, TIFF, camera RAW (CR2/CR3, NEF, ARW, RAF, ORF, RW2, DNG and more) and HEIC/HEIF files. RAW thumbnails use the embedded preview.
- Import scans sub-folders recursively: *Scanning…* → *Generating thumbnails…* → *Done*. You can browse and rate while thumbnails appear.
- *Remove session* only removes the catalog record and caches; **originals are untouched**.
- The *Import from SD card / phone* button is disabled for now.

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
| `Ctrl+J` | Open AI assistant | everywhere (incl. edit page) |
| `Ctrl+E` | Export | everywhere (incl. edit page) |
| `?` | Shortcut help | everywhere (incl. edit page) |

**Device filter.** When a session mixes several cameras or phones (or has photos without camera EXIF), the filter bar shows a *Device* button. Tick one or more devices (each shows a kind icon: phone, camera, drone, action camera) or *No camera info* to narrow the library; the button shows how many are active, and "Clear filters" resets it. The Inspector shows the same kind icon next to the camera name.

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

Loupe: swipe left/right for previous/next; double-tap toggles Fit ↔ 100%; wheel zooms around the cursor, drag pans.

## 6. Smart analysis & stars

Click **Analyze** in the top bar, choose a profile and press *Start analysis*:

![Analysis profiles](../images/analyze-menu.png)

| Profile | Covers | Models |
|---|---|---|
| **Fast** | Blur / exposure / face detection | Almost nothing to download |
| **Standard** (default) | Adds people recognition, aesthetic and image-quality scores, scene classification | About 2 GB depending on tier |

- Analyse only the selection with *Analyse only the N selected*;
- Progress runs: analysing photos → burst grouping → scoring → people clustering;
- In *Settings → Analysis* set the default profile, **analyse after import**, and burst-grouping strictness (loose / normal / strict).

### AI stars & score breakdown

After analysis every photo gets an AI suggested rating (0–5, half stars possible) and a 0–1 overall score. The inspector's *Score breakdown* lists each factor and its contribution:

| Factor | Meaning |
|---|---|
| Sharpness / Exposure / Low noise | Technical metrics |
| Quality / Aesthetic | Model estimates |
| Face | Eyes open, smile, looking at camera |
| Composition | (not active yet, see below) |

Readable reasons are shown, e.g. "Alice has closed eyes", "Sharp focus", "Best of 4 burst frames". Issue tags: closed eyes, blurry, overexposed, underexposed, noisy, tilted (tilt detection is not implemented yet).

- Click *Accept* in the inspector, or press `A`, to write the AI rating into **your** rating; `Ctrl+Shift+A` accepts in bulk.
- The *AI rating* filter filters by suggested stars.

### My taste

*My taste* (on Home) learns from your ratings, flags and in-group choices and blends the base score with your taste using a weight α (0–0.6). It only turns on after it beats the base score on a held-out set; all data stays local and can be reset any time.

> **Honest note:** the issue thresholds were tuned on a 1000-image synthetic test library, not yet on a large set of real photos. Bright or dark frames that keep their detail (white backdrops, paper, neon streets at night) are no longer tagged over- or underexposed; inside a burst, a frame clearly softer than the sharpest one is tagged "blurry". Soft-background portraits can still be tagged blurry. Treat AI stars as suggestions.

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
- **Split here** (B and later become a new group), **Merge with previous / next group**: fix the AI grouping by hand;
- `,` / `.` previous / next group, `Shift+Enter` picks A and moves to the next group;
- **Expression matrix**: rows are people, columns frames, colours show the expression (green: eyes open and smiling, yellow: so-so, red: closed eyes / blurry); the *Best* column lists each person's best frames. A hint names frames "everyone is happy with"; if there is none you can *Compose best expressions for everyone*.

## 8. People & face filters

After analysis, faces are clustered into **people** on your machine (AuraFace identity features).

- **People page** (top bar *People*): a card per person with photo counts. *Rename*, *Hide person*, tick several and *Merge selected*, or *View photos together* (two or more people).

  ![People page](../images/people.png)

- **Best per person**: switch to the *Best per person* view to see N best photos per person (1 / 3 / 5…).
- **Export by person**: exports the selected people's photos into one folder per person.
- **Search by face**: drop or paste (`Ctrl+V`) an external photo (JPG/PNG/WebP) to find the same person in your library; the image is processed locally only.
- **On photos**: with face boxes on (`Shift+F`), click or right-click a face: filter photos with this person / exclude this person / name / "Not this person" (splits into a new person).

### Person filter (`Shift+P`)

![Person filter](../images/person-filter.png)

Click a person to cycle **off → include → exclude**. Match mode: *All together (AND)* or *Any (OR)*. You can also require the selected people to be eyes open / smiling / looking at camera / the subject, limit head count (single, 2–3, many, none), and choose whether photos where they appear only as background bystanders count. The result count updates live; *Save as smart collection* keeps the filter.

> Turn face recognition off in *Settings → Faces & privacy*. Then faces are neither detected nor clustered, and people, best take and beauty profiles are unavailable.

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

*AI one-click* offers **Auto / Portrait / Landscape** and *Match reference…*. **Edit suggestions** (needs the local VLM, T2+) proposes adjustments with a reason; *Apply* applies it (undoable).

### Basic & colour

| Panel | Contents |
|---|---|
| **Basic** | Exposure, contrast, highlights, shadows, whites, blacks, temperature, tint, vibrance, saturation, clarity, dehaze |
| **Curves** | Click to add a point, drag to adjust, double-click or right-click to delete |
| **HSL** | Hue, saturation, luminance for 8 bands (red, orange, yellow, green, aqua, blue, purple, magenta) |
| **Colour grading** | Shadows / midtones / highlights wheels + balance |
| **Crop & straighten** (`R`) | Aspect (original / free / …), angle |
| **Presets** | Built-in presets (Natural, Japanese clean, Cinematic, Classic B&W, Soft portrait, Landscape pop, Warm / Cool film, Vivid, Matte, High-contrast B&W, Teal & orange, Golden hour…); *Save as preset* |
| **LUT** | Built-in LUTs; import your own `.cube` (type the file path, must be inside an allowed folder), with an amount slider |
| **Output sharpen** | Sharpening applied on output |

Each section has *Reset section*; *Reset all* is at the top; *History* lists every step.

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

Generative tasks show progress and offer retry on failure. Upscaling is chosen at [export](#13-export--presets).

> Known limitations: large removals (holes over 512 px) come out blurry or with ghosting; faces on statues or posters can be taken for bystanders, which is why you confirm before erasing; generative tasks cannot be cancelled yet.

## 13. Export & presets

Press `Ctrl+E` (or *Export* in the top bar):

![Export dialog](../images/export.png)

| Option | Description |
|---|---|
| **Preset** | Original (full size, quality 95), WeChat (long edge 1920, quality 82), Xiaohongshu (1440, 88), Instagram (1080, 90) |
| Scope | The N selected / everything in the current filter |
| Destination folder | Type it or *Browse…* (must be inside an allowed folder) |
| Long edge | Original size / custom pixels |
| JPEG quality | Default 90 |
| Naming template | `{name}` is the original file name; a live example is shown |
| Upscale | Off / ×2 / ×4 (on CPU tiers each photo can take tens of seconds) |
| Export into one folder per person | Used from the People page |

Export includes all your edits, writes JPEG and carries the original's metadata over: EXIF (for RAW and TIFF files too; maker notes are not copied), XMP (rating, keywords, title, creator…; entries that only describe the original, such as its orientation or Lightroom/darktable develop settings, are left out) and IPTC. Photos with a wide-gamut colour profile (Display P3, Adobe RGB) are converted to sRGB, as are their thumbnails and previews, and every export embeds an sRGB ICC profile. The API option `strip_gps` removes the GPS position from both EXIF and XMP. Exporting at original size without edits copies the file unchanged.

> Known limitations: HEIC/HEIF photos can only be re-encoded on export where a HEIF decoder is available (as for thumbnails, see [Importing photos](#4-importing-photos)), and HDR (PQ/HLG) content is not tone-mapped; XMP stored inside DNG/CR3 files and extended XMP (over 64 KB, e.g. depth maps) is not copied (`.xmp` sidecars are); thumbnails made by an earlier version keep their colours until the cache is cleared (*Settings → Cache*).

## 14. AI assistant

Press `Ctrl+J` or click *AI Assistant*. Give commands in natural language (Chinese or English). The assistant **shows a plan first**: *Preview impact* shows which photos would change, then *Run*. Steps that reject, bulk-rate or export are marked *Needs confirmation* and ask once more; no original is ever deleted; afterwards *Undo the whole run* (or `Ctrl+Z`) restores everything in one step.

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

Let phones and tablets on the same network use your library in a browser (e.g. a 4090 desktop serves, a laptop or tablet browses).

1. *Settings → LAN / WebUI*: set the **owner password** first (first-time setup only from this computer; at least 8 characters). Optionally set a **guest password** (must differ from the owner's) and tick *Allow read-only guests*;
2. Turn on *Enable LAN access*. Default port 7878 (1024–65535; a port change needs a restart);
3. The page lists the access URLs and a QR code; open the link or scan the code on another device and sign in.

![LAN settings](../images/settings-lan.png)

| Role | Permissions |
|---|---|
| Owner | Full access |
| Read-only guest | Browse only: cannot rate, edit or export; cannot see people or face data |

Security notes: enable only on a trusted LAN and use a strong password; failed logins are limited to 5 per minute; remote devices can only reach the *allowed folders*; request origins are restricted. CLI: `imagepicker serve --lan` (the password may come from the `IMAGEPICKER_PASSWORD` environment variable so it stays off the command line).

## 16. Privacy & data locations

- Photos, face features, ratings and edits are **stored only on your machine** and never uploaded.
- The only network use is downloading models on demand (and the mirror you pick). With *Settings → Faces & privacy → Allow network* off, nothing is downloaded and the app runs fully offline.
- Face recognition can be turned off entirely; *Wipe all face data* deletes every face, person and beauty profile (photos and ratings stay) after you type the confirmation word.
- LAN access is off by default; passwords are stored as Argon2id hashes.

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
| Thumbnail / preview / AI mask / edit render / generated-result caches | `cache/` (clear in *Settings → Cache*; rebuilt on demand) |
| Generated patch layers of edits | `edits/` |
| Imported LUTs | `luts/` |
| Logs | `logs/` |
| AI models | default `<platform data dir>/imagePicker/models`, shown in Settings (`IMAGEPICKER_MODELS_DIR` overrides) |

Backup tip: back up `catalog.db` and `edits/`; caches and models can be rebuilt.

## 17. Lightroom / darktable interop (XMP)

Choose a mode in *Settings → Interop (XMP)*:

| Mode | Behaviour |
|---|---|
| **Off** (default) | XMP is neither read nor written |
| **Sidecar** | Reads `<name>.xmp` on import; after a rating change writes the sidecar (debounced, atomic, keeping other content) |
| **Sidecar + embedded** | Same, and also reads XMP embedded in JPEGs |

Conventions: rating → `xmp:Rating`, **reject → −1** (darktable convention, also understood by Lightroom); colour label → `xmp:Label`; keywords → `dc:subject` / `lr:hierarchicalSubject`.

**Conflicts:** when a sidecar is newer than the catalog the sidecar wins and you are told (choose *Use sidecar / Use catalog* to override). **Sync now:** pick a session and use *Read sidecar / Write sidecar* manually.

## 18. Settings

Open with the gear icon on Home or in the top bar.

| Section | Contents |
|---|---|
| **Hardware & models** | AI worker state, tier, compute device, assistant engine; model download source, models directory, model table (size / licence / status; download and delete) |
| **Analysis** | Default profile, analyse after import, burst-grouping strictness |
| **Faces & privacy** | Face recognition switch, wipe all face data, allow network |
| **Cache** | Usage per cache kind, clear, cache limit (default 20 GB; least recently used is evicted) |
| **Rendering** | Render backend: Auto / GPU / CPU |
| **LAN / WebUI** | See section 15; also *Allowed folders* (import, export and folder browsing are limited to these roots) |
| **Interop (XMP)** | See section 17 |
| **Shortcuts** | Read-only, generated from the built-in keymap |
| **Language & appearance** | UI language (简体中文 / English), theme (dark / light / follow system) |

![Settings: hardware & models](../images/settings-hardware.png)

![Light theme](../images/grid-light.png)

## 19. Troubleshooting & FAQ

| Symptom | What to do |
|---|---|
| Analyze says the AI engine is unavailable / the status dot is red | Install (or reinstall) the AI components in *Settings → Hardware & models*; the install log is `logs/runtime-install.log` in the data directory, worker logs are in `logs/` too |
| NVIDIA GPU but the tier shows T0 / CPU only | Install with `--extra cuda` (driver >= 580); otherwise it is treated as CPU |
| Model download fails / is slow | Switch to the HF mirror or ModelScope in *Settings → Hardware & models → Download source*; check that *Allow network* is on |
| AI stars feel "off" | Thresholds are not calibrated yet; treat them as suggestions, let *My taste* adapt, adjust strictness in Settings |
| Burst grouping too loose / tight | *Settings → Analysis → Burst grouping strictness*, or *Split / Merge* by hand in group view |
| HEIC photos have no thumbnails | No HEIF decoder is available (the log says what is missing): on Windows install *HEIF Image Extensions* + *HEVC Video Extensions* from the Microsoft Store, or the AI components; on Linux install `libheif` with its HEVC plugin (e.g. `libheif-plugin-libde265`). Restart the app and browse again; blurry small thumbnails can be rebuilt by clearing thumbnails in *Settings → Cache* |
| Cannot drop folders in the browser | Browser mode does not receive OS drops; use *Choose folder* or the desktop app |
| "This folder is not in an allowed directory" | Add it under *Settings → LAN → Allowed folders* |
| LAN devices cannot connect | LAN access enabled and owner password set, firewall allows the port (default 7878), same network; restart after changing the port |
| Login says "too many attempts" | Failed logins are limited to 5 per minute; wait as indicated |
| A generative task times out | Retry; upscaling and face restore are slow on CPU tiers; reduce load or use a higher tier |
| Cache too large | Clear kinds one by one or lower the limit in *Settings → Cache*; originals and edits are unaffected |
| Reset what *My taste* learned | *My taste → Reset my taste* |

**FAQ**

- **Does it modify or delete my originals?** No. Ratings, flags and edits live in the catalog; *Reject* is only a mark; export writes to a new folder you choose (with XMP enabled only `.xmp` sidecar files are written next to photos).
- **Does it need the internet?** Only to download models. Afterwards it can run fully offline.
- **Can I use it without a GPU?** Yes: fast analysis, basic editing and culling work; standard analysis, upscaling etc. are slower.
- **Which camera RAW formats?** Common brands (see section 4); RAW is shown from its embedded preview.
- **Android?** There is only a [design document](../08-Android版本设计.md) (Chinese) so far.
- **Natural-language search?** The search box is not active yet; use the assistant's filter commands meanwhile.

More known issues: [backlog](../backlog.md) (Chinese). Please report problems via GitHub Issues and **never attach personal photos**.

## 20. Uninstall

1. Uninstall the app the usual way (Windows: Settings → Apps; macOS: delete the app; Linux: delete the AppImage or `sudo apt remove` / `pacman -R`).
2. To remove all data as well, delete the [data directory](#data-locations) manually (catalog, caches and models). **This deletes your ratings, edits and people data**, so back up what you need first. Your original photos live in your own folders and are not touched.
3. If you used XMP sidecars, the `.xmp` files next to your photos remain (Lightroom / darktable can still use them); delete them yourself if you want.
