# Separate the tool repo from the skill repo

skiman's code lives in its own repo; each user's skills, global instruction files and skiman configuration live in a separate skill repo whose location skiman does not hard-code. Spec v8 put both in one `~/skiman` repo so the pre-commit checker always matched the content it checked, but that made skiman unusable by anyone else and mixed tool upgrades into the user's data history; universal import makes skiman a general tool, so the split wins.
