# Write skiman in Go

skiman runs at the start of every agent session (through session-start hooks), so start-up time and a dependency-free install matter more than library convenience; Go gives a single binary that starts in milliseconds and installs with `brew` without depending on a Python or Node version. Spec v8 chose Python 3.12 for ruamel.yaml's format-preserving YAML edits, but skiman only needs to read frontmatter and at most rewrite the `name` line, which a targeted text edit handles.
