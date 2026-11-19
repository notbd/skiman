//! A command that stops early: the exit status and the message for stderr.

use std::fmt;
use std::path::Path;

use crate::Exit;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Failure {
    pub exit: Exit,
    pub message: String,
}

impl Failure {
    /// Exit 1: rule violation or refused input.
    pub fn refused(message: impl Into<String>) -> Self {
        Failure {
            exit: Exit::Refused,
            message: message.into(),
        }
    }

    /// Exit 1 for what is wrong with the text of `file`, such as a
    /// `skiman.toml` that does not parse or cannot take an edit:
    /// `.map_err(Failure::refused_in(&file))`.
    pub fn refused_in(file: &Path) -> impl Fn(String) -> Self + '_ {
        move |e| Failure::refused(format!("{}: {e}", file.display()))
    }

    /// Exit 2: usage error, including a required flag missing without a TTY.
    pub fn usage(message: impl Into<String>) -> Self {
        Failure {
            exit: Exit::Usage,
            message: message.into(),
        }
    }

    /// Exit 4: operational error (git missing, I/O, lock timeout).
    pub fn operational(message: impl Into<String>) -> Self {
        Failure {
            exit: Exit::OperationalError,
            message: message.into(),
        }
    }

    /// Prints the message to stderr and returns the exit status.
    pub fn report(self) -> Exit {
        err!("error: {}", self.message);
        self.exit
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Runs a command body and turns a [`Failure`] into its printed message and exit status.
pub fn finish(result: Result<Exit, Failure>) -> Exit {
    result.unwrap_or_else(Failure::report)
}
