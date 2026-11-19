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

/// A place another manager keeps skills that an agent tool reads; `skiman
/// overview` lists the skills there and never changes them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsElsewhere {
    /// Skill folders exactly `depth` levels below `dir` (relative to `HOME`),
    /// kept by `owner`.
    Folders {
        dir: &'static str,
        depth: u8,
        owner: &'static str,
    },
    /// The skills of every plugin in this Claude Code plugin registry
    /// (`installed_plugins.json`, relative to `HOME`); `settings` is the
    /// settings file whose `enabledPlugins` says which ones are turned off.
    ClaudePlugins {
        registry: &'static str,
        settings: &'static str,
    },
}

/// Where a tool keeps its session-start hooks, and in what shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionHook {
    /// The JSON file (relative to `HOME`).
    pub file: &'static str,
    pub shape: HookShape,
}

/// How a session-start hook file lists its hooks. A handler is
/// `{"type": "command", "command": ..., "timeout": ...}` in both.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookShape {
    /// `hooks.SessionStart` is a list of groups `{"matcher", "hooks": [handler]}`;
    /// skiman adds one group with this matcher.
    Grouped { matcher: &'static str },
    /// The top level maps hook names to hooks whose `SessionStart` is a flat
    /// list of handlers; skiman adds the hook with this name.
    Named { name: &'static str },
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
    /// Files (relative to `HOME`) the tool loads on top of the instruction
    /// file when they exist; `doctor` reports them.
    pub instruction_additions: &'static [&'static str],
    /// Size limit of the global instruction file in bytes; `None` when none is known.
    pub instruction_size_limit: Option<u64>,
    /// Bytes of a `SKILL.md` body the tool keeps when the skill is used,
    /// counted after the frontmatter's closing `---` line and without
    /// surrounding whitespace; the rest is cut without a message. `None`
    /// when none is known.
    pub skill_body_size_limit: Option<u64>,
    pub managed_elsewhere: &'static [ManagedElsewhere],
    /// Where other managers keep skills this tool reads (`skiman overview`).
    pub skills_elsewhere: &'static [SkillsElsewhere],
    /// Skills this tool reads that `skiman overview` cannot list, each said
    /// as what follows "`<tool id>` also reads".
    pub skills_not_listed: &'static [&'static str],
    /// Skill names this tool reserves (on top of [`crate::names::RESERVED`]).
    pub reserved_names: &'static [&'static str],
    /// Where the tool's session-start hooks are; `None` when the tool has none.
    pub session_hook: Option<SessionHook>,
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
        instruction_additions: &[],
        instruction_size_limit: None,
        skill_body_size_limit: None,
        managed_elsewhere: &[
            ManagedElsewhere::Path(".claude/skills/synced"),
            ManagedElsewhere::Path(".claude/skills/.trash"),
            ManagedElsewhere::RealpathUnder(".claude/plugins"),
        ],
        // One folder per claude.ai account under `synced`.
        skills_elsewhere: &[
            SkillsElsewhere::Folders {
                dir: ".claude/skills/synced",
                depth: 2,
                owner: "claude.ai sync",
            },
            SkillsElsewhere::ClaudePlugins {
                registry: ".claude/plugins/installed_plugins.json",
                settings: ".claude/settings.json",
            },
        ],
        skills_not_listed: &["built-in skills that are not stored on disk"],
        reserved_names: &["synced"],
        // Phase 1 experiment 5: not on `clear` or `compact`.
        session_hook: Some(SessionHook {
            file: ".claude/settings.json",
            shape: HookShape::Grouped {
                matcher: "startup|resume",
            },
        }),
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
        instruction_additions: &[],
        instruction_size_limit: Some(32768),
        skill_body_size_limit: None,
        managed_elsewhere: &[
            ManagedElsewhere::RealpathUnder(".codex/plugins"),
            ManagedElsewhere::Path(".codex/skills/.system"),
        ],
        skills_elsewhere: &[SkillsElsewhere::Folders {
            dir: ".codex/skills/.system",
            depth: 1,
            owner: "Codex, built in",
        }],
        // Which plugin versions Codex enables is in its own config, a format
        // skiman does not read.
        skills_not_listed: &["the skills of its plugins in `~/.codex/plugins`"],
        reserved_names: &[],
        session_hook: Some(SessionHook {
            file: ".codex/hooks.json",
            shape: HookShape::Grouped {
                matcher: "startup|resume",
            },
        }),
        // Phase 1 could not confirm that a trusted entry runs on startup and
        // resume (`docs/phase1-results.md`, experiment 4).
        session_hook_unavailable: Some(
            "Codex runs only the hooks you have trusted with `/hooks`; run `skiman sync` before you start a Codex session, or rely on the canon's git hooks",
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
        // agy 1.5.12 loads all four global locations together.
        instruction_additions: &[
            ".gemini/AGENTS.md",
            ".gemini/config/GEMINI.md",
            ".gemini/config/AGENTS.md",
        ],
        // Bytes, inclusive: phase 1 measured 24000 loads whole, 24001 is
        // truncated (`docs/phase1-results.md`, 1c).
        instruction_size_limit: Some(24000),
        // Bytes, inclusive: agy 1.5.12 keeps a body of 24000 bytes whole and
        // cuts one of 24001; the frontmatter does not count
        // (`docs/project-scope-evidence.md`).
        skill_body_size_limit: Some(24000),
        managed_elsewhere: &[],
        skills_elsewhere: &[],
        // agy 1.5.12 also loads these two folders (`docs/project-scope-evidence.md`).
        skills_not_listed: &[
            "the skills in `~/.gemini/skills`",
            "the skills in `~/.gemini/antigravity-cli/skills`",
        ],
        reserved_names: &[],
        // agy 1.5.12 runs it when a conversation starts, at its first prompt,
        // after it has read the skills for that prompt; a skill it links is
        // there from the next prompt (`docs/project-scope-evidence.md`).
        session_hook: Some(SessionHook {
            file: ".gemini/config/hooks.json",
            shape: HookShape::Named { name: "skiman" },
        }),
        session_hook_unavailable: None,
        project_instruction_files: &["GEMINI.md", "AGENTS.md"],
        project_skill_dirs: &[".agent/skills", ".agents/skills"],
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    /// `sync` decides what to leave alone from `managed_elsewhere`; a place
    /// the overview lists must be one of them, or the two lists drift apart.
    #[test]
    fn every_folder_of_skills_elsewhere_is_managed_elsewhere() {
        for tool in TOOLS {
            for place in tool.skills_elsewhere {
                if let SkillsElsewhere::Folders { dir, .. } = place {
                    assert!(
                        tool.managed_elsewhere.iter().any(|m| matches!(
                            m,
                            ManagedElsewhere::Path(p) | ManagedElsewhere::RealpathUnder(p)
                                if dir.starts_with(p)
                        )),
                        "{} lists {dir} but does not leave it to another manager",
                        tool.id
                    );
                }
            }
        }
    }
}
