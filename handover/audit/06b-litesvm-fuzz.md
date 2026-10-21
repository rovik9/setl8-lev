# 06b. LiteSVM fuzz loop (M12 item 6, second half)

SELF-AUDIT by the builder; not an independent audit.

File: `tests-rs/tests/fuzz.rs` (new, 11 tests). Run: `cd tests-rs && cargo test --offline --release --test fuzz -- --nocapture`
(the default is 50,000 iterations; `FUZZ_SEED`, `FUZZ_ITERS`, `FUZZ_THREADS`, `FUZZ_EPOCH` change it; each worker prints its own
seed so one worker can be replayed with `FUZZ_SEED=<worker seed> FUZZ_THREADS=1 FUZZ_ITERS=<its iterations>`).

What one iteration does: pick an instruction of the program (all 26 incl. an unknown discriminator), build the call as a well-formed
one ("good"), a near miss (one argument or account wrong), a permutation of the account list, garbage instruction data, or fully
random bytes; send it to the real program with the real vault; then assert (1) no program panic/abort (log scan for `panicked`,
`ProgramFailedToComplete`, out-of-memory; the known finding JRN-1 is counted separately and not as a violation), (2) a failing call
leaves every account of the world (including the vault accounts) byte-identical, (3) a succeeding call satisfies the static
invariants of the 207-byte TradingAccount, the SectorConfig, the WalletProfile and the PayoutTally and the transition invariants
(seq and head chained, tally monotone, window arithmetic, counters, status changes) and the authorisation oracle (the signers that
DESIGN 14 demands were present). 8 control tests prove the checker fires on corrupted copies of the world (the checker is itself tested).

Result of the committed default run (seed 0x5e71eb0f0222026, 50,000 iterations, 10 threads, 2.1 s wall in release):

```
==================== FUZZ REPORT ====================
master seed 0x5e71eb0f0222026  iterations 50000 (asked 50000)  threads 10  wall 2.1s  (24329 it/s)
sends 49600  avg send 232 us  avg post-success checks 203 us
successes 19116 / 49600 = 38.5%   (ext vault mark_abandoned: 227 of 289)
multi-instruction transactions: 1840 sent, 190 succeeded
invariant evaluations (check_static + check_transition + auth on a success) 19306   hash-chain recomputations 3589
by kind:
  good          tries   24583  ok  15808  (64.3%)
  near-miss     tries    8551  ok   2279  (26.7%)
  permuted      tries    8587  ok    513  (6.0%)
  garbage-data  tries    4304  ok    516  (12.0%)
  random        tries    2135  ok      0  (0.0%)
by instruction (tries / successes):
  init_config            693 /     10   max CU      0
  set_evaluator          577 /    320   max CU   5249
  set_limits             808 /    464   max CU   5219
  set_paused             529 /    311   max CU   5422
  create_ruleset         559 /    222   max CU  35485
  create_market          548 /    271   max CU  20975
  update_market          746 /    426   max CU   8413
  ban_wallet             577 /    317   max CU  16223
  unban_wallet           797 /    390   max CU   7306
  open_account          1482 /    682   max CU 116381
  reset_account         2012 /    277   max CU 146200
  post_journal          7927 /   3158   max CU  87683
  request_advance       3626 /   1548   max CU  46533
  cancel_pass            764 /     29   max CU   5268
  confirm_pass          3331 /   1240   max CU  24067
  reject_pass            834 /     28   max CU  22256
  veto_pass              888 /    142   max CU  23498
  finalize_pass         2983 /    673   max CU  82830
  report_fail            970 /    387   max CU  93228
  hold_account          1143 /    534   max CU  25487
  release_hold          1176 /    367   max CU  25738
  fail_for_violation     443 /    111   max CU  44885
  touch_activity        4119 /   2525   max CU  54007
  release_account       3997 /   2616   max CU  54745
  close_account         3455 /   1841   max CU   4283
error histogram (instruction -> result: count):
     3158  post_journal -> OK
     2887  unknown-discriminator -> InstructionFallbackNotFound
     2616  release_account -> OK
     2525  touch_activity -> OK
     2026  post_journal -> AccountNotInitialized
     1841  close_account -> OK
     1548  request_advance -> OK
     1240  confirm_pass -> OK
      708  finalize_pass -> AccountNotInitialized
      682  open_account -> OK
      673  finalize_pass -> OK
      667  finalize_pass -> NoPendingPass
      666  request_advance -> AccountNotInitialized
      630  confirm_pass -> AccountNotInitialized
      612  reset_account -> ResetNotAllowed
      566  confirm_pass -> NoPendingPass
      534  hold_account -> OK
      516  close_account -> AccountNotSigner
      484  multi -> AccountNotInitialized
      470  reset_account -> AccountNotInitialized
      464  set_limits -> OK
      426  update_market -> OK
      418  post_journal -> AccountNotActive
      417  touch_activity -> AccountNotInitialized
      407  confirm_pass -> AccountNotActive
      390  unban_wallet -> OK
      388  request_advance -> TargetNotReached
      387  report_fail -> OK
      369  release_account -> AccountNotInitialized
      367  release_hold -> OK
      361  post_journal -> ProgramFailedToComplete
      339  close_account -> AccountNotInitialized
      321  cancel_pass -> NoPendingPass
      320  finalize_pass -> AccountNotActive
      320  set_evaluator -> OK
      317  ban_wallet -> OK
      311  set_paused -> OK
      310  post_journal -> AccountNotSigner
      305  veto_pass -> NoPendingPass
      287  reject_pass -> NoPendingPass
      277  reset_account -> OK
      276  release_hold -> NotHeld
      271  create_market -> OK
      268  release_hold -> AccountNotInitialized
      261  init_config(bare) -> Custom(0)
      257  veto_pass -> AccountNotInitialized
      245  reject_pass -> AccountNotInitialized
      244  request_advance -> AccountNotActive
      229  report_fail -> AccountNotInitialized
      227  ext -> OK
      223  post_journal -> MarketUnknown
      222  create_ruleset -> OK
      215  touch_activity -> ConstraintSeeds
      194  cancel_pass -> AccountNotInitialized
      192  post_journal -> AccountDiscriminatorMismatch
      191  close_account -> CloseTooEarly
      190  multi -> OK
      187  confirm_pass -> AccountNotSigner
      187  release_account -> AccountOwnedByWrongProgram
      187  touch_activity -> AccountNotSigner
      179  release_account -> AccountNotSigner
      177  touch_activity -> AccountOwnedByWrongProgram
      176  hold_account -> AccountNotInitialized
      174  release_account -> ConstraintSeeds
      166  post_journal -> Unauthorized
      165  post_journal -> AccountOwnedByWrongProgram
      157  close_account -> AccountNotTerminal
      154  init_config -> Custom(0)
      153  post_journal -> ConstraintSeeds
      149  finalize_pass -> ProductNotActive
      149  open_account -> WalletBanned
      144  close_account -> Unauthorized
      142  veto_pass -> OK
      138  unban_wallet -> AccountNotInitialized
      136  multi -> AccountOwnedByWrongProgram
      136  multi -> NoPendingPass
      133  request_advance -> AccountNotSigner
      130  reset_account -> WalletBanned
      128  cancel_pass -> AccountNotSigner
      128  post_journal -> BatchMalformed
      122  touch_activity -> Unauthorized
      118  close_account -> AccountOwnedByWrongProgram
      116  multi -> ConstraintSeeds
      115  post_journal -> InstructionDidNotDeserialize
      115  reset_account -> AccountNotSigner
      113  release_hold -> AccountOwnedByWrongProgram
      111  fail_for_violation -> OK
      111  open_account -> AccountNotSigner
      109  multi -> AccountNotSigner
      107  create_ruleset -> InstructionDidNotDeserialize
      105  create_ruleset -> InvalidTier
      105  multi -> AccountNotActive
      103  request_advance -> InvalidTraderStatus
      102  update_market -> InstructionDidNotDeserialize
      101  fail_for_violation -> AccountNotInitialized
      101  reset_account -> AccountOwnedByWrongProgram
       99  touch_activity -> AccountDiscriminatorMismatch
       98  set_limits -> Unauthorized
       97  release_account -> AccountDiscriminatorMismatch
       97  touch_activity -> InvalidVaultProgram
       96  create_market -> InstructionDidNotDeserialize
       96  request_advance -> ConstraintSeeds
       95  finalize_pass -> AccountNotSigner
       95  hold_account -> AccountNotSigner
       95  set_paused -> InstructionDidNotDeserialize
       92  touch_activity -> AccountNotEnoughKeys
       91  open_account -> AccountOwnedByWrongProgram
       91  set_evaluator -> Unauthorized
       91  veto_pass -> AccountOwnedByWrongProgram
       89  release_account -> InvalidVaultProgram
       85  reject_pass -> AccountNotSigner
       84  release_account -> AccountNotEnoughKeys
       83  confirm_pass -> AccountOwnedByWrongProgram
       82  finalize_pass -> InvalidProgramId
       81  request_advance -> AccountOwnedByWrongProgram
       81  unban_wallet -> AccountOwnedByWrongProgram
       79  finalize_pass -> ConstraintSeeds
       76  init_config -> InstructionDidNotDeserialize
       75  report_fail -> ProgramFailedToComplete
       74  update_market -> AccountOwnedByWrongProgram
       73  close_account -> AccountNotEnoughKeys
       73  reset_account -> ProductNotActive
       72  init_config(bare) -> AccountNotSigner
       71  finalize_pass -> AccountOwnedByWrongProgram
       71  multi -> AccountDiscriminatorMismatch
       69  hold_account -> InstructionDidNotDeserialize
       69  set_limits -> AccountOwnedByWrongProgram
       69  touch_activity -> AccountNotActive
       65  multi -> Unauthorized
       65  open_account -> ConstraintSeeds
       63  set_limits -> InvalidLimits
       62  reject_pass -> RejectReasonInvalid
       61  ban_wallet -> InstructionDidNotDeserialize
       61  confirm_pass -> Unauthorized
       61  create_market -> MarketUnknown
       59  post_journal -> MarketReduceOnly
       59  touch_activity -> InvalidVaultAccount
       57  post_journal -> MarketDisabled
       57  release_account -> AccountNotActive
       57  unban_wallet -> InstructionDidNotDeserialize
       57  update_market -> MarketUnknown
       56  hold_account -> EvidenceMissing
       55  confirm_pass -> AccountDiscriminatorMismatch
       54  open_account -> InvalidTier
       53  set_evaluator -> AccountOwnedByWrongProgram
       53  touch_activity -> ConstraintMut
       52  post_journal -> BadSequence
       52  post_journal -> MarketClosed
       52  request_advance -> AccountDiscriminatorMismatch
       51  confirm_pass -> ConstraintSeeds
       51  open_account -> Paused
       51  release_account -> ConstraintMut
       51  release_account -> Unauthorized
       51  unban_wallet -> Unauthorized
       50  request_advance -> NotFlat
       49  finalize_pass -> AccountDiscriminatorMismatch
       49  release_hold -> ConstraintSeeds
       49  set_paused -> Unauthorized
       48  ext -> NotAbandonable
       47  close_account -> ConstraintMut
       46  fail_for_violation -> InstructionDidNotDeserialize
       46  fail_for_violation -> NotHeld
       46  release_hold -> AccountDiscriminatorMismatch
       46  request_advance -> TightModeOn
       45  hold_account -> AccountOwnedByWrongProgram
       45  open_account -> ProductNotActive
       44  request_advance -> AccountNotEnoughKeys
       43  set_evaluator -> InstructionDidNotDeserialize
       43  set_limits -> AccountNotInitialized
       42  hold_account -> AccountDiscriminatorMismatch
       41  release_hold -> Unauthorized
       40  open_account -> InvalidAccountData
       39  init_config -> AccountNotSigner
       39  multi -> ResetNotAllowed
       39  release_account -> InvalidVaultAccount
       39  set_evaluator -> AccountNotInitialized
       38  open_account -> InvalidProgramId
       38  report_fail -> AccountNotActive
       38  reset_account -> InstructionDidNotDeserialize
       37  cancel_pass -> Unauthorized
       37  open_account -> AccountNotInitialized
       37  post_journal -> BadPrevHash
       37  set_limits -> InstructionDidNotDeserialize
       37  set_paused -> AccountOwnedByWrongProgram
       36  fail_for_violation -> AccountOwnedByWrongProgram
       36  hold_account -> ConstraintSeeds
       35  create_ruleset -> InvalidProgramId
       35  reject_pass -> AccountOwnedByWrongProgram
       35  veto_pass -> Unauthorized
       34  ban_wallet -> AccountNotSigner
       34  hold_account -> Unauthorized
       34  init_config(bare) -> InstructionDidNotDeserialize
       34  request_advance -> InvalidVaultProgram
       33  multi -> AccountNotEnoughKeys
       33  reset_account -> InvalidProgramId
       32  unban_wallet -> ConstraintSeeds
       31  ban_wallet -> ConstraintSeeds
       31  report_fail -> InstructionDidNotDeserialize
       30  open_account -> Unauthorized
       29  cancel_pass -> AccountOwnedByWrongProgram
       29  cancel_pass -> OK
       29  close_account -> AccountDiscriminatorMismatch
       29  reset_account -> InvalidAccountData
       28  create_market -> AccountNotSigner
       28  hold_account -> AccountNotActive
       28  reject_pass -> AccountDiscriminatorMismatch
       28  reject_pass -> OK
       28  report_fail -> ConstraintSeeds
       28  request_advance -> AccountHeld
       28  reset_account -> AccountDiscriminatorMismatch
       28  veto_pass -> ConstraintSeeds
       27  create_market -> InvalidProgramId
       27  multi -> NotHeld
       27  reset_account -> ConstraintSeeds
       26  ban_wallet -> EvidenceMissing
       26  confirm_pass -> AccountNotEnoughKeys
       26  create_ruleset -> Unauthorized
       26  multi -> MarketUnknown
       26  request_advance -> Unauthorized
       26  update_market -> AccountNotInitialized
       25  ban_wallet -> Unauthorized
       25  multi -> InstructionFallbackNotFound
       25  report_fail -> AccountDiscriminatorMismatch
       24  fail_for_violation -> AccountNotActive
       24  multi -> ConstraintMut
       24  multi -> InvalidVaultProgram
       24  post_journal -> PassPending
       24  post_journal -> TimeInFuture
       24  unban_wallet -> AccountDiscriminatorMismatch
       23  multi -> TargetNotReached
       23  report_fail -> AccountNotSigner
       23  request_advance -> ConstraintMut
       22  ban_wallet -> InvalidProgramId
       22  reject_pass -> Unauthorized
       21  confirm_pass -> NotFlat
       21  multi -> InstructionDidNotDeserialize
       21  report_fail -> Unauthorized
       21  request_advance -> Paused
       20  finalize_pass -> ConstraintMut
       20  finalize_pass -> InvalidVaultProgram
       20  post_journal -> TimeBackwards
       20  reject_pass -> ConstraintSeeds
       20  request_advance -> PassAlreadyPending
       20  unban_wallet -> AccountNotEnoughKeys
       20  update_market -> AccountNotEnoughKeys
       20  veto_pass -> AccountDiscriminatorMismatch
       19  multi -> ProductNotActive
       19  post_journal -> ConstraintMut
       19  update_market -> AccountDiscriminatorMismatch
       18  ban_wallet -> AccountOwnedByWrongProgram
       18  create_market -> AccountOwnedByWrongProgram
       18  fail_for_violation -> EvidenceMissing
       18  open_account -> AccountDiscriminatorMismatch
       18  post_journal -> MetaTwice
       18  request_advance -> WrongPhase
       17  reset_account -> Paused
       16  create_ruleset -> AccountNotSigner
       16  fail_for_violation -> AccountNotEnoughKeys
       16  hold_account -> AccountNotEnoughKeys
       16  open_account -> InvalidTokenAccount
       16  post_journal -> InvalidFlag
       16  report_fail -> AccountOwnedByWrongProgram
       15  finalize_pass -> Paused
       15  multi -> CloseTooEarly
       15  open_account -> InstructionDidNotDeserialize
       15  report_fail -> FailReasonMismatch
       15  set_paused -> AccountNotInitialized
       14  ban_wallet -> AccountNotEnoughKeys
       14  multi -> WalletBanned
       14  reject_pass -> InstructionDidNotDeserialize
       14  reset_account -> AccountAlreadyReset
       14  set_evaluator -> AccountNotEnoughKeys
       14  set_limits -> AccountDiscriminatorMismatch
       14  set_limits -> AccountNotEnoughKeys
       14  update_market -> ConstraintSeeds
       13  ban_wallet -> AccountDiscriminatorMismatch
       13  ban_wallet -> AccountNotInitialized
       13  create_market -> ConstraintSeeds
       13  multi -> PassAlreadyPending
       13  post_journal -> AccountHeld
       12  create_market -> AccountNotEnoughKeys
       12  create_ruleset -> ConstraintSeeds
       12  finalize_pass -> DailyPayoutCapReached
       12  multi -> InvalidProgramId
       12  open_account -> InvalidVaultProgram
       12  post_journal -> TightStartPOpen
       12  release_hold -> AccountNotEnoughKeys
       12  report_fail -> MarketUnknown
       12  reset_account -> ConstraintMut
       12  reset_account -> InvalidTokenAccount
       11  cancel_pass -> AccountNotEnoughKeys
       11  create_ruleset -> AccountNotEnoughKeys
       11  create_ruleset -> AccountOwnedByWrongProgram
       11  ext -> InvalidTraderStatus
       11  init_config -> InvalidProgramId
       11  init_config(bare) -> InstructionFallbackNotFound
       11  multi -> BadSequence
       11  report_fail -> ConstraintMut
       11  report_fail -> InvalidTraderStatus
       11  set_evaluator -> AccountDiscriminatorMismatch
       10  fail_for_violation -> AccountDiscriminatorMismatch
       10  finalize_pass -> AccountNotEnoughKeys
       10  init_config(bare) -> OK
       10  open_account -> AccountNotEnoughKeys
       10  open_account -> ConstraintMut
       10  post_journal -> MathOverflow
       10  post_journal -> MetaNotLast
       10  set_paused -> AccountNotEnoughKeys
        9  create_market -> AccountNotInitialized
        9  post_journal -> EngineVersionMismatch
        9  post_journal -> FailEventNotAllowed
        9  report_fail -> InvalidVaultProgram
        9  reset_account -> InvalidVaultProgram
        9  set_paused -> AccountDiscriminatorMismatch
        8  fail_for_violation -> ConstraintSeeds
        8  fail_for_violation -> InvalidVaultProgram
        8  init_config(bare) -> AccountOwnedByWrongProgram
        8  multi -> Custom(0)
        8  multi -> InvalidTraderStatus
        8  report_fail -> BatchMalformed
        7  cancel_pass -> AccountDiscriminatorMismatch
        7  fail_for_violation -> Unauthorized
        7  finalize_pass -> PassDelayNotElapsed
        7  hold_account -> AlreadyHeld
        7  init_config -> ConstraintSeeds
        7  multi -> AccountNotTerminal
        7  multi -> EvidenceMissing
        7  multi -> InvalidTier
        7  multi -> InvalidVaultAccount
        7  multi -> ProgramFailedToComplete
        7  post_journal -> BatchTooLarge
        7  post_journal -> MarketUnpriced
        7  reject_pass -> AccountNotEnoughKeys
        7  reset_account -> AccountNotEnoughKeys
        6  create_market -> AccountDiscriminatorMismatch
        6  create_ruleset -> AccountNotInitialized
        6  fail_for_violation -> ConstraintMut
        6  fail_for_violation -> InvalidTraderStatus
        6  init_config -> AccountNotEnoughKeys
        6  multi -> AlreadyHeld
        6  post_journal -> FeeInvalid
        6  post_journal -> TightNotOn
        6  set_evaluator -> ConstraintMut
        6  set_limits -> ConstraintMut
        6  touch_activity -> InvalidTraderStatus
        6  veto_pass -> AccountNotEnoughKeys
        5  create_ruleset -> PrivilegeEscalation
        5  hold_account -> ConstraintMut
        5  multi -> Paused
        5  post_journal -> PnlMismatch
        5  report_fail -> FailNotBeforeMeta
        5  update_market -> Unauthorized
        4  cancel_pass -> CancelTooEarly
        4  cancel_pass -> ConstraintMut
        4  finalize_pass -> InvalidTraderStatus
        4  init_config(bare) -> InvalidLimits
        4  multi -> TightModeOn
        4  post_journal -> BreachNotFlat
        4  post_journal -> EventPriceMismatch
        4  post_journal -> MustReportFail
        4  post_journal -> ProofWrongFeed
        4  post_journal -> TightEndNotFlat
        4  release_hold -> ConstraintMut
        4  report_fail -> MetaTwice
        4  unban_wallet -> ConstraintMut
        4  veto_pass -> ConstraintMut
        3  ban_wallet -> WalletHasHeldAccount
        3  confirm_pass -> ConstraintMut
        3  create_market -> PrivilegeEscalation
        3  create_ruleset -> AccountDiscriminatorMismatch
        3  ext -> ProductAlreadyPaused
        3  multi -> BatchMalformed
        3  multi -> MarketReduceOnly
        3  multi -> TimeInFuture
        3  open_account -> PrivilegeEscalation
        3  post_journal -> CreditMismatch
        3  post_journal -> MetaMissing
        3  post_journal -> TightNotEnded
        3  release_account -> MathOverflow
        3  report_fail -> BadSequence
        3  report_fail -> FailBalanceMismatch
        3  report_fail -> InvalidFlag
        3  report_fail -> PassPending
        3  report_fail -> QuickLimitExceeded
        3  request_advance -> PhaseFailed
        3  reset_account -> InvalidVaultAccount
        3  set_paused -> ConstraintMut
        3  update_market -> ConstraintMut
        2  finalize_pass -> AccountHeld
        2  multi -> AccountAlreadyReset
        2  multi -> EngineVersionMismatch
        2  multi -> MetaTwice
        2  multi -> NotFlat
        2  multi -> RejectReasonInvalid
        2  open_account -> InvalidChallengeTier
        2  post_journal -> PriceInvalid
        2  post_journal -> TightStartR
        2  report_fail -> MathOverflow
        2  request_advance -> InvalidFlag
        2  reset_account -> PrivilegeEscalation
        2  reset_account -> ViolationNoReset
        1  confirm_pass -> AccountHeld
        1  create_market -> ConstraintMut
        1  create_market -> Custom(0)
        1  create_market -> Custom(1)
        1  create_market -> Unauthorized
        1  multi -> Custom(1)
        1  multi -> FailNotBeforeMeta
        1  multi -> FailReasonMismatch
        1  multi -> InvalidAccountData
        1  multi -> InvalidLimits
        1  multi -> InvalidTokenAccount
        1  multi -> MarketDisabled
        1  multi -> MarketUnpriced
        1  multi -> MetaMissing
        1  multi -> MetaNotLast
        1  multi -> PassDelayNotElapsed
        1  multi -> PhaseFailed
        1  multi -> PrivilegeEscalation
        1  multi -> TightStartPOpen
        1  multi -> TimeBackwards
        1  multi -> WalletHasHeldAccount
        1  multi -> WrongPhase
        1  open_account -> Custom(1)
        1  open_account -> RuleSetMissing
        1  open_account -> WalletCapReached
        1  post_journal -> AccountNotEnoughKeys
        1  post_journal -> FloorLLevel
        1  post_journal -> FloorNotRaised
        1  post_journal -> ProofInFuture
        1  post_journal -> ProofUnknownSource
        1  post_journal -> QuantityInvalid
        1  post_journal -> TightStartLLevel
        1  reject_pass -> ConstraintMut
        1  release_account -> InvalidTraderStatus
        1  report_fail -> BatchTooLarge
        1  report_fail -> CreditMismatch
        1  report_fail -> FailMissing
        1  report_fail -> TimeBackwards
        1  report_fail -> TimeInFuture
        1  reset_account -> RuleSetMissing
        1  touch_activity -> MathOverflow
Setl8Error variants: 110 total, 75 produced by the run, never produced: ["AccountLocked", "BadPriceProof", "QuickMismatch", "TightStartInvalid", "FloorFormulaMismatch", "BreachInvalid", "FailInvalid", "PayoutNotAccepted", "ReturnDataInvalid", "EngineStateMismatch", "EmptyBatch", "ProofIndexOutOfRange", "ProofMissing", "ProofTooOld", "ProofMockNotAllowed", "ProofBadSignedUpdate", "ProofValueInvalid", "TimerInvalid", "TightAlreadyOn", "TightStartGPeak", "TightStartLStart", "BreachLLevel", "BreachBalance", "BreachNotBelowFloor", "BreachBelowStaticFloor", "PayoutTallyInvalid", "FailReasonInvalid", "FailTwice", "FailNotFlat", "FailStaticFloorNotReached", "FailQuickNotExceeded", "QuickLimitNeedsPhaseQuick", "ResetTierInvalid", "ResetPhaseOutOfRange", "ResetPriceOverflow"]
journal events accepted on chain (EventLogs of successful post_journal / report_fail):
  Breach       25
  Close        3191
  Fail         392
  Fee          624
  Fill         575
  FloorRaised  27
  Meta         3591
  TightEnd     82
  TightStart   288
state coverage (219 distinct (status, phase, pass_pending, tight, latch, held, flat, locked, q>0) combinations seen on successes):
     88405  account observations
      9478  b >= 1.08 S and flat (pass-ready)
      4067  b below static floor (open position)
      1964  held
     10633  latch
       145  locked (breach lock running)
      1901  pass_pending 1
      1261  pass_pending 2
       935  pass_pending 3 (violation marker)
      7894  phase 2
      3007  phase 3
      3316  phase 4
      3179  phase 5
      5499  q_tally > 0
     16663  status Abandoned
      5657  status Failed
       767  status Graduated
      6101  tight_on
       272  tight_on and latch
      3335  vetoes > 0
compute-budget errors: 0
panics inside the VAULT reached through our program: 0
known FINDING JRN-1 aborts (outer length prefix beyond the heap, not counted as violations): 443
violations by id / instruction (total 0):
first examples (replay one worker: FUZZ_SEED=<the seed in the brackets> FUZZ_THREADS=1 FUZZ_ITERS=<iterations of that worker>):
```

Violations: 0. Panics in the program: 0. Panics inside the vault reached through the program: 0. The only aborts are the 443 known
JRN-1 cases (outer Borsh length prefix beyond the 32 KB heap; state unchanged). Every one of the 26 instructions succeeded at least once.
The 219 distinct (status, phase, pass_pending, tight, latch, held, flat, locked, q_tally>0) combinations were reached on successes.

Limits of this fuzz (be blunt): the world is small (a handful of accounts and markets); the generator follows the instruction builders
of the harness, so account shapes it cannot imagine are not tried; two LiteSVM properties are not exercised (real signature
verification is off, and the clock only moves where the generator moves it); it does not prove absence of bugs, it found none in 50,000
iterations at this seed.
