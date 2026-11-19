//! `skiman check` in both modes on what the canon records about linked
//! folders: the `[linked.<id>]` tables, and nothing under `linked/` in the
//! git index.

use snapbox::str;

use super::append_manifest;
use crate::support::{Sandbox, init};
use crate::support::{skill_md, write};

#[test]
fn a_linked_table_that_is_not_well_formed_breaks_the_rules_in_both_modes() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    append_manifest(
        &sandbox,
        "\n[linked.skiman-manual]\npath = \"/w/a\"\n\n[linked.Bad_Id]\npath = \"/w/b\"\n\n[linked.relative]\npath = \"w/c\"\n\n[linked.extra]\npath = \"~/w/d\"\nenabled = true\n\n[linked.empty]\n\n[linked]\nloose = \"x\"\n",
    );

    let expected = str![[r#"
skiman.toml:4: toml-value: `skiman-manual` is a reserved name and cannot be a linked folder id
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

/// A `SKILL.md` for `name` with `metadata.summary`.
fn with_summary(name: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: Does {name} things\nmetadata:\n  summary: For people\n---\n\n# {name}\n"
    )
}

/// `skiman check` on the working tree warns about what is wrong inside
/// linked skills and about names they share; the exit status stays what
/// the canon makes it. `check --staged` reads no linked content.
#[test]
fn check_warns_about_linked_skills_without_changing_the_exit_status() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let canon = sandbox.skill_repo();
    write(canon.join("skills/pdf/SKILL.md"), skill_md("pdf"));
    let company = sandbox.home().join("work/company");
    for name in ["alpha", "pdf"] {
        write(company.join(name).join("SKILL.md"), with_summary(name));
    }
    write(
        company.join("beta/SKILL.md"),
        "---\nname: beta\ndescription: [unclosed\n---\n",
    );
    write(
        company.join("gamma/SKILL.md"),
        "---\nname: other\ndescription: Does gamma things\n---\n",
    );
    write(
        company.join("delta/SKILL.md"),
        format!("{}{}\n", with_summary("delta"), "x".repeat(24001)),
    );
    write(
        company.join("epsilon/SKILL.md"),
        "---\nname: epsilon\nname: epsilon\ndescription: d\n---\n",
    );
    write(company.join("Bad_Name/SKILL.md"), with_summary("bad"));
    let zeta = sandbox.home().join("work/zeta");
    write(zeta.join("alpha/SKILL.md"), with_summary("alpha"));
    let solo = sandbox.home().join("work/solo-skill");
    write(
        solo.join("SKILL.md"),
        with_summary("solo").replace("name: solo", "name: single"),
    );
    append_manifest(
        &sandbox,
        "\n[skills.gamma]\nsummary = \"Gamma\"\n\n[linked.company]\npath = \"~/work/company\"\n\n[linked.solo]\npath = \"~/work/solo-skill\"\n\n[linked.zeta]\npath = \"~/work/zeta\"\n",
    );
    let skill = |dir: &std::path::Path, name: &str| dir.join(name).join("SKILL.md");
    let unparsable = "; Antigravity drops a skill whose frontmatter does not parse";

    let output = sandbox.skiman().arg("check").output().unwrap();

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "");
    for line in [
        format!(
            "warning: {}:4: frontmatter: the frontmatter is not valid YAML: ",
            skill(&company, "beta").display()
        ),
        format!(
            "warning: {}:3: duplicate-key: duplicate key in the frontmatter{unparsable}",
            skill(&company, "epsilon").display()
        ),
        format!(
            "warning: {}:2: name-folder: `name` is `other` but the linked skill is named `gamma`",
            skill(&company, "gamma").display()
        ),
        format!(
            "warning: {}:2: name-folder: `name` is `single` but the linked skill is named `solo`",
            solo.join("SKILL.md").display()
        ),
        format!(
            "warning: {}: skill-info: linked skill gamma has no `metadata.summary`",
            skill(&company, "gamma").display()
        ),
        format!(
            "warning: {}: skill-body-size: the body is 24009 bytes (without the frontmatter and surrounding whitespace); antigravity cuts it after 24000 bytes without a message",
            skill(&company, "delta").display()
        ),
        format!(
            "warning: {} linked skill folder is not distributed: its name is not a valid skill name",
            company.join("Bad_Name").display()
        ),
        format!(
            "warning: linked skill pdf in {} is not distributed: the skill folder skills/pdf uses the name",
            company.join("pdf").display()
        ),
        format!(
            "warning: linked skill alpha in {} is not distributed: linked folder `company` sorts first and uses the name",
            zeta.join("alpha").display()
        ),
        "warning: [skills.gamma] in skiman.toml names linked skill gamma of linked folder `company`".to_owned(),
    ] {
        assert!(stderr.contains(&line), "{line}\n{stderr}");
    }
    let beta = stderr
        .lines()
        .find(|l| l.contains("beta/SKILL.md"))
        .unwrap();
    assert!(beta.ends_with(unparsable), "{beta}");
    for name in ["alpha", "pdf", "delta"] {
        assert!(
            !stderr.contains(&format!("linked skill {name} has no")),
            "{name}: {stderr}"
        );
    }

    sandbox.git(&canon, ["add", "--all"]);
    let output = sandbox
        .skiman()
        .args(["check", "--staged"])
        .output()
        .unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(!stderr.contains("/work/"), "{stderr}");
}
