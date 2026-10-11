//! Random and mutated bytes in every input file never panic: the result is `Err(InputError)` or a report.
mod common;
use common::*;
use proptest::prelude::*;

fn run(events: &[u8], rules: &[u8], archive: &[u8], intents: &[u8], snapshot: Option<&[u8]>) {
    let inputs = setl8_verify::Inputs { events, rules, archive, intents, snapshot, second_archive: None, as_of_seq: None };
    let _ = setl8_verify::verify_account(&inputs);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(200))]

    #[test]
    fn random_bytes_in_every_file(a in proptest::collection::vec(any::<u8>(), 0..300), b in proptest::collection::vec(any::<u8>(), 0..300),
                                  c in proptest::collection::vec(any::<u8>(), 0..300), d in proptest::collection::vec(any::<u8>(), 0..300)) {
        run(&a, &b, &c, &d, Some(&a));
    }

    #[test]
    fn mutated_valid_files_never_panic(which in 0usize..5, pos in 0usize..4000, byte in any::<u8>(), cut in 0usize..4000, truncate in any::<bool>()) {
        let b = design_4_9();
        let mut f = b.files();
        {
            let target: &mut Vec<u8> = match which {
                0 => &mut f.events,
                1 => &mut f.rules,
                2 => &mut f.archive,
                3 => &mut f.intents,
                _ => f.snapshot.as_mut().unwrap(),
            };
            if truncate {
                let n = cut % (target.len() + 1);
                target.truncate(n);
            } else if !target.is_empty() {
                let n = pos % target.len();
                target[n] = byte;
            }
        }
        let r = setl8_verify::verify_account(&f.inputs());
        if let Ok(rep) = r {
            // whatever it says, the report serialises and has a verdict
            let _ = serde_json::to_string(&rep).unwrap();
        }
    }

    #[test]
    fn mutated_event_hex_never_panics(pos in 0usize..3000, byte in any::<u8>()) {
        let b = design_4_9();
        let mut f = b.files();
        let n = pos % f.events.len();
        f.events[n] = byte;
        let _ = setl8_verify::verify_account(&f.inputs());
    }
}
