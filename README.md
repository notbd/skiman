# skiman

skiman keeps one person's agent skills and global instruction files in a
single git repo, the skill repo, and distributes them to every agent tool that
reads them (Claude Code, Codex, Antigravity). The skill repo holds own skills,
which you write by hand, and third-party skills, which are tracked against
their upstream with your local modifications kept on update. skiman links each
skill into the agent tools' tool directories and the shared body of the global
instructions into each tool's instruction location. Content skiman does not
own is replaced only by `skiman adopt` and `skiman import`, and the originals
go to a backup.

## Install

skiman needs Rust and git 2.43 or later.

```sh
cargo install --path .
```

## Usage

| Command | What it does |
|---|---|
| `skiman init <path> --session-hooks <ids>\|none` | Create or reuse a skill repo, install its git hooks and session-start hooks, then sync. |
| `skiman new <name> -d <description>` | Create an own skill. |
| `skiman add <source>` | Install third-party skills from a git URL, a GitHub tree URL, `<owner>/<repo>` or a local folder (`--skill`, `--all`, `--name`, `--ref`, `--dry-run`, `--yes`). |
| `skiman update [names]` | Merge each third-party skill's new upstream version with its local modifications (`--ref` for one skill). |
| `skiman diff <name>` | Show a third-party skill's local modifications. |
| `skiman remove <name> --yes` | Remove a skill; its folders go to a backup. |
| `skiman mv <old> <new>` | Rename a skill and its records. |
| `skiman adopt <name> --from <tool id> --yes` | Bring a foreign skill from a tool directory into the skill repo. |
| `skiman import [folder] --yes` | Import a prepared folder (default `<skill repo>/.skiman/import/`); `--guide` prints how to prepare one. |
| `skiman sync` | Make every tool directory and instruction location match the skill repo (`--hook` from hooks). |
| `skiman check` | Report every rule violation in the skill repo (`--staged` for the git index). |
| `skiman list` | Show every skill with its kind, source, version and state. |
| `skiman doctor` | Diagnose the setup. |

Without a terminal, every flag that would otherwise be asked for is required
(exit 2). Exit codes: 0 success, 1 rule violation or refused input, 2 usage
error, 3 needs attention, 4 operational error.

Codex session refresh is not available until Phase 1 experiment 4 is
confirmed; see [docs/phase1-results.md](docs/phase1-results.md).

## Verify the install

1. Run `skiman --version`.
2. Try `skiman init` in a throwaway home, so your real agent tools stay
   untouched:

   ```sh
   export HOME=$(mktemp -d) XDG_CONFIG_HOME= XDG_STATE_HOME=
   skiman init "$HOME/skills" --session-hooks none
   skiman doctor
   ```

   Or run `skiman init <path> --session-hooks none` on a real skill repo path,
   then `skiman doctor`.
