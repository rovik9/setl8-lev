//! setl8-lev-trading rules engine (DESIGN.md sections 2, 7, 9).
//!
//! Integer arithmetic only. Money is micro-USD (`i64`), prices are 1e-12 USD (`u64`),
//! quantities are 1e-6 base units (`i64`). Intermediate products use `i128`.
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]

pub mod consts;
pub mod units;

/// Version written into every journal `Meta` event so a verifier uses the right rules.
pub const ENGINE_VERSION: u16 = 1;
