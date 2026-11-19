# skiman

skiman manages one person's agent skills and global instruction files from a single git repo, the canon, and distributes them to every agent tool that reads them.

## Language

### Things skiman manages

**Skill** (skill):
A folder whose top level holds a `SKILL.md` (YAML frontmatter + Markdown body, agentskills.io format).
_Avoid_: plugin, command, rule

**Global skill** (全局 skill):
A skill an agent tool loads from a tool directory, so that every session has it. skiman distributes three kinds: own, third-party and linked.
_Avoid_: user skill, personal skill

**Own skill** (自己的 skill):
A global skill whose folder is in the canon and which has no upstream; the user writes and edits it there.
_Avoid_: mine, local skill, custom skill

**Third-party skill** (第三方 skill):
A global skill that came from somewhere else: its folder is in the canon and is tracked against its upstream, possibly with local modifications.
_Avoid_: vendor skill, external skill, installed skill

**Linked skill** (链入 skill):
A global skill whose folder is outside the canon: a linked folder itself, or a child folder of a linked folder. skiman distributes it and never writes it.
_Avoid_: referenced skill, shared skill, mounted skill

**Upstream** (上游):
The source a third-party skill came from (a git repo path, an archive, a local folder), pinned to a specific version.
_Avoid_: origin, remote, source

**Upstream snapshot** (上游原样):
An unmodified copy of a third-party skill's upstream at the pinned version, without the upstream's `.gitignore`, `.gitattributes` and `.gitmodules` files, kept read-only in the canon and never distributed.
_Avoid_: vendor copy, pristine copy, base

**Local modification** (本地改动):
The difference between a third-party skill as it is in the canon and its upstream snapshot.
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
One line saying what a skill is for, written for a person: in `skiman.toml` for a skill in the canon, in the skill's own frontmatter (`metadata.summary`) for a linked skill. The frontmatter `description` stays the text an agent tool reads; its first sentence stands in, marked as a stand-in, for a linked skill without a summary.
_Avoid_: description, purpose, title

**Group** (组):
A folder under `skills/` without a `SKILL.md` that holds the skill folders, or the inner group folders, which belong together (`skills/<group>/<name>`, where `<group>` may be `a/b`); `skiman overview` shows each group as one section.
_Avoid_: category, tag, bundle

**Origin** (来历):
Where an own skill came from (the setup it was adapted from, or that it was written here); a third-party skill's upstream already says it.
_Avoid_: source, upstream, provenance

### Where things live

**Canon** (正本库):
The user's git repo that is the source of truth for their agent setup: it holds every own skill, every third-party skill, the global instructions and the mirror, registers every linked folder and project root, and holds skiman's configuration.
_Avoid_: skill repo, vault, store, dotfiles repo

**Linked folder** (链入文件夹):
A folder outside the canon, registered in `skiman.toml` as `[linked.<id>]`: one skill folder, or a folder whose child folders holding a `SKILL.md` are skills. `skiman overview` shows each linked folder as one section.
_Avoid_: source, mount, external folder

**Agent tool** (工具):
A program that reads skills or global instruction files, such as Claude Code, Codex or Cursor.
_Avoid_: client, host, agent, IDE

**Tool directory** (工具目录):
A user-level directory an agent tool reads skills from, such as `~/.claude/skills` or `~/.agents/skills`.
_Avoid_: target, skill path, install dir

**Link** (link):
A symlink skiman makes, and makes again when it is missing. One in a tool directory, or where an agent tool looks for its global instruction file, points into the canon; the one the canon holds for a linked folder points to that folder.
_Avoid_: shortcut, alias, pointer

**Distribution** (分发):
Making every tool directory and global instruction file location reflect the canon; the command that performs it is `skiman sync`.
_Avoid_: publish, deploy

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
A skill folder in a project-level skill directory, such as `<project>/.claude/skills/<name>`.
_Avoid_: local skill, workspace skill

**Project item** (项目条目):
One project instruction file or one project skill: the unit skiman copies and syncs.
_Avoid_: local item, entry

**Mirror** (镜像):
`projects/<root id>/<path relative to the root>` in the canon: the recorded copy of a project item. It follows the project; the project is written only by `skiman project restore`.
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
A folder an LLM prepares from an existing setup, laid out exactly like the canon should look after import; skiman checks it with its everyday rules and copies it into the canon.
_Avoid_: import plan, manifest, staging area, migration script
