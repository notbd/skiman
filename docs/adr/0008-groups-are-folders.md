# A skill's group is the folder it sits in

Groups nest since [ADR 0009](0009-groups-nest.md).

A skill's group is the folder it sits in under `skills/`: `skills/<group>/<name>` is in that group, `skills/<name>` in none. A folder directly under `skills/` without a `SKILL.md` that holds only folders is a group folder; groups do not nest. Nothing else records a group, so this replaces the `group` key of ADR 0007, and the folder tree shows the grouping at a glance. Tool directories stay flat: each skill is still linked as `<tool dir>/<name>`, the layout every agent tool documents, so a skill name is unique across groups. `skiman.toml`, `skiman.lock`, `upstream/<name>` and `[[secrets.allow]]` paths (`skills/<name>/<path>`) name a skill without its group, so moving a skill to another group renames one folder and changes no record; sync retargets its links. A skill directly under `skills/` stays valid, and `skiman check` reminds of it like a missing summary.

## Considered Options

- **Keep the `group` key and also check that it matches the folder**: the same fact in two places, one of which can go stale.
- **Keep `skills/` flat and generate a `groups/<group>/<name>` view of links**: a smaller change, but a derived view that is not where the skills are edited.
- **Group folders in the tool directories too**: whether an agent tool finds a skill below `<tool dir>/<name>` differs by tool and is not documented for all of them, and other managers' entries share those directories.
- **Paths with the group in exemptions and records**: every move to another group would rewrite them.
