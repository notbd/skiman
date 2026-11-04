//! The agent tool table (spec "Agent tools (data)"). Everything skiman knows
//! about an agent tool is data in [`TOOLS`]; code iterates over it and never
//! matches on a tool id.

use std::path::{Path, PathBuf};

/// How skiman puts a skill or the shared body into an agent tool's location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A symlink to the skill repo (the spec default).
    Link,
    /// A copy refreshed at every refresh point (for tools that cannot follow symlinks).
    Copy,
}

/// Content in a tool directory that another manager owns; skiman never touches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedElsewhere {
    /// This path (relative to `HOME`) and everything below it.
    Path(&'static str),
    /// Any entry whose realpath lies under this directory (relative to `HOME`).
    RealpathUnder(&'static str),
}

/// Any folder that contains this file is managed elsewhere (a plugin), in every tool.
pub const PLUGIN_MARKER: &str = ".claude-plugin/plugin.json";

/// One row of the agent tool table. Paths are relative to `HOME`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentTool {
    pub id: &'static str,
    /// The tool counts as present on this machine when this directory exists.
    pub presence_dir: &'static str,
    /// Where the tool reads skills from.
    pub tool_dir: &'static str,
    /// The global instruction file location.
    pub instruction_file: &'static str,
    /// Size limit of the global instruction file in bytes; `None` when none is known.
    pub instruction_size_limit: Option<u64>,
    pub skill_mode: Mode,
    pub instruction_mode: Mode,
    pub managed_elsewhere: &'static [ManagedElsewhere],
    /// Skill names this tool reserves (on top of [`crate::names::RESERVED`]).
    pub reserved_names: &'static [&'static str],
    /// Settings file that holds the tool's session-start hooks; `None` when the tool has none.
    pub session_hook_file: Option<&'static str>,
}

impl AgentTool {
    pub fn presence_dir(&self, home: &Path) -> PathBuf {
        home.join(self.presence_dir)
    }

    pub fn tool_dir(&self, home: &Path) -> PathBuf {
        home.join(self.tool_dir)
    }

    /// Whether the presence directory exists on this machine.
    pub fn is_present(&self, home: &Path) -> bool {
        self.presence_dir(home).is_dir()
    }
}

/// Every known agent tool, in the order the spec lists them. A new
/// `skiman.toml` lists the present ones in this order.
pub const TOOLS: &[AgentTool] = &[
    AgentTool {
        id: "claude-code",
        presence_dir: ".claude",
        tool_dir: ".claude/skills",
        instruction_file: ".claude/CLAUDE.md",
        instruction_size_limit: None,
        skill_mode: Mode::Link,
        instruction_mode: Mode::Link,
        managed_elsewhere: &[
            ManagedElsewhere::Path(".claude/skills/synced"),
            ManagedElsewhere::Path(".claude/skills/.trash"),
            ManagedElsewhere::RealpathUnder(".claude/plugins"),
        ],
        reserved_names: &["synced"],
        session_hook_file: Some(".claude/settings.json"),
    },
    AgentTool {
        id: "codex",
        presence_dir: ".codex",
        tool_dir: ".agents/skills",
        instruction_file: ".codex/AGENTS.md",
        instruction_size_limit: Some(32768),
        skill_mode: Mode::Link,
        instruction_mode: Mode::Link,
        managed_elsewhere: &[
            ManagedElsewhere::RealpathUnder(".codex/plugins"),
            ManagedElsewhere::Path(".codex/skills/.system"),
        ],
        reserved_names: &[],
        session_hook_file: Some(".codex/hooks.json"),
    },
    AgentTool {
        id: "antigravity",
        presence_dir: ".gemini",
        tool_dir: ".gemini/config/skills",
        instruction_file: ".gemini/GEMINI.md",
        // Unit to confirm in phase 1 (#3).
        instruction_size_limit: Some(24000),
        skill_mode: Mode::Link,
        instruction_mode: Mode::Link,
        managed_elsewhere: &[],
        reserved_names: &[],
        session_hook_file: None,
    },
];
