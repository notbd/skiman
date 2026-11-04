# Import existing setups through an LLM-prepared import folder

skiman's core job is managing, installing and updating skills and global instruction files reliably; it does not promise to understand every possible existing setup. For a complex setup, any LLM follows instructions shipped with skiman, reads the setup, and prepares an import folder laid out exactly like the skill repo should look afterwards; skiman checks it with the same rules it uses every day, previews the result, copies it into the skill repo and takes over the tool directories (originals go to a backup, not deleted). skiman never calls an LLM.

## Considered Options

- **Built-in recognisers in skiman** (spec v8's source detection and its general-purpose `skiman adopt`): every new setup shape needs a code change, and some shapes will always be missed. v1 keeps only a narrow `skiman adopt` that copies one named foreign skill from a tool directory into the skill repo; it recognises nothing on its own.
- **An action-list import plan** ("register this folder as third-party", "drop that copy"): a second format for skiman to define and validate, and the user has to imagine the result instead of seeing it.
- **Let the LLM move files directly**: no preview, not reproducible, hard to audit.
