//! Dependency-free, on-device image analysis: the worker's classic `phash` and `quality`
//! steps (the latter including the horizon / level estimate, `tilt`) ported to Rust so phones
//! (and machines without the AI runtime) can analyse photos.
//!
//! The functions take upright RGB8 buffers at analysis size (long edge about 1024); decoding is
//! the caller's job (`ip-imaging`). Results are interchangeable with the Python worker's:
//! the pHash is bit-exact on the cross-check fixtures (`tests/parity.rs`) and the quality
//! numbers agree to float32 noise (the tilt estimate to a few hundredths of a degree).

pub mod imgops;
pub mod phash;
pub mod quality;
pub mod tilt;

pub use phash::{hamming_hex, phash_bits, phash_hex, phash_u64};
pub use quality::{analyze_quality, face_sharpness, Quality};
pub use tilt::{estimate_tilt, Tilt};
