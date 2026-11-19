# `skill_dirs` is retired; a folder of skills kept elsewhere is a linked folder

A project root's `skill_dirs` key listed folders of skills that are kept outside every project skill directory and reach the agent tools by other means, such as a folder of company skills linked into the tool directories by a script. skiman copied those skills into the mirror for their history, and treated the tool-directory links into them as managed elsewhere. Linked folders ([ADR 0010](0010-linked-folders-beside-skills.md)) serve the same folder better: skiman distributes the skills itself, keeps no second copy that is stale between syncs, and keeps their content out of the canon's history, which the folder's own home already keeps. Two ways to bring in the same folder, one copying and one linking, would leave each user to find out which one applies, so `skill_dirs` is removed rather than kept beside linked folders.

`skiman check` reports a `skill_dirs` key as a rule violation that names `skiman linked add` as the replacement, so a canon that still has one fails its pre-commit hook instead of silently changing behaviour. Mirror items that were copied from a `skill_dirs` folder lose their item shape and are reported by `check` as before (`docs/project-scope.md`, sync row 3); deleting `projects/<id>/<folder>` removes them. Links into such a folder that another tool made are foreign once the key is gone, until the folder is registered as a linked folder and its skills are linked by skiman; links that skiman does not own and that hold a wanted name are reported as occupied, as for any skill.

## Considered Options

- **Keep both**: no migration, but two answers to one question, and the rule that a folder may use only one of them.
- **Accept the key and ignore it**: an old canon would lose its mirror items without a word.
- **Turn `skill_dirs` entries into linked folders automatically**: it changes what the tools load behind the user's back, and a project-relative path is not the absolute path a linked folder records.
