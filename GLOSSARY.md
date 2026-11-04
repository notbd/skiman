# skiman

skiman keeps one person's agent skills and global instruction files in a single git repo as the source of truth, and distributes them to every agent tool that reads them.

## Language

### Things skiman manages

**Skill** (skill):
A folder whose top level holds a `SKILL.md` (YAML frontmatter + Markdown body, agentskills.io format).
_Avoid_: plugin, command, rule

**Own skill** (自己的 skill):
A skill the user writes and edits by hand; it has no upstream.
_Avoid_: mine, local skill, custom skill

**Third-party skill** (第三方 skill):
A skill that came from somewhere else and is tracked against its upstream, possibly with local modifications.
_Avoid_: vendor skill, external skill, installed skill

**Upstream** (上游):
The source a third-party skill came from (a git repo path, an archive, a local folder), pinned to a specific version.
_Avoid_: origin, remote, source

**Upstream snapshot** (上游原样):
An unmodified copy of a third-party skill's upstream at the pinned version, kept read-only in the skill repo and never distributed.
_Avoid_: vendor copy, pristine copy, base

**Local modification** (本地改动):
The difference between a third-party skill as it is in the skill repo and its upstream snapshot.
_Avoid_: patch, override, customization

**Global instruction file** (全局 instruction file):
A user-level file an agent tool loads into every session automatically, such as `~/.claude/CLAUDE.md` or `~/.codex/AGENTS.md`.
_Avoid_: system prompt, memory file, rules file

**Shared body** (共享正文):
The part of the global instructions that applies to every agent tool.
_Avoid_: common rules, base file

**Tool-specific addition** (专属补充):
Global instructions that apply to one agent tool only and are combined with the shared body for that tool.
_Avoid_: override, tool rules, extension

### Where things live

**Skill repo** (skill 仓库):
The user's git repo that holds the canonical copy of every skill and global instruction file, plus skiman's configuration for them.
_Avoid_: vault, store, dotfiles repo

**Agent tool** (工具):
A program that reads skills or global instruction files, such as Claude Code, Codex or Cursor.
_Avoid_: client, host, agent, IDE

**Tool directory** (工具目录):
A user-level directory an agent tool reads skills from, such as `~/.claude/skills` or `~/.agents/skills`.
_Avoid_: target, skill path, install dir

**Distribution** (分发):
Making every tool directory and global instruction file location reflect the skill repo.
_Avoid_: publish, deploy, link, sync

### Bringing an existing setup in

**Foreign skill** (外来 skill):
A skill folder in a tool directory that skiman did not put there and that no other manager (a plugin system, claude.ai sync, a tool's bundled skills) owns.
_Avoid_: stray skill, unmanaged skill, orphan

**Import folder** (导入文件夹):
A folder an LLM prepares from an existing setup, laid out exactly like the skill repo should look after import; skiman checks it with its everyday rules and moves it into the skill repo.
_Avoid_: import plan, manifest, staging area, migration script
