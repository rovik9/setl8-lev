//! M0 smoke test: the localnet build of the program and the vault fixture are both present.
//! Fixtures:
//!   ../target/test-deploy/setl8_lev_trading.so   (scripts/build-test-so.sh; localnet = public test admin keys)
//!   fixtures/core_vault.so                       (copied from the vault repo, see handover/module-0.md)
use std::path::Path;

#[test]
fn program_and_vault_binaries_exist() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(
        root.join("../target/test-deploy/setl8_lev_trading.so").exists(),
        "run scripts/build-test-so.sh first"
    );
    assert!(root.join("fixtures/core_vault.so").exists(), "copy the vault .so into tests-rs/fixtures/");
}
