//! On-device ONNX inference for the lite profile (`docs/api-contract-m8.md` section B.2).
//!
//! First slice: [`FaceDetector`], a Rust port of OpenCV's `FaceDetectorYN` around the YuNet
//! model (`face_detection_yunet_2023mar.onnx`), matching the AI worker's faces step
//! (score threshold 0.7, NMS 0.3, top-k 5000, BGR input in 0..255, input padded to a multiple
//! of 32). CPU only; execution providers (NNAPI / XNNPACK) can be added in [`FaceDetector::load`]
//! later.

mod yunet;

pub use yunet::{Detection, FaceDetector, Options};

/// Environment variable that overrides the YuNet model location (tests / development).
pub const YUNET_ENV: &str = "IMAGEPICKER_YUNET_MODEL";
/// File name of the YuNet model in the models directory.
pub const YUNET_FILE: &str = "face_detection_yunet_2023mar.onnx";

/// Locates the YuNet model: `IMAGEPICKER_YUNET_MODEL`, then `<models_dir>/yunet/<file>` (the AI
/// worker's layout), then `<models_dir>/<file>`.
pub fn find_yunet(models_dir: Option<&std::path::Path>) -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os(YUNET_ENV).map(std::path::PathBuf::from) {
        return p.is_file().then_some(p);
    }
    let d = models_dir?;
    [d.join("yunet").join(YUNET_FILE), d.join(YUNET_FILE)]
        .into_iter()
        .find(|p| p.is_file())
}
