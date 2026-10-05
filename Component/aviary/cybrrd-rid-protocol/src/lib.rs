// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
// #179: Sentinel-indestructibility, compiler-enforced. Runtime (non-test) code
// must never .unwrap()/.expect() — a panic under panic=abort aborts the whole
// engine. Tests are exempt. Use ? / match / graceful logging instead.
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used))]

pub mod models;
pub mod router;
mod nan;
pub mod astm;
pub mod ble;
