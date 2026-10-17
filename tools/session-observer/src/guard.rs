//! Back-off of one lane: any 429 or 403 pauses the lane for 60 s; the third in a row (no success in between) pauses it for
//! 10 minutes. The clock is passed in, so the logic is tested with a fake one. A lane never exits on errors.

pub const BACKOFF_MS: u64 = 60_000;
pub const LONG_PAUSE_MS: u64 = 600_000;
pub const IN_A_ROW: u32 = 3;

#[derive(Debug, Default, Clone)]
pub struct Guard {
    consecutive: u32,
    paused_until: u64,
    pub total_429: u64,
    pub total_403: u64,
    pub long_pauses: u64,
}

/// What the lane should do after a response.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum Verdict {
    Continue,
    /// Paused for 60 s.
    Backoff,
    /// Three in a row: paused for 10 minutes.
    LongPause,
}

impl Guard {
    pub fn new() -> Guard {
        Guard::default()
    }

    /// Whether the lane may send a request at `now_ms`.
    pub fn may_send(&self, now_ms: u64) -> bool {
        now_ms >= self.paused_until
    }

    /// Milliseconds until the lane may send again (0 when it may).
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        self.paused_until.saturating_sub(now_ms)
    }

    /// Records an HTTP status (or `None` for a transport error, which neither counts nor resets).
    pub fn on_status(&mut self, status: Option<u16>, now_ms: u64) -> Verdict {
        match status {
            Some(429) | Some(403) => {
                if status == Some(429) {
                    self.total_429 += 1;
                } else {
                    self.total_403 += 1;
                }
                self.consecutive += 1;
                if self.consecutive >= IN_A_ROW {
                    self.consecutive = 0;
                    self.long_pauses += 1;
                    self.paused_until = now_ms + LONG_PAUSE_MS;
                    Verdict::LongPause
                } else {
                    self.paused_until = now_ms + BACKOFF_MS;
                    Verdict::Backoff
                }
            }
            Some(s) if (200..300).contains(&s) => {
                self.consecutive = 0;
                Verdict::Continue
            }
            _ => Verdict::Continue,
        }
    }
}

/// The exit rule of the whole run: only a signal or `--until` ends it.
pub fn should_stop(now_ms: u64, until_ms: Option<u64>, signalled: bool) -> bool {
    signalled || until_ms.is_some_and(|u| now_ms >= u)
}
