//! Sync ownership (spec Ownership): sync creates, retargets and removes only
//! entries it owns, leaves everything else alone and reports it once.

use std::path::{Path, PathBuf};

use snapbox::str;

use crate::support::{Sandbox, Tool, assert_symlink, init, tree};

fn write_skill(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir.join(name)).unwrap();
    std::fs::write(
        dir.join(name).join("SKILL.md"),
        format!("---\nname: {name}\ndescription: d\n---\n"),
    )
    .unwrap();
}

fn own_skill(sandbox: &Sandbox, name: &str) -> PathBuf {
    write_skill(&sandbox.skill_repo().join("skills"), name);
    sandbox.skill_repo().join("skills").join(name)
}

fn sync_hook_stdout(sandbox: &Sandbox) -> String {
    let output = sandbox.skiman().args(["sync", "--hook"]).output().unwrap();
    assert_eq!(output.status.code(), Some(0), "sync --hook always exits 0");
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn a_foreign_skill_stays_untouched_and_sync_hook_reports_it_once_until_it_reappears() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&claude, "theirs");
    std::fs::write(claude.join("theirs/notes.txt"), "keep me").unwrap();
    let before = tree(claude.join("theirs"));

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: foreign skill [..]/home/.claude/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from claude-code`

"#]])
        .stderr_eq("");
    assert_eq!(tree(claude.join("theirs")), before);

    assert_eq!(sync_hook_stdout(&sandbox), "");
    assert_eq!(tree(claude.join("theirs")), before);

    let aside = sandbox.root().join("aside");
    std::fs::rename(claude.join("theirs"), &aside).unwrap();
    assert_eq!(sync_hook_stdout(&sandbox), "");
    std::fs::rename(&aside, claude.join("theirs")).unwrap();
    assert!(
        sync_hook_stdout(&sandbox).contains("foreign skill"),
        "a foreign skill that disappears and reappears is new again"
    );
}

#[test]
fn sync_without_hook_reports_every_foreign_skill_on_stderr_every_time_and_exits_0() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write_skill(&sandbox.tool_dir(Tool::Codex), "theirs");

    for _ in 0..2 {
        sandbox
            .skiman()
            .arg("sync")
            .assert()
            .code(0)
            .stdout_eq("")
            .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.agents/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from codex`

"#]]);
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn a_foreign_skill_with_the_name_of_a_repo_skill_is_not_replaced_and_sync_exits_3() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&claude, "pdf");
    let before = tree(&claude);
    let target = own_skill(&sandbox, "pdf");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .stdout_eq(str![[r#"
linked [..]/home/.agents/skills/pdf
linked [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq(str![[r#"
warning: [..]/home/.claude/skills/pdf is occupied by an entry skiman does not own, so skill pdf is not linked there; move it away, then run `skiman sync`

"#]]);

    assert_eq!(tree(&claude), before);
    assert_symlink(sandbox.tool_dir(Tool::Codex).join("pdf"), &target, &target);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn a_regular_file_and_a_folder_without_skill_md_at_a_wanted_name_are_occupied_paths() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let codex = sandbox.tool_dir(Tool::Codex);
    std::fs::write(codex.join("notes"), "a file").unwrap();
    std::fs::create_dir(codex.join("pdf")).unwrap();
    std::fs::write(codex.join("pdf/readme.txt"), "no SKILL.md").unwrap();
    let before = tree(&codex);
    own_skill(&sandbox, "notes");
    own_skill(&sandbox, "pdf");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(3)
        .stderr_eq(str![[r#"
warning: [..]/home/.agents/skills/notes is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`
warning: [..]/home/.agents/skills/pdf is occupied by an entry skiman does not own, so skill pdf is not linked there; move it away, then run `skiman sync`

"#]]);
    assert_eq!(tree(&codex), before);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn sync_hook_exits_0_on_occupied_paths_and_reports_each_one_once() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    std::fs::write(sandbox.tool_dir(Tool::Codex).join("notes"), "a file").unwrap();
    own_skill(&sandbox, "notes");

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: [..]/home/.agents/skills/notes is occupied by an entry skiman does not own, so skill notes is not linked there; move it away, then run `skiman sync`

"#]]);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn links_into_another_tool_directory_and_copies_with_identical_content_are_foreign() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let codex = sandbox.tool_dir(Tool::Codex);
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    write_skill(&claude, "theirs");
    std::os::unix::fs::symlink(claude.join("theirs"), codex.join("theirs")).unwrap();
    std::os::unix::fs::symlink(claude.join("pdf"), codex.join("via-claude")).unwrap();
    std::fs::create_dir(codex.join("pdf-copy")).unwrap();
    std::fs::copy(target.join("SKILL.md"), codex.join("pdf-copy/SKILL.md")).unwrap();
    let before = tree(&codex);

    let output = sandbox.skiman().arg("sync").output().unwrap();
    assert_eq!(output.status.code(), Some(0));
    let stderr = String::from_utf8(output.stderr).unwrap();
    for name in ["pdf-copy", "theirs", "via-claude"] {
        assert!(
            stderr.contains(&format!(
                "foreign skill {} is not managed",
                codex.join(name).display()
            )),
            "{name} is foreign: {stderr}"
        );
    }
    assert_eq!(tree(&codex), before);
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn managed_elsewhere_entries_stay_untouched_and_are_never_reported() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let home = sandbox.home();
    let claude = sandbox.tool_dir(Tool::ClaudeCode);
    let codex = sandbox.tool_dir(Tool::Codex);
    // Each one would be a foreign skill without the managed-elsewhere rule.
    write_skill(&claude, "synced");
    write_skill(&claude, ".trash");
    write_skill(&home.join(".codex/skills"), ".system");
    write_skill(
        &home.join(".claude/plugins/cache/acme/skills"),
        "claude-plugin-skill",
    );
    write_skill(
        &home.join(".codex/plugins/acme/skills"),
        "codex-plugin-skill",
    );
    std::os::unix::fs::symlink(
        home.join(".claude/plugins/cache/acme/skills/claude-plugin-skill"),
        codex.join("claude-plugin-skill"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        home.join(".codex/plugins/acme/skills/codex-plugin-skill"),
        claude.join("codex-plugin-skill"),
    )
    .unwrap();
    write_skill(&codex, "bundle");
    std::fs::create_dir(codex.join("bundle/.claude-plugin")).unwrap();
    std::fs::write(codex.join("bundle/.claude-plugin/plugin.json"), "{}").unwrap();
    let before = tree(&home);

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert_eq!(sync_hook_stdout(&sandbox), "");

    assert_eq!(tree(&home), before);
}

#[test]
fn after_a_hand_delete_sync_removes_the_dangling_owned_links() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    std::fs::remove_dir_all(&target).unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
removed [..]/home/.claude/skills/pdf
removed [..]/home/.agents/skills/pdf
removed [..]/home/.gemini/config/skills/pdf

"#]])
        .stderr_eq("");
    for tool in Tool::ALL {
        assert!(std::fs::symlink_metadata(sandbox.tool_dir(tool).join("pdf")).is_err());
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn after_a_hand_rename_of_an_own_skill_sync_removes_the_old_link_and_creates_the_new_one() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let old = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let new = sandbox.skill_repo().join("skills/pdf-tools");
    std::fs::rename(&old, &new).unwrap();
    std::fs::write(
        new.join("SKILL.md"),
        "---\nname: pdf-tools\ndescription: d\n---\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq("");

    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert!(std::fs::symlink_metadata(dir.join("pdf")).is_err());
        assert_symlink(dir.join("pdf-tools"), &new, &new);
    }
}

#[test]
fn enabled_false_removes_the_links_and_enabled_true_recreates_them() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let tools = std::fs::read_to_string(&manifest).unwrap();

    std::fs::write(
        &manifest,
        format!("{tools}\n[skills.pdf]\nenabled = false\n"),
    )
    .unwrap();
    sandbox.skiman().arg("sync").assert().code(0);
    for tool in Tool::ALL {
        assert!(std::fs::symlink_metadata(sandbox.tool_dir(tool).join("pdf")).is_err());
    }

    std::fs::write(
        &manifest,
        format!("{tools}\n[skills.pdf]\nenabled = true\n"),
    )
    .unwrap();
    sandbox.skiman().arg("sync").assert().code(0);
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("pdf"), &target, &target);
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn a_skill_whose_skill_md_has_broken_frontmatter_stays_linked() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let target = own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    std::fs::write(
        target.join("SKILL.md"),
        "---\nname: [unclosed\nno end marker\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    for tool in Tool::ALL {
        assert_symlink(sandbox.tool_dir(tool).join("pdf"), &target, &target);
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn folders_under_skills_that_are_not_distributable_are_not_linked_and_sync_hook_reports_them_once()
{
    let sandbox = Sandbox::new();
    init(&sandbox);
    let skills = sandbox.skill_repo().join("skills");
    write_skill(&skills, "Bad_Name");
    write_skill(&skills, "synced");
    std::fs::create_dir(skills.join("no-skill-md")).unwrap();
    std::fs::write(skills.join("no-skill-md/README.md"), "x").unwrap();
    // An empty folder is an empty group folder: nothing to report.
    std::fs::create_dir(skills.join("empty")).unwrap();
    std::fs::write(skills.join(".DS_Store"), "x").unwrap();

    sandbox
        .skiman()
        .args(["sync", "--hook"])
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
skiman: skill folder [..]/skill-repo/skills/Bad_Name is not distributed: its name is not a valid skill name
skiman: skill folder [..]/skill-repo/skills/no-skill-md is not distributed: it holds no SKILL.md
skiman: skill folder [..]/skill-repo/skills/synced is not distributed: its name is reserved

"#]])
        .stderr_eq("");
    assert_eq!(sync_hook_stdout(&sandbox), "");

    for tool in Tool::ALL {
        let names: Vec<_> = tree(sandbox.tool_dir(tool)).into_keys().collect();
        assert_eq!(names, [PathBuf::from("skiman-manual")]);
    }
}

#[test]
fn two_tool_directories_that_resolve_to_one_real_directory_are_reconciled_once() {
    let sandbox = Sandbox::new();
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_dir(&gemini).unwrap();
    std::os::unix::fs::symlink(sandbox.tool_dir(Tool::Codex), &gemini).unwrap();
    init(&sandbox);
    write_skill(&sandbox.tool_dir(Tool::Codex), "theirs");
    let target = own_skill(&sandbox, "pdf");

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
linked [..]/home/.claude/skills/pdf
linked [..]/home/.agents/skills/pdf

"#]])
        .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.agents/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from codex`

"#]]);
    assert_symlink(gemini.join("pdf"), &target, &target);
    let before = tree(sandbox.home());

    for _ in 0..2 {
        sandbox.skiman().arg("sync").assert().code(0).stdout_eq("");
        assert_eq!(sync_hook_stdout(&sandbox), "");
    }
    assert_eq!(tree(sandbox.home()), before);
}

#[test]
fn removing_a_tool_from_tools_enabled_removes_only_skimans_links_from_its_tool_directory() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    own_skill(&sandbox, "pdf");
    sandbox.skiman().arg("sync").assert().code(0);
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    write_skill(&gemini, "theirs");
    std::fs::write(gemini.join("loose.txt"), "x").unwrap();
    std::os::unix::fs::symlink("/nowhere", gemini.join("dangling")).unwrap();
    std::fs::write(
        sandbox.skill_repo().join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\"]\n",
    )
    .unwrap();

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
removed [..]/home/.gemini/config/skills/pdf
removed [..]/home/.gemini/config/skills/skiman-manual
removed [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq("");

    let names: Vec<_> = std::fs::read_dir(&gemini)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert_eq!(names, ["dangling", "loose.txt", "theirs"]);
    assert!(sandbox.tool_dir(Tool::Codex).join("pdf").exists());
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn init_pointed_at_a_different_skill_repo_retargets_or_removes_the_links_of_the_previous_one() {
    let sandbox = Sandbox::new();
    let first = sandbox.root().join("first-repo");
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&first)
        .assert()
        .code(0);
    for name in ["notes", "pdf"] {
        write_skill(&first.join("skills"), name);
    }
    sandbox.skiman().arg("sync").assert().code(0);
    let second = sandbox.skill_repo();
    std::fs::create_dir_all(second.join("skills")).unwrap();
    sandbox.git(&second, ["init", "--quiet"]);
    std::fs::write(
        second.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\", \"codex\", \"antigravity\"]\n",
    )
    .unwrap();
    write_skill(&second.join("skills"), "pdf");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&second)
        .assert()
        .code(0)
        .stdout_eq(str![[r#"
added .skiman/ to [..]/skill-repo/.gitignore
added /linked/ to [..]/skill-repo/.gitignore
installed git hook [..]/skill-repo/.git/hooks/pre-commit
installed git hook [..]/skill-repo/.git/hooks/post-commit
installed git hook [..]/skill-repo/.git/hooks/post-merge
installed git hook [..]/skill-repo/.git/hooks/post-checkout
installed git hook [..]/skill-repo/.git/hooks/post-rewrite
removed [..]/home/.claude/skills/notes
retargeted [..]/home/.claude/skills/pdf
retargeted [..]/home/.claude/skills/skiman-manual
removed [..]/home/.agents/skills/notes
retargeted [..]/home/.agents/skills/pdf
retargeted [..]/home/.agents/skills/skiman-manual
removed [..]/home/.gemini/config/skills/notes
retargeted [..]/home/.gemini/config/skills/pdf
retargeted [..]/home/.gemini/config/skills/skiman-manual
retargeted [..]/home/.claude/CLAUDE.md
retargeted [..]/home/.codex/AGENTS.md
retargeted [..]/home/.gemini/GEMINI.md

"#]])
        .stderr_eq("");

    let pdf = second.join("skills/pdf");
    let manual = second.join(".skiman/skiman-manual");
    for tool in Tool::ALL {
        let dir = sandbox.tool_dir(tool);
        assert_symlink(dir.join("pdf"), &pdf, &pdf);
        assert_symlink(dir.join("skiman-manual"), &manual, &manual);
        assert!(std::fs::symlink_metadata(dir.join("notes")).is_err());
    }
    assert_eq!(sync_hook_stdout(&sandbox), "");
}

#[test]
fn init_with_a_foreign_skill_points_to_import_when_the_skill_repo_has_no_skills() {
    let sandbox = Sandbox::new();
    write_skill(&sandbox.tool_dir(Tool::ClaudeCode), "theirs");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0)
        .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.claude/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from claude-code`
hint: the skill repo has no skills yet and the tool directories or instruction locations hold content skiman does not own; bring your existing setup in with `skiman import` (see `skiman import --guide`)

"#]]);
}

#[test]
fn init_with_a_foreign_skill_points_to_adopt_when_the_skill_repo_has_skills() {
    let sandbox = Sandbox::new();
    write_skill(&sandbox.tool_dir(Tool::ClaudeCode), "theirs");
    let repo = sandbox.skill_repo();
    std::fs::create_dir_all(repo.join("skills")).unwrap();
    sandbox.git(&repo, ["init", "--quiet"]);
    std::fs::write(
        repo.join("skiman.toml"),
        "[tools]\nenabled = [\"claude-code\"]\n",
    )
    .unwrap();
    write_skill(&repo.join("skills"), "pdf");

    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(&repo)
        .assert()
        .code(0)
        .stderr_eq(str![[r#"
warning: foreign skill [..]/home/.claude/skills/theirs is not managed by skiman; bring it in with `skiman adopt theirs --from claude-code`
hint: the tool directories hold foreign skills; bring each one into the skill repo with `skiman adopt <name> --from <tool id>`

"#]]);
}

#[test]
fn a_relative_link_to_the_skill_is_owned_and_resolves_against_the_real_tool_directory() {
    let sandbox = Sandbox::new();
    let codex = sandbox.tool_dir(Tool::Codex);
    let gemini = sandbox.tool_dir(Tool::Antigravity);
    std::fs::remove_dir(&codex).unwrap();
    std::os::unix::fs::symlink(&gemini, &codex).unwrap();
    init(&sandbox);
    own_skill(&sandbox, "pdf");
    // The kernel resolves it against `home/.gemini/config/skills`, not against
    // the alias `home/.agents/skills` that sync reaches the directory through.
    std::os::unix::fs::symlink(
        "../../../../skill-repo/skills/../skills/./pdf",
        codex.join("pdf"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "../../.claude/skills/../../../skill-repo/skills/pdf",
        sandbox.tool_dir(Tool::ClaudeCode).join("pdf"),
    )
    .unwrap();
    assert!(codex.join("pdf/SKILL.md").is_file());
    let before = tree(sandbox.home());

    sandbox
        .skiman()
        .arg("sync")
        .assert()
        .code(0)
        .stdout_eq("")
        .stderr_eq("");
    assert_eq!(tree(sandbox.home()), before);
}
