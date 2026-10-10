//! Property tests for the rules (256 cases each): monotonicity and the order of checks.
use engine::*;
use proptest::prelude::*;

const S: i64 = 100_000_000_000;

#[derive(Clone, Debug)]
enum Step {
    /// A closed slice: credit in micro-USD and a non-negative quick-tally delta.
    Credit(i64, i64),
    /// A price sample: open profit and a closing-commission estimate.
    Sample(i64, i64),
    /// The evaluator says the account is flat (ends tight mode).
    Flat,
    /// The evaluator says positions are open.
    Open,
}

fn arb_step() -> impl Strategy<Value = Step> {
    prop_oneof![
        (-3_000_000_000i64..=3_000_000_000, 0i64..=400_000_000).prop_map(|(c, q)| Step::Credit(c, q)),
        (-12_000_000_000i64..=8_000_000_000, 0i64..=200_000_000).prop_map(|(p, c)| Step::Sample(p, c)),
        Just(Step::Flat),
        Just(Step::Open),
    ]
}

fn arb_rules() -> impl Strategy<Value = Rules> {
    (any::<bool>(), any::<bool>()).prop_map(|(frozen_ratchet, quick_strict)| Rules { frozen_ratchet, quick_strict })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// L never falls inside a tight mode, the static floor never falls, the latch is monotone, Q never falls, and the
    /// order of checks holds at every sample.
    #[test]
    fn rules_invariants_hold_over_random_sequences(rules in arb_rules(), steps in proptest::collection::vec(arb_step(), 1..60)) {
        let mut st = AccountState::new_phase(S);
        let mut now = 1_760_000_000i64;
        for step in steps {
            let prev = st;
            match step {
                Step::Credit(c, q) => {
                    let d = apply_closed_slice(&st, &rules, c, q).unwrap();
                    st = d.state;
                    prop_assert_eq!(st.q_tally, prev.q_tally + q);
                    prop_assert!(st.q_tally >= prev.q_tally);
                    prop_assert_eq!(st.b, prev.b + c);
                    let limit = 2_800_000_000i64;
                    let over = if rules.quick_strict { st.q_tally > limit } else { st.q_tally >= limit };
                    prop_assert_eq!(d.fail.is_some(), over);
                    prop_assert_eq!(st.latch, prev.latch || st.b >= 104_000_000_000);
                }
                Step::Sample(p, comm) => {
                    now += 1;
                    let d = evaluate_sample(&st, &rules, p, comm, now).unwrap();
                    let e = prev.b + p - comm;
                    prop_assert_eq!(d.equity_micro, e);
                    let f_s = static_floor_micro(&prev).unwrap();
                    // Fail iff E <= F_s, whatever the tight state
                    prop_assert_eq!(matches!(d.action, SampleAction::Fail(_)), e <= f_s);
                    // a breach never happens without tight mode, and needs E <= the effective floor
                    if matches!(d.action, SampleAction::Breach { .. }) {
                        prop_assert!(prev.tight_on);
                        prop_assert!(e <= effective_floor_micro(&prev).unwrap());
                        prop_assert!(e > f_s);
                    }
                    if !prev.tight_on {
                        let tight_only = matches!(d.action, SampleAction::Breach { .. } | SampleAction::FloorRaised { .. });
                        prop_assert!(!tight_only);
                    }
                    if let SampleAction::TightStart { .. } = d.action {
                        prop_assert!(!prev.tight_on);
                        prop_assert!(p > 1_000_000_000);
                    }
                    if matches!(d.action, SampleAction::Fail(_)) {
                        prop_assert_eq!(d.state, prev);
                        break; // the phase is over
                    }
                    st = d.state;
                }
                Step::Flat => st = end_tight_if_flat(&st, true),
                Step::Open => st = end_tight_if_flat(&st, false),
            }
            // monotone pieces
            prop_assert!(st.latch || !prev.latch, "the latch never clears");
            prop_assert!(static_floor_micro(&st).unwrap() >= static_floor_micro(&prev).unwrap());
            if prev.tight_on && st.tight_on {
                prop_assert!(st.l_level >= prev.l_level, "L never falls: {} -> {}", prev.l_level, st.l_level);
                prop_assert!(st.g_peak >= prev.g_peak);
                prop_assert_eq!(st.r, prev.r);
            }
            if st.tight_on {
                prop_assert!(st.l_level >= st.l_start);
                prop_assert!(st.l_start <= st.r);
            } else {
                prop_assert_eq!((st.r, st.l_start, st.g_peak, st.l_level), (0, 0, 0, 0));
            }
        }
    }

    /// The decision for the same state and sample is a pure function.
    #[test]
    fn evaluate_sample_is_deterministic(p in -20_000_000_000i64..=20_000_000_000, c in 0i64..=1_000_000_000, tight in any::<bool>()) {
        let mut st = AccountState::new_phase(S);
        if tight {
            st = evaluate_sample(&st, &Rules::default(), 1_200_000_000, 0, 1).unwrap().state;
        }
        let a = evaluate_sample(&st, &Rules::default(), p, c, 5).unwrap();
        let b = evaluate_sample(&st, &Rules::default(), p, c, 5).unwrap();
        prop_assert_eq!(a, b);
    }

    /// Passing needs all three conditions.
    #[test]
    fn pass_check_matches_its_definition(b in 100_000_000_000i64..=120_000_000_000, flat in any::<bool>(), failed in any::<bool>()) {
        let mut st = AccountState::new_phase(S);
        st.b = b;
        let ok = pass_check(&st, flat, failed).is_ok();
        prop_assert_eq!(ok, b >= 108_000_000_000 && flat && !failed);
    }

    /// Nothing panics on arbitrary state.
    #[test]
    fn rules_never_panic(
        b in any::<i64>(), r in any::<i64>(), l in any::<i64>(), g in any::<i64>(), q in any::<i64>(), tight in any::<bool>(),
        latch in any::<bool>(), p in any::<i64>(), c in any::<i64>(), now in any::<i64>(), size in any::<i64>(), rules in arb_rules()
    ) {
        let st = AccountState { size_micro: size, b, latch, tight_on: tight, r, l_start: l, g_peak: g, l_level: l, q_tally: q, lock_until: now, flat: false };
        let _ = evaluate_sample(&st, &rules, p, c, now);
        let _ = apply_closed_slice(&st, &rules, p, c);
        let _ = effective_floor_micro(&st);
        let _ = pass_check(&st, true, false);
        let _ = payout_micro(size);
        let _ = end_tight_if_flat(&st, true);
    }
}
