//! ParamILS — automated algorithm configuration, Rust rewrite.
//!
//! This crate is used in two ways:
//!   1. As a CLI binary (`src/main.rs`) — drop-in for `param_ils_2_3_run.rb`
//!   2. As a Python extension module (feature = "python") — called from Grackle

pub mod params;    // parameter space: domains, defaults, conditionals, forbidden
pub mod scenario;  // scenario file parser (algo, instances, cutoff_time, …)
pub mod eval;      // parallel evaluation engine (rayon thread pool + capping)
pub mod cache;     // persistent result cache (SQLite via rusqlite)
pub mod ils;       // ILS loop: local search, perturbation, acceptance

#[cfg(feature = "python")]
mod python;        // PyO3 bindings — only compiled when building the Python .so
