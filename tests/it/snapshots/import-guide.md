# Import guide for skiman <VERSION>

This guide is for an agent. You read the user's existing setup and prepare an
import folder: a folder laid out exactly like the canon should look after
the import. skiman then checks it with its everyday rules, shows the user a
preview, copies it into the canon and takes over the tool directories and
global instruction files, moving every original it replaces into a backup.

You write only the import folder, plus temporary folders outside the
canon when you need them; step 6 of "Extract the shared body" is the one
exception. The user's existing files stay where they are; skiman moves them
during the import.

## Before you start

- The canon must exist (`skiman init <path>`). It may already hold
  skills, but only ones the import folder holds too, with the same content
  (a skill adopted earlier, or one an interrupted import wrote). Any other
  entry in `skills/` or `upstream/` (names starting with `.` do not count)
  and any other entry in `skiman.lock` make `skiman import` refuse, naming
  each.
- The default import folder is `<canon>/.skiman/import/`; any other path
  works too.
- The import writes the canon's `instructions/shared.md`, `skiman.toml`
  and `skiman.lock`. One that already holds what the import writes stays; one
  that differs goes to the backup first. When the canon's shared body
  already holds instructions, see step 6 of "Extract the shared body".
- The existing setup lives in these locations (`~` is the home folder):

  | Agent tool | Tool directory | Global instruction file | Size limit |
  |---|---|---|---|
  | `claude-code` | `~/.claude/skills` | `~/.claude/CLAUDE.md` | none known |
  | `codex` | `~/.agents/skills` | `~/.codex/AGENTS.md` | 32768 bytes |
  | `antigravity` | `~/.gemini/config/skills` | `~/.gemini/GEMINI.md` | 24000 bytes |

- Other managers own the following; leave it out of the import folder (skiman
  skips it during the import):
  - `~/.claude/skills/synced/`
  - `~/.claude/skills/.trash/`
  - anything whose realpath is under `~/.claude/plugins/`
  - anything whose realpath is under `~/.codex/plugins/`
  - `~/.codex/skills/.system/`
  - any folder containing `.claude-plugin/plugin.json`
- Follow symlinks to find where content really lives. Several entries can
  point to the same folder.
- No tool directory may be a link into the canon (such as
  `~/.claude/skills -> <canon>/skills`) or into an entry the import
  moves (such as `~/.agents/skills -> ~/.claude/skills/bundle/skills`, where
  `bundle` is a foreign skill). Ask the user to replace such a link with a
  real folder first. Otherwise the import can move the canon's own
  skills to the backup, or leave that tool directory dangling.

## The import folder

It holds these three entries and nothing else:

```
<import folder>/
  skills/<name>/          one folder per skill, own and third-party,
  skills/<group>/<name>/  or one per skill inside a group folder
  instructions/shared.md  the shared body of the global instructions
  skiman.toml             agent tools, skills and their sources
```

skiman fetches every upstream itself and builds `upstream/` and `skiman.lock`;
the import folder holds neither.

### skills/<name>/

One folder per skill, holding the files as the user should have them after the
import, including the user's local modifications to third-party skills. A
folder directly under `skills/` without a `SKILL.md` is a group folder: every
folder in it is a skill of that group (`skills/<group>/<name>/`), and the
canon gets the same layout. Groups nest: a folder in a group folder that holds
only folders and no `SKILL.md` is a group folder too. Each group folder's name
follows the skill name rule and is no skill's name. Put skills in groups only as the user
wants them grouped. Every skill passes the everyday rules:

- `SKILL.md` sits at the top of the folder. Its YAML frontmatter starts on
  line 1, has no duplicate keys, and holds `name` and `description`; other
  fields are allowed.
- `name` matches `^[a-z0-9]+(-[a-z0-9]+)*$`, has 1 to 64 characters, equals
  the skill's own folder name, is in one place only whatever its group, and
  is not reserved (`skiman-manual`, `synced`, or any name starting with
  `.`). When you rename a skill, rename both.
- `description` has 1 to 1024 characters.
- Every entry is a regular file or folder: replace a symlink with the content
  it points to. At most 10 MiB per file, 1000 files and 50 MiB per skill.
- Every name is valid UTF-8 without control or direction-changing
  characters, and no name is `.git`: copy a skill's files, not its git
  repository. No two paths in a skill, and no two skill folders, differ only
  in letter case or Unicode normalisation.
- No Git LFS pointer files: skiman does not fetch LFS content.
- No conflict markers (lines starting with `<<<<<<< local (skiman)` or
  `>>>>>>> upstream (skiman)`), no names ending in `.skiman-upstream` or
  `.skiman-upstream-deleted`, and no `.skiman-conflicts` folder.
- No secrets: private keys, AWS access keys, GitHub, OpenAI and Slack tokens,
  and quoted `password`, `secret` or `token` values. A secret that must stay
  (a test fixture) needs a `[[secrets.allow]]` entry in `skiman.toml`; the
  refusal prints the entry ready to paste.
- `.DS_Store`, `Thumbs.db` and `Icon\r` (a folder's custom icon on macOS)
  files are ignored.

Leave out a third-party skill's `.gitignore`, `.gitattributes` and
`.gitmodules`: skiman does not install them from its upstream, and in the
canon they would act on the canon's git.

### instructions/shared.md

The shared body: the global instructions every agent tool loads. Its size is
at most the smallest size limit among the enabled agent tools (see the table
above). A word starting with `@` at the start of a line or after whitespace,
outside inline code and fenced code blocks, is a rule violation. The secret
rule of the skills applies to it too.

### skiman.toml

```toml
[tools]                     # optional; without it the canon keeps its own
enabled = ["claude-code", "codex", "antigravity"]

[skills.notes]              # optional for an own skill
enabled = false             # to disable it
summary = "Keep meeting notes"  # one line each: what it is for,
origin = "written here"     # and where an own skill came from (a URL, or "written here")
installed = 2026-01-31      # optional: the day it was installed

[skills.pdf.source]         # a third-party skill
url = "https://github.com/anthropics/skills.git"  # https or ssh git URL, or a local absolute or ~ path
path = "skills/pdf"         # required: folder inside the upstream; "" for its root
ref = "<40-hex sha>"        # the commit the user's copy came from

[[secrets.allow]]           # only for a secret that must stay
path = "skills/pdf/tests/fixture.env"  # the skill's path without its group
rule = "aws-access-key"
fingerprint = "sha256:<hash of the matched text>"
```

- A skill is third-party exactly when it has a `source` table.
- Fill `summary`, `origin` and `installed` only with what the user
  says or the existing setup shows; leave out what you would have to guess.
- Unknown keys are an error. Leave out `ref_kind`; skiman derives it.
- skiman keeps the canon's project scope: every `[projects.<id>]` table
  whose id the folder does not define, and every `[[secrets.allow]]` entry for
  a path under `projects/`. A project root the folder adds must not contain,
  or lie inside, a root the canon already has; otherwise skiman skips
  both roots, and `skiman sync` and `skiman doctor` name them.
- skiman keeps the canon's linked folders too: every `[linked.<id>]`
  table stays. The import folder's `skiman.toml` holds no `[linked]` table
  (see "Skill folders kept elsewhere").

## Skill folders kept elsewhere

Some skills have their home outside the tool directories: a folder the user
edits in another place, such as a folder of company skills in a work
directory, which a script or the user linked into the tool directories. Do
not copy such a folder's skills into `skills/`. Tell the user which folders
you left out; after the import, the user registers each one with
`skiman linked add <path>`, and skiman distributes its skills from where
they are, as linked skills. The links to it in the tool directories go to
the backup like any foreign entry; the folder itself stays as it is.

The canon may already register linked folders (`skiman list` shows their
skills, of kind `linked`). Their skills are distributed already: leave them
out too, and give no skill or group folder in the import folder a linked
skill's name. While a registered linked folder cannot be read, the import
refuses.

## Choose the canonical copy among diverged copies

The same skill often exists in several places: in several tool directories,
behind links to folders elsewhere, or as older copies left behind. The import
folder holds exactly one canonical copy per skill.

1. Group the candidates by skill: same folder name or same frontmatter `name`.
2. Resolve links: entries whose realpath is the same folder are one copy.
3. Compare the remaining copies file by file (for example `diff -r`).
   Identical copies are one copy.
4. When copies diverge, choose the canonical copy in this order:
   1. the copy other entries link to, because the user already treats it as
      the source;
   2. the copy with the most recent edits (file modification times, or git
      history when the folder is in a git repo);
   3. when a newer copy lacks a change an older copy has, or the order is
      unclear, show the user the differences and ask which copy to keep or
      how to merge them.
5. Tell the user which copies you left out. The ones in the tool directories
   of the agent tools `[tools] enabled` lists go to the backup; copies
   elsewhere stay where they are.

## Extract the shared body

1. Read each agent tool's global instruction file (see the table above),
   following links, and every file it imports with an `@path` line. When the
   canon's `instructions/shared.md` holds instructions, it is one more
   source.
2. Replace each `@path` import with the text it imports.
3. Write each instruction that applies to every agent tool once into
   `instructions/shared.md`. When files word the same instruction
   differently, keep the newest wording; when they contradict each other, ask
   the user.
4. List for the user any text that applies to one agent tool only, and ask
   whether to keep it in the shared body or leave it out. This version of
   skiman has no tool-specific instructions; the originals stay in the backup.
5. Check the size against the smallest limit among the enabled agent tools.
6. When the canon's `instructions/shared.md` held instructions, copy it
   to a file outside the canon, then empty it right before the import
   runs. Left as it is, it would be replaced and kept only in the backup, or
   the import would refuse (see "When the shared bodies differ").
   If the import then does not run (the user declines at the preview, or it
   fails before it writes the shared body), put that copy back: until then
   every agent session loads empty global instructions, and `skiman sync`
   and `skiman check` do not report it.

## Pin each upstream

1. Decide for each skill whether it is third-party: it is when you can name its
   upstream (a git remote in the folder it lives in now, an installer's
   record, a source URL in its files). Every other skill is an own skill and
   gets no `source` table.
2. For a git upstream, find the commit the user's copy came from: clone the
   upstream into a temporary folder outside the canon and pick the commit
   whose folder at `path` differs least from the user's copy.
3. Pin it with `ref = "<40-hex sha>"`, the full 40-character commit. skiman
   fetches each upstream at its declared ref, and the preview shows every file
   that differs from that commit as a local modification, so a pinned commit
   keeps the preview truthful. After the import the user can follow a branch
   with `skiman update <name> --ref <branch>`.
4. For a local folder upstream, set `url` to the folder's plain absolute or
   `~` path (`~/src/pdf`: the real folder, not a link to it; no trailing `/`,
   no `..`, each name in the letter case it has on disk) and leave out
   `ref`: skiman versions a local folder by its content. The folder must
   still be there after the import, so its path must not lead through an
   entry the import moves to the backup (a foreign skill, or a link to one,
   in the tool directory of an enabled agent tool), and it must not hold
   one. Otherwise the import refuses: copy the upstream to a folder outside
   the tool directories and point `url` there, or drop the `source` table to
   import the skill as an own skill.

## Run the import

1. Ask the user to run `skiman import <folder>` in a terminal. skiman checks
   the folder, fetches every upstream and shows the preview: the skills to
   add (`keep` for one the canon already holds as the import folder
   does), the files of each third-party skill that differ from its upstream
   and the upstream files it does not install, the canon files it
   replaces, and every entry it moves to the backup. The user confirms
   there. Run `skiman import <folder> --yes` yourself only when the user
   tells you to import without the preview.
2. Act on the exit code:
   - 0: imported. skiman prints where this run moved the originals. The
     import folder can then be deleted.
   - 3: imported, but something needs attention, such as an entry left in
     place because it is not a skill. Follow each warning, then run
     `skiman sync`. This is not a failure.
   - 1: refused; nothing changed. A problem in the import folder is named
     with its file and line: fix the folder. A problem in the canon is
     named with the ways to go on; for a `skiman.lock` that is not valid
     TOML, delete that file. Then run the import again.
   - 2: there is no terminal and no `--yes`.
   - 4, or the import was interrupted: run the same command again (see
     below).

## Run it again

Running the same command again finishes an import that stopped part-way;
skiman keeps no record of a run. Each run works the whole import out again
from what is on disk: it keeps what the canon already holds as the
import writes it, writes the rest, and moves each original still in place.

- Each run shows the preview again, and moves its originals to a new folder
  in the backups folder the preview names. The originals of an interrupted
  import are spread over these folders. The folder printed after
  `moved tmp leftovers to` holds copies skiman had staged, not originals.
- Until the canon holds every piece of the import, a run fetches the
  upstreams again, except a git upstream the canon already holds as
  declared: the same `url`, `path` and `ref` in its `skiman.toml`, and a
  `skiman.lock` entry with that `ref` and a commit. That one stays at the
  commit: its snapshot is `upstream/<name>` when that matches the entry,
  else the commit read from the upstream cache, which goes to the network
  only when it lacks the commit. Any other upstream not pinned to
  a commit (a branch, a tag, the default branch, a local folder) may bring
  newer content: the preview shows the new commit, and the old snapshot
  goes to the backup.
- The canon holds the whole import when it has exactly the import
  folder's skills, each the same, the same sources (`url` and `path`), the
  snapshots and `skiman.lock` that go with them, and the same shared body (a
  missing one, or one of nothing but spaces, tabs and line breaks, is empty,
  and two empty ones are the same). Then a run fetches nothing and writes
  nothing to the canon; it moves the originals still in place. This
  can be true on the first run too: the skills adopted earlier and both
  shared bodies empty.
- From then on, changes to the import folder's `skiman.toml` other than a
  source's `url` or `path` do not reach the canon. The preview names a
  `[tools] enabled`, `enabled` or `ref` that differs from the canon's
  and keeps the canon's. Change those in the canon's `skiman.toml`
  and run `skiman sync`; for a `ref`, run `skiman update <name> --ref <ref>`.
- When the canon no longer holds the whole import for another reason
  than the shared body (a skill removed from it, a skill added to the import
  folder or a source's `url` or `path` changed there, `skiman.lock`
  deleted), a run is a full import again: the shared body and `skiman.toml`
  become the import folder's (the canon's project roots and linked
  folders stay), the old ones go to the backup, and the preview shows it.
- A skill in the canon that changed since the import wrote it (for
  example through its link in a session), or one the import folder does not
  hold, is refused and named with the two ways to go on.

### When the shared bodies differ

When the canon holds everything the import writes except the shared
body (exactly the import's skills, each the same, the same sources, and the
snapshots and `skiman.lock` that go with them), but its
`instructions/shared.md` holds something other than the import folder's,
the import refuses. Go on by whether this import has run on this canon
before:

- It has (it finished, or stopped part-way): copy the file to keep over the
  other one, then run it again. That run finishes what is left without
  fetching and writes nothing else to the canon. skiman keeps no copy
  of the file you overwrite.
- It never has (the skills came from `skiman new`, `adopt` or `add`, or the
  import folder holds no skills): empty the canon's file, after putting
  what it holds into the import folder's shared body if the user wants to
  keep it, then run it again. That run imports everything, the import
  folder's `skiman.toml` included.
- When you cannot tell, copy. Emptying the file on a canon the import
  has run on imports everything again: the canon's `skiman.toml`
  becomes the import folder's again, dropping the `enabled`, `[tools]` and
  `ref` changes made since, and an upstream whose `ref` changed since is
  fetched again at the import folder's `ref` while the skill stays as it
  was (`skiman diff` then shows the difference as local modifications). If
  that run cannot fetch an upstream, it stops with the shared body empty:
  copy the shared body you want back into the canon and run it again.

### Known limits

- When the backups folder is on another volume than an original, the move
  copies, checks, then deletes the original. Interrupted while copying, it
  leaves a part copy in the backup folder and the original whole; while
  deleting, part of the original in place. If what is in place still has
  its `SKILL.md`, running again moves it to a new backup folder; otherwise
  it stays where it is, with a warning (exit code 3) only when the import
  links a skill of that name there. Of two copies of one original in the
  backups folder, the one with more files is whole. Delete a part left in a
  tool directory, then run `skiman sync`.
- A link in a tool directory to another skill's folder in the canon
  (`notes -> <canon>/skills/pdf`) dangles until the import writes that
  skill: the first run leaves it in place, and running again moves it to
  the backup.
- skiman compares paths byte for byte. A link in a tool directory whose
  target spells a folder in other letter case than on disk, or with the
  `/System/Volumes/Data` prefix, can be left in place, dangling, after an
  interrupted import: move it to the backup by hand, then run `skiman sync`.
  A local source whose `url` has other letter case than the disk can lose
  the link it is reached through to the backup; `skiman update` then fails
  for it until its `url` in the canon's and the import folder's
  `skiman.toml` names the real folder.
