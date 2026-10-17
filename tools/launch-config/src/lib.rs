//! `launch-config`: the reproducible, validated generator of the launch configuration (P19 part B, decision 61). It reads
//! `config/launch-markets.toml` and emits `config/launch.json` (the exact arguments of `create_market` and
//! `create_ruleset`) and `config/launch.md`. It never touches the network and never sees a key.
pub mod check;
pub mod derive;
pub mod emit;
pub mod model;
pub mod probe;
pub mod session;

pub use derive::{generate, Launch};
pub use model::Input;

/// Parses the input file; unknown fields and a wrong format marker are errors.
pub fn parse(text: &str) -> Result<Input, String> {
    let input: Input = toml::from_str(text).map_err(|e| format!("launch-markets.toml: {e}"))?;
    if input.meta.format != "setl8-launch-markets" || input.meta.version != 1 {
        return Err(format!("launch-markets.toml: format {:?} version {} is not setl8-launch-markets 1", input.meta.format, input.meta.version));
    }
    Ok(input)
}
