# Project scope: project instruction files and project skills

Status: plan, council round 2. Branch `feat/local-scope`.

## Goal

skiman manages global content only: `skills/<name>` linked into every tool
directory, and `instructions/shared.md` linked into every global instruction
file. Most of a real setup is per project: 70+ `AGENTS.md` files under
`~/Work/Five-Continents`, project skills such as
`<project>/.claude/skills/fcusa-weekly-po`, a `CLAUDE.md` per code project.
They have no history and no backup.

The user registers one or more folders (project roots). skiman finds every
project instruction file and project skill under them, keeps a copy of each
in the skill repo (the mirror), and keeps the two in step: an edit on either
side reaches the other at the next sync, a conflicting edit is reported and
never overwritten, and nothing is ever lost (whatever is replaced goes to a
backup or is already in git).

| Requirement | Where |
|---|---|
| Register one or more folders | `skiman project add`, `[projects.<id>]` |
| Recursive, exhaustive scan | Discovery |
| Lay the results out elegantly in the skill repo | Mirror layout |
| Copy into the skill repo, keep the original working | Copy mode, `adopt` |
| Local edits skiman did not see | Two-way sync and the state table |
| New providers, models, harnesses are easy to add | Tool table |
| Easy, lossless migration to another tool | Mirror layout, project files stay real files |

## Why copy mode, not links (changed after council round 1)

Round 1 proposed replacing each project file with a symlink into the skill
repo, like global skills. Measured on this machine, that breaks the main way
these files are edited:

- Claude Code 2.1.311 asked to edit a symlinked `proj/AGENTS.md`: it treats
  the link target as a file outside the working directory and does not edit
  it (reproduced 2026-11-06, `claude -p --permission-mode acceptEdits`; the
  council reported the explicit message "Refusing to write …: it is a
  symbolic link. Write to the link's target path instead").
- Claude Code resolves `@` imports in a symlinked `CLAUDE.md` against the
  link target's folder (evidence below), so relative imports break.
- Claude Code labels a symlinked instruction file with its target path in
  the session context (seen for `~/.claude/rules/legacy-global-rules.md`), so
  relative paths written in a project `AGENTS.md` read as relative to the
  mirror.

Global locations are derived (nobody edits `~/.claude/skills/x` in place), so
links stay right there. Project folders are where people and agents edit in
place, so the project keeps a real file and the skill repo keeps a copy.
The analogy is git itself: the mirror is the repository, the project file is
the working copy, and sync is a checkout plus an automatic commit to the
working tree, with a conflict instead of a silent overwrite.

What copy mode removes from round 1: every symlink in a project, the
"untracked but not ignored" git gate, the `@` import gate, the states
misdirected / moved-by-link / alias / dangling, backups of originals on
adopt, restoring on eject, and the doctor check for links in git work trees.
Projects never depend on skiman: uninstall it and every project still works.

## Evidence

Run on 2026-11-06 in a throwaway folder with Claude Code 2.1.311, codex-cli
0.166.0 and agy 1.5.0; summarised in `docs/project-scope-evidence.md`. Each
file and folder held a random token; the subject was asked to quote the
tokens it had loaded.

| Location in the project | Claude Code | Codex | Antigravity |
|---|---|---|---|
| `CLAUDE.md` | loaded | no | no |
| `AGENTS.md` | loaded | loaded | loaded |
| `GEMINI.md` | no | no | loaded |
| `.claude/skills/<n>/` | loaded | no | no |
| `.agents/skills/<n>/` | no | loaded | loaded |
| `.agent/skills/<n>/` | no | no | loaded |
| `.gemini/skills/<n>/` | no | no | no |

## Terms (added to GLOSSARY.md)

"Local" is avoided on purpose: the glossary already lists "local skill" as a
term to avoid, and Claude Code uses "local" for personal uncommitted files
(`CLAUDE.local.md`).

- **Project root** (项目根目录): a folder registered as `[projects.<id>]`;
  skiman discovers project items under it.
- **Project instruction file** (项目 instruction file): a file an agent tool
  loads when it works inside a project folder, such as `AGENTS.md` there.
- **Project skill** (项目 skill): a skill folder in a project-level skill
  directory, such as `<project>/.claude/skills/<name>`.
- **Project item**: a project instruction file or a project skill; the unit
  skiman copies and syncs.
- **Mirror** (镜像): `projects/<root id>/<path relative to the root>` in the
  skill repo: the canonical copy of a project item.
- **Base**: per machine, the content hash both sides had at the last sync that
  left them equal; it tells which side changed.

## Design

### Tool table

Two new columns in `tools.rs`, from the evidence and the tools' documented
names:

| Tool | `project_instruction_files` | `project_skill_dirs` |
|---|---|---|
| claude-code | `CLAUDE.md`, `CLAUDE.local.md`, `.claude/CLAUDE.md`, `AGENTS.md` | `.claude/skills` |
| codex | `AGENTS.md`, `AGENTS.override.md` | `.agents/skills` |
| antigravity | `GEMINI.md`, `AGENTS.md` | `.agent/skills`, `.agents/skills` |

A path is a project item when its root-relative path ends with one of the
union of these patterns; the longest matching pattern decides its anchor (the
project folder: `proj/.claude/CLAUDE.md` and `proj/.claude/skills/x` both
have anchor `proj`). Adding a tool is one row. Project items are not per
tool, so `[tools].enabled` does not apply to them.

### Configuration

```toml
[projects.work]
path = "~/Work/Five-Continents"
exclude = ["_archive", "00_个人事务_me/五洲工作系统_work-system/05_working"]
```

- `<id>` follows the skill name rule. `path` is absolute or starts with `~/`
  (`project add` writes `~/` when under `HOME`).
- `exclude` (optional) limits discovery: an entry without `/` matches a file
  or folder with that name at any depth; an entry with `/` is a root-relative
  path and everything under it. Real roots hold backup copies of skills
  (three copies of `fcusa-weekly-po` under `~/Work/Five-Continents`, two of
  them in backup folders), so this is needed.
- `skiman project add <path> [--id <id>]` writes the table. It refuses a path
  that is not an existing folder, `HOME`, `/` or any ancestor of `HOME`, a
  folder inside the skill repo, a folder that contains or is inside another
  root (by realpath), and a taken or invalid id. It prints that the root's
  project files will be stored in the skill repo (and pushed wherever the
  skill repo is pushed).
- `skiman check` checks syntax: `unknown-key`, `toml-value`.

### Mirror layout

```
skill repo/
  skills/  upstream/  instructions/shared.md  skiman.toml    (unchanged)
  projects/
    work/                                        [projects.work]
      AGENTS.md
      30_美国业务_FiveC-USA/
        AGENTS.md
        .claude/skills/fcusa-weekly-po/SKILL.md
    dev/
      wec-hist/AGENTS.md
  .skiman/projects-base.toml                     (git-ignored, per machine)
```

`<root>/<rel>` on disk pairs with `projects/<id>/<rel>`. The pairing is path
arithmetic, so the mirror tree is the only record of what is managed. It is
the layout the user sees on disk, readable by any person, LLM or tool, and
every project skill is a standard agentskills.io folder.

`projects-base.toml` holds one hash per item (`[<id>]`, `"<rel>" =
"<sha256>"`; a skill folder uses the existing tree hash). It is a sync cache,
not a journal: losing it costs no data, only a conflict report for items
whose two sides differ (equal sides re-baseline silently).

### Discovery (`adopt`, `doctor`)

Walks each root with `read_dir` and `lstat`, never following a directory
symlink. It never enters: `.git`, `node_modules`, a linked git worktree (a
folder whose `.git` is a file pointing into `…/worktrees/`), an `exclude`
entry, the skill repo, or skiman's state directory. In a skill-dir folder,
each child folder holding `SKILL.md` is a skill candidate and is not entered.
A symlink at an item location is not a candidate (an alias such as `CLAUDE.md
-> AGENTS.md`, or a link to a global skill): left alone, never reported.

### Gates (why `adopt` skips a candidate)

1. **Tracked by a project's git repo.** The containing repo is found with
   `git rev-parse --show-toplevel` from the candidate's folder (works for
   `.git` files and roots inside a repo), then one `git ls-files` per repo.
   A skill with any tracked file is skipped. That repo is already the item's
   source of truth. If git fails for a repo, its candidates are skipped with
   the error, never adopted.
2. **The everyday rules** on the copy as it would sit in the mirror: path
   safety (no symlinks or special files inside a skill, size limits, Unicode
   collisions, UTF-8 names), the skill rules (frontmatter, name equals folder
   name), the secret rule with the ready-to-paste `[[secrets.allow]]` hint.

### Sync: the state table

`D` = content on disk, `M` = mirror content, `B` = base, "anchor" = the
project folder. Evaluated per mirror item and per base entry, never by
walking the roots, so sync stays fast enough for session start: it reads and
hashes about 2 × (number of items) small files, and stops at skill folders.

| Disk | Mirror | Base | Sync does | Exit |
|---|---|---|---|---|
| = B | = B | set | nothing | 0 |
| ≠ B | = B | set | copy disk to mirror (if it passes the rules), base = disk | 0 |
| = B | ≠ B | set | write mirror to disk, base = mirror | 0 |
| = M | = D | any | base = both | 0 |
| ≠ M, both ≠ B | | set | **conflict**, report | 3 |
| ≠ M | | none | **conflict**, report | 3 |
| absent, anchor exists | exists | none | write mirror to disk (a new machine, a fresh mirror item) | 0 |
| absent, anchor exists | exists | set | **deleted on disk**, report | 3 |
| exists | absent | set | **deleted in the mirror**, report | 3 |
| absent | absent | set | drop the base entry | 0 |
| anchor absent | exists | any | project gone, warn | 0 |
| root folder absent | | | one warning per root | 0 |
| a link or other kind, or a symlink on the path between root and item | exists | | **occupied**, report | 3 |
| disk change fails the rules | | | **blocked**, report with the violations | 3 |

`sync --hook` prints only new reports and always exits 0, as today.

Writes, so a concurrent edit is never lost:

- **Mirror to disk**: stage the new content as a temporary sibling, rename
  the current entry aside (same folder, atomic), check the set-aside copy
  still hashes to the base, then put the new entry in place without
  replacing anything (a hard link for a file, `rename` for a folder, which
  cannot replace a non-empty folder or a file). If the check fails or a new
  entry appeared, the set-aside copy goes back or to a backup, and the item
  is a conflict.
- **Disk to mirror**: copy into staging under `.skiman/tmp/`, check the mirror
  still hashes to the base, rename into place (a folder: old one aside, new
  one in, old one removed). If the mirror changed, conflict.
- Every path component between the root and the item must be a real folder
  (checked with `lstat`); intermediate folders such as `.claude/skills` are
  created only under an existing anchor.

### Local edits skiman did not see

- **Edited on disk** (an agent, an editor, `sed`): the next sync copies it to
  the mirror; `git status`/`git diff` in the skill repo shows it, and the
  pre-commit hook checks it. Session-start and git-hook syncs make this
  happen without the user doing anything.
- **Edited in the mirror** (by hand, `git pull` from another machine): the
  next sync writes it to the project.
- **Edited on both sides**: conflict, exit 3, printed once at session start.
  `skiman project adopt <path>` keeps the disk version, `skiman project
  restore <path>` keeps the mirror version; the other version goes to a
  backup.
- **Deleted on disk**: reported. `restore` brings it back; `eject` stops
  managing it (the mirror goes to a backup).
- **Deleted in the mirror** (`git rm`, a checkout): reported. `adopt` takes
  the disk file back in; `eject` forgets it (the disk file stays).
- **Project folder moved inside the root** (for example into `_archive/`):
  the old path shows "project gone", the new path holds an unadopted file.
  `adopt` pairs them when the new candidate has exactly the content and the
  final path components of exactly one "project gone" item: it moves the
  mirror (history continues) and the base. Anything ambiguous stays as is.
- **Project folder deleted or moved out of every root**: "project gone";
  `eject <old path>` moves the mirror to a backup. The project files, if
  they still exist somewhere, are real files and keep working.
- **A project folder copied**: the copy holds real files, unmanaged until
  adopted; the two never share content.
- **Another machine**: the mirror is committed; sync there writes every item
  whose anchor exists and is missing, and reports a conflict where that
  machine already has different content.

### Commands

| Command | What it does |
|---|---|
| `skiman project add <path> [--id <id>]` | Register a project root. |
| `skiman project adopt [<path>...] [--dry-run] [--yes]` | Discover under the roots (or under `<path>`), print the plan (new, moved, conflicts taken from disk, skipped with reason), confirm, then copy each into the mirror through staging and the rules, and set its base. A conflict's previous mirror content goes to the backup. Ends with `sync`. |
| `skiman project restore <path>... [--yes]` | Write the mirror version over the disk for conflicts and deleted-on-disk items under `<path>`; the disk version goes to the backup. |
| `skiman project eject <path>... [--yes]` | Stop managing the items under `<path>`: their mirrors go to the backup, their base entries go, the disk is not touched. A root's own path also removes `[projects.<id>]`, after every item is out. |

Conventions as today: `--yes` without a terminal, exit codes 0/1/2/3/4, the
repo lock, staging under `.skiman/tmp/`, backups under machine state
(`backups/<ts>/repo/projects/...` for mirror content, `backups/<ts>/projects/
<id>/<rel>` for disk content).

### Rules (`skiman check`, pre-commit)

- `project-path` (exit 1): anything under `projects/` not inside a configured
  root id, or not part of a recognised project item. A hand-deleted
  `[projects.<id>]` blocks the commit until `projects/<id>` goes too.
- The skill rules on every project skill folder.
- Instruction files: regular file, file size limit.
- The secret rule on every file under `projects/`.

### Other changes

- `doctor`: per root, whether it exists, item count, every item that is not
  in step with its fix, and how many candidates are not adopted yet.
- `managing-skills`: a short section: project items are real files, edit
  them in place; after creating a project `AGENTS.md` or project skill under
  a root, run `skiman project adopt <path>`.
- `README.md`, `GLOSSARY.md`, ADR 0006 (copy mode for project items: why not
  links; mirror layout; git-tracked items stay with their repo).

### Decisions made for the user

- Mirrors live in the skill repo, not in a second store: one source of truth,
  checked by the same rules and hooks. The user already keeps work-related
  instructions there (`instructions/legacy-global-rules.md`). Registering a
  root is the user's explicit act, and `project add` says where the content
  goes.
- The three company skills in `~/Work/Five-Continents/fivec-core/skills/` are
  not in a project skill directory (they reach the agents through links in
  the global tool directories), so they are global skills and out of this
  scope; `skiman adopt <name> --from claude-code` already brings such a
  skill in.

## Not in scope

- A file-watching daemon: session-start and git-hook syncs plus `doctor`
  cover detection.
- Three-way merging of conflicts: the base stores only a hash; conflicts are
  rare, and the user picks a side with the other in the backup.
- Managing files tracked by another git repo.
- `skiman list` output for project items; `doctor` and `adopt --dry-run`
  show the status.

## Test plan (`tests/project.rs`, sandbox harness)

1. `project add`: every refusal; the `~/` path.
2. Discovery: every pattern; skill folders not entered; dir symlinks not
   followed; `.git`, `node_modules`, linked worktree, `exclude` (name and
   path), skill repo and state dir skipped; alias links ignored.
3. Gates: tracked file; skill with one tracked file; git failure; secret;
   bad frontmatter; symlink inside a skill.
4. Adopt: new file and skill copied byte-identically with base set; disk
   untouched; conflict taken from disk with the old mirror in the backup;
   moved pairing (unique) and no pairing (ambiguous); `--dry-run` writes
   nothing; rerun is a no-op.
5. Sync: every row of the state table, including a new machine (no base),
   occupied by a link and by a symlinked parent, blocked by a secret, hook
   mode printing each report once, the set-aside check catching a concurrent
   change.
6. Restore and eject: as specified, including eject of a root.
7. Check: `project-path`, secret in a mirror file, skill rules.
8. Doctor: the project section.

Plus an end-to-end run with the real tools on a throwaway root: adopt, edit
through Claude Code, sync, see the mirror change.

## Acceptance

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test` (the CI job).
- Council on the plan and on the implementation: every delegate above 9.5
  and approving.
