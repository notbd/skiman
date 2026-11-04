//! `skiman import --guide`: the instructions an agent follows to prepare an import folder.

mod support;

use support::{Sandbox, tree};

fn guide(sandbox: &Sandbox) -> String {
    let output = sandbox
        .skiman_confined()
        .args(["import", "--guide"])
        .assert()
        .code(0)
        .stderr_eq("")
        .get_output()
        .stdout
        .clone();
    String::from_utf8(output).unwrap()
}

#[test]
fn import_guide_prints_the_installed_version_and_every_section_and_writes_nothing() {
    let sandbox = Sandbox::new();
    sandbox
        .skiman()
        .args(["init", "--session-hooks", "none"])
        .arg(sandbox.skill_repo())
        .assert()
        .code(0);
    let before = tree(sandbox.root());

    let text = guide(&sandbox);

    assert_eq!(tree(sandbox.root()), before);
    let first = text.lines().next().unwrap();
    assert!(
        first.contains(&format!("skiman {}", env!("CARGO_PKG_VERSION"))),
        "{first}"
    );
    let required = [
        "## The import folder",
        "skills/<name>/",
        "instructions/shared.md",
        "skiman.toml",
        "## Choose the canonical copy among diverged copies",
        "## Extract the shared body",
        "## Pin each upstream",
        "ref = \"<40-hex sha>\"",
    ];
    let missing: Vec<_> = required.iter().filter(|s| !text.contains(**s)).collect();
    assert!(missing.is_empty(), "missing: {missing:#?}");
}

#[test]
fn import_guide_works_before_a_skill_repo_exists_and_matches_its_snapshot() {
    let sandbox = Sandbox::new();
    let before = tree(sandbox.root());

    let text = guide(&sandbox);

    assert_eq!(tree(sandbox.root()), before);
    snapbox::assert_data_eq!(
        text.replace(env!("CARGO_PKG_VERSION"), "<VERSION>"),
        snapbox::file!["snapshots/import-guide.md"].raw()
    );
}
