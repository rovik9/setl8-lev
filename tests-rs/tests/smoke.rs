//! M0 smoke test (verified: cargo test passes). Checks that the built program
//! .so and the vault .so fixture are both present (it does not load them into LiteSVM yet). Fixtures:
//!   ../target/deploy/setl8_lev_trading.so   (anchor build)
//!   fixtures/core_vault.so                  (copied from the vault repo, see handover/module-0.md)
use std::path::Path;

#[test]
fn program_and_vault_binaries_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(root.join("../target/deploy/setl8_lev_trading.so").exists(), "run scripts/anchor-build-checked.sh first");
    assert!(root.join("fixtures/core_vault.so").exists(), "copy the vault .so into tests-rs/fixtures/");
}
