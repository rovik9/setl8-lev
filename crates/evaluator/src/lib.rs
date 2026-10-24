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
//!   clock (the newest second of any accepted sample or `tick`; an order never moves the clock, decision 68, F-18) moves past it; a closed second is processed once:
//!   rollover fees, the decision pass over its samples, the events that follow, then the second's queued orders.
//! * Decisions use the account at the START of the second (handover/verifier-formats.md 7); an action changes the state
//!   for the next second. Of several samples of one second giving the same kind of decision, the last one's numbers count.
//! * Orders execute when their second closes. A market or close order is priced with the newest sample of its market, in stream
//!   order, whose source second is at or before the second it executes in (`exec.rs` `order_sample`, decision 68, D-F08); a triggered
//!   limit or stop and a stop-loss or take-profit with the TRIGGER sample (`decide.rs`); the forced closes of a breach or a fail with
//!   the marks of the decision's sample.
//! * Lifecycle (the host contract, handover/runtime-api.md "Lifecycle"): each chain event is fed once, in chain order, after the host
//!   authenticated it as finalized. `request_pass` (PassRequested) freezes the account; `pass_resolved` (PassRejected, PassCancelled,
//!   PassVetoed) unfreezes it; `phase_started(p, chain)` (PhaseAdvanced 1 -> 2 at confirm_pass, PassFinalized + PhaseAdvanced p-1 -> p
//!   at finalize_pass) starts phase p; `end_externally` (PassFinalized to phase 0, AccountAbandoned, ViolationFail) ends the account;
//!   `set_held` (AccountHeld, HoldReleased) holds and releases it. Each returns a `LifecycleError` and changes NOTHING when the
//!   evaluator is not in the state in which that event is possible. A breach lock survives a phase change.
//! * `drain_batches` turns the queue into batches: at most 8 events (the `Meta` last), 3 proofs, 1,232 bytes WITH the
//!   `SetComputeUnitLimit` instruction, and the compute limit `compute::recommended_cu_limit(kind, events, proofs)` (the worst case
//!   measured on Agave 4.0.2 x 1.20, at most 1,400,000; the 200,000 default is not enough for 7 or 8 events,
//!   handover/backend-compute-contract.md).
#![forbid(unsafe_code)]
#![deny(clippy::float_arithmetic)]
#![cfg_attr(not(test), deny(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

pub mod batch;
pub mod codec;
pub mod compute;
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
