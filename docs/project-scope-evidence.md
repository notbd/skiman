# Project scope: evidence from the real agent tools

Run on 2026-11-06 on the user's Mac, in a throwaway folder outside every git
repo, with Claude Code 2.1.311 (`claude -p`), codex-cli 0.166.0 (`codex exec
--skip-git-repo-check`) and agy 1.5.0 (`agy -p`). Each test file or skill
held a random token that appeared nowhere else; each subject was asked,
without tools, to quote every token it had loaded.

## Which project locations each tool loads

The project folder held `CLAUDE.md`, `AGENTS.md`, `GEMINI.md` and one skill
in each of `.claude/skills/`, `.agents/skills/`, `.agent/skills/` and
`.gemini/skills/`, every one a symlink into a separate folder (a symlink is
at least as hard to load as a regular file, so a pass also covers copies).

| Location | Claude Code | Codex | Antigravity |
|---|---|---|---|
| `CLAUDE.md` | `CLMD-7f31a9` | no | no |
| `AGENTS.md` | `AGMD-2c8e44` | `AGMD-2c8e44` | `AGMD-2c8e44` |
| `GEMINI.md` | no | no | `GEMD-a03d17` |
| `.claude/skills/p1l-claude` | `SKC-11aa` | no | no |
| `.agents/skills/p1l-agents` | no | `SKA-22bb` | `SKA-22bb` |
| `.agent/skills/p1l-agent` | no | no | `SKG-33cc` |
| `.gemini/skills/p1l-gemini` | no | no | no |

## Claude Code `@` imports through a symlink

| Setup | Loaded |
|---|---|
| Regular `CLAUDE.md` with `@imported.md`, sibling `imported.md` (control) | the sibling's token |
| `CLAUDE.md` symlink to `store/CLAUDE.md` inside the project; `imported.md` both beside the link and in `store/` | only the `store/` token: the import resolves against the link target |
| `CLAUDE.md` symlink to a file outside the project; `imported.md` on both sides | neither (the target side is an external import, which `claude -p` does not approve) |

## Claude Code editing a symlinked `AGENTS.md`

`claude -p --permission-mode acceptEdits`, asked to append a line to the
symlinked `AGENTS.md` with its Edit tool and without Bash: the edit did not
happen; Claude Code treated the link target as a file outside the working
directory and asked for it to be added as a working directory. The target
file was unchanged afterwards.

## Git ignore patterns and symlinks

In a scratch repo, `.gitignore` holding `.claude/skills/x/` (a folder
pattern) does not ignore a symlink at `.claude/skills/x`: `git status`
lists it and `git check-ignore` exits 1.

These three findings are why project items are kept as real files (copy
mode) rather than replaced by links; see ADR 0006.
