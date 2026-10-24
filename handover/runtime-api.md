# Runtime API reference (running document)

The public API of the canonical runtime (`crates/evaluator`, package `setl8-lev-evaluator`, library `evaluator`) as the runtime-hosting work (decision 74, `handover/runtime-hosting-design.md`) changes it. Each prompt appends its section; P29i finalizes this file and writes the export manifest. Sections present now: Threading (Send), Lifecycle, For the backend.

Versions: `RUNTIME_API_VERSION` is 1 for the revision the backend imported (2c70883); the constant itself is introduced by P29i (value 2).

## Threading (Send)

(P29c, design section 8.)

* `Evaluator` is `Send` (and stays `!Sync`): it may be built on one thread and moved to another, between calls or in the middle of its life; a moved evaluator gives exactly the outputs of an unmoved one (tests: `crates/evaluator/tests/send.rs`, a compile-time `assert_send::<Evaluator>()`, a campaign path run on another thread, and a mid-life move).
* The rule set is an `Arc<RulesCfg>` inside the evaluator (it was an `Rc` that was never cloned).
* `Evaluator::new(rules, acct, verifier)` takes `verifier: Box<dyn IntentVerifier + Send>`. BREAKING only for a verifier that is not `Send`; `AcceptAll` and the ed25519 verifiers of the tests are. The trait `IntentVerifier` itself is unchanged.
* The type is `Send` because every field is; there is no `unsafe` (the crate forbids it).

## Lifecycle

(P29c, design section 7.) Six entry points drive the account's lifecycle from finalized chain events. The evaluator checks what it can verify itself and returns a typed error without changing anything; the host authenticates the rest (that the chain event is finalized, its order, reorgs).

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifecycleError {
    Ended, NotFrozen, NotActive, NotFlat, PendingOrders, QueuedEvents, OrdersQueued,
    WrongPhase { have: u8, want: u8 }, ChainMismatch { what: &'static str }, AlreadyHeld, NotHeld, AlreadyEnded,
}                                                              // + Display + std::error::Error

impl Evaluator {
    pub fn request_pass(&mut self, now: i64, chain: &ChainView, replay: ReplayCheck) -> PassDecision;   // unchanged
    pub fn pass_resolved(&mut self) -> Result<(), LifecycleError>;                                      // was ()
    pub fn phase_started(&mut self, phase: u8, chain: &ChainView) -> Result<(), LifecycleError>;        // was (phase)
    pub fn set_held(&mut self, held: bool) -> Result<(), LifecycleError>;                               // was ()
    pub fn end_externally(&mut self) -> Result<(), LifecycleError>;                                     // was ()
    pub fn tick(&mut self, now: i64);                                                                   // unchanged
}
```

**A refused call changes NOTHING**: no field, no counter, no queue. This is tested with a full fingerprint of every private field before and after each refusal (`eval.rs` unit tests) and with the public state digest of the byte gate (`tests/lifecycle_typed.rs`).

### Preconditions and errors

| Call | Checked, in this order (the first violated one is returned) |
|---|---|
| `phase_started(phase, chain)` | `Ended` (faulted, or ended outside the journal); `NotActive` (the journal ended with a `Fail`); `NotFrozen` (no `request_pass` pending); `WrongPhase{have: phase, want}` where `want` is `engine::next_phase(current)` (2 after 1, 3 after 2, 4 after 3, 5 after 4; after 5 `want = 0`: there is no phase to start, call `end_externally`; `phase = 0` is always wrong); `NotFlat`; `PendingOrders`; `QueuedEvents` (drain and post first); `OrdersQueued` (an accepted order still waits for its second to close); then `ChainMismatch{what}` for `phase`, `seq`, `journal_hash`, `b`, `flat` |
| `pass_resolved()` | `Ended`; `NotActive`; `NotFrozen` |
| `set_held(true)` | `Ended`; `NotActive` (the program's `hold_account` needs an Active account); `AlreadyHeld` |
| `set_held(false)` | `Ended`; `NotHeld` (`release_hold` works on any status, so a Failed + held account can be released) |
| `end_externally()` | `AlreadyEnded` only. It is allowed on a Failed account (`fail_for_violation` ends a Failed + held account) and on a faulted evaluator |

`WrongPhase.have` is the phase the host passed, `want` the phase the evaluator expects (0 = graduation). `ChainMismatch.what` names the first disagreeing field of the `ChainView`.

### The chain view of `phase_started`

The view is the chain's account AFTER the transition, as `confirm_pass` (phase 1) or `finalize_pass` (phases 2 to 4) leaves it (`start_phase`, `pass_common.rs`): `seq` and `journal_hash` are the journal head and are unchanged (a phase change does not touch the journal), `b` is S (`engine::reset_phase_state`), `flat` is true, `phase` is the new phase. The program emits `PassFinalized` and `PhaseAdvanced` for one `finalize_pass`: call `phase_started` ONCE per transition.

### The phase-end contract

| Finalized chain event | The host calls, in this order | The evaluator then |
|---|---|---|
| `PassRequested{phase}` | `request_pass(now, chain, replay)`; on `Confirm` the host sends `confirm_pass`, on `Reject(r)` it sends `reject_pass(r)` | frozen: arrival refuses orders (`AccountFrozen`) |
| `PassRejected`, `PassCancelled`, `PassVetoed` | `pass_resolved()` | unfrozen, same phase |
| `PhaseAdvanced{1 -> 2}` (from `confirm_pass` of phase 1, no delay) | `phase_started(2, chain)` | fresh phase state, breach lock kept, unfrozen |
| `PassConfirmed{phase 2..5}` | nothing | stays frozen through the delay |
| `PassFinalized{p -> p+1}` followed by `PhaseAdvanced{p -> p+1}` (p = 2..4) | `phase_started(p+1, chain)` once | as above |
| `PassFinalized{5 -> 0}` (graduation) | `end_externally()` | ended |
| `AccountAbandoned`, `ViolationFail` | `end_externally()` | ended (also when the evaluator is Failed already) |
| `AccountFailed` (the evaluator's own `report_fail`) | nothing | already `Status::Failed` |
| `AccountHeld`, `HoldReleased` | `set_held(true)`, `set_held(false)` | held / released |

What `phase_started` does and does not touch: it sets the engine state to a fresh phase (B = S, Q = 0, latch cleared, tight mode off, flat) and KEEPS an active 12-hour breach lock (`lock_until`), exactly as the program's `start_phase` and the verifier's `on_phase_change` do (a test runs a breach, then a pass inside the lock, then checks the evaluator, an order refused during the lock, and the verifier's snapshot comparison with the lock, and shows that a snapshot without the lock is a MISMATCH); it advances the phase and unfreezes. It does NOT touch `seq`, `head`, the sample archive, the intents file, both hash states, the nonces (unique across the whole log, phases included) or `trades_closed`.

### Host duties that the evaluator cannot check

* Feed a lifecycle call only after the chain event is FINALIZED, in chain order, and never twice. A reorg is the host's problem (rebuild from a checkpoint once P29f exists, or replay its inputs).
* Drain and post every batch before `phase_started` (the evaluator refuses with `QueuedEvents`; a batch it sealed but the chain does not have shows as `ChainMismatch{seq}`).
* The chain does not allow a hold at the moment of `confirm_pass` or `finalize_pass` (both require `held == 0`), so a held evaluator at `phase_started` means a `HoldReleased` was fed late; the evaluator does not check it (no error variant) and keeps its hold flag.
* A known hazard, tracked for P29h (design 7.4): an order accepted in the same second as the pass request can execute while the account is frozen; `phase_started` refuses it as `OrdersQueued`, but `request_pass` still answers as before.

## For the backend

`setl8-back/crates/evaluator-service/src/lib.rs` (read only; nothing there was edited). Four call sites change; `Evaluator::new(.., Box::new(WalletVerifier))` at :83 compiles as it is (`WalletVerifier` is a unit struct, hence `Send`).

| File:line | Old call | New call |
|---|---|---|
| `evaluator-service/src/lib.rs:185` | `self.ev.set_held(*held);` | `if let Err(e) = self.ev.set_held(*held) { ...refused... }` |
| `evaluator-service/src/lib.rs:213` | `self.ev.pass_resolved();` | `if let Err(e) = self.ev.pass_resolved() { ...refused... }` |
| `evaluator-service/src/lib.rs:219` | `self.ev.phase_started(*phase);` | `self.ev.phase_started(*phase, &view)` where `view = ChainView { seq, journal_hash, b: size_micro, flat: true, phase: *phase }` taken from the finalized chain observation of the transition; this needs the chain's `seq` and `journal_hash` in `Input::PhaseStarted` (today it carries only `phase` and `evidence`) |
| `evaluator-service/src/lib.rs:224` | `self.ev.end_externally();` | `if let Err(e) = self.ev.end_externally() { ...refused... }` |

How to map a `LifecycleError` to a refused input with evidence: today these inputs return `Outcome::Applied` or abort the runtime with an error (`bail!`/`ensure!`). A refused lifecycle call is a normal, durable, deterministic result (the evaluator is unchanged), so use the path of the other refusals:

```rust
Err(e) => Outcome::Refused { reason: format!("lifecycle: {e} ({e:?})") }   // keep `evidence` in the stored input; do not retry
```

and treat the classes differently in operations: `NotFrozen`, `WrongPhase`, `ChainMismatch`, `AlreadyHeld`, `NotHeld`, `AlreadyEnded` mean the indexer's ordering or its chain view is wrong (alert; the input is consumed, a corrected one is a NEW input); `NotFlat`, `PendingOrders`, `QueuedEvents`, `OrdersQueued` mean the host did not drain/post/settle first (fix the sequencing, then append the input again); `Ended`, `NotActive` mean the account is over (nothing to do, record). Replay must reproduce the same refusals: they are deterministic.
