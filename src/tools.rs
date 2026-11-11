//! The agent tool table (spec "Agent tools (data)"). Everything skiman knows
//! about an agent tool is data in [`TOOLS`]; code iterates over it and never
//! matches on a tool id.

use std::path::{Path, PathBuf};

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
    /// Files (relative to `HOME`) the tool loads instead of the instruction
    /// file when they exist; `doctor` reports them.
    pub instruction_overrides: &'static [&'static str],
    /// Size limit of the global instruction file in bytes; `None` when none is known.
    pub instruction_size_limit: Option<u64>,
    pub managed_elsewhere: &'static [ManagedElsewhere],
    /// Skill names this tool reserves (on top of [`crate::names::RESERVED`]).
    pub reserved_names: &'static [&'static str],
    /// Settings file that holds the tool's session-start hooks; `None` when the tool has none.
    pub session_hook_file: Option<&'static str>,
    /// Why skiman writes no session-start hook entry for this tool yet; `None` when it does.
    pub session_hook_unavailable: Option<&'static str>,
    /// Instruction files the tool loads inside a project folder, as patterns
    /// relative to that folder (`/`-separated; `**` matches any number of
    /// folders, `*` any part of one name). See `crate::project::pattern`.
    pub project_instruction_files: &'static [&'static str],
    /// Folders inside a project folder whose child folders the tool loads as skills.
    pub project_skill_dirs: &'static [&'static str],
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
        instruction_overrides: &[],
        instruction_size_limit: None,
        managed_elsewhere: &[
            ManagedElsewhere::Path(".claude/skills/synced"),
            ManagedElsewhere::Path(".claude/skills/.trash"),
            ManagedElsewhere::RealpathUnder(".claude/plugins"),
        ],
        reserved_names: &["synced"],
        session_hook_file: Some(".claude/settings.json"),
        session_hook_unavailable: None,
        // `docs/project-scope-evidence.md`: Claude Code also loads `AGENTS.md`,
        // and every `.md` under `.claude/rules/` at any depth.
        project_instruction_files: &[
            "CLAUDE.md",
            "CLAUDE.local.md",
            ".claude/CLAUDE.md",
            ".claude/rules/**/*.md",
            "AGENTS.md",
        ],
        project_skill_dirs: &[".claude/skills"],
    },
    AgentTool {
        id: "codex",
        presence_dir: ".codex",
        tool_dir: ".agents/skills",
        instruction_file: ".codex/AGENTS.md",
        // Codex reads `AGENTS.override.md` instead of `AGENTS.md` when it exists.
        instruction_overrides: &[".codex/AGENTS.override.md"],
        instruction_size_limit: Some(32768),
        managed_elsewhere: &[
            ManagedElsewhere::RealpathUnder(".codex/plugins"),
            ManagedElsewhere::Path(".codex/skills/.system"),
        ],
        reserved_names: &[],
        session_hook_file: Some(".codex/hooks.json"),
        // Phase 1 could not confirm that a trusted entry runs on startup and
        // resume (`docs/phase1-results.md`, experiment 4).
        session_hook_unavailable: Some(
            "Codex runs only the hooks you have trusted with `/hooks`; run `skiman sync` before you start a Codex session, or rely on the skill repo's git hooks",
        ),
        // Codex reads `AGENTS.override.md` instead of `AGENTS.md` at each level.
        project_instruction_files: &["AGENTS.md", "AGENTS.override.md"],
        project_skill_dirs: &[".agents/skills"],
    },
    AgentTool {
        id: "antigravity",
        presence_dir: ".gemini",
        tool_dir: ".gemini/config/skills",
        instruction_file: ".gemini/GEMINI.md",
        instruction_overrides: &[],
        // Bytes, inclusive: phase 1 measured 24000 loads whole, 24001 is
        // truncated (`docs/phase1-results.md`, 1c).
        instruction_size_limit: Some(24000),
        managed_elsewhere: &[],
        reserved_names: &[],
        session_hook_file: None,
        session_hook_unavailable: None,
        project_instruction_files: &["GEMINI.md", "AGENTS.md"],
        project_skill_dirs: &[".agent/skills", ".agents/skills"],
    },
];
