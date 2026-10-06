//! M2 smart analysis: worker batches -> ingest -> grouping -> scoring -> people clustering.
//! Contract: `docs/api-contract-m2.md`.

pub mod cluster;
pub mod faces;
pub mod grouping;
pub mod scoring;
pub mod store;
pub mod types;
pub mod vecs;

mod run;

pub(crate) use run::map_worker_err;
pub use run::{ModelsMissing, RunInfo};
pub use types::*;
