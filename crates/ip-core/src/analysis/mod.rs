//! M2 smart analysis: worker batches -> ingest -> grouping -> scoring -> people clustering.
//! Contract: `docs/api-contract-m2.md`.

pub mod cluster;
pub mod grouping;
pub mod scoring;
pub mod store;
pub mod types;
pub mod vecs;

mod run;

pub use run::{ModelsMissing, RunInfo};
pub use types::*;
