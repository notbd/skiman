# Project scope: project instruction files and project skills

Status: plan, council round 3. Branch `feat/local-scope`.

## Goal

skiman manages global content only: `skills/<name>` linked into every tool
directory, and `instructions/shared.md` linked into every global instruction
file. Most of a real setup is per project: 76 `AGENTS.md` files under
`~/Work/Five-Continents`, project skills such as
`<project>/.claude/skills/fcusa-weekly-po`, a `CLAUDE.md` per code project.
They have no history and no backup.

The user registers one or more folders (project roots). skiman finds every
project instruction file and project skill under them and keeps a copy of
each in the skill repo (the mirror). The project files stay real files and
stay where they are; the mirror follows them automatically, so the skill repo
holds their history. The project is written only when the user asks, and
whatever skiman replaces on either side goes to a backup first.

| Requirement | Where |
|---|---|
| Register one or more folders | `skiman project add`, `[projects.<id>]` |
| Recursive, exhaustive scan | Discovery |
| Lay the results out elegantly in the skill repo | Mirror layout |
| Copy into the skill repo, keep the original working | Copy mode, `adopt` |
| Local edits skiman did not see | Sync and the state table |
| New providers, models, harnesses are easy to add | Tool table |
| Easy, lossless migration to another tool | Project files stay real files; plain mirror |

## Model: the mirror follows the project

Two decisions came out of measuring the real tools and two council rounds:

1. **Copy, not links.** Round 1 replaced each project file with a symlink into
   the skill repo, like global skills. That breaks how these files are edited
   (`docs/project-scope-evidence.md`): Claude Code does not edit a symlinked
   `AGENTS.md` (it treats the target as outside the working directory),
   resolves `@` imports in a symlinked `CLAUDE.md` against the target folder,
   and labels the file with its target path. Global locations are derived
   (nobody edits `~/.claude/skills/x` in place), so links stay right there;
   project folders are where people and agents edit, so they keep real files.
2. **One automatic direction.** Round 2 also wrote mirror changes back to the
   project at every sync. A routine `git restore .` in the skill repo then
   overwrote project content that existed nowhere else, and a conflicted
   `git pull` could write conflict markers into a project. Now sync only
   copies project → mirror; mirror → project happens only through
   `skiman project restore`, which the user runs and which backs up what it
   replaces.

The analogy is a backup tool with history: the project is the live copy, the
mirror is the versioned record, `restore` brings a version back. Projects
never depend on skiman: uninstall it and every project works unchanged.

## Terms (added to GLOSSARY.md)

"Local" is avoided: the glossary lists "local skill" as a term to avoid, and
Claude Code uses "local" for personal uncommitted files (`CLAUDE.local.md`).

- **Project root** (项目根目录): a folder registered as `[projects.<id>]`;
  skiman discovers project items under it.
- **Project instruction file** (项目 instruction file): a file an agent tool
  loads when it works inside a project folder, such as `AGENTS.md` there, or
  a file in a project-level rules folder (`.claude/rules/`).
- **Project skill** (项目 skill): a skill folder in a project-level skill
  directory, such as `<project>/.claude/skills/<name>`.
- **Project item**: the unit skiman copies: one instruction file, one rules
  folder, or one project skill folder.
- **Mirror** (镜像): `projects/<root id>/<path relative to the root>` in the
  skill repo: the recorded copy of a project item.
- **Base**: per machine, the content hash both sides had when skiman last saw
  them equal; it tells which side changed.

## Design

### Tool table

Three new columns in `tools.rs`, from the evidence and the tools' docs:

| Tool | `project_instruction_files` | `project_instruction_dirs` | `project_skill_dirs` |
|---|---|---|---|
| claude-code | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md`, `AGENTS.md` | `.claude/rules` | `.claude/skills` |
| codex | `AGENTS.md`, `AGENTS.override.md` | | `.agents/skills` |
| antigravity | `GEMINI.md`, `AGENTS.md` | | `.agent/skills`, `.agents/skills` |

A project item is a root-relative path ending with one of the union of these
patterns: a file pattern (a file item), an instruction-dir pattern (the whole
folder is one item, `.claude/rules` loads recursively), or a skill-dir
pattern plus one child folder holding `SKILL.md` (a skill item). The longest
matching pattern decides the item's anchor, the project folder
(`proj/.claude/CLAUDE.md`, `proj/.claude/rules` and `proj/.claude/skills/x`
all have anchor `proj`). Adding a tool, or a location such as
`.claude/agents`, is data only. Project items are not per tool, so
`[tools].enabled` does not apply.

### Configuration

```toml
[projects.work]
path = "~/Work/Five-Continents"
exclude = ["_archive", "00_个人事务_me/五洲工作系统_work-system/05_working"]
```

- `<id>` follows the skill name rule; `path` is absolute or starts with `~/`
  (`project add` writes `~/` when under `HOME`). The path is shared by every
  machine (no per-machine override in v1); a machine without it gets one
  warning per root.
- `exclude` (optional) limits discovery only: an entry without `/` matches a
  file or folder of that name at any depth; an entry with `/` is a
  root-relative path and everything under it. Real roots hold backup copies
  (two of the three `fcusa-weekly-po` copies sit in backup folders;
  `~/Developer/.agents-config/agent-setup-migration/backup-pre-import/` holds
  an old `.codex/AGENTS.md`). An item adopted before its path was excluded
  keeps syncing; eject it to stop.
- One function validates a root wherever it is used (`project add`, `adopt`,
  `restore`, `sync`, `doctor`), because `skiman.toml` is edited by hand and
  shared. A root must be an existing folder, and must not be: `HOME`, `/`
  or an ancestor of `HOME`; inside the skill repo (containing it is fine,
  discovery skips it); inside or containing any tool's presence directory;
  overlapping another root (all compared by realpath). A failing root is
  reported once and skipped.
- `project add <path> [--id <id>]` also refuses a taken or invalid id, and
  prints that the root's project files will be stored in the skill repo (and
  pushed wherever the skill repo is pushed).
- `skiman check` checks the syntax (`unknown-key`, `toml-value`).

### Mirror layout and machine state

```
skill repo/
  skills/  upstream/  instructions/shared.md  skiman.toml   (unchanged)
  projects/
    work/                                       [projects.work]
      AGENTS.md
      30_美国业务_FiveC-USA/
        AGENTS.md
        .claude/skills/fcusa-weekly-po/SKILL.md
    dev/
      wec-hist/AGENTS.md
<state dir>/projects-base.toml                  (per machine)
```

`<root>/<rel>` on disk pairs with `projects/<id>/<rel>`: path arithmetic, so
the mirror tree is the only record of what is managed, laid out as the user
sees it on disk, and every project skill is a standard agentskills.io folder.

`projects-base.toml` holds per item the content hash (a folder uses the
existing tree hash) and a stat fingerprint of each side (for a file: size,
modification time in ns, inode; for a folder: the same over every entry,
from `lstat` only). It sits in machine state next to `reported.toml`, so a
re-clone or `git clean` does not lose it. Losing it loses no content: equal
sides re-baseline silently, differing sides show as conflicts.

### Discovery (`adopt`, `doctor`)

Walks each root with `read_dir` and `lstat`, never following a directory
symlink. It never enters `.git`, `node_modules`, a linked git worktree (a
folder whose `.git` is a file pointing into `…/worktrees/`), an `exclude`
entry, the skill repo or skiman's state directory. Item folders (skills,
rules folders) are not entered. A symlink at an item location (an alias such
as `CLAUDE.md -> AGENTS.md`, a link to a global skill) is not a candidate
and is not reported.

### Gates (why `adopt` skips a candidate)

1. **Tracked by a project's git repo.** The repo is found with `git rev-parse
   --show-toplevel` from the candidate's folder (works for `.git` files and
   for roots inside a repo); one `git ls-files` per repo; a folder item with
   any tracked file is skipped. That repo is already the item's source of
   truth. If git fails for a repo, its candidates are skipped with the error.
2. **The rules** on the snapshot as it would sit in the mirror: path safety
   (no symlinks or special files inside a folder item, size limits, Unicode
   collisions, UTF-8 names), the skill rules (frontmatter, name equals folder
   name), conflict markers (skiman's and git's: a line starting with
   `<<<<<<< ` or `>>>>>>> `), the secret rule with the `[[secrets.allow]]`
   hint.

### Sync: the state table

Evaluated per mirror item and per base entry (never by walking the roots),
first matching row wins. `D` = disk, `M` = mirror, `B` = base.

| # | Condition | Sync does | Exit |
|---|---|---|---|
| 1 | The skill repo is mid-operation: `HEAD` detached, or `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`, `rebase-merge/` or `rebase-apply/` in its git dir (read as files, no git spawned) | skip every project item; one notice | 0 |
| 2 | Root fails validation or its folder is absent | skip the root; one warning | 0 |
| 3 | `projects/<id>` has no `[projects.<id>]` | skip (`check` reports it) | 0 |
| 4 | M is not a regular file / folder of the item's kind | report "mirror invalid" | 3 |
| 5 | Anchor absent, B set | report "project gone", once per anchor | 0 |
| 6 | Anchor absent, B none | silent (not on this machine; `doctor` counts) | 0 |
| 7 | A path component between root and item is not a real folder, or D is a link or the wrong kind | report "occupied" | 3 |
| 8 | D absent, M exists, B set | report "deleted in the project" (`restore` or `eject`) | 3 |
| 9 | D absent, M exists, B none | report once per root "N items not on this machine yet" (`restore <root>`) | 0 |
| 10 | D exists, M absent, B set | report "deleted in the mirror" (`adopt <item>` or `eject`) | 3 |
| 11 | D absent, M absent, B set | drop B | 0 |
| 12 | D = M | set B if it differs | 0 |
| 13 | B set, M = B, D ≠ B | copy D to M (backup of old M), B = D; if D fails the rules, report "blocked" with the violations | 0 / 3 |
| 14 | B set, D = B, M ≠ B | report "mirror changed" (`restore <item>` writes it to the project, `adopt <item>` keeps the project's) | 3 |
| 15 | otherwise (both differ from B, or B none and D ≠ M) | report "conflict" (`adopt <item>` or `restore <item>`) | 3 |

Sync writes only the mirror (row 13) and machine state. It never writes a
project folder and never spawns git. `sync --hook` prints only new reports
and always exits 0, as today. Cost at session start: an `lstat` fingerprint
per item; content is read and hashed only for items whose fingerprint
changed, and a file over the size limit is reported "blocked" without being
read.

### Writing: one protocol for every replace

Used by sync row 13, `adopt`, `restore` and `eject`:

1. Read the source into an in-memory snapshot and run the rules on it. Its
   hash becomes the new base; the live source is not re-read.
2. Write the snapshot to a temporary location on the destination's
   filesystem: `.skiman/tmp/` for the mirror, a hidden folder in the anchor
   for the project (`<anchor>/.skiman-restore-<ts>/`, removed at the end).
3. If the destination exists, rename it aside into that same temporary
   location (atomic, keeps the inode so a late write through an open handle
   lands in the set-aside copy), and hash the set-aside copy. If it no longer
   matches what the plan saw, rename it back and report a conflict.
4. Put the new entry in place without replacing anything: a hard link for a
   file (fails if something appeared), `rename` for a folder (fails over a
   file or a non-empty folder).
5. Move the set-aside copy into the backup (`backups/<ts>/repo/projects/…`
   for mirror content, `backups/<ts>/projects/<id>/…` for project content).
   Nothing skiman replaces is deleted.
6. Write the new base.

If the process stops between steps 3 and 4 the original sits in the
temporary location under a known name: the next command that stages (any
write command, including sync's row 13) moves mirror-side leftovers to a
backup, as `.skiman/tmp/` leftovers are handled today; a leftover
`.skiman-restore-*` folder in a project is reported by `sync` and `doctor`
with its path, and the next `restore` there moves it to a backup first.

### Commands

| Command | What it does |
|---|---|
| `skiman project add <path> [--id <id>]` | Register a project root. |
| `skiman project adopt [<path>...] [--dry-run] [--yes]` | Discover under the roots (or `<path>`), print the plan, confirm, copy each new candidate into the mirror and set its base. Named exactly, an item in row 10, 14 or 15 takes the project's version into the mirror. Ends with `sync`. |
| `skiman project restore <path>... [--dry-run] [--yes]` | Write mirror items to the project: under a folder, items missing on disk (rows 8, 9: a new machine, a deleted file); named exactly, also rows 14 and 15. |
| `skiman project eject <path>... [--yes]` | Stop managing the items under `<path>`: mirrors to the backup, base entries dropped, the project untouched. Works when the mirror or the project is already gone. A root's own path also removes `[projects.<id>]`, after every item is out. |

One rule for all three: **a command given a folder never replaces existing
content on either side; replacing needs the item's exact path.** So an
`adopt` an agent runs with `--yes` cannot settle conflicts in bulk.
Conventions as today: `--yes` without a terminal, exit codes 0/1/2/3/4, the
repo lock, staging, backups under machine state.

### Local edits skiman did not see

- **Edited in the project** (an agent, an editor, `sed`): the next sync
  copies it into the mirror; `git diff` in the skill repo shows it; the
  previous mirror content is in a backup.
- **Edited in the mirror, or the mirror reset by git** (`git restore`,
  `git pull` from another machine): row 14, reported; nothing is written to
  the project until the user picks `restore` or `adopt` for that item.
- **Both edited**: conflict; the user picks a side per item, the other side
  goes to a backup.
- **Deleted in the project**: reported; `restore` brings it back, `eject`
  stops managing it.
- **Deleted in the mirror**: reported; `adopt <item>` takes it back in,
  `eject` forgets it.
- **Project folder moved or deleted**: the old path is "project gone" (once
  per project folder); the moved folder holds real files, unmanaged until
  adopted. `eject <old path>` plus `adopt <new path>` in one commit keeps the
  history (`git log --follow` follows identical content across the move).
- **The skill repo mid-merge, mid-rebase, detached for `git checkout <sha>`
  or `git bisect`**: project sync pauses (row 1), so no project content is
  copied into an old checkout and nothing half-merged moves anywhere.
- **Another machine**: sync there copies nothing into the project; `restore
  <root>` writes every item whose project folder exists and whose file is
  missing; differing files show as conflicts, never overwritten.
- **An item later committed to a project's own git repo**: `doctor` reruns
  gate 1 on managed items and reports "now tracked by <repo>; eject it".

### Rules (`skiman check`, pre-commit)

- `project-path` (exit 1): anything under `projects/` not inside a configured
  root id, or not part of a recognised project item. A hand-deleted
  `[projects.<id>]` blocks the commit until `projects/<id>` goes too.
- Every project item: the rules of gate 2.
- The secret rule on every file under `projects/`; `[[secrets.allow]]` paths
  may name `projects/...`.

### Other changes

- `doctor`: per root, whether it is valid, item count, every item not in
  step with its fix, managed items now tracked by a project repo, how many
  candidates are not adopted yet, how many items exist only in the mirror
  (row 6), leftover `.skiman-restore-*` folders, and whether `projects/` has
  uncommitted changes (skiman never commits; mirror changes are committed by
  the user like any other change).
- `managing-skills`: project items are real files, edit them in place; after
  creating a project `AGENTS.md`, rules file or project skill under a root,
  run `skiman project adopt <path>`; edits made in the mirror reach the
  project only through `restore`.
- `README.md`, `GLOSSARY.md`, ADR 0006 (project items: copy mode, one
  automatic direction, git-tracked items stay with their repo).

### Decisions made for the user

- Mirrors live in the skill repo, not a second store: one source of truth,
  checked by the same rules and hooks. The user already keeps work-related
  instructions there (`instructions/legacy-global-rules.md`). Registering a
  root is an explicit act, and `project add` says where content goes.
- The three company skills in `~/Work/Five-Continents/fivec-core/skills/` are
  not in a project skill directory (they reach agents through links in the
  global tool directories), so they are global skills: `skiman adopt <name>
  --from claude-code` brings them in.

## Not in scope

- Writing the project automatically (see Model); a file-watching daemon.
- Three-way merging of conflicts: the base stores hashes only.
- Files tracked by another git repo.
- Files a project item points to (`@` import targets, documents named in
  `AGENTS.md`): only the item itself is mirrored.
- `.claude/agents/` and other non-instruction locations (one data entry each
  if wanted later).
- Detecting moves; `skiman list` output for project items.

## Test plan (`tests/project.rs`, sandbox harness)

1. `project add` and root validation: every refusal, also for a hand-edited
   `skiman.toml`.
2. Discovery: every pattern kind; item folders not entered; dir symlinks not
   followed; `.git`, `node_modules`, linked worktree, `exclude` (name, path),
   skill repo and state dir skipped; alias links ignored.
3. Gates: tracked file, folder with one tracked file, git failure, secret,
   bad frontmatter, symlink inside a skill, git conflict markers.
4. Adopt: new file, rules folder and skill copied byte-identically with base
   set; project untouched; `--dry-run` writes nothing; rerun is a no-op;
   folder adopt never replaces; exact-path adopt of rows 10, 14, 15 with the
   old mirror in a backup.
5. Sync: every row of the state table, including the pause on each git
   state, the fingerprint fast path, an oversized file, hook mode printing
   each report once, and the set-aside check catching a concurrent change.
6. Restore: missing items under a folder; exact-path replace with the old
   project content in a backup; nothing replaced under a folder; leftover
   `.skiman-restore-*` handling.
7. Eject: item, folder, root (table removed), with mirror or project gone.
8. Check: `project-path`, secret, skill rules, conflict markers.
9. Doctor: the project section.

Plus an end-to-end run with the real tools on a throwaway root: adopt, edit
the project file through Claude Code, sync, see the mirror change.

## Acceptance

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (the CI job).
- Council on the plan and on the implementation: every delegate above 9.5
  and approving.
