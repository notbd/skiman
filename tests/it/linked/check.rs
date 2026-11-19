//! `skiman check` in both modes on what the canon records about linked
//! folders: the `[linked.<id>]` tables, and nothing under `linked/` in the
//! git index.

use snapbox::str;

use super::append_manifest;
use crate::support::{Sandbox, init};

#[test]
fn a_linked_table_that_is_not_well_formed_breaks_the_rules_in_both_modes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    append_manifest(
        &sandbox,
        "\n[linked.managing-skills]\npath = \"/w/a\"\n\n[linked.Bad_Id]\npath = \"/w/b\"\n\n[linked.relative]\npath = \"w/c\"\n\n[linked.extra]\npath = \"~/w/d\"\nenabled = true\n\n[linked.empty]\n\n[linked]\nloose = \"x\"\n",
    );

    let expected = str![[r#"
skiman.toml:4: toml-value: `managing-skills` is a reserved name and cannot be a linked folder id
skiman.toml:7: toml-value: `Bad_Id` is not a valid linked folder id: use lowercase letters, digits and single hyphens
skiman.toml:11: toml-value: `path` in [linked.relative] must be a string: an absolute path or one starting with `~/`
skiman.toml:15: unknown-key: unknown key `enabled` in [linked.extra] (allowed: path)
skiman.toml:17: toml-value: [linked.empty] has no `path` (a string: an absolute path or one starting with `~/`)
skiman.toml:20: toml-value: [linked.loose] must be a table

"#]];
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(1)
        .stdout_eq(expected.clone());
    sandbox.git(sandbox.skill_repo(), ["add", "--all"]);
    sandbox
        .skiman()
        .args(["check", "--staged"])
        .assert()
        .code(1)
        .stdout_eq(expected);
}

#[test]
fn a_well_formed_linked_table_passes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    append_manifest(
        &sandbox,
        "\n[linked.company]\npath = \"~/work/company\"\n\n[linked.tools]\npath = \"/opt/tools\"\n",
    );
    sandbox.skiman().arg("check").assert().code(0).stdout_eq("");
}

#[test]
fn linked_written_as_a_value_breaks_the_rules() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let manifest = sandbox.skill_repo().join("skiman.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, format!("linked = 3\n{text}")).unwrap();
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(1)
        .stdout_eq(str![[r#"
skiman.toml:1: toml-value: `linked` must be a table

"#]]);
}

/// A `linked/x` staged with `git add -f` stores only a link, but the index
/// rule stops even that from being committed; no stage hint is printed
/// while it stands, since staging the working tree would not fix it.
#[test]
fn a_force_staged_path_under_linked_is_a_violation_without_a_stage_hint() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let repo = sandbox.skill_repo();
    std::fs::create_dir(repo.join("linked")).unwrap();
    std::os::unix::fs::symlink("/w/company", repo.join("linked/x")).unwrap();
    sandbox.git(&repo, ["add", "--all"]);
    sandbox.git(&repo, ["add", "-f", "linked/x"]);

    let expected = str![[r#"
linked/x: linked-tracked: nothing under `linked/` may be in the git index; run `git rm -r --cached linked`

"#]];
    sandbox
        .skiman()
        .args(["check", "--staged"])
        .assert()
        .code(1)
        .stdout_eq(expected.clone());
    sandbox
        .skiman()
        .arg("check")
        .assert()
        .code(1)
        .stdout_eq(expected);

    sandbox.git(&repo, ["rm", "-r", "--cached", "--quiet", "linked"]);
    sandbox
        .skiman()
        .args(["check", "--staged"])
        .assert()
        .code(0)
        .stdout_eq("");
}
