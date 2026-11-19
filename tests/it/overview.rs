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
    sandbox
        .skiman()
        .args(["new", "notes", "-d", "Take notes", "--group", "writing"])
        .assert()
        .code(0);
    sandbox
        .skiman()
        .args(["new", "todo", "-d", "Track todos"])
        .assert()
        .code(0);
    let up = sandbox.upstream("acme/pdf");
    up.write("SKILL.md", skill_md("pdf"));
    up.commit("v1");
    sandbox
        .skiman()
        .args(["add", "acme/pdf", "--yes", "--group", "documents"])
        .assert()
        .code(0);
    append(
        sandbox.skill_repo().join("skiman.toml"),
        "\n[skills.notes]\nsummary = \"Keep notes quickly\"\n\
         origin = \"adapted from a blog post\"\ninstalled = 2026-01-31\n\
         \n[skills.pdf]\nsummary = \"Fill PDF forms\"\nenabled = false\n",
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
    write(plugin.join("skills/base/SKILL.md"), skill_md("base"));
    write(home.join("outside/escape/SKILL.md"), skill_md("escape"));
    symlink(home.join("outside/escape"), plugin.join("extra")).unwrap();
    write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name": "kit", "skills": ["./skills/eng/tdd", "../outside", "./extra"]}"#,
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

## Not managed by skiman (6)

skiman lists these here and never changes them.

| Skill | Managed by | Location | Read by |
| --- | --- | --- | --- |
| base | Claude Code plugin kit@market 1.0 | ~/.claude/plugins/cache/market/kit/1.0/skills/base | claude-code |
| tdd | Claude Code plugin kit@market 1.0 | ~/.claude/plugins/cache/market/kit/1.0/skills/eng/tdd | claude-code, codex |
| imagegen | Codex, built in | ~/.codex/skills/.system/imagegen | codex |
| docx | claude.ai sync | ~/.claude/skills/synced/account-1/docx | claude-code |
| copied | unknown: a folder skiman did not make | ~/.gemini/config/skills/copied | antigravity |
| team | unknown: a link skiman did not make | ~/work/team | claude-code, codex |

## Notes

- An installed day marked (git) is the day of the latest commit that added the skill's SKILL.md to the skill repo, which can be later than the real install; `installed = YYYY-MM-DD` in its [skills.<name>] table records the real day.
- Claude Code plugin kit@market 1.0: the skills path `./extra` leads outside the plugin.
- `claude-code` also reads built-in skills that are not stored on disk; they are not listed.
- `codex` also reads the skills of its plugins in `~/.codex/plugins`; they are not listed.
- `antigravity` also reads the skills in `~/.gemini/skills`; they are not listed.
- `antigravity` also reads the skills in `~/.gemini/antigravity-cli/skills`; they are not listed.

"#]])
        .stderr_eq("");
    assert_eq!(tree(sandbox.root()), before, "overview changed a file");
}

#[test]
fn a_registry_in_an_unknown_format_is_a_note_not_a_failure() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let registry = sandbox
        .home()
        .join(".claude/plugins/installed_plugins.json");
    let overview = || {
        let output = sandbox.skiman().arg("overview").assert().code(0);
        String::from_utf8(output.get_output().stdout.clone()).unwrap()
    };
    write(&registry, "[1, 2]");
    let stdout = overview();
    assert!(
        stdout.contains("installed_plugins.json is not in a format skiman reads; its plugins' skills are not listed."),
        "{stdout}"
    );

    // An unreadable entry is named; the others are still listed.
    let plugin = sandbox.home().join("kit");
    write(plugin.join("skills/tdd/SKILL.md"), skill_md("tdd"));
    // A plugin that is one skill: SKILL.md at its root, nothing listed.
    let solo = sandbox.home().join("solo/2.0");
    write(solo.join("SKILL.md"), skill_md("solo"));
    write(
        &registry,
        format!(
            r#"{{"version": 2, "plugins": {{"bad@market": {{}}, "kit@market": [{{"installPath": "{}", "scope": "project"}}], "solo@market": [{{"installPath": "{}"}}]}}}}"#,
            plugin.display(),
            solo.display()
        ),
    );
    write(
        sandbox.home().join(".claude/settings.json"),
        r#"{"enabledPlugins": {"kit@market": false}}"#,
    );
    let stdout = overview();
    assert!(
        stdout.contains("The entry of plugin bad@market in ")
            && stdout.contains("| solo | Claude Code plugin solo@market | ~/solo/2.0 | claude-code |")
            && stdout.contains(
                "| tdd | Claude Code plugin kit@market (project scope) (disabled) | ~/kit/skills/tdd | claude-code |"
            ),
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

#[test]
fn a_skill_adapted_from_another_shows_the_day_it_was_added() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let body: String = (1..=40)
        .map(|i| format!("Step {i} of the routine.\n"))
        .collect();
    let repo = sandbox.skill_repo();
    write(
        repo.join("skills/alpha/SKILL.md"),
        format!("---\nname: alpha\ndescription: d\n---\n\n{body}"),
    );
    commit_on(&sandbox, "2026-01-05");
    write(
        repo.join("skills/zeta/SKILL.md"),
        format!("---\nname: zeta\ndescription: d\n---\n\n{body}One more step.\n"),
    );
    commit_on(&sandbox, "2026-06-20");

    let output = sandbox.skiman().arg("overview").assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("| zeta | - | own | - | 2026-06-20 (git) | yes | - |")
            && stdout.contains("| alpha | - | own | - | 2026-01-05 (git) | yes | - |"),
        "{stdout}"
    );
}
