// #179: Sentinel-indestructibility, compiler-enforced. Runtime (non-test) code
// must never .unwrap()/.expect() — a panic under panic=abort aborts the whole
// engine. Tests are exempt. Use ? / match / graceful logging instead.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod models;
pub mod router;
pub mod parsers;
pub mod astm;
pub mod ble;
