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

/// A `SKILL.md` for `name` with `description`, and `metadata.summary` when given.
fn skill_md_with(name: &str, description: &str, summary: Option<&str>) -> String {
    let metadata = summary
        .map(|s| format!("metadata:\n  summary: {s}\n"))
        .unwrap_or_default();
    format!("---\nname: {name}\ndescription: {description}\n{metadata}---\n\n# {name}\n")
}

/// Each linked folder is one section after the canon's groups, titled with
/// its id and path. A summary comes from `metadata.summary`; without one,
/// the description's first sentence (cut to 120 characters) stands in, in
/// italics, and one note under that table says so.
#[test]
fn overview_shows_each_linked_folder_as_a_section_with_summaries_or_stand_ins() {
    let sandbox = Sandbox::new();
    setup(&sandbox);
    let work = sandbox.home().join("work");
    write(
        work.join("company/alpha/SKILL.md"),
        skill_md_with("alpha", "Does alpha things", Some("Alpha for people")),
    );
    write(
        work.join("company/pdf/SKILL.md"),
        skill_md_with("pdf", "Fills forms", Some("\"Forms | for people\"")),
    );
    write(
        work.join("solo-skill/SKILL.md"),
        skill_md_with("solo", "Solo does one thing. Then more.", Some("\"  \"")),
    );
    write(
        work.join("zeta/beta/SKILL.md"),
        skill_md_with("beta", "处理中文文档。然后再说别的。", None),
    );
    let long = "长".repeat(130);
    write(
        work.join("zeta/long/SKILL.md"),
        skill_md_with("long", &long, None),
    );

    let output = sandbox.skiman().arg("overview").output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let start = stdout.find("## Linked folder").expect("a linked section");
    let end = stdout
        .find("## Built into skiman")
        .expect("the built-in section");
    assert!(stdout[..start].contains("### office (1)"), "{stdout}");
    let note = "A summary in italics stands in for a missing `metadata.summary`: it is the first sentence of the skill's description.";
    let expected = format!(
        "## Linked folder company (2): `~/work/company`

| Skill | Summary | Distributed |
| --- | --- | --- |
| alpha | Alpha for people | yes |
| pdf | Forms \\| for people | no: skills/office/pdf holds the name |

## Linked folder gone: `~/work/gone`

Not on this machine.

## Linked folder solo (1): `~/work/solo-skill`

| Skill | Summary | Distributed |
| --- | --- | --- |
| solo | *Solo does one thing.* | yes |

{note}

## Linked folder zeta (3): `~/work/zeta`

| Skill | Summary | Distributed |
| --- | --- | --- |
| alpha | *Does alpha things* | no: linked folder `company` holds the name |
| beta | *处理中文文档。* | yes |
| long | *{}…* | yes |

{note}

",
        "长".repeat(119)
    );
    assert_eq!(stdout[start..end], expected, "{stdout}");
}
