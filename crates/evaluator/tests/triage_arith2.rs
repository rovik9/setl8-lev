//! Triage tests for the M12 audit claims arith-09 .. arith-16 that turned out to be defects.
use std::panic::catch_unwind;

// FINDING arith-16: `engine::consts::frac` and `static_floor` multiply in plain i64 (consts.rs:34-36,
// `#[allow(clippy::arithmetic_side_effects)]`). With overflow-checks on (test/dev profile and the workspace release profile) a size above
// i64::MAX / 10_800 (about 8.5e14 micro-USD) panics instead of returning an error. Unused by production code, so latent.
// Severity info; plain implementation error (return a Result, or call frac_exact). Same defect as FINDING ARI-1 in rounding_properties.rs.
#[test]
#[ignore = "FINDING arith-16: engine::consts::frac / static_floor panic on overflow for a size above ~8.5e14 micro-USD (latent, unused)"]
fn finding_arith_16_consts_frac_does_not_panic_for_an_extreme_size() {
    let size = i64::MAX / 2;
    assert!(catch_unwind(|| engine::consts::frac(size, 10_800)).is_ok(), "consts::frac panicked on overflow instead of returning an error");
    assert!(catch_unwind(|| engine::consts::static_floor(size, true)).is_ok(), "consts::static_floor panicked on overflow");
}
