//! `session-observer`: an unattended, read-only recorder of the launch markets' prices (GMX tickers, the Hyperliquid xyz
//! oracle, a public reference) and the analyzer that turns the recordings into `handover/session-observation-report.md`.
//! The network code (`lanes`) is thin; everything that decides something (change filtering, rotation, back-off, gap
//! classification, session comparison, offsets) is pure and tested offline.
pub mod analyze;
pub mod guard;
pub mod lanes;
pub mod markets;
pub mod store;
pub mod timeutil;
