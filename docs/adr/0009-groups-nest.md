# Groups nest

A group folder may hold group folders, to any depth: `skills/third-party/tstack/<name>` is a skill in the group `third-party/tstack`. A grouping by source and then by purpose needs two levels, and one level forces a choice between them. Under `skills/` and under every group folder, a folder without a `SKILL.md` that holds only folders is a group folder; anything else is a skill folder, so nothing inside a skill is ever read as a group. Each group folder's name follows the skill-name rule and is no skill's name, at any depth, so the name of a group folder is taken like a skill's. `--group` takes the folders joined with `/`. Records still never name a group, and tool directories stay flat, so nothing outside `skills/` changes with the depth: a link to `skills/<x>/<name>` is skiman's when no folder of `<x>` holds a `SKILL.md`, and a move out of a group removes each group folder it leaves empty.

## Considered Options

- **Stay at one level**: names like `tstack` and `writing` side by side mix two ways of grouping in one list.
- **Cap the depth at two**: the reading and the checks are the same code at any depth, so a cap adds a rule without making anything simpler.
- **Group names unique across the tree**: records never name a group, so two `tools` folders in different groups confuse nothing skiman reads.
