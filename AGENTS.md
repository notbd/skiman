# skiman

## Agent skills

### Issue tracker

Issues and specs live in this repo's GitHub Issues, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.

## Working in this repo

- Each commit passes what CI runs: `cargo fmt --check` (on Linux), then `cargo clippy --all-targets --locked -- -D warnings` and `cargo test --locked` (on macOS and Linux).
- Never run skiman against your real home. Use a throwaway one, outside any git repository (its `.git` folder counts too): `(export HOME=$(mktemp -d) XDG_CONFIG_HOME= XDG_STATE_HOME=; ...)`. A canon inside another repo (a folder there that already holds `skiman.toml`) gets its git hooks installed into that repo's `.git/hooks`.
- Every agent tool is one row of `TOOLS` in `src/tools.rs`; code iterates over the table and never matches on a tool id.
- skiman never commits. `skiman sync` never spawns git, and `sync --hook` always exits 0; `scripts/perf_sync_hook.py` times it against a 100 ms target (not in CI).

## Tests

- Integration tests are black-box: each runs the built binary in a `Sandbox` (`tests/it/support/sandbox.rs`) with its own home. Name a test as the sentence it checks.
- `test_hook::at(scope, point)` marks a step of a write command where a test can act (`SKIMAN_TEST_HOOK`, `SKIMAN_TEST_HOOK_RUN`; debug builds only, `src/test_hook.rs`).
- Run one test module: `cargo test --test it import::`. The filter matches any part of a test's path, so `add::` also runs `project_add::`.
- Rewrite the two rendered snapshots after an intended change: `SNAPSHOTS=overwrite cargo test --test it -- import_guide:: skiman_manual::`.
