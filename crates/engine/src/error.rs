//! Engine errors. `InvalidQuantity` and `InvalidStopLevel` are the rejection codes the evaluator API returns to a
//! trader (DESIGN 7.6); they are not program errors (DESIGN 12).

/// Why an engine function refused its input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineError {
    /// A checked arithmetic step overflowed, or a result does not fit its type.
    Overflow,
    /// Zero quantity, not a multiple of the step, below the minimum, or a fill larger than `i64::MAX` units (N-1).
    InvalidQuantity,
    /// A stop-loss or take-profit on the wrong side of the market, or a level of 0 (N-4).
    InvalidStopLevel,
    /// Everything else that is not a legal input: zero price, zero leverage or step, half-spread above 100%, a
    /// position with zero quantity, a fill dated before the position's timer, a negative notional, or a fee day in the future.
    InvalidInput,
}

/// Result alias of the engine.
pub type Result<T> = core::result::Result<T, EngineError>;
