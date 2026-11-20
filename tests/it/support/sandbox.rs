use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::time::Duration;

use snapbox::cmd::Command;

use super::pty::Pty;
use super::upstream::{Upstream, run_git};

/// How long a single command may run before the test fails.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// The agent tools of the spec's "Agent tools (data)" table, as seen from the test side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    ClaudeCode,
    Codex,
    Antigravity,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::ClaudeCode, Tool::Codex, Tool::Antigravity];

    /// Presence directory, relative to `HOME`.
    pub fn presence_dir(self) -> &'static str {
        match self {
            Tool::ClaudeCode => ".claude",
            Tool::Codex => ".codex",
            Tool::Antigravity => ".gemini",
        }
    }

    /// Tool directory, relative to `HOME`.
    pub fn tool_dir(self) -> &'static str {
        match self {
            Tool::ClaudeCode => ".claude/skills",
            Tool::Codex => ".agents/skills",
            Tool::Antigravity => ".gemini/config/skills",
        }
    }

    /// Instruction location, relative to `HOME`.
    pub fn instruction_file(self) -> &'static str {
        match self {
            Tool::ClaudeCode => ".claude/CLAUDE.md",
            Tool::Codex => ".codex/AGENTS.md",
            Tool::Antigravity => ".gemini/GEMINI.md",
        }
    }
}

/// What the sandbox creates for one agent tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSetup {
    /// Neither the presence directory nor the tool directory exists.
    Absent,
    /// Only the presence directory exists.
    PresenceOnly,
    /// The presence directory and the tool directory exist (the default).
    Installed,
}

pub struct SandboxBuilder {
    tools: Vec<(Tool, ToolSetup)>,
}

impl SandboxBuilder {
    pub fn tool(mut self, tool: Tool, setup: ToolSetup) -> Self {
        self.tools.retain(|(t, _)| *t != tool);
        self.tools.push((tool, setup));
        self
    }

    pub fn build(self) -> Sandbox {
        let temp = tempfile::Builder::new()
            .prefix("skiman-test-")
            .tempdir()
            .expect("create sandbox temp dir");
        // macOS temp dirs sit behind `/var -> /private/var`; skiman compares
        // realpaths, so every sandbox path is a realpath from the start.
        let root = temp.path().canonicalize().expect("canonicalize sandbox");
        let sandbox = Sandbox { _temp: temp, root };
        for dir in [
            sandbox.home(),
            sandbox.config_home(),
            sandbox.state_home(),
            sandbox.tmp_dir(),
            sandbox.upstreams_dir(),
        ] {
            std::fs::create_dir_all(dir).expect("create sandbox dir");
        }
        for (tool, setup) in &self.tools {
            if *setup != ToolSetup::Absent {
                std::fs::create_dir_all(sandbox.presence_dir(*tool)).expect("create presence dir");
            }
            if *setup == ToolSetup::Installed {
                std::fs::create_dir_all(sandbox.tool_dir(*tool)).expect("create tool dir");
            }
        }
        let git_config = format!(
            "[url \"{}/\"]\n\tinsteadOf = https://github.com/\n",
            sandbox.upstreams_dir().display()
        );
        std::fs::write(sandbox.git_config(), git_config).expect("write sandbox git config");
        sandbox
    }
}

/// A fresh, isolated world for one test: its own `HOME`, XDG directories,
/// global git config, temp dir, agent tool directories and upstream repos.
/// Everything is deleted when the value is dropped.
pub struct Sandbox {
    _temp: tempfile::TempDir,
    root: PathBuf,
}

impl Default for Sandbox {
    fn default() -> Self {
        Self::new()
    }
}

impl Sandbox {
    /// A sandbox with all three agent tools installed.
    pub fn new() -> Self {
        Self::builder().build()
    }

    pub fn builder() -> SandboxBuilder {
        SandboxBuilder {
            tools: Tool::ALL
                .iter()
                .map(|t| (*t, ToolSetup::Installed))
                .collect(),
        }
    }

    /// Realpath of the sandbox; every other path is below it.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn home(&self) -> PathBuf {
        self.root.join("home")
    }

    pub fn config_home(&self) -> PathBuf {
        self.home().join(".config")
    }

    pub fn state_home(&self) -> PathBuf {
        self.home().join(".local/state")
    }

    /// The file `GIT_CONFIG_GLOBAL` points at.
    pub fn git_config(&self) -> PathBuf {
        self.root.join("gitconfig")
    }

    /// `TMPDIR` for every command.
    pub fn tmp_dir(&self) -> PathBuf {
        self.root.join("tmp")
    }

    /// Where upstream repos live; `https://github.com/` is rewritten to this directory.
    pub fn upstreams_dir(&self) -> PathBuf {
        self.root.join("upstreams")
    }

    /// A location for the canon. Not created, so `skiman init` can create it.
    pub fn canon(&self) -> PathBuf {
        self.root.join("canon")
    }

    pub fn presence_dir(&self, tool: Tool) -> PathBuf {
        self.home().join(tool.presence_dir())
    }

    pub fn tool_dir(&self, tool: Tool) -> PathBuf {
        self.home().join(tool.tool_dir())
    }

    pub fn instruction_file(&self, tool: Tool) -> PathBuf {
        self.home().join(tool.instruction_file())
    }

    /// The complete environment every command runs with (nothing is inherited).
    pub fn envs(&self) -> Vec<(OsString, OsString)> {
        let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/bin:/bin".into());
        [
            ("PATH", path),
            ("HOME", self.home().into()),
            ("XDG_CONFIG_HOME", self.config_home().into()),
            ("XDG_STATE_HOME", self.state_home().into()),
            ("GIT_CONFIG_GLOBAL", self.git_config().into()),
            ("GIT_CONFIG_NOSYSTEM", "1".into()),
            ("TMPDIR", self.tmp_dir().into()),
            ("LC_ALL", "C".into()),
        ]
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect()
    }

    /// A `std::process::Command` for `program` with the sandbox environment and
    /// `HOME` as the working directory.
    pub fn std_command(&self, program: impl AsRef<OsStr>) -> StdCommand {
        let mut cmd = StdCommand::new(program);
        cmd.env_clear().envs(self.envs()).current_dir(self.home());
        cmd
    }

    /// Any program, run like `skiman()`.
    pub fn command(&self, program: impl AsRef<OsStr>) -> Command {
        Command::from_std(self.std_command(program)).timeout(COMMAND_TIMEOUT)
    }

    /// The real `skiman` binary, without a TTY (stdin is a pipe).
    pub fn skiman(&self) -> Command {
        self.command(skiman_bin())
    }

    /// `program` under macOS `sandbox-exec`: network access fails, and any
    /// write outside the sandbox root (and `/dev`) kills the process with
    /// SIGKILL, so the command cannot succeed after an escaped write. One
    /// write outside is allowed: Apple's `/usr/bin/git` is an xcrun shim
    /// that, on its first run under a `HOME`, creates an `xcrun_db-*` cache
    /// in the user's temp folder (`DARWIN_USER_TEMP_DIR`, not `TMPDIR`).
    /// Other systems have no such tool on every machine: there `program`
    /// runs like `command()`, and the macOS run is the one that proves it.
    pub fn confined(&self, program: impl AsRef<OsStr>) -> Command {
        if cfg!(not(target_os = "macos")) {
            return self.command(program);
        }
        let profile = format!(
            "(version 1)(allow default)(deny network*)\
             (deny file-write* (with send-signal SIGKILL) (require-not (require-any (subpath \"{}\") (subpath \"/dev\") \
             (regex #\"^/private/var/folders/[^/]+/[^/]+/T/xcrun_db\"))))",
            self.root.display()
        );
        self.command("/usr/bin/sandbox-exec")
            .arg("-p")
            .arg(profile)
            .arg(program)
    }

    /// The real `skiman` binary, confined like `confined()`.
    pub fn skiman_confined(&self) -> Command {
        self.confined(skiman_bin())
    }

    /// The real `skiman` binary under a pseudo-terminal.
    pub fn skiman_pty<I, S>(&self, args: I) -> Pty
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = self.std_command(skiman_bin());
        cmd.args(args);
        Pty::spawn(cmd)
    }

    /// Runs git with the sandbox environment in `dir`; panics on failure and
    /// returns stdout with the trailing newline removed.
    pub fn git<I, S>(&self, dir: impl AsRef<Path>, args: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        run_git(&self.envs(), dir.as_ref(), args)
    }

    /// Creates an empty upstream repo for `slug` (`owner/repo`) on branch `main`.
    /// `https://github.com/<slug>.git` resolves to it through the sandbox git config.
    pub fn upstream(&self, slug: &str) -> Upstream {
        Upstream::init(self.envs(), self.upstreams_dir(), slug)
    }
}

fn skiman_bin() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_skiman"))
}
