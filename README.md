# skiman

skiman keeps one person's agent skills and global instruction files in a
single git repo, the canon, and distributes them to every agent tool that
reads them (Claude Code, Codex, Antigravity). It also keeps a versioned copy
of the project instruction files and project skills under the folders you
register (`AGENTS.md`, `CLAUDE.md`, `.claude/skills/<n>` in each project).
The canon holds own skills,
which you write by hand, and third-party skills, which are tracked against
their upstream with your local modifications kept on update. skiman links each
skill into the agent tools' tool directories and the shared body of the global
instructions into each tool's instruction location. Content skiman does not
own is replaced only by `skiman adopt` and `skiman import`, and the originals
go to a backup.

## Install

skiman needs Rust 1.99 or later and git 2.43 or later.

```sh
cargo install --locked --path .
```

Install a new version on every machine that uses the canon before you
commit what it adds: keys of `skiman.toml` (such as `summary`), which older
versions report as unknown keys, or group folders under `skills/` (nested
ones too), which older versions read as skill folders without a `SKILL.md`. The pre-commit
hook then refuses the commit.

## Usage

| Command | What it does |
|---|---|
| `skiman init <path> --session-hooks <ids>\|none` | Create or reuse a canon, install its git hooks and session-start hooks, then sync. |
| `skiman new <name> -d <description>` | Create an own skill (`--group`). |
| `skiman add <source>` | Install third-party skills from a git URL, a GitHub tree URL, `<owner>/<repo>` or a local folder (`--skill`, `--all`, `--name`, `--group`, `--ref`, `--dry-run`, `--yes`). |
| `skiman update [names]` | Merge each third-party skill's new upstream version with its local modifications (`--ref` for one skill, `--dry-run`). |
| `skiman diff <name>` | Show a third-party skill's local modifications. |
| `skiman remove <name> --yes` | Remove a skill; its folders go to a backup. |
| `skiman mv <old> [<new>]` | Rename a skill and its records, or move it to another group folder (`--group`, `--no-group`). |
| `skiman adopt <name> --from <tool id> --yes` | Bring a foreign skill from a tool directory into the canon (`--group`). |
| `skiman import [folder] --yes` | Import a prepared folder (default `<canon>/.skiman/import/`); `--guide` prints how to prepare one. |
| `skiman sync` | Make every tool directory and instruction location match the canon (`--hook` from hooks). |
| `skiman check` | Report every rule violation in the canon (`--staged` for the git index). |
| `skiman list` | Show every skill with its kind, source, version and state. |
| `skiman overview` | Show every skill on this machine as Markdown: by group with summary, source and install day, then the skills other managers keep (read-only). |
| `skiman doctor` | Diagnose the setup. |
| `skiman project add <path>` | Register a project root (`--id`). |
| `skiman project adopt [paths]` | Copy project items into the mirror under `projects/` (`--dry-run`, `--yes`). |
| `skiman project restore <paths>` | Write mirror items to the project (`--dry-run`, `--yes`). |
| `skiman project eject <paths>` | Stop managing project items; the project files stay (`--yes`). |

### Groups

A folder under `skills/` without a `SKILL.md` that holds only folders is a
group folder: every folder in it is a skill in that group
(`skills/<group>/<name>`), or an inner group folder, so the folder tree shows
how the skills are grouped. Groups nest to any depth
(`skills/third-party/tstack/<name>`, `--group third-party/tstack`). A group
folder's name follows the skill-name rule and is no skill's name. A skill
directly under
`skills/` is in no group, and `skiman check` reminds you of it. Tool
directories stay flat: each skill is linked under its own name, wherever its
folder sits, so skill names are unique across groups. Moving a skill to
another group (`skiman mv <name> --group <group>`, or by hand followed by
`skiman sync`) changes no record. Why: [docs/adr/0008-groups-are-folders.md](docs/adr/0008-groups-are-folders.md), [docs/adr/0009-groups-nest.md](docs/adr/0009-groups-nest.md).

### Project instruction files and project skills

Project items stay real files in the project, where people and agents edit
them. `skiman sync` (also run at session start and by the git hooks) copies
every change into the mirror `projects/<id>/<path>`, so the canon
records their history once you commit. The project is written only by
`skiman project restore`, and whatever skiman replaces on either side goes to
a backup. An item edited on both sides is reported as a conflict, never
overwritten; settle it per item with `adopt <item>` (project wins) or
`restore <item>` (mirror wins). A folder given to any of these commands never
replaces existing content. Files a project's own git repo tracks are left to
that repo. Design and evidence: [docs/project-scope.md](docs/project-scope.md),
[docs/adr/0006-project-items-in-copy-mode.md](docs/adr/0006-project-items-in-copy-mode.md).

Without a terminal, every flag that would otherwise be asked for is required
(exit 2). Exit codes: 0 success, 1 rule violation or refused input, 2 usage
error, 3 needs attention, 4 operational error.

skiman adds no session-start hook for Codex yet: Codex runs only the hooks
you have trusted with its `/hooks` command. Run `skiman sync` before you
start a Codex session, or rely on the canon's git hooks, which run it
after each commit, merge, checkout and rebase there. Evidence:
[docs/phase1-results.md](docs/phase1-results.md).

For Antigravity, `--session-hooks antigravity` adds a hook named `skiman`
to `~/.gemini/config/hooks.json`. Antigravity runs it when a conversation
starts, at its first prompt, but after it has read the skills for that
prompt: a skill the hook links is there from the next prompt (with `agy -p`,
from the next run), and a resumed conversation does not run it. So after you
add or rename a skill, run `skiman sync` before you start an Antigravity
session that needs it from the first prompt. What the hook reports goes only
to Antigravity's log. Antigravity also cuts a skill's body after 24000 bytes
without a message, so while it is enabled skiman warns about a longer one.
Evidence: [docs/project-scope-evidence.md](docs/project-scope-evidence.md).

## Verify the install

1. Run `skiman --version`.
2. Try `skiman init` in a throwaway home, so your real agent tools stay
   untouched (the parentheses keep your shell's `HOME` as it is):

   ```sh
   (
     export HOME=$(mktemp -d) XDG_CONFIG_HOME= XDG_STATE_HOME=
     mkdir "$HOME/.claude"
     skiman init "$HOME/skills" --session-hooks none
     skiman doctor
     ls -l "$HOME/.claude/skills"
   )
   ```

   `doctor` reports the canon and Claude Code as installed, and
   `~/.claude/skills/skiman-manual` is a link into the canon.

   Or run `skiman init <path> --session-hooks none` on a real canon path,
   then `skiman doctor`.
