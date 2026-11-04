---
name: managing-skills
description: How to create, install, edit, update, remove and rename agent skills and the global instructions with skiman. Use whenever you are asked to change skills or global instructions.
---

# Managing skills

The skill repo is `{{SKILL_REPO}}`. It holds the canonical copy of every skill
(`skills/<name>/`) and of the shared body of the global instructions
(`instructions/shared.md`). Tool directories only hold links to it.

- Create a skill with `skiman new <name> -d "<description>"`, then write its body
  in `{{SKILL_REPO}}/skills/<name>/SKILL.md`.
- Edit skills and the shared body in the skill repo, never in a tool directory.
- Never create folders in a tool directory.
- Pass flags so that no command prompts; exit code 2 means a flag is missing.
