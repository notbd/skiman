# No crash-recovery journal

skiman keeps no write-ahead journal. The skill repo is under git, so a half-finished change can be inspected and reverted there; everything in tool directories is derived from the skill repo and `skiman sync` can rebuild it at any time; single entries are written under a temporary name and renamed into place. The one thing that cannot be rebuilt, the originals taken over during import, is moved into a backup before its replacement is created. Spec v8's per-scope journal and recovery table (§8) solved a problem this design no longer has.
