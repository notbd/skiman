//! Exit statuses of the spec's CLI contract.

/// How a command ended. When several apply, the highest wins: combine them
/// with [`Exit::combine`] (or `max`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum Exit {
    /// 0: success.
    #[default]
    Success = 0,
    /// 1: rule violation or refused input.
    Refused = 1,
    /// 2: usage error, including a required flag missing without a TTY.
    Usage = 2,
    /// 3: needs attention (conflicts written, occupied paths, leftover records).
    NeedsAttention = 3,
    /// 4: operational error (git missing, fetch failed, I/O, lock timeout).
    OperationalError = 4,
}

impl Exit {
    pub fn code(self) -> u8 {
        self as u8
    }

    /// The more severe of the two.
    pub fn combine(self, other: Exit) -> Exit {
        self.max(other)
    }
}

impl From<Exit> for std::process::ExitCode {
    fn from(exit: Exit) -> Self {
        std::process::ExitCode::from(exit.code())
    }
}
