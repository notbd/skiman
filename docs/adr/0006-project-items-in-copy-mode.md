# Project items stay real files; the mirror follows them in one direction

Project instruction files and project skills (an `AGENTS.md` in a project folder, `<project>/.claude/skills/<n>`) stay real files in the project, and the skill repo keeps a copy of each under `projects/<root id>/<same path>`. Sync copies project changes into the mirror automatically; the project is written only by `skiman project restore`, and every replace on either side moves what it replaces to a backup. Global locations keep links (the v1 design), because nobody edits `~/.claude/skills/<n>` in place; project folders are where people and agents edit, and the real agent tools do not cope with links there (`docs/project-scope-evidence.md`: Claude Code does not edit a symlinked `AGENTS.md`, resolves `@` imports against the link target, and labels the file with the target's path). Items a project's own git repo tracks are left to that repo, which already is their source of truth.

## Considered Options

- **Replace each project file with a link into the skill repo**, like global skills: one copy, but agents could no longer edit project instructions where they work, and relative imports broke.
- **Copy mode with automatic sync in both directions**: a routine `git restore .` or a conflicted `git pull` in the skill repo would overwrite project content that existed nowhere else at the next session start.
- **A per-item record of what is managed**: the mirror tree is the record already; the pairing `<root>/<rel>` ↔ `projects/<id>/<rel>` is path arithmetic, so there is nothing to keep in step.
