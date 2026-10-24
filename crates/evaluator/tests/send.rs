//! P29c: `Evaluator` is `Send` (handover/runtime-hosting-design.md 8). A compile-time assertion, and a run on another thread that gives
//! exactly the result of an unmoved run.
mod common;
use common::bytes_gate::*;
use common::runner::*;
use common::*;
use evaluator::testkit::generate;
use evaluator::*;

fn assert_send<T: Send>() {}

#[test]
fn the_evaluator_is_send() {
    assert_send::<Evaluator>();
    assert_send::<Box<dyn IntentVerifier + Send>>();
    assert_send::<AcceptAll>();
}

fn world_of(seed: u64) -> (evaluator::testkit::PathSpec, World) {
    let spec = generate(seed);
    let w = world_for(&spec);
    (spec, w)
}

/// Runs the first half of a path on this thread, moves the evaluator to another thread, and finishes there.
#[test]
fn an_evaluator_moved_to_another_thread_gives_the_result_of_an_unmoved_run() {
    for seed in [3u64, 17, 42] {
        let (spec, w) = world_of(seed);
        let mut chain = SimChain::default();
        let unmoved = SegmentView::of(&run_path(&spec, w, &mut chain));
        // the same path, but the evaluator is built here and the whole run happens inside a spawned thread after a move
        let (spec2, w2) = world_of(seed);
        let handle = std::thread::spawn(move || {
            let mut chain = SimChain::default();
            SegmentView::of(&run_path(&spec2, w2, &mut chain))
        });
        let moved = handle.join().expect("the thread ran");
        assert_eq!(path_digest(&parts_of(&[unmoved])), path_digest(&parts_of(&[moved])), "seed {seed}");
    }
}

/// An evaluator that has already processed input is moved between threads in the middle of its life.
#[test]
fn an_evaluator_with_state_can_change_threads_in_the_middle() {
    let mk = || {
        let (_, w) = world_of(5);
        w.ev
    };
    let mut ev = mk();
    for s in 0..40i64 {
        let _ = ev.on_sample_mid(0, ev.clock() + s, 1_080_000_000_000);
    }
    let digest_here = StateView::of(&ev);
    let ev = std::thread::spawn(move || {
        let mut ev = ev;
        ev.tick(ev.clock() + 5);
        ev
    })
    .join()
    .expect("thread");
    let mut again = mk();
    for s in 0..40i64 {
        let _ = again.on_sample_mid(0, again.clock() + s, 1_080_000_000_000);
    }
    again.tick(again.clock() + 5);
    assert_eq!(StateView::of(&ev), StateView::of(&again));
    assert_ne!(digest_here, StateView::of(&ev), "the thread really did something");
}
