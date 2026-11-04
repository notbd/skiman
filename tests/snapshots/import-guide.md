# Import guide for skiman <VERSION>

This guide is for an agent. You read the user's existing setup and prepare an
import folder: a folder laid out exactly like the skill repo should look after
the import. skiman then checks it with its everyday rules, shows the user a
preview, copies it into the skill repo and takes over the tool directories and
global instruction files, moving every original it replaces into a backup.

You write only the import folder, plus temporary folders outside the skill
repo when you need them. The user's existing files stay where they are; skiman
moves them during the import.

## Before you start

- The skill repo must exist (`skiman init <path>`) and hold no skills: no
  entries in `skills/` or `upstream/` other than names starting with `.`, and
  no entries in `skiman.lock`. Otherwise `skiman import` refuses.
- The default import folder is `<skill repo>/.skiman/import/`; any other path
  works too.
- The import replaces the skill repo's `instructions/shared.md` and
  `skiman.toml`; skiman moves the existing ones to the backup.
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

## The import folder

It holds these three entries and nothing else:

```
<import folder>/
  skills/<name>/          one folder per skill, own and third-party
  instructions/shared.md  the shared body of the global instructions
  skiman.toml             agent tools, skills and their sources
```

skiman fetches every upstream itself and builds `upstream/` and `skiman.lock`;
the import folder holds neither.

### skills/<name>/

One folder per skill, holding the files as the user should have them after the
import, including the user's local modifications to third-party skills. Every
skill passes the everyday rules:

- `SKILL.md` sits at the top of the folder. Its YAML frontmatter starts on
  line 1, has no duplicate keys, and holds `name` and `description`; other
  fields are allowed.
- `name` matches `^[a-z0-9]+(-[a-z0-9]+)*$`, has 1 to 64 characters, equals
  the folder name, and is not reserved (`managing-skills`, `synced`, or any
  name starting with `.`). When you rename a skill, rename both.
- `description` has 1 to 1024 characters.
- Every entry is a regular file or folder: replace a symlink with the content
  it points to. At most 10 MiB per file, 1000 files and 50 MiB per skill.
- No conflict markers (lines starting with `<<<<<<< local (skiman)` or
  `>>>>>>> upstream (skiman)`), no names ending in `.skiman-upstream` or
  `.skiman-upstream-deleted`, and no `.skiman-conflicts` folder.
- No secrets: private keys, AWS access keys, GitHub, OpenAI and Slack tokens,
  and quoted `password`, `secret` or `token` values. A secret that must stay
  (a test fixture) needs a `[[secrets.allow]]` entry in `skiman.toml`; the
  refusal prints the entry ready to paste.
- `.DS_Store` and `Thumbs.db` files are ignored.

### instructions/shared.md

The shared body: the global instructions every agent tool loads. Its size is
at most the smallest size limit among the enabled agent tools (see the table
above). A word starting with `@` at the start of a line or after whitespace,
outside inline code and fenced code blocks, is a rule violation.

### skiman.toml

```toml
[tools]                     # optional; without it the skill repo keeps its own
enabled = ["claude-code", "codex", "antigravity"]

[skills.notes]              # an own skill needs a table only to disable it
enabled = false

[skills.pdf.source]         # a third-party skill
url = "https://github.com/anthropics/skills.git"  # https or ssh git URL, or a local absolute or ~ path
path = "skills/pdf"         # folder inside the upstream; "" for its root
ref = "<40-hex sha>"        # the commit the user's copy came from
upstream_name = "PDF"       # only when the upstream name differs from the folder name

[[secrets.allow]]           # only for a secret that must stay
path = "skills/pdf/tests/fixture.env"
rule = "aws-access-key"
fingerprint = "sha256:<hash of the matched text>"
```

- A skill is third-party exactly when it has a `source` table.
- Unknown keys are an error. Leave out `ref_kind`; skiman derives it.

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
5. Tell the user which copies you left out. The ones in tool directories go
   to the backup; copies elsewhere stay where they are.

## Extract the shared body

1. Read each agent tool's global instruction file (see the table above),
   following links, and every file it imports with an `@path` line.
2. Replace each `@path` import with the text it imports.
3. Write each instruction that applies to every agent tool once into
   `instructions/shared.md`. When files word the same instruction
   differently, keep the newest wording; when they contradict each other, ask
   the user.
4. List for the user any text that applies to one agent tool only, and ask
   whether to keep it in the shared body or leave it out. This version of
   skiman has no tool-specific instructions; the originals stay in the backup.
5. Check the size against the smallest limit among the enabled agent tools.

## Pin each upstream

1. Decide for each skill whether it is third-party: it is when you can name its
   upstream (a git remote in its folder, an installer's record, a source URL
   in its files). Every other skill is an own skill and gets no `source` table.
2. For a git upstream, find the commit the user's copy came from: clone the
   upstream into a temporary folder outside the skill repo and pick the commit
   whose folder at `path` differs least from the user's copy.
3. Pin it with `ref = "<40-hex sha>"`, the full 40-character commit. skiman
   fetches each upstream at its declared ref, and the preview shows every file
   that differs from that commit as a local modification, so a pinned commit
   keeps the preview truthful. After the import the user can follow a branch
   with `skiman update <name> --ref <branch>`.
4. For a local folder upstream, set `url` to the folder's absolute or `~` path
   and leave out `ref`: skiman versions a local folder by its content.

## Run the import

1. Ask the user to run `skiman import <folder>` in a terminal. skiman checks
   the folder, fetches every upstream and shows the preview: the skills to
   add, the files of each third-party skill that differ from its upstream,
   and every entry it moves to the backup. The user confirms there. Run
   `skiman import <folder> --yes` yourself only when the user tells you to
   import without the preview.
2. Exit code 1: the folder breaks a rule, and the message names the file and
   line. Fix the import folder and run the import again; nothing has changed.
3. If the import stops part-way (exit code 4, or an interruption), run the
   same command again with the import folder unchanged. It resumes.
4. When it succeeds, it prints the backup folder. The import folder can then
   be deleted.
