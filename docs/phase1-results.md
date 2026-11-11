# Phase 1 gate results (ticket #3)

Run on 2026-11-04, 04:06 to 04:20 local time, on the user's Mac, against the real agent tools and the real tool directories, instruction locations and settings files. Experiment 6 (`skiman sync --hook` timing) belongs to #5.

Raw evidence (logs, marker files and backups) is kept outside the repo, because it contains copies of the user's global instructions.

## Agent tool versions

| Agent tool | Binary | Version |
|---|---|---|
| Claude Code | `/opt/homebrew/bin/claude` | 2.1.311 |
| Codex | `/opt/homebrew/bin/codex` | codex-cli 0.166.0 |
| Antigravity CLI | `/opt/homebrew/bin/agy` | 1.5.0 |
| Antigravity through T3 Code | T3's own ACP runtime (`antigravity-acp`), not `agy` | not reported by the binary |

Codex through T3 Code ran the same version (its rollout records `originator: "T3 Code"`, `cli_version: "0.166.0"`). The Claude Code version inside T3 Code was not checked.

## Summary

| # | Experiment | Result | Decision |
|---|---|---|---|
| 1a | Antigravity loads a symlinked skill folder in `~/.gemini/config/skills` | pass | Antigravity skills: `link` |
| 1b | Antigravity loads a symlinked `~/.gemini/GEMINI.md` | pass | Antigravity instruction file: `link` |
| 1c | Exact value of Antigravity's 24 KB limit | measured: 24000 bytes | instruction size limit `24000` bytes per file; a file of 24000 bytes loads whole, 24001 bytes is truncated |
| 2a | Claude Code CLI loads a symlinked `~/.claude/CLAUDE.md` | pass | Claude Code instruction file: `link` |
| 2b | Claude Code through T3 Code loads a symlinked `~/.claude/CLAUDE.md` | pass | (same) |
| 2c | Codex loads a symlinked `~/.codex/AGENTS.md` | pass | Codex instruction file: `link` |
| 3a | Claude Code and Codex with a symlink inside a skill folder | recorded (see below) | no change; skiman already forbids symlinks inside skills |
| 3b | Codex loads a symlinked skill folder from `~/.agents/skills` | pass | Codex skills: `link` |
| 3c | Codex still reads `~/.codex/skills` | recorded: yes | outside v1 (no second Codex directory) |
| 4 | Codex `~/.codex/hooks.json` SessionStart fires on startup and resume, including from T3 Code | **unconfirmed** | Codex session refresh is not available until the user trusts the entry in Codex `/hooks`; trusted behaviour unconfirmed. Acts: #9 |
| 5 | Claude Code SessionStart matcher `startup|resume` excludes `clear` and `compact` | pass | Claude Code session-start hook: supported with matcher `startup|resume` |

Session-start support per agent tool: Claude Code yes; Codex fires on startup and resume only for a trusted entry (an untrusted entry never runs), trusted case unconfirmed; Antigravity none (no hook mechanism, as the spec already assumes).

## Method

- Test names used the prefix `skiman-p1-154c4a-` (random suffix `154c4a`). Before creating anything, the names were checked to be free in `~/.claude/skills`, `~/.agents/skills`, `~/.codex/skills`, `~/.claude/settings.json`, `~/.codex/hooks.json` and `~/.claude/CLAUDE.md`.
- Instruction locations: each one under test was atomically replaced (symlink to a temporary name, then rename) by a symlink to a file holding the user's full original content plus one marker line with its own random token (`CLAUDEMD-3b387932`, `CODEXMD-b478a68f`, `GEMINIMD-…`). Only the location under test was changed; each window lasted 9 to 46 seconds and was restored the same way, after checking the marker file was unchanged (nobody edited it meanwhile).
- Subjects: fresh sessions with `claude -p`, `codex exec`, `agy -p`, and throwaway T3 Code sessions (Claude Sonnet 5.5, Codex `gpt-6.1-terra` with low effort, Antigravity Gemini 3.8 Flash). Each was asked, without tools, to quote any line starting with `skiman phase-1 marker`, the last line of its global instructions, and the verification token of every `skiman-p1-` skill. The tokens are random and appear only in the test files, so a correct token proves the content reached the session. For Codex the session rollout file was also searched for each token; for Antigravity the CLI log was read.
- Hook markers: a command that appends the hook's stdin JSON (session id, `source`, transcript path, cwd) to a marker file outside the tool directories and prints nothing: `{ cat; echo; } >> <scratch folder>/markers/<kind>.jsonl 2>/dev/null; exit 0 # skiman-p1-154c4a <kind> marker`. Records were attributed by session id, so sessions started by other agents on the machine did not count.

## Experiment 1: Antigravity

Test entries: `~/.agents/skills/skiman-p1-154c4a-linked` (symlink to a skill folder kept in a scratch folder outside the tool directories), plus the two experiment 3 skills. `~/.gemini/config/skills` is itself a symlink to `~/.agents/skills` on this machine, so these entries were inside Antigravity's tool directory.

| Run | `GEMINI.md` size | Marker seen | Last original line seen | `agy` log |
|---|---|---|---|---|
| `agy -p` | 24000 bytes (original 23700 + marker line) | yes, complete | yes | no truncation line |
| `agy -p` | 24001 bytes | yes (inferred: the first 24000 bytes are everything but the final newline) | yes | `rules.go:442] Global rule truncated from 24001 characters to 24020 characters` |
| `agy -p` | 24576 bytes | no | no | `Global rule truncated from 24576 characters to 24021 characters` |
| `agy -p` | 24577 bytes | no | no | `Global rule truncated from 24577 characters to 24021 characters` |

- Skills: every run listed `skiman-p1-154c4a-linked=LINKED-0248495c` (symlinked folder), `-inner` and `-symmd`.
- Symlinked `GEMINI.md`: the 24000-byte run quoted the marker token `GEMINI24000-669abb22`. Pass.
- Limit: the files held about 9,800 to 10,600 characters (the user's content is mostly Chinese), yet the log reports the byte length, so the counted unit is bytes. Files up to 24000 bytes load without truncation; from 24001 bytes on, Antigravity truncates and appends its own notice (the injected text is 24020 to 24021 bytes). The binary's built-in documentation says: "Each rule file is capped at 24,000 bytes … and truncated on line boundaries when over the cap." Decision for #10: limit `24000` bytes, inclusive.
- Same built-in documentation, not tested here: "Always-on and global rules share a dedicated 20,000-token rules budget … Over-budget rules are demoted from full inline text to file path pointers." Ticket #10 must know this budget exists (see Open items).

Observation, outside the criterion: an Antigravity session started through T3 Code (T3's `antigravity-acp` runtime) listed the test skills (including the symlinked one) but saw no global instructions at all (`MARKER: NONE`, `LASTLINE: NONE`) while `GEMINI.md` held the marker. T3's Antigravity runtime does not load `~/.gemini/GEMINI.md`, whether linked or copied; skiman cannot change that.

## Experiment 2: global instruction files

| Subject | Result |
|---|---|
| `claude -p` | quoted `… token CLAUDEMD-3b387932 …` and the last original line. Pass |
| Claude Code through T3 Code | quoted `… token CLAUDEMD-3b387932 …`. Pass |
| `codex exec` | quoted `… token CODEXMD-b478a68f …`; the token appears in the rollout's injected context. Pass |

`~/.codex/AGENTS.md` and `~/.gemini/GEMINI.md` were already symlinks to `~/.claude/CLAUDE.md` before the run; the experiment pointed each one at its own marker file instead and then restored the original target.

## Experiment 3: symlinks inside skills, Codex skill directories

Test entries, created in `~/.agents/skills` first and then in `~/.claude/skills`:

- `skiman-p1-154c4a-inner`: real folder, regular `SKILL.md`, plus `extra.md` that is a symlink to a file outside the skill.
- `skiman-p1-154c4a-symmd`: real folder whose `SKILL.md` is a symlink to a file outside the skill.
- `skiman-p1-154c4a-linked`: the folder itself is a symlink (also linked into `~/.claude/skills`).
- `~/.codex/skills/skiman-p1-154c4a-legacy`: real folder.

| Skill | Claude Code (CLI and T3) | Codex `exec` | Antigravity `agy` |
|---|---|---|---|
| `-inner` (symlink inside) | listed | listed | listed |
| `-symmd` (`SKILL.md` is a symlink) | listed | **not listed** (token absent from the rollout) | listed |
| `-linked` (folder is a symlink) | listed | listed | listed |
| `-legacy` in `~/.codex/skills` | n/a | listed | n/a |

- 3a, recorded: none of the three tools rejects a skill because a file inside it is a symlink. Codex ignores a skill whose `SKILL.md` is a symlink; Claude Code and Antigravity load it. Whether a tool's skill loader follows `extra.md` was not observable (the agents read skill files with their general file tools, which follow symlinks). skiman forbids symlinks inside skills, so no decision changes.
- 3b, pass: Codex loads a symlinked skill folder from `~/.agents/skills`.
- 3c, recorded: Codex 0.166.0 still reads `~/.codex/skills`.
- Also seen: Claude Code loads a symlinked skill folder from `~/.claude/skills`, and a running Claude Code session picked up the new test skills without a restart.

## Experiment 4: Codex SessionStart (unconfirmed)

The marker entry (no matcher) was added to `~/.codex/hooks.json` for 04:18 to 04:19.

| Run | Marker record |
|---|---|
| `codex exec` (startup), entry untrusted | none |
| `codex exec --dangerously-bypass-hook-trust` (startup) | `source: "startup"` |
| `codex exec resume <id> --dangerously-bypass-hook-trust` | `source: "resume"` |
| `codex exec resume <id>`, entry untrusted | none |
| Codex through T3 Code (`originator: "T3 Code"`), entry untrusted | none |

- Codex runs a hook only after the user trusts it in `/hooks` ([Codex hooks docs](https://developers.openai.com/codex/hooks.md); the binary's strings show the trust record as `hooks.state."…".trusted_hash` in `~/.codex/config.toml`, and this machine's `config.toml` has no `hooks.state` entry). The untrusted entry never ran, on the CLI or through T3 Code. The rollouts confirm that no hook ran at all in the untrusted sessions (no context item with `"hook":true`), while the bypass session has two (from other SessionStart entries already configured).
- With trust bypassed for one invocation (the flag changes no stored state), SessionStart fires on startup and on resume, with `source` set accordingly.
- Not done, by instruction: trusting the entry on the user's behalf. So the criterion "a marker command runs" is not met for an entry as skiman would install it, and the trusted case (CLI and T3 Code) is unconfirmed. Resume through T3 Code was not attempted: with an untrusted entry it cannot show anything.
- Codex `fork` source: not observed (`codex fork` is interactive only; not part of the criterion).
- How to confirm: add a marker entry to `~/.codex/hooks.json`, open interactive `codex`, run `/hooks` and trust the entry, then start `codex exec`, `codex exec resume <id>` and a Codex session in T3 Code, and check the marker file for `startup` and `resume` records with those session ids. Afterwards remove the entry and its trust record.
- Affected user stories: 10 and 54 (session refresh for Codex), and 4 (`init` adds session-start entries: the user must also trust the Codex entry in `/hooks` before it runs). Ticket that acts: #9.

## Experiment 5: Claude Code SessionStart matcher (pass)

One marker entry in `~/.claude/settings.json` `hooks.SessionStart`, first without a matcher as a control (about 04:12 to 04:15), then with matcher `startup|resume` (about 04:15 to 04:18). Actions per phase, all in a fresh folder: `claude -p` (startup), `claude -p --resume <id>`, `claude -p --resume <id> --fork-session`, `claude -p --resume <id> "/compact"`, `claude -p --resume <id> "/clear"`.

| Action | Control (no matcher): `source` recorded | Matcher `startup|resume` |
|---|---|---|
| `claude -p` | `startup` | `startup` |
| `--resume` | `resume` | `resume` |
| `--resume --fork-session` | `fork` (new session id, no `resume` record) | nothing |
| `--resume "/compact"` | `resume`, then `compact` | `resume` only |
| `--resume "/clear"` | `clear` (new session id) | nothing |

- Interactive check through a pseudo-terminal (`tui-clear.py`): interactive `claude` started (`startup` recorded), one prompt, `/compact` (screen showed "Compacted"), `/clear` (executed), `/exit`. With matcher `startup|resume`, only the `startup` record appeared.
- Result: the matcher excludes `clear` and `compact`. Pass.
- Also recorded: the matcher also excludes `fork`. A session forked with `--fork-session` gets no skiman refresh. Ticket #9 decides whether that matters for story 10 ("a new or resumed agent session").

## Open items for later tickets

| Item | Status | Affected user stories | Ticket |
|---|---|---|---|
| Codex SessionStart for a trusted entry, on startup, resume and through T3 Code (experiment 4) | unconfirmed; an untrusted entry never runs | 10, 54, 4 | #9 |
| Claude Code matcher `startup|resume` does not cover `fork` | recorded | 10, 54 | #9 |
| Antigravity 20,000-token budget for always-on and global rules (from the binary's own docs; may demote `GEMINI.md` to a path pointer) | unconfirmed, not tested | 32, 50 | #10 |
| Antigravity through T3 Code does not load `~/.gemini/GEMINI.md` | recorded (T3 runtime behaviour, outside skiman) | 32, 50 | #10 |
