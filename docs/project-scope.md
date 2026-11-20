# Project scope: project instruction files and project skills

## Goal

skiman manages global content only: `skills/<name>` linked into every tool
directory, and `instructions/shared.md` linked into every global instruction
file. Most of a real setup is per project: dozens of `AGENTS.md` files under
a work folder such as `~/Work/Acme-Corp`, project skills such as
`<project>/.claude/skills/weekly-report`, company skills kept in
`~/Work/Acme-Corp/acme-core/skills/`, the `AGENTS.md` of code projects
that their own git repo does not track. They have no history and no backup.

You register one or more folders (project roots). skiman finds every
project instruction file and project skill under them and keeps a copy of
each in the canon (the mirror). The project files stay real files and
stay where they are; the mirror follows them automatically, so the canon
holds their history once you commit it. The project is written only when
you ask, and whatever skiman replaces on either side goes to a backup first.

| Requirement | Where |
|---|---|
| Register one or more folders | `skiman project add`, `[projects.<id>]` |
| Recursive, exhaustive scan | Discovery |
| Lay the results out elegantly in the canon | Mirror layout |
| Copy into the canon, keep the original working | Copy mode, `adopt` |
| Local edits skiman did not see | Sync and the state table |
| New providers, models, harnesses are easy to add | Tool table |
| Easy, lossless migration to another tool | Project files stay real files; plain mirror |

## Model: the mirror follows the project

Two decisions came out of measuring the real tools against two earlier drafts:

1. **Copy, not links.** The first draft replaced each project file with a symlink into
   the canon, like global skills. That breaks how these files are edited
   (`docs/project-scope-evidence.md`): Claude Code does not edit a symlinked
   `AGENTS.md` (it treats the target as outside the working directory),
   resolves `@` imports in a symlinked `CLAUDE.md` against the target folder,
   and labels the file with its target path. Global locations are derived
   (nobody edits `~/.claude/skills/x` in place), so links stay right there;
   project folders are where people and agents edit, so they keep real files.
2. **One automatic direction.** The second draft also wrote mirror changes back to the
   project at every sync. A routine `git restore .` in the canon then
   overwrote project content that existed nowhere else, and a conflicted
   `git pull` could write conflict markers into a project. Now sync only
   copies project → mirror; mirror → project happens only through
   `skiman project restore`, which you run and which backs up what it
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
  canon: the recorded copy of a project item.
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
```

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
- `skill_dirs` is retired (ADR 0013): no command reads it, and `check`
  reports the key as one `toml-value` violation that names
  `skiman linked add` as the replacement. Mirror items it made have no item
  shape any more (sync row 3); deleting `projects/<id>/<folder>` removes
  them. A tool-directory link into such a folder is foreign like any other.
- One function validates a root wherever it is used (`project add`, `adopt`,
  `restore`, `sync`, `doctor`; `eject` maps a path to its root lexically,
  after expanding `~`, so a root that is gone can still be ejected), because
  `skiman.toml` is edited by hand and shared. A root must be an existing folder, and must not be: `HOME`, `/`
  or an ancestor of `HOME`; inside the canon (containing it is fine,
  discovery skips it); inside or containing any tool's presence directory;
  overlapping another root (all compared by realpath). A failing root is
  reported once and skipped.
- `project add <path> [--id <id>]` also refuses a taken or invalid id, and
  prints that the root's project files will be stored in the canon (and
  pushed wherever the canon is pushed).
- `skiman check` checks the syntax (`unknown-key`, `toml-value`).

### Mirror layout and machine state

```
canon/
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
you see it on disk.

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
folder whose `.git` is a file pointing into `…/worktrees/`), a cache folder
(one holding a `CACHEDIR.TAG` that starts with the signature of the [Cache
Directory Tagging Specification](https://bford.info/cachedir/), such as Rust's
`target/`), an `exclude` entry, a `.skiman-swap-*` folder, the canon or
skiman's state directory. Skill folders are not entered. A symlink at an item location (an alias such
as `CLAUDE.md -> AGENTS.md`, a link to a global skill) is not a candidate
and is not reported. `adopt <folder>` walks from that folder only; each
folder on the way to it must be one the whole-root walk enters, under the
name it has on disk: a folder given in another spelling (another case, on a
volume that ignores case) is not entered.

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
| 1 | The canon is mid-operation: `HEAD` detached, or `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD`, `BISECT_LOG`, `rebase-merge/` or `rebase-apply/` in its git dir (read as files, no git spawned; a `.git` file is followed to the worktree's git dir) | skip every project item; one notice | 0 |
| 2 | Root fails validation or its folder is absent | skip the root; one warning | 0 |
| 3 | `projects/<id>` has no `[projects.<id>]`, or a mirror path has no item shape (one copied from a folder a retired `skill_dirs` key named, say; shape only: a skill folder that lost its `SKILL.md` still has the shape and falls to row 5) | skip, drop its S (`check` reports the path) | 0 |
| 4 | M absent, S set, D is not a regular item of its kind | nothing; keep S, so a side that comes back is judged against it (an item created in the project later is row 11, one back in the mirror is row 10; `eject` drops S) | 0 |
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
are read and hashed (measured on two real roots: 100 files, 835 KB,
3.5 ms of hashing); a file over the size limit is reported "blocked" from
its `lstat` size without being read.

`adopt`, `restore` and `eject` refuse with exit 4 while row 1 holds, naming
the git state, so no command writes into an old checkout or a half-merged
tree.

### Writing: one protocol for every replace

Used by sync row 13, `adopt` and `restore` (`eject` only moves the mirror to a
backup and drops the synced hash), and by the write commands of the
canon (`src/staging.rs`). Every temporary entry lives in a staging folder on
the destination's filesystem, one per replace: `<repo>/.skiman/swap/<ts>/`
for the mirror, `<anchor>/.skiman-swap-<ts>/` for the project (holding a
`.gitignore` of `*` while it exists, so a project repo never picks it up).
The new entry is staged at `old/<name>` in it, `<name>` being the item's
file or folder name: a staging folder serves one replace.

1. Read the source into an in-memory snapshot and run the safety rules on it.
   Its hash becomes the new synced hash; the live source is not re-read.
2. Open the staging folder: the tmp leftovers of that place go to the backup
   first (below). Compare the destination with the plan: one that appeared,
   disappeared or no longer hashes as the plan saw is a conflict and nothing
   moves. Write the snapshot into the staging folder. For a file, also place
   it under a probe name and back, as step 3 places it: on a filesystem with
   neither a rename that never replaces nor hard links (exFAT) this probe
   fails, and the destination is left untouched.
3. Where nothing is, and the plan saw nothing, put the new entry in place
   without replacing anything: a rename that never replaces
   (`RENAME_NOREPLACE`); where the filesystem has none, a hard link for a
   file, and for a folder a `rename` once the name is found free (`rename`
   never replaces a file or a non-empty folder).
4. Where the destination exists, the new entry and the destination swap
   places in one rename (`RENAME_EXCHANGE`, `RENAME_SWAP`), so the path is
   never empty. What came out is in the staging folder (same filesystem, so
   it keeps its inode and a late write through an open handle lands in it)
   and is hashed: if it no longer matches what the plan saw, the two swap
   back and a conflict is reported. Where the filesystem has no such rename
   (HFS+, exFAT, NFS), the destination is first renamed to
   `old/<name>.skiman-prev` and hashed there; changed, it is put back without
   replacing anything, else the new entry is renamed in. The path is empty
   in between. Whatever a swap back or a put back leaves in the staging
   folder (the new entry, or what took its place) is never removed: the
   conflict says where it is. A folder that cannot be renamed into the
   staging folder (on Linux, one from a lower overlayfs layer) stops the
   replace before anything moved.
5. Move what came out into the backup (`backups/<ts>/repo/projects/…` for
   mirror content, `backups/<ts>/projects/<id>/…` for project content): a
   rename, or on another filesystem a copy compared entry by entry, then
   the set-aside copy deleted. Then fingerprint it again: a different
   fingerprint means a late write landed there, and is reported with the
   backup path. Nothing skiman replaces is lost.
6. Write the new synced hash; remove the staging folder.

A set-aside copy is only renamed while it is in the staging folder: copying
and deleting it would lose a late write through an open handle. Only its
move into a backup on another filesystem copies and deletes it (step 5),
and a write after the comparison is lost (ADR 0005). Failure handling, so
an error never deletes the only copy:

- Swapping back is a second swap of the same two: what came out goes back,
  and whatever was at the path then (the new entry, or what took its place)
  comes out into the staging folder and stays there. Putting back, without
  a swapping rename, uses the same no-replace placement as step 3: if the
  original path is taken meanwhile, both versions stay where they are and
  the command reports both paths.
- A second rename in step 4 that fails leaves both versions in the staging
  folder, exits 4 and says where each is.
- A failure in step 5 keeps the staging folder, exits 4 and reports where
  the set-aside copy is: in the staging folder, or, when a move to another
  filesystem fails after its copy is complete, in the backup, with what is
  left of it to delete.
- A staging folder that outlives its command (a failure above, a swap back, a
  killed process) is a tmp leftover. `doctor` lists it, and the next command
  that replaces something in that place (`restore`, or for `.skiman/swap/` any
  mirror write), once per command, first finishes a move into a backup cut short
  inside it (what is left is deleted; the complete copy stays where that move
  made it), then moves it whole to the backup
  (`backups/<ts>/projects/<id>/<anchor>/`, `backups/<ts>/repo/.skiman/swap/`);
  empty folders and a `.gitignore` alone are dropped. One that cannot be moved
  stays where it is, with those not reached yet; the command says where the
  moved ones went and which one stopped it, and the replace goes on. Nothing in a
  leftover is put back: a replace killed between the two renames of step 4
  leaves the path empty. `adopt` or `restore` run again writes the item from its
  source, as it is then, like one that never ran; `sync` reports the empty side
  instead: row 11 after a killed `adopt` or `sync` (the mirror is empty), row 10
  after a killed `restore` (the project is).

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
locks, staging, backups under machine state.

### Local edits skiman did not see

- **Edited in the project** (an agent, an editor, `sed`): the next sync
  copies it into the mirror; `git diff` in the canon shows it; the
  previous mirror content is in a backup.
- **Edited in the mirror, or the mirror reset by git** (`git restore`,
  `git pull` from another machine): row 14, reported; nothing is written to
  the project until you pick `restore` or `adopt` for that item.
- **Both edited**: conflict; you pick a side per item, the other side
  goes to a backup.
- **Deleted in the project**: reported; `restore <item>` brings it back,
  `eject` stops managing it.
- **Deleted in the mirror**: reported; `adopt <item>` takes it back in,
  `eject` forgets it.
- **A new item created in the project**: sync does not walk the roots, so
  it stays unmanaged until `adopt`; `skiman-manual` tells agents to adopt
  what they create, and `doctor` counts candidates not adopted yet.
- **Project folder moved or deleted**: the old path is "project gone" (once
  per anchor); the moved folder holds real files, unmanaged until
  adopted. `eject <old path>` plus `adopt <new path>` in one commit keeps the
  history (`git log --follow` follows identical content across the move).
- **The canon mid-merge, mid-rebase, detached for `git checkout <sha>`
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
  (row 7), leftover staging folders, mirror paths git ignores, and whether
  `projects/` has uncommitted changes (skiman never commits; you commit
  mirror changes like any other change). While a git operation
  in the canon pauses sync, doctor still reports each item.
- `skiman-manual`: project items are real files, edit them in place; after
  creating a project `AGENTS.md`, rules file or project skill under a root,
  run `skiman project adopt <path>`; edits made in the mirror reach the
  project only through `restore`; compare the two sides with `diff` or
  `git diff`, not by opening mirror files (Claude Code loads the `AGENTS.md`,
  rules and skills of a folder it reads a file in, so reading the mirror
  pulls that project's instructions into the session).
- `README.md`, `GLOSSARY.md`, ADR 0006 (project items: copy mode, one
  automatic direction, git-tracked items stay with their repo).

### Decisions made for the user

- Mirrors live in the canon, not a second store: one source of truth,
  checked by the same rules and hooks. The user already keeps work-related
  instructions there. Registering a root is an explicit act, and
  `project add` says where content goes.
- Company skills kept in a folder of their own are not project items: they
  are registered as a linked folder (ADR 0010). `skill_dirs`, which mirrored
  them, is retired (ADR 0013).

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
  `skiman-manual`).

## Test plan (`tests/project_*.rs`, sandbox harness)

1. `project add` and root validation: every refusal, also for a hand-edited
   `skiman.toml`.
2. Discovery: every pattern (including `.claude/rules/**/*.md`); skill folders not entered; dir symlinks not followed; `.git`, `node_modules`, linked worktree, a `CACHEDIR.TAG` cache, `exclude` (name, path),
   canon and state dir skipped; alias links ignored.
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
   a tool-directory link into a former `skill_dirs` folder reported as foreign.
6. Restore: missing items under a folder; exact-path replace with the old
   project content in a backup; nothing replaced under a folder; a path the
   project repo tracks is not written; a failing step 4 or 5 keeps the
   original; leftover staging folder handling; a folder restore does not undo a
   deletion; every project command refuses during a canon merge.
7. Eject: item, folder, root (table removed), with mirror or project gone.
8. Check: `project-path`, a retired `skill_dirs` key, the gate 2 rules (a skill
   without frontmatter `name` passes).
9. Doctor: the project section.

Plus an end-to-end run with the real tools on a throwaway root: adopt, edit
the project file through Claude Code, sync, see the mirror change.

## Acceptance

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (the CI job).
