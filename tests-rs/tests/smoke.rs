//! UNVERIFIED (M0): written without a compiler for litesvm. Loads the built program
//! .so and the vault .so fixture and checks both are present. Fixtures:
//!   ../target/deploy/setl8_lev_trading.so   (anchor build)
//!   fixtures/core_vault.so                  (copied from the vault repo at a pinned commit)
use std::path::Path;

#[test]
fn program_and_vault_binaries_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(root.join("../target/deploy/setl8_lev_trading.so").exists(), "run scripts/anchor-build-checked.sh first");
    assert!(root.join("fixtures/core_vault.so").exists(), "copy the vault .so into tests-rs/fixtures/");
}
