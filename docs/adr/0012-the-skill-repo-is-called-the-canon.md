# The skill repo is called the canon

The git repo skiman works from was called the skill repo. It also holds the global instructions, the mirror of project items and skiman's configuration, and since [ADR 0010](0010-linked-folders-beside-skills.md) it registers skills it does not hold, so the name described one part of it. It is now the canon (正本库): the one authoritative record of a person's agent setup, from which everything in the tool directories is derived. ADRs 0001 to 0009, `docs/phase1-results.md` and the issues written before this one say "skill repo" and mean the same thing; they are records and are not rewritten.

## Considered Options

- **Keep "skill repo"**: no rename to carry out, but the name leaves out most of what the repo holds and suggests that every skill skiman distributes is in it.
- **Master repo**: "master" says which copy wins and nothing about the content, and it reads like the name of a branch.
- **Skill hub**: a hub is where things are shared or downloaded from, like a registry; this repo is one person's private record.
