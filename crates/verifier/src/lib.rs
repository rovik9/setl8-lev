//! `setl8-verify`: the public verifier of DESIGN 4.5 and 4.6. The replay core is pure (no I/O, no clock); only
//! `main.rs` reads files.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod archive;
pub mod cli;
pub mod finding;
pub mod hexutil;
pub mod intents;
pub mod log;
pub mod replay;
pub mod rules;
pub mod snapshot;
pub mod verify;

pub use finding::{Finding, FindingClass, PassVerdict, PhaseVerdict, Report, Verdict};
pub use verify::{verify_account, InputError, Inputs};
