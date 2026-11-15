# What a person reads about a skill lives in skiman.toml

The `group` key is replaced by group folders ([ADR 0008](0008-groups-are-folders.md)).

A skill's summary, group, origin and install day are optional keys of its `[skills.<name>]` table in `skiman.toml`, next to `enabled` and `source`. `skiman overview` shows them, and `skiman check` on the working tree warns about each skill without a summary, a group or a record of where it came from; the warning never changes an exit status, and neither `check --staged` (the pre-commit hook), `sync --hook` nor the write commands print it, so a skill repo whose skills have none of them works as before. The install day falls back to the latest commit that added the skill's `SKILL.md`.

## Considered Options

- **Keys in each `SKILL.md` frontmatter**: a third-party skill's `SKILL.md` is upstream content, so every key added there is a local modification that each update has to merge, and the frontmatter is what agent tools load.
- **A separate file per skill or one `skills.md` table**: one more record to keep in step on `mv` and `remove`, which already carry the `[skills.<name>]` table.
- **Missing keys as a rule violation**: every existing skill repo would fail its pre-commit hook on the day it upgrades. A failing rule can come once the keys are filled in.
- **Install day only from git**: a skill repo's history starts at its import, and a rename that also changes the file is not followed, so the recorded day wins when there is one.
