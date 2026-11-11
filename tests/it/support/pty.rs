use std::process::Command;

use rexpect::process::WaitStatus;
use rexpect::session::{PtySession, spawn_with_options};

use super::sandbox::COMMAND_TIMEOUT;

/// A command running under a pseudo-terminal: stdin, stdout and stderr are
/// all the terminal, and terminal echo is off.
pub struct Pty {
    session: PtySession,
}

impl Pty {
    /// Spawns `cmd` (usually from `Sandbox::std_command`) under a new PTY.
    pub fn spawn(mut cmd: Command) -> Self {
        cmd.env("TERM", "dumb");
        let options = rexpect::reader::Options::new()
            .timeout_ms(Some(COMMAND_TIMEOUT.as_millis() as u64))
            .strip_ansi_escape_codes(true);
        let session = spawn_with_options(cmd, options).expect("spawn command under a PTY");
        Pty { session }
    }

    /// Waits until `text` appears in the output and returns what came before it.
    #[track_caller]
    pub fn expect(&mut self, text: &str) -> String {
        self.session
            .exp_string(text)
            .unwrap_or_else(|e| panic!("waiting for {text:?}: {e}"))
    }

    /// Types `line` followed by Enter.
    #[track_caller]
    pub fn send_line(&mut self, line: &str) {
        self.session.send_line(line).expect("write to PTY");
    }

    /// Reads the remaining output until the command exits and returns its exit code.
    #[track_caller]
    pub fn wait(mut self) -> (i32, String) {
        let rest = self.session.exp_eof().expect("wait for end of output");
        match self.session.process().wait().expect("wait for command") {
            WaitStatus::Exited(_, code) => (code, rest),
            other => panic!("command did not exit normally: {other:?}"),
        }
    }
}
