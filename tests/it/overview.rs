//! `skiman overview`: every skill on this machine as Markdown, the skill
//! repo's by group, then the skills other managers keep. Read-only.

use std::os::unix::fs::symlink;
use std::path::Path;

use snapbox::str;

use crate::support::{Sandbox, init, skill_md, tree};

fn write(path: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
    let path = path.as_ref();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn append(path: impl AsRef<Path>, text: &str) {
    let mut old = std::fs::read_to_string(&path).unwrap();
    old.push_str(text);
    std::fs::write(path, old).unwrap();
}

/// Commits everything in the skill repo as made on `day`.
fn commit_on(sandbox: &Sandbox, day: &str) {
    let repo = sandbox.skill_repo();
    sandbox.git(&repo, ["add", "--all"]);
    sandbox
        .command("git")
        .arg("-C")
        .arg(&repo)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.test"])
        .args(["commit", "--quiet", "-m", "skills"])
        .env("GIT_AUTHOR_DATE", format!("{day}T12:00:00"))
        .env("GIT_COMMITTER_DATE", format!("{day}T12:00:00"))
        .assert()
        .code(0);
}

#[test]
fn the_skill_repo_comes_by_group_and_other_managers_skills_are_listed_once() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    for (name, description) in [("notes", "Take notes"), ("todo", "Track todos")] {
        sandbox
            .skiman()
            .args(["new", name, "-d", description])
            .assert()
            .code(0);
    }
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"));
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes"])
        .assert()
        .code(0);
    append(
        sandbox.skill_repo().join("skiman.toml"),
        "\n[skills.notes]\nsummary = \"Keep notes quickly\"\ngroup = \"writing\"\n\
         origin = \"adapted from a blog post\"\ninstalled = 2026-01-31\n\
         \n[skills.pdf]\nsummary = \"Fill PDF forms\"\ngroup = \"documents\"\nenabled = false\n",
    );
    commit_on(&sandbox, "2026-02-03");

    // Other managers' skills, and what must not be listed.
    let home = sandbox.home();
    write(
        home.join(".claude/skills/synced/account-1/docx/SKILL.md"),
        skill_md("docx"),
    );
    write(
        home.join(".claude/skills/synced/account-1/manifest.json"),
        "{}",
    );
    write(
        home.join(".claude/skills/.trash/1/old/SKILL.md"),
        skill_md("old"),
    );
    write(
        home.join(".codex/skills/.system/imagegen/SKILL.md"),
        skill_md("imagegen"),
    );
    let plugin = home.join(".claude/plugins/cache/market/kit/1.0");
    write(plugin.join("skills/eng/tdd/SKILL.md"), skill_md("tdd"));
    write(
        plugin.join("skills/eng/unlisted/SKILL.md"),
        skill_md("unlisted"),
    );
    write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name": "kit", "skills": ["./skills/eng/tdd", "../outside"]}"#,
    );
    write(
        home.join(".claude/plugins/installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"kit@market": [{{"scope": "user", "installPath": "{}", "version": "1.0"}}]}}}}"#,
            plugin.display()
        ),
    );
    symlink(
        plugin.join("skills/eng/tdd"),
        home.join(".agents/skills/tdd"),
    )
    .unwrap();
    write(home.join("work/team/SKILL.md"), skill_md("team"));
    symlink(home.join("work/team"), home.join(".claude/skills/team")).unwrap();
    symlink(home.join("work/team"), home.join(".agents/skills/team")).unwrap();
    write(
        home.join(".gemini/config/skills/copied/SKILL.md"),
        skill_md("copied"),
    );

    let before = tree(sandbox.root());
    sandbox
        .skiman()
        .arg("overview")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
# Skills

## In the skill repo (3)

### documents (1)

| Skill | Summary | Kind | Source | Installed | Enabled | Modified |
| --- | --- | --- | --- | --- | --- | --- |
| pdf | Fill PDF forms | third-party | https://github.com/acme/pdf.git | 2026-02-03 (git) | no | no |

### writing (1)

| Skill | Summary | Kind | Source | Installed | Enabled | Modified |
| --- | --- | --- | --- | --- | --- | --- |
| notes | Keep notes quickly | own | adapted from a blog post | 2026-01-31 | yes | - |

### No group (1)

| Skill | Summary | Kind | Source | Installed | Enabled | Modified |
| --- | --- | --- | --- | --- | --- | --- |
| todo | - | own | - | 2026-02-03 (git) | yes | - |

## Built into skiman (1)

| Skill | Summary | Kind | Source | Installed | Enabled | Modified |
| --- | --- | --- | --- | --- | --- | --- |
| managing-skills | How to change skills and global instructions with skiman | built-in | skiman [..] | - | yes | - |

## Managed elsewhere (5)

skiman only lists these; it never changes them.

| Skill | Managed by | Location | Read by |
| --- | --- | --- | --- |
| tdd | Claude Code plugin kit@market 1.0 | ~/.claude/plugins/cache/market/kit/1.0/skills/eng/tdd | claude-code, codex |
| imagegen | Codex, built in | ~/.codex/skills/.system/imagegen | codex |
| docx | claude.ai sync | ~/.claude/skills/synced/account-1/docx | claude-code |
| copied | unknown: a folder skiman did not make | ~/.gemini/config/skills/copied | antigravity |
| team | unknown: a link skiman did not make | ~/work/team | claude-code, codex |

## Notes

- An installed day marked (git) is the day of the first commit that added the skill's SKILL.md to the skill repo, which can be later than the real install; `installed = YYYY-MM-DD` in its [skills.<name>] table records the real day.
- `claude-code` also has built-in skills that are not stored on disk, so they are not listed.

"#]])
        .stderr_eq("");
    assert_eq!(tree(sandbox.root()), before, "overview changed a file");
}

#[test]
fn a_registry_in_an_unknown_format_is_a_note_not_a_failure() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(
        sandbox
            .home()
            .join(".claude/plugins/installed_plugins.json"),
        "[1, 2]",
    );

    let output = sandbox.skiman().arg("overview").assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("installed_plugins.json is not in a format skiman reads; its plugins' skills are not listed."),
        "{stdout}"
    );
}

#[test]
fn a_renamed_skill_keeps_the_day_its_first_name_was_committed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes"])
        .assert()
        .code(0);
    commit_on(&sandbox, "2026-02-03");
    sandbox
        .skiman()
        .args(["mv", "notes", "journal"])
        .assert()
        .code(0);
    commit_on(&sandbox, "2026-03-04");

    let output = sandbox.skiman().arg("overview").assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("| journal | - | own | - | 2026-02-03 (git) | yes | - |"),
        "{stdout}"
    );
}
