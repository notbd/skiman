# Project scope: project instruction files and project skills

## Goal

skiman manages global content only: `skills/<name>` linked into every tool
directory, and `instructions/shared.md` linked into every global instruction
file. Most of a real setup is per project: dozens of `AGENTS.md` files under
a work folder such as `~/Work/Acme-Corp`, project skills such as
`<project>/.claude/skills/weekly-report`, company skills kept in
`~/Work/Acme-Corp/acme-core/skills/`, the `AGENTS.md` of code projects
that their own git repo does not track. They have no history and no backup.

The user registers one or more folders (project roots). skiman finds every
project instruction file and project skill under them and keeps a copy of
each in the skill repo (the mirror). The project files stay real files and
stay where they are; the mirror follows them automatically, so the skill repo
holds their history once the user commits it. The project is written only when the user asks, and
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

Two decisions came out of measuring the real tools against two earlier drafts:

1. **Copy, not links.** The first draft replaced each project file with a symlink into
   the skill repo, like global skills. That breaks how these files are edited
   (`docs/project-scope-evidence.md`): Claude Code does not edit a symlinked
   `AGENTS.md` (it treats the target as outside the working directory),
   resolves `@` imports in a symlinked `CLAUDE.md` against the target folder,
   and labels the file with its target path. Global locations are derived
   (nobody edits `~/.claude/skills/x` in place), so links stay right there;
   project folders are where people and agents edit, so they keep real files.
2. **One automatic direction.** The second draft also wrote mirror changes back to the
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
  a `.md` file anywhere under a project-level `.claude/rules/`.
- **Project skill** (项目 skill): a skill folder in a project-level skill
  directory, such as `<project>/.claude/skills/<name>`.
- **Project item**: the unit skiman copies: one project instruction file or
  one project skill folder.
- **Mirror** (镜像): `projects/<root id>/<path relative to the root>` in the
  skill repo: the recorded copy of a project item.
- **Anchor**: the project folder an item belongs to: the item's path without
  its pattern (`proj` for `proj/AGENTS.md` and `proj/.claude/skills/x`).
- **Synced hash** (同步 hash): per machine, the content hash both sides had
  when skiman last saw them equal; it tells which side changed. (Not "base":
  the glossary reserves that word's meaning for the upstream snapshot.)

## Design

### Tool table

Two new columns in `tools.rs`, from the evidence and the tools' docs:

| Tool | `project_instruction_files` | `project_skill_dirs` |
|---|---|---|
| claude-code | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md`, `.claude/rules/**/*.md`, `AGENTS.md` | `.claude/skills` |
| codex | `AGENTS.md`, `AGENTS.override.md` | `.agents/skills` |
| antigravity | `GEMINI.md`, `AGENTS.md` | `.agent/skills`, `.agents/skills` |

Patterns are `/`-separated; `**` matches any number of folders and `*` any
part of one name. A project item is a root-relative path that ends with an
instruction-file pattern (a file item), or a skill-dir pattern plus one child
folder holding `SKILL.md` (a skill item). The earliest position where a
pattern matches decides the item's anchor (`proj/.claude/CLAUDE.md`,
`proj/.claude/rules/sub/x.md` and `proj/.claude/skills/x` all have anchor
`proj`). Adding a tool, or a location such as `.claude/agents`, is data
only. Project items are not per tool, so `[tools].enabled` does not apply.

### Configuration

```toml
[projects.work]
path = "~/Work/Acme-Corp"
exclude = ["_archive", "项目甲/草稿"]
skill_dirs = ["acme-core/skills"]
```

(An example: this change configures nothing on the user's machine.)

- `<id>` follows the skill name rule; `path` is absolute or starts with `~/`
  (`project add` writes `~/` when under `HOME`). The path is shared by every
  machine (no per-machine override in v1); a machine without it gets one
  warning per root.
- `exclude` (optional) limits discovery only: an entry without `/` matches a
  file or folder of that name at any depth; an entry with `/` is a
  root-relative path and everything under it. Real roots hold backup copies
  (copies of a project skill in backup folders, an old `.codex/AGENTS.md`
  kept after a migration). An item adopted before its path was excluded
  keeps syncing; eject it to stop. An exact path given to `adopt` wins over
  `exclude`.
- `skill_dirs` (optional): root-relative folders whose child folders holding
  `SKILL.md` are project skill items too, for skills kept in a folder of
  their own and linked into the tools by other means (such as company
  skills in `acme-core/skills/`, linked by another tool). Same
  item kind, gates and protocol; skiman never touches the tool directories
  for them, and a tool-directory link whose realpath lies under a configured
  `skill_dirs` folder counts as managed elsewhere, so global sync never
  reports it as foreign. Anchor: the configured folder's parent. `check`
  refuses an entry that is absolute, contains `..` or a tool's skill-dir
  pattern anywhere, or lies inside another entry (a skill would have two
  anchors).
- One function validates a root wherever it is used (`project add`, `adopt`,
  `restore`, `sync`, `doctor`; `eject` maps a path to its root lexically,
  after expanding `~`, so a root that is gone can still be ejected), because
  `skiman.toml` is edited by hand and shared. A root must be an existing folder, and must not be: `HOME`, `/`
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
      项目甲/
        AGENTS.md
        .claude/skills/weekly-report/SKILL.md
    dev/
      website/AGENTS.md
<state dir>/projects-synced.toml                (per machine)
```

`<root>/<rel>` on disk pairs with `projects/<id>/<rel>`: path arithmetic, so
the mirror tree is the only record of what is managed, laid out exactly as
the user sees it on disk.

`projects-synced.toml` holds, per root id, the root's realpath and one synced
hash per item (a folder uses the existing tree hash). It sits in machine
state next to `reported.toml`, so a re-clone or `git clean` does not lose it.
An entry whose recorded realpath differs from the root's current one (the
`path` changed on another machine), or whose root id is no longer
configured, counts as absent and is dropped. Losing the file loses no
content: equal sides are recorded again silently, differing sides show as
conflicts, and items missing on disk show as row 9.

### Discovery (`adopt`, `doctor`)

Walks each root with `read_dir` and `lstat`, never following a directory
symlink. It never enters `.git`, `node_modules`, a linked git worktree (a
folder whose `.git` is a file pointing into `…/worktrees/`), an `exclude`
entry, a `.skiman-swap-*` folder, the skill repo or skiman's state
directory. Skill folders are not entered. A symlink at an item location (an alias such
as `CLAUDE.md -> AGENTS.md`, a link to a global skill) is not a candidate
and is not reported.

### Gates (why `adopt` skips a candidate)

1. **Tracked by a project's git repo.** The repo is found with `git rev-parse
   --show-toplevel` from the candidate's folder (works for `.git` files and
   for roots inside a repo); one `git ls-files` per repo; a folder item with
   any tracked file is skipped. That repo is already the item's source of
   truth. If git fails for a repo, its candidates are skipped with the error.
2. **The safety rules** on the snapshot as it would sit in the mirror: the
   existing content rules (`SKILL.md` present at the top of a skill, no
   symlinks or special files inside it, file size, file count and skill size
   limits), path safety (Unicode collisions, UTF-8 names), conflict markers
   (skiman's and git's: a line starting with `<<<<<<< ` or `>>>>>>> `), and
   the secret rule with the `[[secrets.allow]]` hint. The skill frontmatter
   rules and the skill name rule do not apply: the mirror records what the
   tools load, and Claude Code loads a skill without `name` or
   `description`, in a folder of any name.

### Sync: the state table

Evaluated per mirror item and per synced-hash entry, never by walking the
roots.

| # | Condition | Sync does | Exit |
|---|---|---|---|
| 1 | The skill repo is mid-operation: `HEAD` detached, or `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`, `rebase-merge/` or `rebase-apply/` in its git dir (read as files, no git spawned; a `.git` file is followed to the worktree's git dir) | skip every project item; one notice | 0 |
| 2 | Root fails validation or its folder is absent | skip the root; one warning | 0 |
| 3 | `projects/<id>` has no `[projects.<id>]`, or a mirror path has no item shape (a `skill_dirs` entry removed, say; shape only: a skill folder that lost its `SKILL.md` still has the shape and falls to row 5) | skip, drop its S (`check` reports the path) | 0 |
| 4 | M absent, S set, D is not a regular item of its kind | drop S | 0 |
| 5 | M exists but is not a regular file / folder of the item's kind, or is a skill folder without `SKILL.md` | report "mirror invalid" | 3 |
| 6 | Anchor absent, S set | report "project gone", once per anchor | 0 |
| 7 | Anchor absent, S none | silent (not on this machine; `doctor` counts) | 0 |
| 8 | A path component between root and item exists but is not a real folder, or D exists but is a link or the wrong kind | report "occupied" | 3 |
| 9 | D absent, M exists, S none | report once per root "N items not on this machine yet" with the paste-ready `skiman project restore …` | 0 |
| 10 | D absent, M exists, S set | report "deleted in the project" (`restore` or `eject`) | 3 |
| 11 | D exists, M absent, S set | report "deleted in the mirror" (`adopt <item>` or `eject`) | 3 |
| 12 | D = M | record S if it differs | 0 |
| 13 | S set, M = S, D ≠ S | copy D to M (old M to a backup), S = D; if D fails the safety rules (a skill whose `SKILL.md` was renamed away, a secret), report "blocked" with the violations and keep M | 0 / 3 |
| 14 | S set, D = S, M ≠ S | report "mirror changed" (`restore <item>` writes it to the project, `adopt <item>` keeps the project's) | 3 |
| 15 | otherwise (both differ from S, or S none and D ≠ M) | report "conflict" (`adopt <item>` or `restore <item>`) | 3 |

`D` = disk, `M` = mirror, `S` = synced hash; first matching row wins. A
missing intermediate folder (such as `.claude/skills` under an existing
anchor) means D is absent, not occupied. Rows 10, 11, 14 and 15 end with one
paste-ready command line naming every affected item.

Sync writes only the mirror (row 13) and machine state. It never writes a
project folder and never spawns git. `sync --hook` prints only new reports
and always exits 0, as today; nothing in the project part prints outside
that mechanism. Cost at session start: both sides of every item
are read and hashed (measured on the user's two roots: 100 files, 835 KB,
3.5 ms of hashing); a file over the size limit is reported "blocked" from
its `lstat` size without being read.

`adopt`, `restore` and `eject` refuse with exit 4 while row 1 holds, naming
the git state, so no command writes into an old checkout or a half-merged
tree.

### Writing: one protocol for every replace

Used by sync row 13, `adopt` and `restore` (`eject` only moves the mirror to a
backup and drops the synced hash). Every temporary entry lives in a swap
folder, one per replaced item, on the destination's filesystem:
`<repo>/.skiman/swap/<ts>/` for the mirror, `<anchor>/.skiman-swap-<ts>/` for
the project (created with a `.gitignore` holding `*`, so a project repo never
picks it up). Inside it, `new/` holds the snapshot (step 2), `old/` the
set-aside copy (step 3), and `target` the destination path.

1. Read the source into an in-memory snapshot and run the safety rules on it.
   Its hash becomes the new synced hash; the live source is not re-read.
2. Write the snapshot into the swap folder.
3. If the destination exists, rename it into the swap folder (same
   filesystem, so it keeps its inode and a late write through an open handle
   lands in it) and hash it. If it no longer matches what the plan saw, put
   it back without replacing anything, and report a conflict.
4. Put the new entry in place without replacing anything: a hard link for a
   file (fails if something appeared; a filesystem without hard links fails
   here too), `rename` for a folder (fails over a file or a non-empty
   folder).
5. Rename the set-aside copy into the backup (`backups/<ts>/repo/projects/…`
   for mirror content, `backups/<ts>/projects/<id>/…` for project content),
   then hash it again: a different hash means a late write landed there, and
   is reported with the backup path. Nothing skiman replaces is deleted.
6. Write the new synced hash; remove the swap folder.

A set-aside copy is only ever renamed, never copied and deleted, for its
whole life: copying and deleting it would lose a late write through an open
handle. Failure handling, so an error never deletes the only copy:

- "Put back" (step 3) and every undo use the same no-replace placement as
  step 4. If the original path is taken meanwhile, both versions stay where
  they are and the command reports both paths.
- A failure in step 4 puts the set-aside copy back and exits 4.
- A failure in step 5, including a backup folder on another filesystem,
  leaves the copy in the swap folder, keeps the folder, and reports the path.
- A swap folder that outlives its command (a failure above, a killed
  process) is never cleaned up automatically: `doctor` lists it with what it
  holds, and the next `restore` in that project (or the next mirror write,
  for `.skiman/swap/`) renames what is in `old/` into a backup first, or
  leaves it and reports it when that rename crosses filesystems. `new/` is
  deleted, never backed up: it is a copy of a source that still exists, or a
  second link to an entry already in place.

### Commands

| Command | What it does |
|---|---|
| `skiman project add <path> [--id <id>]` | Register a project root. |
| `skiman project adopt [<path>...] [--dry-run] [--yes]` | Discover under the roots (or `<path>`), print the plan, confirm, copy each new candidate into the mirror and record its synced hash. Named exactly, an item in row 11, 14 or 15 takes the project's version into the mirror. Warns when git ignores a new mirror path (`git check-ignore -v`, naming the rule), since an ignored mirror never gets history. Ends with `sync`. |
| `skiman project restore <path>... [--dry-run] [--yes]` | Write mirror items to the project: under a folder, items this machine has never had (row 9); named exactly, also rows 10, 13 (blocked: the mirror holds the last good version), 14 and 15. Reruns gate 1 first: a path a project repo now tracks is not written. |
| `skiman project eject <path>... [--yes]` | Stop managing the items under `<path>`: mirrors to the backup, synced hashes dropped, the project untouched. Works when the mirror or the project is already gone. A root's own path also removes `[projects.<id>]`, after every item is out. |

One rule for all three: **a command given a folder never replaces existing
content or undoes a local deletion on either side; that needs the item's
exact path.** So an
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
- **Deleted in the project**: reported; `restore <item>` brings it back,
  `eject` stops managing it.
- **Deleted in the mirror**: reported; `adopt <item>` takes it back in,
  `eject` forgets it.
- **A new item created in the project**: sync does not walk the roots, so
  it stays unmanaged until `adopt`; `managing-skills` tells agents to adopt
  what they create, and `doctor` counts candidates not adopted yet.
- **Project folder moved or deleted**: the old path is "project gone" (once
  per anchor); the moved folder holds real files, unmanaged until
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
  root id, or not part of a recognised project item. The message names both
  fixes: restore the configuration, or `git rm -r` the path. A hand-deleted
  `[projects.<id>]` blocks the commit until `projects/<id>` goes too.
- Every project item: the rules of gate 2.
- The secret rule on every file under `projects/`; `[[secrets.allow]]` paths
  may name `projects/...`.

### Other changes

- `doctor`: per root, whether it is valid, item count, every item not in
  step with its fix, managed items now tracked by a project repo, how many
  candidates are not adopted yet, how many items exist only in the mirror
  (row 7), leftover swap folders, mirror paths git ignores, and whether
  `projects/` has uncommitted changes (skiman never commits; mirror changes
  are committed by the user like any other change).
- Ownership: a tool-directory link whose realpath is a managed project skill
  item (from `skill_dirs`) is managed elsewhere, not foreign.
- `managing-skills`: project items are real files, edit them in place; after
  creating a project `AGENTS.md`, rules file or project skill under a root,
  run `skiman project adopt <path>`; edits made in the mirror reach the
  project only through `restore`; compare the two sides with `diff` or
  `git diff`, not by opening mirror files (Claude Code loads the `AGENTS.md`,
  rules and skills of a folder it reads a file in, so reading the mirror
  pulls that project's instructions into the session).
- `README.md`, `GLOSSARY.md`, ADR 0006 (project items: copy mode, one
  automatic direction, git-tracked items stay with their repo).

### Decisions made for the user

- Mirrors live in the skill repo, not a second store: one source of truth,
  checked by the same rules and hooks. The user already keeps work-related
  instructions there. Registering a root is an explicit act, and
  `project add` says where content goes.
- Company skills kept in a folder of their own and linked into the tools by
  another tool can stay outside skiman by a rule the user keeps in that
  folder. `skill_dirs` makes mirroring them possible without touching their
  links; whether to use it is the user's call (it means changing that rule),
  so this change does not configure it or edit that rule.

## Not in scope

- Writing the project automatically (see Model); a file-watching daemon.
- Three-way merging of conflicts: the synced hash is a hash only.
- Files tracked by another git repo.
- Files a project item points to (`@` import targets, documents named in
  `AGENTS.md`): only the item itself is mirrored.
- `.claude/agents/` and other non-instruction locations (one data entry each
  if wanted later).
- Detecting moves; `skiman list` output for project items.
- Pruning `backups/` (none exists today; row 13 adds a small backup per
  change).
- Claude Code loading a mirror folder's instructions when a session reads a
  file there (measured; avoided by not opening mirror files, see
  `managing-skills`).

## Test plan (`tests/project_*.rs`, sandbox harness)

1. `project add` and root validation: every refusal, also for a hand-edited
   `skiman.toml`.
2. Discovery: every pattern (including `.claude/rules/**/*.md`),
   `skill_dirs`; skill folders not entered; dir symlinks not followed; `.git`, `node_modules`, linked worktree, `exclude` (name, path),
   skill repo and state dir skipped; alias links ignored.
3. Gates: tracked file, skill with one tracked file, git failure, secret,
   symlink inside a skill, missing `SKILL.md`, git conflict markers; a skill
   without frontmatter `name` is adopted.
4. Adopt: new file, rules file and skill copied byte-identically with the
   synced hash recorded; project untouched; `--dry-run` writes nothing; rerun is a no-op;
   folder adopt never replaces; exact-path adopt of rows 11, 14, 15 with the
   old mirror in a backup.
5. Sync: every row of the state table, including the pause on each git
   state, an oversized file, a `path` changed under recorded synced hashes,
   hook mode printing each report once, the set-aside check catching a
   concurrent change, a failing backup step keeping the set-aside copy, and
   a tool-directory link into a `skill_dirs` item not reported as foreign.
6. Restore: missing items under a folder; exact-path replace with the old
   project content in a backup; nothing replaced under a folder; a path the
   project repo tracks is not written; a failing step 4 or 5 keeps the
   original; leftover swap folder handling; a folder restore does not undo a
   deletion; every project command refuses during a skill repo merge.
7. Eject: item, folder, root (table removed), with mirror or project gone.
8. Check: `project-path`, `skill_dirs` values, the gate 2 rules (a skill
   without frontmatter `name` passes).
9. Doctor: the project section.

Plus an end-to-end run with the real tools on a throwaway root: adopt, edit
the project file through Claude Code, sync, see the mirror change.

## Acceptance

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (the CI job).
