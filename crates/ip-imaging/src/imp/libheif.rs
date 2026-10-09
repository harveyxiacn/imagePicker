//! libheif (LGPL) loaded at runtime, never linked, so builds need no C toolchain and the app
//! ships no HEVC decoder. Candidates, first usable wins: `IMAGEPICKER_LIBHEIF` (path of the
//! library), the system `libheif.so.1` (Linux), then `libheif*` files in the directories set by
//! [`set_library_dirs`] (the copy pillow-heif installs into the AI runtime).
use std::ffi::{c_char, c_int, c_void, CStr};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::sync::Mutex;

use libloading::Library;

use super::thumb::Rgb;

pub const ENV_LIBHEIF: &str = "IMAGEPICKER_LIBHEIF";

#[repr(C)]
#[derive(Clone, Copy)]
struct HeifError {
    code: c_int,
    subcode: c_int,
    message: *const c_char,
}

const COLORSPACE_RGB: c_int = 1;
const CHROMA_INTERLEAVED_RGB: c_int = 10;
const CHANNEL_INTERLEAVED: c_int = 10;

type Ptr = *mut c_void;

/// The entry points used (all present since libheif 1.0, so 1.x is ABI compatible).
struct Api {
    _lib: Library,
    version: String,
    /// Has an HEVC decoder (plugin builds can lack one; AVIF may still work).
    hevc: bool,
    context_alloc: unsafe extern "C" fn() -> Ptr,
    context_free: unsafe extern "C" fn(Ptr),
    read_from_memory: unsafe extern "C" fn(Ptr, *const c_void, usize, *const c_void) -> HeifError,
    primary_handle: unsafe extern "C" fn(Ptr, *mut Ptr) -> HeifError,
    handle_release: unsafe extern "C" fn(Ptr),
    handle_width: unsafe extern "C" fn(Ptr) -> c_int,
    handle_height: unsafe extern "C" fn(Ptr) -> c_int,
    thumbnail_count: unsafe extern "C" fn(Ptr) -> c_int,
    thumbnail_ids: unsafe extern "C" fn(Ptr, *mut u32, c_int) -> c_int,
    thumbnail: unsafe extern "C" fn(Ptr, u32, *mut Ptr) -> HeifError,
    options_alloc: unsafe extern "C" fn() -> Ptr,
    options_free: unsafe extern "C" fn(Ptr),
    decode_image: unsafe extern "C" fn(Ptr, *mut Ptr, c_int, c_int, Ptr) -> HeifError,
    image_width: unsafe extern "C" fn(Ptr, c_int) -> c_int,
    image_height: unsafe extern "C" fn(Ptr, c_int) -> c_int,
    plane: unsafe extern "C" fn(Ptr, c_int, *mut c_int) -> *const u8,
    image_release: unsafe extern "C" fn(Ptr),
}

struct State {
    dirs: Vec<PathBuf>,
    private_dir: Option<PathBuf>,
    api: Option<&'static Api>,
    /// Why the last lookup failed; cleared by [`set_library_dirs`] so the next decode retries.
    failure: Option<String>,
}

static STATE: Mutex<State> = Mutex::new(State {
    dirs: Vec::new(),
    private_dir: None,
    api: None,
    failure: None,
});

/// See `crate::set_heif_library_dirs`.
pub fn set_library_dirs(dirs: Vec<PathBuf>, private_dir: Option<PathBuf>) {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    s.dirs = dirs;
    s.private_dir = private_dir;
    s.failure = None;
}

fn api() -> Result<&'static Api, String> {
    let mut s = STATE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(a) = s.api {
        return Ok(a);
    }
    if let Some(f) = &s.failure {
        return Err(f.clone());
    }
    let mut errors = Vec::new();
    let mut fallback = None;
    for (path, from_dir) in candidates(&s.dirs) {
        let path = match (from_dir, &s.private_dir) {
            (true, Some(private)) if cfg!(windows) => match private_copy(&path, private) {
                Ok(p) => p,
                Err(e) => {
                    errors.push(format!("{}: copy failed: {e}", path.display()));
                    continue;
                }
            },
            _ => path,
        };
        match load(&path) {
            Ok(a) if a.hevc => {
                let a: &'static Api = Box::leak(Box::new(a));
                s.api = Some(a);
                return Ok(a);
            }
            // e.g. a system libheif without its HEVC plugin: keep looking, keep it for AVIF
            Ok(a) => {
                errors.push(format!("{}: no HEVC decoder", path.display()));
                if fallback.is_none() {
                    fallback = Some(a);
                } else {
                    // never unload a library whose plugins were initialised
                    std::mem::forget(a);
                }
            }
            // a missing system library is the normal "not installed" case
            Err(_) if path.parent().is_none_or(|p| p.as_os_str().is_empty()) => {}
            Err(e) => errors.push(format!("{}: {e}", path.display())),
        }
    }
    if let Some(a) = fallback {
        let a: &'static Api = Box::leak(Box::new(a));
        s.api = Some(a);
        return Ok(a);
    }
    let msg = if errors.is_empty() {
        format!("libheif not found (install libheif or the AI components, or set {ENV_LIBHEIF})")
    } else {
        format!("libheif unusable: {}", errors.join("; "))
    };
    s.failure = Some(msg.clone());
    Err(msg)
}

/// `(path, found in a search directory)`.
fn candidates(dirs: &[PathBuf]) -> Vec<(PathBuf, bool)> {
    let mut out = Vec::new();
    if let Some(p) = std::env::var_os(ENV_LIBHEIF).filter(|v| !v.is_empty()) {
        out.push((PathBuf::from(p), false));
    }
    if cfg!(target_os = "linux") {
        out.push((PathBuf::from("libheif.so.1"), false));
    }
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else {
            continue;
        };
        let mut found: Vec<PathBuf> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| is_libheif(p))
            .collect();
        found.sort();
        out.extend(found.into_iter().map(|p| (p, true)));
    }
    out
}

pub(super) fn is_libheif(p: &Path) -> bool {
    let Some(n) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let n = n.to_ascii_lowercase();
    n.starts_with("libheif") && (n.ends_with(".dll") || n.ends_with(".dylib") || n.contains(".so"))
}

/// Windows keeps a loaded DLL locked until exit, which would break removing or re-installing
/// the AI runtime it came from: load a private copy of the DLLs next to it (its dependencies),
/// one directory per source version.
pub(super) fn private_copy(lib: &Path, private: &Path) -> std::io::Result<PathBuf> {
    let (Some(src_dir), Some(name)) = (lib.parent(), lib.file_name()) else {
        return Err(std::io::Error::other("not a file path"));
    };
    let md = std::fs::metadata(lib)?;
    let mtime = md
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut h = blake3::Hasher::new();
    h.update(lib.as_os_str().as_encoded_bytes());
    h.update(&md.len().to_le_bytes());
    h.update(&mtime.to_le_bytes());
    let key = h.finalize().to_hex()[..16].to_string();
    let dst_dir = private.join(&key);
    if !dst_dir.join(name).is_file() {
        let tmp = private.join(format!("{key}.tmp{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp)?;
        for e in std::fs::read_dir(src_dir)?.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dll")) {
                std::fs::copy(&p, tmp.join(e.file_name()))?;
            }
        }
        if std::fs::rename(&tmp, &dst_dir).is_err() {
            // another process won the race
            let _ = std::fs::remove_dir_all(&tmp);
        }
    }
    // older versions (best effort: a copy still loaded by another process stays locked)
    for e in std::fs::read_dir(private)?.flatten() {
        if !e.file_name().to_string_lossy().starts_with(&key) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    Ok(dst_dir.join(name))
}

#[cfg(windows)]
fn open(path: &Path) -> Result<Library, libloading::Error> {
    use libloading::os::windows::{
        Library as WinLibrary, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
    };
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    // SAFETY: loading runs the DLL's initialisers. Candidates come from the user's own setting
    // or the app's AI runtime, and dependencies resolve only next to the DLL or from system
    // directories (never the current directory or PATH).
    unsafe {
        WinLibrary::load_with_flags(
            abs,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
        )
    }
    .map(Library::from)
}

#[cfg(not(windows))]
fn open(path: &Path) -> Result<Library, libloading::Error> {
    // SAFETY: loading runs the library's initialisers; candidates come from the user's own
    // setting, the system library path or the app's AI runtime.
    unsafe { Library::new(path) }
}

fn load(path: &Path) -> Result<Api, String> {
    let lib = open(path).map_err(|e| e.to_string())?;
    macro_rules! sym {
        ($name:literal) => {
            // SAFETY: the field's type is the C signature of this libheif 1.x function.
            *unsafe { lib.get(concat!($name, "\0").as_bytes()) }
                .map_err(|e| format!("{}: {e}", $name))?
        };
    }
    let get_version: unsafe extern "C" fn() -> *const c_char = sym!("heif_get_version");
    // SAFETY: returns a static NUL-terminated string (or NULL).
    let version = unsafe {
        let v = get_version();
        if v.is_null() {
            String::new()
        } else {
            CStr::from_ptr(v).to_string_lossy().into_owned()
        }
    };
    if !version.starts_with("1.") {
        return Err(format!("unsupported libheif version {version:?}"));
    }
    // SAFETY: `heif_init` (1.13+) takes optional parameters; NULL selects the defaults. It is
    // reference counted and this runs once per process (under `STATE`).
    if let Ok(init) = unsafe { lib.get::<unsafe extern "C" fn(Ptr) -> HeifError>(b"heif_init\0") } {
        unsafe { init(null_mut()) };
    }
    // SAFETY: `int heif_have_decoder_for_format(enum heif_compression_format)`; 1 = HEVC.
    let hevc = unsafe {
        lib.get::<unsafe extern "C" fn(c_int) -> c_int>(b"heif_have_decoder_for_format\0")
    }
    .map(|f| unsafe { f(1) } != 0)
    .unwrap_or(true);
    Ok(Api {
        version,
        hevc,
        context_alloc: sym!("heif_context_alloc"),
        context_free: sym!("heif_context_free"),
        read_from_memory: sym!("heif_context_read_from_memory_without_copy"),
        primary_handle: sym!("heif_context_get_primary_image_handle"),
        handle_release: sym!("heif_image_handle_release"),
        handle_width: sym!("heif_image_handle_get_width"),
        handle_height: sym!("heif_image_handle_get_height"),
        thumbnail_count: sym!("heif_image_handle_get_number_of_thumbnails"),
        thumbnail_ids: sym!("heif_image_handle_get_list_of_thumbnail_IDs"),
        thumbnail: sym!("heif_image_handle_get_thumbnail"),
        options_alloc: sym!("heif_decoding_options_alloc"),
        options_free: sym!("heif_decoding_options_free"),
        decode_image: sym!("heif_decode_image"),
        image_width: sym!("heif_image_get_width"),
        image_height: sym!("heif_image_get_height"),
        plane: sym!("heif_image_get_plane_readonly"),
        image_release: sym!("heif_image_release"),
        _lib: lib,
    })
}

fn check(e: HeifError) -> Result<(), String> {
    if e.code == 0 {
        return Ok(());
    }
    let msg = if e.message.is_null() {
        String::new()
    } else {
        // SAFETY: libheif error messages are static NUL-terminated strings.
        unsafe { CStr::from_ptr(e.message) }
            .to_string_lossy()
            .into_owned()
    };
    Err(format!("libheif error {}.{}: {msg}", e.code, e.subcode))
}

/// Owned libheif object, released with `free` on drop.
struct Owned(Ptr, unsafe extern "C" fn(Ptr));

impl Drop for Owned {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: `self.0` came from the matching allocation function and is freed once.
            unsafe { (self.1)(self.0) }
        }
    }
}

/// Decodes the primary image (or, for thumbnails, a large enough thumbnail item) to stored,
/// untransformed RGB8. `stored` is the primary image's stored size from the container.
/// Returns whether a thumbnail item was used.
pub fn decode(
    data: &[u8],
    long_edge: u32,
    thumbnails: bool,
    stored: (u32, u32),
) -> Result<(Rgb, bool), String> {
    let api = api()?;
    // SAFETY: every handle comes from libheif and is released exactly once by `Owned`; `data`
    // outlives the context reading it without a copy.
    unsafe {
        let ctx = Owned((api.context_alloc)(), api.context_free);
        if ctx.0.is_null() {
            return Err("libheif: cannot allocate a context".into());
        }
        check((api.read_from_memory)(
            ctx.0,
            data.as_ptr().cast(),
            data.len(),
            null(),
        ))?;
        let mut h = null_mut();
        let err = (api.primary_handle)(ctx.0, &mut h);
        let primary = Owned(h, api.handle_release);
        check(err)?;
        if thumbnails {
            if let Some(t) = thumbnail(api, &primary, long_edge) {
                if let Ok(rgb) = decode_handle(api, &t) {
                    if same_aspect((rgb.w, rgb.h), stored) {
                        return Ok((rgb, true));
                    }
                }
            }
        }
        decode_handle(api, &primary).map(|rgb| (rgb, false))
    }
}

/// `true` if `stored` is unknown or the aspect ratios agree within 2%.
fn same_aspect((w, h): (u32, u32), stored: (u32, u32)) -> bool {
    if stored.0 == 0 || stored.1 == 0 || w == 0 || h == 0 {
        return stored.0 == 0 || stored.1 == 0;
    }
    let a = stored.0 as f64 / stored.1 as f64;
    ((w as f64 / h as f64) - a).abs() < 0.02 * a
}

/// The smallest thumbnail item whose long edge reaches `long_edge`.
///
/// # Safety
/// `primary` must be a live image handle of `api`.
unsafe fn thumbnail(api: &Api, primary: &Owned, long_edge: u32) -> Option<Owned> {
    let n = (api.thumbnail_count)(primary.0);
    if n <= 0 {
        return None;
    }
    let mut ids = vec![0u32; n as usize];
    let n = (api.thumbnail_ids)(primary.0, ids.as_mut_ptr(), n).clamp(0, n) as usize;
    let mut best: Option<(u64, Owned)> = None;
    for &id in &ids[..n] {
        let mut h = null_mut();
        let err = (api.thumbnail)(primary.0, id, &mut h);
        let t = Owned(h, api.handle_release);
        if check(err).is_err() || t.0.is_null() {
            continue;
        }
        let (w, hh) = ((api.handle_width)(t.0), (api.handle_height)(t.0));
        if w <= 0 || hh <= 0 || (w.max(hh) as u32) < long_edge {
            continue;
        }
        let area = w as u64 * hh as u64;
        if best.as_ref().is_none_or(|(a, _)| area < *a) {
            best = Some((area, t));
        }
    }
    best.map(|(_, t)| t)
}

/// # Safety
/// `handle` must be a live image handle of `api`.
unsafe fn decode_handle(api: &Api, handle: &Owned) -> Result<Rgb, String> {
    let opts = Owned((api.options_alloc)(), api.options_free);
    if opts.0.is_null() {
        return Err("libheif: cannot allocate decoding options".into());
    }
    // `struct heif_decoding_options` starts with `uint8_t version; uint8_t
    // ignore_transformations;` in every version: decode the stored pixels, the caller applies
    // the orientation from `irot`/`imir` like for every other decoder.
    *opts.0.cast::<u8>().add(1) = 1;
    let mut img = null_mut();
    let err = (api.decode_image)(
        handle.0,
        &mut img,
        COLORSPACE_RGB,
        CHROMA_INTERLEAVED_RGB,
        opts.0,
    );
    let img = Owned(img, api.image_release);
    check(err)?;
    let w = (api.image_width)(img.0, CHANNEL_INTERLEAVED);
    let h = (api.image_height)(img.0, CHANNEL_INTERLEAVED);
    let mut stride: c_int = 0;
    let plane = (api.plane)(img.0, CHANNEL_INTERLEAVED, &mut stride);
    if plane.is_null() || w <= 0 || h <= 0 || (stride as i64) < w as i64 * 3 {
        return Err(format!(
            "libheif {}: unexpected decoded image {w}x{h}, stride {stride}",
            api.version
        ));
    }
    let (w, h, stride) = (w as usize, h as usize, stride as usize);
    let mut data = Vec::with_capacity(w * h * 3);
    for y in 0..h {
        // SAFETY (caller contract): the plane holds `h` rows of `stride` bytes.
        data.extend_from_slice(std::slice::from_raw_parts(plane.add(y * stride), w * 3));
    }
    Ok(Rgb {
        w: w as u32,
        h: h as u32,
        data,
    })
}
