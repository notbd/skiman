//! `skiman overview`: every skill on this machine as Markdown, the canon's
//! by group, then the skills other managers keep. Read-only.

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

/// Commits everything in the canon as made on `day`.
fn commit_on(sandbox: &Sandbox, day: &str) {
    let repo = sandbox.canon();
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
fn the_canon_comes_by_group_and_other_managers_skills_are_listed_once() {
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
        sandbox.canon().join("skiman.toml"),
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
        r#"{"name": "kit", "skills": ["./skills/eng/tdd", "../outside", "./extra", 5]}"#,
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

## In the canon (3)

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
| skiman-manual | How to change skills and global instructions with skiman | built-in | skiman [..] | - | yes | - |

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

- An installed day marked (git) is the day of the latest commit that added the skill's SKILL.md to the canon, which can be later than the real install; `installed = YYYY-MM-DD` in its [skills.<name>] table records the real day.
- Claude Code plugin kit@market 1.0: `skills` in ~/.claude/plugins/cache/market/kit/1.0/.claude-plugin/plugin.json lists `5`, which is not a path.
- Claude Code plugin kit@market 1.0: the skills path `../outside` does not exist.
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
    let cache = sandbox.home().join(".claude/plugins/cache/market");
    let plugin = cache.join("kit/1.0");
    write(plugin.join("skills/tdd/SKILL.md"), skill_md("tdd"));
    // A plugin that is one skill: SKILL.md at its root, nothing listed.
    let solo = cache.join("solo/2.0");
    write(solo.join("SKILL.md"), skill_md("solo"));
    write(
        &registry,
        format!(
            r#"{{"version": 2, "plugins": {{"bad@market": {{}}, "kit@market": [{{"installPath": "{}", "scope": "project", "projectPath": "{}"}}], "solo@market": [{{"installPath": "{}", "scope": "project", "projectPath": ""}}]}}}}"#,
            plugin.display(),
            sandbox.home().join("work/app").display(),
            solo.display()
        ),
    );
    write(
        sandbox.home().join(".claude/settings.json"),
        r#"{"enabledPlugins": {"kit@market": false}}"#,
    );
    let stdout = overview();
    assert!(
        stdout.contains("The entry of plugin bad@market in ~/.claude/plugins/installed_plugins.json is not in a format")
            && stdout.contains(
                "| solo | Claude Code plugin solo@market (project scope) | ~/.claude/plugins/cache/market/solo/2.0 | claude-code |"
            )
            && stdout.contains(
                "| tdd | Claude Code plugin kit@market (project scope: ~/work/app) (disabled) | ~/.claude/plugins/cache/market/kit/1.0/skills/tdd | claude-code |"
            ),
        "{stdout}"
    );

    // Settings that cannot be read turn no plugin off, and say so.
    write(
        sandbox.home().join(".claude/settings.json"),
        r#"{"enabledPlugins": {"kit@market": false"#,
    );
    let stdout = overview();
    assert!(
        stdout.contains(
            "- Cannot tell which plugins ~/.claude/settings.json turns off: it is not valid JSON: "
        ) && stdout.contains("; no plugin is marked (disabled).\n")
            && stdout
                .contains("| tdd | Claude Code plugin kit@market (project scope: ~/work/app) | "),
        "{stdout}"
    );
    // So do settings that are JSON but not an object.
    write(sandbox.home().join(".claude/settings.json"), "[]");
    let stdout = overview();
    assert!(
        stdout.contains(
            "- Cannot tell which plugins ~/.claude/settings.json turns off: it is not a JSON object; no plugin is marked (disabled).\n"
        ),
        "{stdout}"
    );
}

/// A plugin installed outside `~/.claude/plugins` is not one sync leaves
/// to Claude Code: a link to its skill is one skiman did not make, in the
/// overview as in sync, and the plugin is a note.
#[test]
fn a_plugin_outside_the_plugins_folder_is_shown_as_sync_treats_it() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    let plugin = home.join("local-plugin");
    write(
        plugin.join(".claude-plugin/plugin.json"),
        r#"{"name": "local"}"#,
    );
    write(plugin.join("skills/notes/SKILL.md"), skill_md("notes"));
    // Registered through a link: the note names where it really is.
    let registered = home.join("plugin-link");
    symlink(&plugin, &registered).unwrap();
    // Turned off: Claude Code does not read its skills.
    let off = home.join("off-plugin");
    write(off.join("skills/old/SKILL.md"), skill_md("old"));
    write(
        home.join(".claude/plugins/installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"local@fixture": [{{"installPath": "{}"}}], "off@fixture": [{{"installPath": "{}"}}]}}}}"#,
            registered.display(),
            off.display()
        ),
    );
    write(
        home.join(".claude/settings.json"),
        r#"{"enabledPlugins": {"off@fixture": false}}"#,
    );
    let link = home.join(".agents/skills/notes");
    symlink(plugin.join("skills/notes"), &link).unwrap();

    let output = sandbox.skiman().arg("overview").assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains(
            "| notes | unknown: a link skiman did not make | ~/local-plugin/skills/notes | codex |"
        ) && stdout.contains(
            "- Claude Code plugin local@fixture is installed at ~/local-plugin, outside the folders skiman leaves to other managers; its skills are not listed, and Claude Code still reads them."
        ) && stdout.contains(
            "- Claude Code plugin off@fixture (disabled) is installed at ~/off-plugin, outside the folders skiman leaves to other managers; its skills are not listed.\n"
        ),
        "{stdout}"
    );
    let sync = sandbox.skiman().arg("sync").assert().code(0);
    let stderr = String::from_utf8(sync.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains(&format!("foreign skill {} is not managed", link.display())),
        "{stderr}"
    );
}

/// A plugin cache moved to another disk and linked back is still Claude
/// Code's: its plugins are registered in the registry's folder, and their
/// skills are listed where they really are.
#[test]
fn a_plugin_in_a_cache_linked_elsewhere_is_listed() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    let disk = home.join("bigdisk/claude-cache");
    write(
        disk.join("market/kit/1.0/skills/tdd/SKILL.md"),
        skill_md("tdd"),
    );
    let cache = home.join(".claude/plugins/cache");
    std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
    symlink(&disk, &cache).unwrap();
    write(
        home.join(".claude/plugins/installed_plugins.json"),
        format!(
            r#"{{"version": 2, "plugins": {{"kit@market": [{{"installPath": "{}"}}]}}}}"#,
            cache.join("market/kit/1.0").display()
        ),
    );

    let output = sandbox.skiman().arg("overview").assert().code(0);
    let stdout = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains(
            "| tdd | Claude Code plugin kit@market | ~/bigdisk/claude-cache/market/kit/1.0/skills/tdd | claude-code |"
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
    let repo = sandbox.canon();
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
