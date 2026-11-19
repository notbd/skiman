//! `skiman doctor` on linked folders: one line per folder, and each
//! problem sync would report (missing, breaking a rule, a collision,
//! `linked/` or `linked/<id>` blocked), without changing anything.

use super::{append_manifest, contents, folder};
use crate::support::{Sandbox, init, skill_md, tree, write};

fn doctor(sandbox: &Sandbox) -> String {
    let output = sandbox.skiman().arg("doctor").output().unwrap();
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    String::from_utf8(output.stdout).unwrap()
}

fn assert_line(stdout: &str, line: &str) {
    assert!(
        stdout.lines().any(|l| l == line),
        "missing line:\n{line}\nin doctor output:\n{stdout}"
    );
}

#[test]
fn doctor_reports_each_linked_folder_and_its_problems() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    write(sandbox.canon().join("skills/pdf/SKILL.md"), skill_md("pdf"));
    let company = folder(&sandbox, "work/company", &["alpha", "pdf"]);
    folder(&sandbox, "work/tools", &["beta"]);
    let inside = sandbox.canon().join("skills");
    append_manifest(
        &sandbox,
        &format!(
            "\n[linked.company]\npath = \"~/work/company\"\n\n[linked.gone]\npath = \"~/work/gone\"\n\n[linked.inside]\npath = \"{}\"\n\n[linked.tools]\npath = \"~/work/tools\"\n",
            inside.display()
        ),
    );
    sandbox.skiman().arg("sync").output().unwrap();
    let linked = sandbox.canon().join("linked");
    std::fs::remove_file(linked.join("tools")).unwrap();
    std::fs::create_dir(linked.join("tools")).unwrap();
    let home = sandbox.home();
    let before = (tree(sandbox.canon()), tree(&home), contents(&[&company]));

    let stdout = doctor(&sandbox);

    assert_line(
        &stdout,
        &format!(
            "linked folder company: {} (skills: alpha, pdf)",
            company.display()
        ),
    );
    assert_line(
        &stdout,
        &format!(
            "linked folder `gone` ({}) is not on this machine, so its skills are not distributed",
            home.join("work/gone").display()
        ),
    );
    assert_line(
        &stdout,
        &format!(
            "linked folder `inside` is skipped: {} lies in {}, the canon, where skiman writes; fix [linked.inside] in skiman.toml",
            inside.display(),
            sandbox.canon().display()
        ),
    );
    assert_line(
        &stdout,
        &format!(
            "linked skill pdf in {} is not distributed: the skill folder skills/pdf uses the name",
            company.join("pdf").display()
        ),
    );
    assert_line(
        &stdout,
        &format!(
            "{} is not a link, so linked folder `tools` is not linked; move it away, then run `skiman sync`",
            linked.join("tools").display()
        ),
    );
    assert_eq!(
        (tree(sandbox.canon()), tree(&home), contents(&[&company])),
        before
    );
}

#[test]
fn doctor_reports_linked_that_is_a_file() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha"]);
    append_manifest(&sandbox, "\n[linked.company]\npath = \"~/work/company\"\n");
    let linked = sandbox.canon().join("linked");
    std::fs::write(&linked, "x").unwrap();

    let stdout = doctor(&sandbox);

    assert_line(
        &stdout,
        &format!(
            "{} is a file, not a folder, so no linked skill is linked; move it away, then run `skiman sync`",
            linked.display()
        ),
    );
}
