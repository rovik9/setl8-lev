//! Journal types (DESIGN 8.2). Filled in by module M2; M0 only fixes the limits.
#![forbid(unsafe_code)]

/// Maximum events per `post_journal` batch (DESIGN 8.2).
pub const MAX_EVENTS_PER_BATCH: usize = 8;
/// Maximum price proofs per batch.
pub const MAX_PROOFS_PER_BATCH: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn limits_match_design() {
        assert_eq!(MAX_EVENTS_PER_BATCH, 8);
        assert_eq!(MAX_PROOFS_PER_BATCH, 3);
    }
}
