# A linked skill's summary is read from its own `SKILL.md`

ADR 0007 keeps a skill's summary in its `[skills.<name>]` table. A linked skill has no such table ([ADR 0010](0010-linked-folders-beside-skills.md)), and a summary copied into the canon would go stale when the skill is renamed or its folder moves, and would put what a company skill is about into the canon's history. So a linked skill's summary is `metadata.summary` in the frontmatter of its own `SKILL.md`, where it travels with the skill. Three steps decide what a person sees:

1. A linked skill that has `metadata.summary` shows it.
2. For one that has none, `skiman linked add` asks for it: the command names each such skill and prints the frontmatter lines to add, and does not wait. The user, or the agent that ran the command, writes them in the linked folder; skiman does not, because it never writes there.
3. A linked skill that still has none shows the first sentence of its frontmatter `description` as a stand-in. Wherever the stand-in is shown (`skiman overview` today), it is marked as one, so it never passes for a summary somebody wrote, and `skiman check` still warns that the skill has no summary.

A description is written to make an agent tool load the skill at the right moment and often does not say what the skill is for; that is why it is the last step and not the first. A linked folder that is not the user's to edit has no summaries and shows the stand-in. Summaries are written once, by a person or an agent, and kept: skiman calls no model.

`metadata` is the map the Agent Skills specification leaves for properties it does not define (string keys, string values). Claude Code and Codex loaded the test skills of ADR 0010, which carried `metadata.summary`, and two third-party skills in a canon in use carry a `metadata` map.

ADR 0007 rejected frontmatter keys because a third-party skill's `SKILL.md` is upstream content that every update has to merge. That reason does not reach a linked skill: skiman neither writes nor merges its `SKILL.md`, and whoever keeps the linked folder writes the key. Skills in the canon stay as ADR 0007 describes them.

## Considered Options

- **Summaries of linked skills in the `[linked.<id>]` table**: next to the other summaries, but a record per skill in the canon is what ADR 0010 avoids. It stays behind under the old name when the skill is renamed in its folder, and it puts company wording into the canon's history.
- **Only the first sentence of the description, no summary of its own**: nothing to write, but the sentence is trigger text and often wrong as a summary.
- **Leave the column empty when no summary was written**, as for a skill in the canon: the person learns nothing about exactly the skills nobody described. The marked stand-in says more and still shows that the summary is missing.
- **Write the summary when it is displayed**: skiman would have to call a model, and the text would differ from run to run.
