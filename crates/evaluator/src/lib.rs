//! The reference evaluator of setl8-lev-trading (DESIGN 1.3, 4.3, 4.5, 8.5): the trusted backend as a library.
//!
//! It owns the order book of one account, values it on every sample, applies the rules of DESIGN 2, 7 and 9 with the
//! `engine` crate, and produces what the program and the public verifier read: journal batches in the exact on-chain
//! format, the sample archive, the signed-intents file, and the answers of the pass flow. No I/O, no clock, no floats,
//! no randomness; the same inputs give the same bytes. A backend in another language must reproduce its outputs bit for bit
//! (`tests/golden/`).
//!
//! # Model
//! * Time is whole seconds. Samples arrive in non-decreasing source-second order. A second is *closed* when the evaluator's
//!   clock (the newest second of any sample, order or `tick`) moves past it; a closed second is processed once:
//!   rollover fees, the decision pass over its samples, the events that follow, then the second's queued orders.
//! * Decisions use the account at the START of the second (handover/verifier-formats.md 7); an action changes the state
//!   for the next second. Of several samples of one second giving the same kind of decision, the last one's numbers count.
//! * Orders execute when their second closes, at the newest sample of their market that existed when they arrived.
//! * `drain_batches` turns the queue into batches: at most 8 events (the `Meta` last), 3 proofs, 1,232 bytes, 200,000 CU.
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod batch;
pub mod codec;
pub mod config;
mod decide;
mod drain;
pub mod eval;
mod exec;
mod pass;
pub mod sha256;
pub mod testkit;
pub mod types;

pub use config::*;
pub use eval::{AcceptAll, Evaluator, IntentVerifier, SignedIntent, Stats};
pub use types::*;
