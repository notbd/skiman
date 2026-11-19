# skiman

## Agent skills

### Issue tracker

Issues and specs live in this repo's GitHub Issues, managed with the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

Default five-role vocabulary (`needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`). See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.

## Tests

- Run one test module: `cargo test --test it import::`. The filter matches any part of a test's path, so `add::` also runs `project_add::`.
- Rewrite the two rendered snapshots after an intended change: `SNAPSHOTS=overwrite cargo test --test it -- import_guide:: skiman_manual::`.
