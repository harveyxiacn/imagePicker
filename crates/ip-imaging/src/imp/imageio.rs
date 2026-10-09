//! macOS ImageIO (HEIC since 10.13, AVIF since 13): decodes, scales and colour-matches to sRGB.
use std::ffi::c_void;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::ptr::null;

use super::thumb::{fit_dims, Rgb};

type CFRef = *const c_void;

/// `CGRect` (`CGPoint origin; CGSize size;` of 64-bit `CGFloat`s), passed by value.
#[repr(C)]
#[allow(dead_code)] // read by CoreGraphics
struct CGRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// `kCGImageAlphaNoneSkipLast`: RGBX, 8 bits per component.
const ALPHA_NONE_SKIP_LAST: u32 = 5;
/// `kCGInterpolationHigh`.
const INTERPOLATION_HIGH: i32 = 3;

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFRef,
        buffer: *const u8,
        len: isize,
        is_directory: u8,
    ) -> CFRef;
    fn CFRelease(cf: CFRef);
}

#[link(name = "ImageIO", kind = "framework")]
extern "C" {
    fn CGImageSourceCreateWithURL(url: CFRef, options: CFRef) -> CFRef;
    fn CGImageSourceCreateImageAtIndex(source: CFRef, index: usize, options: CFRef) -> CFRef;
}

#[allow(non_upper_case_globals)]
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceSRGB: CFRef;
    fn CGColorSpaceCreateWithName(name: CFRef) -> CFRef;
    fn CGImageGetWidth(image: CFRef) -> usize;
    fn CGImageGetHeight(image: CFRef) -> usize;
    fn CGBitmapContextCreate(
        data: *mut c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: CFRef,
        bitmap_info: u32,
    ) -> CFRef;
    fn CGContextSetInterpolationQuality(context: CFRef, quality: i32);
    fn CGContextDrawImage(context: CFRef, rect: CGRect, image: CFRef);
}

/// Owned Core Foundation object (images, colour spaces and contexts are CF types too).
struct Owned(CFRef);

impl Owned {
    fn new(p: CFRef) -> Option<Self> {
        (!p.is_null()).then_some(Self(p))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: a non-NULL object from a Create function, released exactly once.
        unsafe { CFRelease(self.0) }
    }
}

/// RGB8 of the first image with long edge <= `long_edge`, as stored (ImageIO reports the
/// orientation but does not apply it).
pub fn decode(path: &Path, long_edge: u32) -> Result<Rgb, String> {
    let p = path.as_os_str().as_bytes();
    // SAFETY: every Create result is checked for NULL and released by `Owned`; the bitmap
    // context draws into `buf`, which outlives it and is exactly `bytes_per_row * th` bytes.
    unsafe {
        let url = Owned::new(CFURLCreateFromFileSystemRepresentation(
            null(),
            p.as_ptr(),
            p.len() as isize,
            0,
        ))
        .ok_or("macOS ImageIO: invalid path")?;
        let source = Owned::new(CGImageSourceCreateWithURL(url.0, null()))
            .ok_or("macOS ImageIO: cannot open the file")?;
        let image = Owned::new(CGImageSourceCreateImageAtIndex(source.0, 0, null()))
            .ok_or("macOS ImageIO: cannot decode the image")?;
        let (w, h) = (CGImageGetWidth(image.0), CGImageGetHeight(image.0));
        let (Ok(w), Ok(h)) = (u32::try_from(w), u32::try_from(h)) else {
            return Err("macOS ImageIO: image too large".into());
        };
        if w == 0 || h == 0 {
            return Err("macOS ImageIO: empty image".into());
        }
        let (tw, th) = fit_dims(w, h, long_edge);
        let bytes_per_row = tw as usize * 4;
        let mut buf = vec![0u8; bytes_per_row * th as usize];
        let srgb = Owned::new(CGColorSpaceCreateWithName(kCGColorSpaceSRGB))
            .ok_or("macOS ImageIO: no sRGB colour space")?;
        let ctx = Owned::new(CGBitmapContextCreate(
            buf.as_mut_ptr().cast(),
            tw as usize,
            th as usize,
            8,
            bytes_per_row,
            srgb.0,
            ALPHA_NONE_SKIP_LAST,
        ))
        .ok_or("macOS ImageIO: cannot create a bitmap context")?;
        CGContextSetInterpolationQuality(ctx.0, INTERPOLATION_HIGH);
        let rect = CGRect {
            x: 0.0,
            y: 0.0,
            width: tw as f64,
            height: th as f64,
        };
        CGContextDrawImage(ctx.0, rect, image.0);
        drop(ctx);
        let mut data = Vec::with_capacity(tw as usize * th as usize * 3);
        for px in buf.as_chunks::<4>().0 {
            data.extend_from_slice(&px[..3]);
        }
        Ok(Rgb { w: tw, h: th, data })
    }
}
