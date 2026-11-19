//! `skiman list` and `skiman overview` (spec "Display"): linked skills
//! with their folder, a folder missing on this machine, and a linked skill
//! that lost its name to another skill.

use super::{append_manifest, folder};
use crate::support::{Sandbox, init, skill_md, write};

/// A canon with own skills `notes` and `office/pdf`, and four linked
/// folders: `company` (`alpha`, `pdf`, which loses to the canon's), `gone`
/// (not on this machine), `solo` (one skill) and `zeta` (`alpha`, which
/// loses to `company`'s, and `beta`).
fn setup(sandbox: &Sandbox) {
    init(sandbox);
    let skills = sandbox.skill_repo().join("skills");
    write(skills.join("notes/SKILL.md"), skill_md("notes"));
    write(skills.join("office/pdf/SKILL.md"), skill_md("pdf"));
    folder(sandbox, "work/company", &["alpha", "pdf"]);
    folder(sandbox, "work/zeta", &["alpha", "beta"]);
    write(
        sandbox.home().join("work/solo-skill/SKILL.md"),
        skill_md("solo"),
    );
    append_manifest(
        sandbox,
        "\n[linked.zeta]\npath = \"~/work/zeta\"\n\n[linked.company]\npath = \"~/work/company\"\n\n[linked.solo]\npath = \"~/work/solo-skill\"\n\n[linked.gone]\npath = \"~/work/gone\"\n",
    );
}

/// The cells of each line of `stdout`, a table whose columns are
/// separated by two spaces or more, with `HOME` written as `~`.
fn cells(stdout: &[u8], sandbox: &Sandbox) -> Vec<Vec<String>> {
    let home = sandbox.home().display().to_string();
    String::from_utf8(stdout.to_vec())
        .unwrap()
        .lines()
        .map(|line| {
            line.replace(&home, "~")
                .split("  ")
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

#[test]
fn list_shows_each_linked_skill_with_its_folder_and_state() {
    let sandbox = Sandbox::new();
    setup(&sandbox);

    let output = sandbox.skiman().arg("list").output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let row = |cells: [&str; 7]| cells.map(str::to_owned).to_vec();
    assert_eq!(
        cells(&output.stdout, &sandbox),
        [
            row([
                "NAME",
                "KIND",
                "SOURCE",
                "VERSION",
                "MODIFIED",
                "CONFLICTS",
                "ENABLED"
            ]),
            row(["notes", "own", "-", "-", "-", "no", "yes"]),
            row(["pdf", "own", "-", "-", "-", "no", "yes"]),
            row([
                "alpha",
                "linked",
                "company (~/work/company)",
                "-",
                "-",
                "-",
                "yes"
            ]),
            row([
                "pdf",
                "linked",
                "company (~/work/company)",
                "-",
                "-",
                "-",
                "no: skills/office/pdf holds the name",
            ]),
            row([
                "-",
                "linked",
                "gone (~/work/gone)",
                "-",
                "-",
                "-",
                "not on this machine"
            ]),
            row([
                "solo",
                "linked",
                "solo (~/work/solo-skill)",
                "-",
                "-",
                "-",
                "yes"
            ]),
            row([
                "alpha",
                "linked",
                "zeta (~/work/zeta)",
                "-",
                "-",
                "-",
                "no: linked folder `company` holds the name",
            ]),
            row(["beta", "linked", "zeta (~/work/zeta)", "-", "-", "-", "yes"]),
        ]
    );
}
