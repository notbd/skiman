# Local scope: project instruction files and project skills

Status: plan for review (council round 1). Branch `feat/local-scope`.

## Goal

skiman today manages global content only: `skills/<name>` linked into every
tool directory, and `instructions/shared.md` linked into every global
instruction file. Most of a real setup is local: 60+ `AGENTS.md` files under
`~/Work/Five-Continents`, project skills such as
`<project>/.claude/skills/fcusa-weekly-po`, a `CLAUDE.md` per code project.
None of them is in a git repo of its own, has history, or is backed up.

The user registers one or more folders (local roots). skiman scans each root
recursively and exhaustively, finds every local instruction file and local
skill, copies each one into the skill repo, and replaces the original with a
link to that copy. From then on the skill repo is the source of truth, `sync`
keeps the links in place, and every way a link can break is detected and has
one command that repairs it without losing content.

Requirements from the user, and where this plan meets them:

| Requirement | Where |
|---|---|
| Register one or more folders | `skiman local add`, `[local.<id>]` |
| Recursive, exhaustive scan | Scan |
| Lay the results out elegantly in the skill repo | Mirror layout |
| Copy, then replace the original with a link | `skiman local adopt` |
| Local edits skiman did not see | Drift and the state table |
| New providers, models, harnesses are easy to add | Tool table (data only) |
| Easy, lossless migration to another tool | Mirror layout, `skiman local eject` |

## Evidence the plan rests on

Run on 2026-11-06 in a throwaway folder with the real agent tools (Claude Code
2.1.311, codex-cli 0.166.0, agy 1.5.0); logs in the session scratchpad. Every
file and skill folder below was a symlink into a separate "repo" folder, each
holding a random token.

| Location in the project | Claude Code | Codex | Antigravity |
|---|---|---|---|
| `CLAUDE.md` (symlink) | loaded | no | no |
| `AGENTS.md` (symlink) | loaded | loaded | loaded |
| `GEMINI.md` (symlink) | no | no | loaded |
| `.claude/skills/<n>` (folder symlink) | loaded | no | no |
| `.agents/skills/<n>` (folder symlink) | no | loaded | loaded |
| `.agent/skills/<n>` (folder symlink) | no | no | loaded |
| `.gemini/skills/<n>` (folder symlink) | no | no | no |

Claude Code `@` imports: a regular `CLAUDE.md` with `@imported.md` loads the
sibling file (control). A symlinked `CLAUDE.md` resolves `@imported.md`
against the **link target's** folder, not the link's: with the target inside
the project the target-side file loaded and the link-side file did not; with
the target outside the project neither loaded (an external import, which
`claude -p` does not approve). So adopting a `CLAUDE.md` whose relative
imports work today would break them. This is a gate (below).

## Terms (added to GLOSSARY.md)

- **Local root** (本地根目录): a folder registered in `skiman.toml` as
  `[local.<id>]`; skiman manages the local items under it.
- **Local instruction file** (本地 instruction file): a file an agent tool
  loads when it works inside a project folder, such as `AGENTS.md`,
  `CLAUDE.md` or `GEMINI.md` there.
- **Local skill** (本地 skill): a skill folder in a project-level skill
  directory, such as `<project>/.claude/skills/<name>`.
- **Local item**: a local instruction file or a local skill; the unit skiman
  copies and links.
- **Mirror** (镜像): `local/<root id>/<path relative to the root>` in the skill
  repo: the canonical copy of a local item, and the target of its link.

## Design

### Tool table (the only place tool knowledge lives)

Two new columns in `tools.rs`, filled from the evidence above and the tools'
documented file names:

| Tool | `local_instruction_files` | `local_skill_dirs` |
|---|---|---|
| claude-code | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md` | `.claude/skills` |
| codex | `AGENTS.md`, `AGENTS.override.md` | `.agents/skills` |
| antigravity | `GEMINI.md`, `AGENTS.md` | `.agent/skills`, `.agents/skills` |

A local item is recognised when its root-relative path ends with one of the
union of these patterns. Adding a tool is one new row; the mirror layout,
scan, sync and rules never change. Local items are not per tool: one file on
disk is one mirror file, so `[tools].enabled` does not apply to them (a
disabled tool's files are still the user's content).

### Configuration

```toml
[local.work]
path = "~/Work/Five-Continents"
```

- `<id>` follows the skill name rule; `path` is absolute or starts with `~/`
  (written with `~/` when under `HOME`, so the repo stays portable).
- `skiman local add <path> [--id <id>]` writes the table. It refuses: a path
  that is not an existing folder; `HOME`, `/` or any ancestor of `HOME` (they
  overlap the global scope and hold whole tool directories); a folder inside
  the skill repo; a folder that contains or is inside another root (compared
  by realpath); a taken or invalid id. The default id is the folder name made
  into a valid name.
- `skiman check` checks the syntax (`unknown-key`, `toml-value`); the
  machine-dependent checks run where the path is resolved (`add`, `sync`,
  `scan`, `adopt`, `doctor`).

### Mirror layout

```
skill repo/
  skills/  upstream/  instructions/shared.md  skiman.toml   (unchanged)
  local/
    work/                                   [local.work]
      AGENTS.md
      30_美国业务_FiveC-USA/
        AGENTS.md
        .claude/skills/fcusa-weekly-po/SKILL.md
    dev/                                    [local.dev]
      wec-hist/AGENTS.md
```

`<root>/<rel>` on disk links to `<repo>/local/<id>/<rel>`. The mapping is pure
path arithmetic, so no per-item record exists anywhere: the mirror tree is the
record (nothing to keep in sync). The layout is what the user already sees on
disk, so anyone (a person, an LLM, another manager) can read it, and every
local skill is a standard agentskills.io folder.

### Ownership

A link at `<root>/<rel>` is owned when its target, normalised lexically, is
under `<repo>/local/` (or under the previous skill repo during `init`'s
switch, like today). Prefix ownership, not exact-target ownership, so a link
stays owned after its project folder moved inside the root or the root's id
changed; sync then retargets it. Global ownership is unchanged.

### Scan (read-only; `skiman local scan`, `adopt`, `doctor`)

Walks each root with `read_dir` and `lstat`:

- Never follows a directory symlink (no cycles, never leaves the root).
- Never enters `.git` or `node_modules`, the skill repo, skiman's state
  directory (its backups hold earlier originals), or any tool's presence
  directory and global tool directory (`~/.claude`, `~/.codex`, `~/.gemini`,
  `~/.agents`): those are the global scope.
- An entry whose relative path ends with an instruction pattern is an
  instruction candidate. Each entry of a directory whose relative path ends
  with a skill-dir pattern is a skill candidate when it is a folder holding
  `SKILL.md`, or a symlink. A skill folder is never entered, so files inside
  a skill are its content, not items of their own.
- A `SKILL.md` anywhere else is listed as "not in a skill directory any tool
  reads" (for example `fivec-core/skills/fivec`) and left alone.

### The gates (why a candidate is skipped)

1. **Tracked by a project's git repo** (nearest `.git` above the file; one
   `git ls-files` per repo): that repo is already its source of truth, and a
   symlink to an absolute path committed there would break every other
   clone. Skipped, listed per repo.
2. **Inside a git work tree, untracked and not ignored**: the next
   `git add -A` would commit the link. Skipped with the fix: commit it there,
   or ignore it (`.gitignore` or `.git/info/exclude`), then run adopt again.
   Ignored files (`git ls-files --others --exclude-standard` lists nothing)
   pass.
3. **Relative `@` imports that resolve today** (instruction files only):
   Claude Code would resolve them against the mirror (evidence above).
   Skipped with the import named.
4. **Content rules**: the everyday rules on the copy as it would sit in the
   mirror: path safety (no symlinks or special files inside a skill, size
   limits, Unicode collisions), the skill rules (frontmatter, name equals
   folder name), the secret rule (with the ready-to-paste `[[secrets.allow]]`
   hint). Skipped with the violations.
5. **A symlink at the item location** that is not owned: a link to another
   candidate or mirror item is an alias (for example `CLAUDE.md ->
   AGENTS.md`) and stays as it is; it keeps working once its target is
   adopted. A link elsewhere is listed and left alone.
6. **Paths that are not UTF-8**: listed and left alone.

### The state table (one classifier for scan, adopt, sync and doctor)

`P` is the path on disk, `M` its mirror. "Anchor" is the folder the item
belongs to: `P` without its pattern suffix (`proj/AGENTS.md` and
`proj/.claude/skills/x` both have anchor `proj`).

| State | Disk `P` | Mirror `M` | `sync` | `adopt` | Exit |
|---|---|---|---|---|---|
| managed | owned link to `M` | exists | nothing | nothing | 0 |
| misdirected | owned link elsewhere | exists | retarget | (sync does it) | 0 |
| missing | absent, anchor exists | exists | link (creates `.claude/skills` if needed) | (sync does it) | 0 |
| new | regular file or folder | absent | nothing | copy to `M`, original to backup, link | 0 |
| replaced | regular, same kind | exists | report | copy over `M` (old `M` to backup), original to backup, link | 3 |
| occupied | foreign link or other kind | exists | report | report | 3 |
| moved | owned link to existing `M'`, and `M'`'s own location is gone | absent | nothing (works) | move `M'` to `M` (git sees a rename), retarget | 0 |
| alias | owned link to `M'` whose own location still links to it | absent | nothing | nothing | 0 |
| dangling | owned link, target gone | absent | nothing | remove the link | 3 |
| project gone | anchor absent | exists | warn | `eject` drops `M` to a backup | 0 |

`sync` never writes outside owned links and never spawns git, so it stays fast
enough for session start: it walks only the mirror tree (tens of entries), not
the roots. The full disk walk runs only in `scan`, `adopt` and `doctor`.

### Local edits skiman did not see

- **Edited through the link** (most editors, agent tools that write through
  symlinks): the edit lands in the mirror; `git status` in the skill repo
  shows it and the git hooks check it on commit. Nothing to do.
- **The link was replaced by a regular file** (atomic save by some editors,
  `sed -i`, a tool that writes a temp file and renames it, a restored copy,
  `npx skills add` rewriting a skill folder): state `replaced`. `sync` and
  `sync --hook` report it, so the next agent session start prints it once,
  and the exit status is 3. `skiman local adopt <path>` takes the local
  content into the mirror; the previous mirror content and the original both
  go to the backup, and git shows the difference before the user commits.
  To drop the local edit instead: delete the file, run `skiman sync`.
  sync never replaces it on its own: content skiman does not own is replaced
  only by an explicit command (today's rule, kept).
- **The link was deleted**: state `missing`; `sync` relinks it, as it does
  for global skills. To stop managing an item: `skiman local eject`.
- **The project folder moved inside the root** (for example into
  `_archive/`): the link moves with it and keeps working. `sync` warns
  "project gone" for the old path; `adopt` sees `moved` and moves the mirror
  along.
- **The project folder was deleted**: "project gone" warning; `skiman local
  eject <old path>` moves the mirror to a backup.
- **The mirror was deleted or renamed in the skill repo** (by hand, by a
  `git checkout`): the link dangles; `scan` and `doctor` report it, `adopt`
  removes it.
- **A file changes while adopt copies it**: adopt hashes the original before
  copying and again right before moving it to the backup. On a mismatch it
  leaves the original in place, so the item shows as `replaced` and the next
  adopt takes the newer content; nothing is lost.
- **A project becomes a git repo after adoption**: `doctor` reports every
  managed link inside a git work tree that is not ignored.
- **The skill repo moved** (`init` to a new path): handled by today's
  previous-repo ownership; local links are retargeted like global ones.
- **Another machine**: the mirror is committed, so `sync` links every item
  whose root and anchor exist there; a machine that has its own regular file
  shows `replaced`, never overwritten.

### Commands

| Command | What it does |
|---|---|
| `skiman local add <path> [--id <id>]` | Register a local root. |
| `skiman local scan [<path>]` | Read-only: every candidate under the roots (or under `<path>`) with its state, gate and what adopt would do. Exit 3 when something needs attention. |
| `skiman local adopt [<path>...] [--yes]` | Under the lock: scan, print the plan, confirm, then for each adoptable item stage the copy, run the rules, rename it into the mirror, move the original to `backups/<ts>/local/<id>/<rel>`, link. Then repairs moved and dangling links, warns when git ignores a new mirror path, and ends with `sync` like every write command. |
| `skiman local eject <path>... [--yes]` | The inverse: every mirror item under `<path>` becomes a regular file or folder again (copy to a temporary sibling, verify, rename over the link), and its mirror goes to `backups/<ts>/repo/local/...`. A root's own path also removes its `[local.<id>]` table. |

All four follow today's conventions: without a terminal `--yes` is required,
exit codes 0/1/2/3/4, the repo lock, staging under `.skiman/tmp/`, backups
under machine state.

### Rules (`skiman check`, the pre-commit hook)

- `local-path` (exit 1): anything under `local/` that is not inside a
  configured root id, or not part of a recognised local item (an instruction
  file matching a pattern, or a skill folder in a skill-dir pattern). Config
  and mirror must agree, so a hand-deleted `[local.<id>]` blocks the commit
  until `local/<id>` goes too.
- The skill rules on every local skill folder (frontmatter, name equals
  folder name, sizes, no symlinks, unresolved conflicts).
- Instruction files: a regular file, the file size limit.
- The secret rule on every file under `local/`; `[[secrets.allow]]` paths may
  name `local/...`.

### Other commands

- `doctor`: per root, whether it exists, the number of mirror items, every
  item not `managed` with its fix, managed links inside git work trees that
  are not ignored, and how many local candidates are not adopted yet.
- `managing-skills` (the skill shipped to agents): a short section on local
  items, so an agent that creates a project `AGENTS.md` or skill under a root
  runs `skiman local adopt <path>`, and knows imports and links.
- `README.md`, `GLOSSARY.md`, ADR 0006 (mirror layout, prefix ownership,
  git-tracked files stay with their repo, sync never absorbs).

### Migration to another tool

- The mirror is plain files in the layout the user sees on disk, and local
  skills are standard skill folders: any tool can read `local/` directly, and
  `npx skills add <repo>/local/<id>/<rel>/.claude/skills/<n>` works.
- `skiman local eject <root>` puts every real file back where it was and
  unregisters the root: the exact pre-skiman state, with the history kept in
  the skill repo.

## Not in scope (kept out on purpose)

- Watching files continuously (a daemon). Session-start and git-hook syncs
  plus `doctor` cover the detection; a daemon adds a process to keep alive.
- Absorbing drift automatically. A silent absorb hides edits; the explicit
  `adopt` shows the diff.
- Fanning one mirror file out to several names (one `AGENTS.md` serving as
  `CLAUDE.md` too). Aliases on disk already do it and survive adoption.
- `skiman list` output for local items; `local scan` is the status view.
- Per-root exclude patterns. Adopt takes paths, so the user adopts a subset.
- Managing files tracked by another git repo.

## Test plan

Integration tests in the existing sandbox harness (a fake `HOME`, real git),
one file `tests/local.rs`:

1. `local add`: refusals (HOME, ancestor of HOME, inside repo, nesting both
   ways, taken id, missing folder); the written `~/` path.
2. Scan: every pattern from the tool table; skill folders not entered; dir
   symlinks not followed; exclusions (`.git`, `node_modules`, skill repo,
   state dir, global tool dirs); loose `SKILL.md` listed.
3. Gates: tracked, untracked-unignored, ignored (adopted), relative import
   that resolves (skipped) and one that does not (adopted), secret, bad
   frontmatter, symlink inside a skill, alias link kept.
4. Adopt: new file and new skill end as owned links with identical content
   and the originals in the backup; replaced (both old versions in the
   backup); moved; dangling removed; concurrent change detected (test hook);
   rerun is a no-op.
5. Sync: missing relinked (creating `.claude/skills`), misdirected retargeted,
   replaced and occupied reported with exit 3, project gone warned, hook mode
   prints each once, `init` repo switch retargets local links.
6. Eject: file and skill restored byte-identical, mirror in the backup, root
   eject removes the table and `local/<id>`.
7. Check: `local-path` cases, secret in a mirror file, skill rules on a local
   skill.
8. Doctor: the local section.

Plus a manual end-to-end run with the real tools (the evidence experiment
repeated through `skiman local adopt` on a throwaway root).

## Acceptance

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` pass (the CI job).
- Council round on the plan and on the implementation: every delegate scores
  above 9.5 and approves.
