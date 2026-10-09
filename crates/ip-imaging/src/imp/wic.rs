//! Windows Imaging Component. HEIF needs the Microsoft Store codecs "HEIF Image Extensions" and
//! "HEVC Video Extensions" (AVIF: "AV1 Video Extension"), which are often missing; the caller
//! then falls back to libheif.
use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Foundation::{
    E_INVALIDARG, GENERIC_READ, RPC_E_CHANGED_MODE, WINCODEC_ERR_COMPONENTINITIALIZEFAILURE,
    WINCODEC_ERR_COMPONENTNOTFOUND,
};
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat24bppRGB, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

use super::thumb::Rgb;

thread_local! {
    /// COM is initialised once per thread and never torn down: worker threads live long and
    /// re-initialising per image would reload the codec DLLs every time.
    static COM_READY: bool = {
        // SAFETY: plain per-thread COM initialisation; a thread already in an STA reports
        // RPC_E_CHANGED_MODE and can use WIC as it is.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        hr.is_ok() || hr == RPC_E_CHANGED_MODE
    };
}

/// Full-size RGB8 of the first frame, as stored (WIC does not apply the orientation).
pub fn decode(path: &Path) -> Result<Rgb, String> {
    if !COM_READY.with(|r| *r) {
        return Err("Windows WIC: COM initialisation failed".into());
    }
    decode_com(path).map_err(|e| {
        if e.code() == WINCODEC_ERR_COMPONENTNOTFOUND {
            "Windows has no HEIF/AVIF codec (Microsoft Store: \"HEIF Image Extensions\" and \
             \"HEVC Video Extensions\")"
                .to_string()
        } else if e.code() == WINCODEC_ERR_COMPONENTINITIALIZEFAILURE {
            // seen when the Store codec package is not installed for the current user
            "the Windows HEIF codec failed to initialise (Microsoft Store: \"HEIF Image \
             Extensions\" and \"HEVC Video Extensions\", installed for this user)"
                .to_string()
        } else {
            format!(
                "Windows WIC: {:#010x} {} (is the \"HEVC Video Extensions\" codec installed?)",
                e.code().0 as u32,
                e.message().trim()
            )
        }
    })
}

fn decode_com(path: &Path) -> windows::core::Result<Rgb> {
    // SAFETY: COM calls on interfaces owned by this function; `data` is exactly `stride * h`
    // bytes as `CopyPixels` requires for the whole frame.
    unsafe {
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder = factory.CreateDecoderFromFilename(
            &HSTRING::from(path),
            None,
            GENERIC_READ,
            WICDecodeMetadataCacheOnDemand,
        )?;
        let frame = decoder.GetFrame(0)?;
        let conv = factory.CreateFormatConverter()?;
        conv.Initialize(
            &frame,
            &GUID_WICPixelFormat24bppRGB,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let (mut w, mut h) = (0u32, 0u32);
        conv.GetSize(&mut w, &mut h)?;
        let stride = w as usize * 3;
        if w == 0 || h == 0 || stride.checked_mul(h as usize).is_none_or(|n| n > 1 << 31) {
            return Err(windows::core::Error::from_hresult(E_INVALIDARG));
        }
        let mut data = vec![0u8; stride * h as usize];
        conv.CopyPixels(std::ptr::null(), stride as u32, &mut data)?;
        Ok(Rgb { w, h, data })
    }
}
