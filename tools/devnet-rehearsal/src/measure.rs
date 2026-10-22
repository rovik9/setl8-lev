//! S4: the compute units and transaction size the cluster reported for every instruction of the run, against the LiteSVM numbers of
//! handover/module-11.md (the worst cases it measured) and the ceilings of `tests-rs/tests/budget.rs`.
use std::collections::BTreeMap;

use crate::state::SigRec;

/// `(instruction, ceiling CU, ceiling bytes)` as in `CEILINGS` of `tests-rs/tests/budget.rs`.
pub const CEILINGS: &[(&str, u64, usize)] = &[
    ("ban_wallet", 31_100, 660),
    ("cancel_pass", 22_100, 400),
    ("close_account", 3_900, 360),
    ("confirm_pass", 27_600, 440),
    ("create_market", 36_500, 690),
    ("create_ruleset", 53_400, 1_180),
    ("fail_for_violation", 50_700, 770),
    ("finalize_pass", 97_300, 780),
    ("hold_account", 28_400, 510),
    ("init_config", 18_500, 680),
    ("open_account", 156_000, 970),
    ("post_journal", 144_100, 1_232),
    ("reject_pass", 25_000, 440),
    ("release_account", 60_000, 620),
    ("release_hold", 28_300, 580),
    ("report_fail", 174_000, 1_232),
    ("request_advance", 52_900, 620),
    ("reset_account", 182_000, 1_050),
    ("set_evaluator", 22_300, 550),
    ("set_limits", 21_700, 530),
    ("set_paused", 21_600, 510),
    ("touch_activity", 60_000, 620),
    ("unban_wallet", 23_700, 580),
    ("update_market", 35_400, 650),
    ("veto_pass", 26_100, 550),
];

/// The worst cases the LiteSVM budget test measures TODAY (`cargo test --release --test budget -- --nocapture` of tests-rs at the freeze
/// candidate, `(instruction, CU, bytes)`). handover/module-11.md still prints the older table for the instructions that gained
/// `emit_cpi!` in audit part 1 (F-27): its `create_market` row says 16,815 CU, the budget test says 31,705.
pub const LITESVM_WORST: &[(&str, u64, usize)] = &[
    ("ban_wallet", 27_036, 569),
    ("cancel_pass", 19_205, 341),
    ("close_account", 3_340, 307),
    ("confirm_pass", 23_971, 374),
    ("create_market", 31_705, 594),
    ("create_ruleset", 46_405, 1_018),
    ("fail_for_violation", 44_045, 668),
    ("finalize_pass", 84_594, 671),
    ("hold_account", 24_633, 439),
    ("init_config", 16_050, 583),
    ("open_account", 135_595, 838),
    ("post_journal", 125_290, 1_181),
    ("reject_pass", 21_688, 375),
    ("release_account", 52_151, 539),
    ("release_hold", 24_600, 504),
    ("report_fail", 151_284, 1_154),
    ("request_advance", 45_937, 539),
    ("reset_account", 158_203, 911),
    ("set_evaluator", 19_338, 470),
    ("set_limits", 18_850, 454),
    ("set_paused", 18_729, 439),
    ("touch_activity", 52_145, 539),
    ("unban_wallet", 20_591, 503),
    ("update_market", 30_766, 559),
    ("veto_pass", 22_643, 471),
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub n: u64,
    pub max_cu: u64,
    pub min_cu: u64,
    pub max_bytes: u64,
}

/// The instruction a label names: its first word.
pub fn instruction_of(label: &str) -> &str {
    label.split_whitespace().next().unwrap_or("")
}

/// Per instruction, over the transactions that SUCCEEDED (a refused attack costs what it costs but is not a measurement).
pub fn summarize(sigs: &[SigRec]) -> BTreeMap<String, Row> {
    let mut out: BTreeMap<String, Row> = BTreeMap::new();
    for s in sigs.iter().filter(|s| s.ok) {
        let r = out.entry(instruction_of(&s.label).to_string()).or_default();
        r.n += 1;
        r.max_cu = r.max_cu.max(s.cu);
        r.min_cu = if r.n == 1 { s.cu } else { r.min_cu.min(s.cu) };
        r.max_bytes = r.max_bytes.max(s.size);
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub name: String,
    pub n: u64,
    pub max_cu: u64,
    pub max_bytes: u64,
    pub litesvm_cu: Option<u64>,
    pub litesvm_bytes: Option<usize>,
    pub ceiling_cu: Option<u64>,
    pub ceiling_bytes: Option<usize>,
    /// Above a ceiling: a finding.
    pub over_ceiling: bool,
    /// More than 5% above the LiteSVM worst case (CU or bytes).
    pub above_litesvm_5pct: bool,
    /// More than 5% below it (cheaper than the worst case: expected for a run that is not the worst case, listed for completeness).
    pub below_litesvm_5pct: bool,
}

fn pct_over(x: u64, base: u64) -> bool {
    u128::from(x) * 100 > u128::from(base) * 105
}
fn pct_under(x: u64, base: u64) -> bool {
    u128::from(x) * 100 < u128::from(base) * 95
}

pub fn compare(rows: &BTreeMap<String, Row>) -> Vec<Line> {
    rows.iter()
        .map(|(name, r)| {
            let lite = LITESVM_WORST.iter().find(|x| x.0 == name);
            let ceil = CEILINGS.iter().find(|x| x.0 == name);
            let over_ceiling = ceil.is_some_and(|c| r.max_cu > c.1 || r.max_bytes > c.2 as u64);
            let above = lite.is_some_and(|l| pct_over(r.max_cu, l.1) || pct_over(r.max_bytes, l.2 as u64));
            let below = lite.is_some_and(|l| pct_under(r.max_cu, l.1) || pct_under(r.max_bytes, l.2 as u64));
            Line {
                name: name.clone(),
                n: r.n,
                max_cu: r.max_cu,
                max_bytes: r.max_bytes,
                litesvm_cu: lite.map(|l| l.1),
                litesvm_bytes: lite.map(|l| l.2),
                ceiling_cu: ceil.map(|c| c.1),
                ceiling_bytes: ceil.map(|c| c.2),
                over_ceiling,
                above_litesvm_5pct: above,
                below_litesvm_5pct: below,
            }
        })
        .collect()
}

/// A markdown table of the lines.
pub fn table(lines: &[Line]) -> String {
    let mut s = String::from("| instruction | n | devnet CU (max) | LiteSVM worst CU | ceiling CU | devnet bytes (max) | LiteSVM bytes | ceiling bytes | verdict |\n|---|---:|---:|---:|---:|---:|---:|---:|---|\n");
    for l in lines {
        let f = |x: Option<u64>| x.map(|v| v.to_string()).unwrap_or_else(|| "-".into());
        let verdict = if l.over_ceiling {
            "OVER CEILING"
        } else if l.above_litesvm_5pct {
            "above LiteSVM worst by more than 5%"
        } else if l.below_litesvm_5pct {
            "below LiteSVM worst by more than 5% (not the worst case)"
        } else {
            "within 5% of LiteSVM"
        };
        s.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            l.name,
            l.n,
            l.max_cu,
            f(l.litesvm_cu),
            f(l.ceiling_cu),
            l.max_bytes,
            f(l.litesvm_bytes.map(|x| x as u64)),
            f(l.ceiling_bytes.map(|x| x as u64)),
            verdict
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(label: &str, ok: bool, cu: u64, size: u64) -> SigRec {
        SigRec { label: label.into(), sig: "s".into(), slot: 1, ok, err: String::new(), cu, size, fee: 5000, block_time: 0 }
    }

    #[test]
    fn labels_name_their_instruction_by_the_first_word() {
        assert_eq!(instruction_of("post_journal ev=3 pr=1 A1"), "post_journal");
        assert_eq!(instruction_of("open_account B1"), "open_account");
        assert_eq!(instruction_of(""), "");
    }

    #[test]
    fn only_successful_transactions_are_measured_and_the_maximum_is_kept() {
        let rows = summarize(&[rec("post_journal a", true, 30_000, 440), rec("post_journal b", true, 60_000, 900), rec("post_journal c", false, 999_999, 1_232), rec("open_account x", true, 130_000, 838)]);
        assert_eq!(rows["post_journal"], Row { n: 2, max_cu: 60_000, min_cu: 30_000, max_bytes: 900 });
        assert_eq!(rows["open_account"].n, 1);
    }

    #[test]
    fn a_line_is_flagged_over_its_ceiling_or_more_than_five_percent_from_litesvm() {
        let rows = summarize(&[rec("post_journal a", true, 144_101, 440), rec("open_account x", true, 135_595, 838), rec("request_advance y", true, 40_000, 539), rec("hold_account z", true, 26_000, 439)]);
        let lines = compare(&rows);
        let by = |n: &str| lines.iter().find(|l| l.name == n).unwrap().clone();
        assert!(by("post_journal").over_ceiling, "144,101 CU is one over the ceiling 144,100");
        assert!(!by("open_account").over_ceiling && !by("open_account").above_litesvm_5pct && !by("open_account").below_litesvm_5pct);
        assert!(by("request_advance").below_litesvm_5pct, "40,000 against 45,937 is 13% under");
        assert!(by("hold_account").above_litesvm_5pct, "26,000 against 24,633 is 5.5% over");
        let t = table(&lines);
        assert!(t.contains("OVER CEILING") && t.contains("| open_account |"));
    }

    #[test]
    fn the_embedded_ceilings_equal_the_ones_in_budget_rs() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests-rs/tests/budget.rs");
        let src = std::fs::read_to_string(path).expect("budget.rs");
        for (name, cu, bytes) in CEILINGS {
            let want = format!("(\"{name}\", {}, {}),", group(*cu), group(*bytes as u64));
            assert!(src.contains(&want), "budget.rs has no line {want}");
        }
        let in_file = src.lines().filter(|l| l.trim_start().starts_with("(\"") && l.trim_end().ends_with("),")).filter(|l| l.contains(", ") && !l.contains("//")).count();
        assert!(in_file >= CEILINGS.len(), "{in_file} ceiling lines in budget.rs");
    }

    #[test]
    fn the_litesvm_worst_cases_are_below_their_ceilings_and_cover_every_instruction() {
        for (name, cu, bytes) in LITESVM_WORST {
            let c = CEILINGS.iter().find(|c| c.0 == *name).unwrap_or_else(|| panic!("no ceiling for {name}"));
            assert!(*cu <= c.1 && *bytes <= c.2, "{name}: {cu}/{bytes} above its ceiling {}/{}", c.1, c.2);
        }
        assert_eq!(LITESVM_WORST.len(), CEILINGS.len());
    }

    fn group(n: u64) -> String {
        let s = n.to_string();
        let mut out = String::new();
        for (i, c) in s.chars().rev().enumerate() {
            if i > 0 && i % 3 == 0 {
                out.push('_');
            }
            out.push(c);
        }
        out.chars().rev().collect()
    }
}
