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

agy 1.5.12 no longer loads the two Antigravity skill links in this table:
their targets lie outside the project (see "agy 1.5.12" below). skiman's
project skills are copies, so they still load.

## Project rules folders

A second project held `.claude/rules/style.md`, `.claude/rules/sub/deep.md`,
`.agent/rules/r.md`, `.agent/workflows/w.md` and `.agents/rules/r.md`, each
with a token. Claude Code loaded both `.claude/rules` files, including the
one in the subfolder. Codex and Antigravity loaded none of them.

## Codex `AGENTS.override.md`

In a scratch git repo with `AGENTS.md` and `AGENTS.override.md` at the top
and again in `sub/`, `codex exec` started in `sub/` quoted both override
tokens and neither plain token: Codex reads `AGENTS.override.md` at every
level, instead of that level's `AGENTS.md`.

## Claude Code and a mirror inside the canon

Observed with Claude Code 2.1.311 (`claude -p` at the top
of a git repo holding `projects/w/p/AGENTS.md`, `.claude/rules/r.md` and
`.claude/skills/<n>` below it): none of the three loaded at session start;
after the session read one file in `projects/w/p/`, all three were in
context. `claudeMdExcludes` stopped the instruction files but not the skill.

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

The symlink findings are why project items are kept as real files (copy
mode) rather than replaced by links; see ADR 0006.

## End to end through skiman

A throwaway root held `proj/AGENTS.md` (token `E2E-ORIG-4411`). After
`skiman project adopt --yes`, `claude -p --permission-mode acceptEdits` in
`proj/` appended `Edited token: E2E-EDIT-7722` with its Edit tool (the file
stayed a regular file); the next `skiman sync` printed `mirrored
…/proj/AGENTS.md`, and the mirror held both lines.

## agy 1.5.12

Run on 2026-11-19 with `agy` 1.5.12, CLI only (the Antigravity app is not
installed on that machine). Each run used a throwaway `HOME` whose
`Library/Keychains` was a link to the real `~/Library/Keychains`, so agy
could sign in; the links were removed afterwards. Skill lists come from
`agy -p '/skills'` (the loader's own list, no model involved) or from asking
the model, without tools, to quote each test skill's token; the two always
agreed.

### Global skills

- One real skill folder in each candidate folder: agy listed those in
  `~/.gemini/config/skills`, `~/.gemini/skills` and
  `~/.gemini/antigravity-cli/skills`, and none in `~/.agents/skills`,
  `~/.gemini/antigravity/skills` or `~/.agent/skills`. Its built-in docs name
  only `~/.gemini/config/` as the global root, so skiman keeps
  `.gemini/config/skills` as Antigravity's tool directory, and `skiman
  overview` notes the other two folders as read but not listed.
- In `~/.gemini/config/skills`, every link shape loads: a real folder, an
  absolute or relative link, both two-hop shapes of a linked skill (a link to
  `<canon>/linked/<id>/<name>` where `<id>` is itself a link, and a link to
  `<canon>/linked/<id>`), a symlinked `SKILL.md`, and `config/skills` itself
  being a link to `~/.agents/skills`. A skill without `name` in its
  frontmatter takes the link's name.

### Project skills

- Project folders: `.agents/skills`, `.agent/skills`, `_agents/skills` and
  `_agent/skills` all load; `.gemini/skills` does not. `_agents/skills` and
  `_agent/skills` are not in Antigravity's `project_skill_dirs`: nobody
  keeps project items there; add them when someone does.
- A project skill link loads only when its final target lies inside a
  workspace root: the current folder, the git repository above it, or an
  `--add-dir` folder. A link to anything outside is skipped without a log
  line, whatever the workspace trust says; for a two-hop link only the final
  target counts. Real folders always load. The 1.5.0 result above differs;
  the likely cause (not confirmed with an older agy) is the 1.5.2 changelog
  entry "plugins and skills listed in a workspace's `.agents/plugins.json` or
  `.agents/skills.json` now load only from inside the workspace or its
  repository root".

### Global instruction files and ACP

- `~/.gemini/GEMINI.md`, `~/.gemini/AGENTS.md`, `~/.gemini/config/GEMINI.md`
  and `~/.gemini/config/AGENTS.md` all load together, as files, one-hop links
  or two-hop links. `skiman doctor` names the three skiman does not write
  when they exist.
- `agy acp`, driven directly by a minimal JSON-RPC client (`initialize`,
  `session/new`, `session/prompt`), quoted the tokens of the global
  instruction files and rules just as `agy -p` did. T3 Code's own Antigravity
  runtime quoted none of them (and no line of `~/.gemini/GEMINI.md`) but
  listed the global skills: T3 Code does not pass the global instruction
  files on, which is outside skiman.

### Skill body size

A test skill was run as a slash command (`agy -p '/<name> …'`); the session
transcript (`brain/<id>/.system_generated/logs/transcript_full.jsonl`) holds
the skill text as the model got it, ending in `<truncated N bytes>` when cut.
The body is everything after the frontmatter's closing `---` line.

| Body | Frontmatter | Result |
|---|---|---|
| 24000 bytes ending in `\n` | 69 bytes | whole |
| 24001 bytes ending in `\n` | 69 bytes | whole (24000 once the newline is trimmed) |
| 24000 bytes ending in `\n` | 3089 bytes | whole |
| `\n\n` + 24000 bytes | 61 bytes | whole |
| 24001 bytes | 61 bytes | `<truncated 1 bytes>` |
| `\n` + 24003 bytes of mostly `中` + `\n` | 61 bytes | `<truncated 3 bytes>` |
| 26000 bytes ending in `\n` | 69 bytes | `<truncated 1999 bytes>` |

agy keeps 24000 bytes of the body, counted in bytes after leading and
trailing whitespace is trimmed; the frontmatter does not count. Nothing is
logged and the skill still loads. Measured only through slash-command
expansion and only with newlines as surrounding whitespace. While
Antigravity is enabled, skiman warns about a skill over this limit
(`skill-body-size`).

### Session-start hook

agy reads user hooks from `~/.gemini/config/hooks.json`: an object of named
hooks, each holding, among other events, a flat `SessionStart` list of
handlers `{"type": "command", "command": ..., "timeout": ...}` (its built-in
`hooks.json` docs). These runs used the file `skiman init` writes, with a
skiman built from this change. A skill was added to the canon by hand and not
synced, so only the hook could link it. `$R` is the throwaway root,
`HOME=$R/home`; `$Q` asks for every skill ending in `-demo` and its token.

```
$ $R/bin/skiman init $R/canon --session-hooks antigravity
…
added the session-start hook entry to $R/home/.gemini/config/hooks.json
$ cat $R/home/.gemini/config/hooks.json
{
  "skiman": {
    "SessionStart": [
      {
        "type": "command",
        "command": "$R/bin/skiman sync --hook",
        "timeout": 10
      }
    ]
  }
}
$ agy -p /hooks
skiman	enabled	SessionStart	-	command	$R/bin/skiman sync --hook
```

`agy -p`, one prompt per run:

```
# added $R/canon/skills/zz-a1-demo by hand (token T-A1-7712), not synced
$ ls $R/home/.gemini/config/skills
managing-skills
$ agy -p "$Q"
NONE
$ ls $R/home/.gemini/config/skills
managing-skills
zz-a1-demo
# the next session
$ agy -p "$Q"
- `zz-a1-demo`: `T-A1-7712`
```

`agy acp`, two prompts in one session:

```
# added $R/canon/skills/zz-a2-demo by hand (token T-A2-21009), not synced
after initialize: link exists = False
after session/new (+3s): link exists = False
prompt 1: stopReason=end_turn link exists = True
ANSWER 1: - `zz-a1-demo`: `T-A1-7712`
prompt 2: stopReason=end_turn link exists = True
ANSWER 2: - `zz-a1-demo`: `T-A1-7712`
- `zz-a2-demo`: `T-A2-21009`
```

A resumed conversation (`agy --continue -p="$Q"`) did not run the hook: a
skill added before it had no link afterwards, and the next new session made
it. `skiman sync` before a session makes a new skill available at once:

```
# zz-a3-demo was added before an earlier command that agy refused (a flag error), so nothing linked it yet
# added $R/canon/skills/zz-a4-demo by hand (token T-A4-2906), not synced
$ $R/bin/skiman sync
linked $R/home/.gemini/config/skills/zz-a3-demo
linked $R/home/.gemini/config/skills/zz-a4-demo
$ agy -p "$Q"
- `zz-a1-demo`: `T-A1-7712`
- `zz-a2-demo`: `T-A2-21009`
- `zz-a3-demo`: `T-A3-1811`
- `zz-a4-demo`: `T-A4-2906`
```

When `sync --hook` has something to report, agy expects JSON on stdout, so
it logs one error line and goes on; the report reaches neither the user nor
the model (measured with `agy -p` only):

```
E1119 02:56:08.690539     713 provider.go:80] session start hook "jsonhook__skiman_SessionStart_0_0" failed: failed to unmarshal result from hook jsonhook__skiman_SessionStart_0_0 via protojson: skiman: $R/home/.gemini/config/skills/zz-a5-demo is occupied by an entry skiman does not …
```

Result: the hook runs when a conversation starts, at its first prompt, after
agy has read the skills for that prompt. A skill it links is available from
the next prompt; with `agy -p`, which has one prompt, from the next run. A
resumed conversation does not run it. The interactive TUI was not measured.
skiman installs the hook anyway (it keeps every tool directory current for
the next prompt and the next session), and the README says to run `skiman
sync` when a new skill must be there from the first prompt.
