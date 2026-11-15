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
An unmodified copy of a third-party skill's upstream at the pinned version, without the upstream's `.gitignore`, `.gitattributes` and `.gitmodules` files, kept read-only in the skill repo and never distributed.
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

**Summary** (用途):
One line in `skiman.toml` saying what a skill is for, written for a person; the frontmatter `description` stays the text an agent tool reads.
_Avoid_: description, purpose, title

**Group** (组):
A free-form label in `skiman.toml` that puts skills which belong together in one section of `skiman overview`.
_Avoid_: category, tag, bundle

**Origin** (来历):
Where an own skill came from (the setup it was adapted from, or that it was written here); a third-party skill's upstream already says it.
_Avoid_: source, upstream, provenance

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
Making every tool directory and global instruction file location reflect the skill repo; the command that performs it is `skiman sync`.
_Avoid_: publish, deploy, link

**Managed elsewhere** (别的管理器管的):
Content in a tool directory that another manager owns (a plugin system, claude.ai sync, a tool's bundled skills); skiman never touches it.
_Avoid_: system skill, protected entry

### Project scope

**Project root** (项目根目录):
A folder registered in `skiman.toml` as `[projects.<id>]`; skiman discovers project items under it.
_Avoid_: local root, workspace, scan folder

**Project instruction file** (项目 instruction file):
A file an agent tool loads when it works inside a project folder, such as that folder's `AGENTS.md`, or a `.md` file under its `.claude/rules/`.
_Avoid_: local instruction file, project rules file

**Project skill** (项目 skill):
A skill folder in a project-level skill directory, such as `<project>/.claude/skills/<name>`, or in a folder a project root lists in `skill_dirs`.
_Avoid_: local skill, workspace skill

**Project item** (项目条目):
One project instruction file or one project skill: the unit skiman copies and syncs.
_Avoid_: local item, entry

**Mirror** (镜像):
`projects/<root id>/<path relative to the root>` in the skill repo: the recorded copy of a project item. It follows the project; the project is written only by `skiman project restore`.
_Avoid_: copy, backup, shadow

**Anchor** (所属项目文件夹):
The project folder a project item belongs to: the item's path without its pattern (`proj` for `proj/AGENTS.md` and `proj/.claude/skills/x`).
_Avoid_: project dir, parent

**Synced hash** (同步 hash):
Per machine, the content hash a project item's two sides had when skiman last saw them equal; it tells which side changed.
_Avoid_: base, baseline, last hash

### Bringing an existing setup in

**Foreign skill** (外来 skill):
A skill folder in a tool directory that skiman does not own and that is not managed elsewhere.
_Avoid_: stray skill, unmanaged skill, orphan

**Import folder** (导入文件夹):
A folder an LLM prepares from an existing setup, laid out exactly like the skill repo should look after import; skiman checks it with its everyday rules and copies it into the skill repo.
_Avoid_: import plan, manifest, staging area, migration script
