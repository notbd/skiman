//! `skiman linked add` and `skiman linked remove` (spec "Commands").

use snapbox::str;

use super::{COMPANY, append_manifest, contents, folder};
use crate::support::{Sandbox, Tool, init, read, write};

/// `rel` below `HOME`, as the shell expands `~/<rel>`.
fn home(sandbox: &Sandbox, rel: &str) -> String {
    sandbox.home().join(rel).display().to_string()
}

fn linked_add(
    sandbox: &Sandbox,
    args: &[impl AsRef<std::ffi::OsStr>],
) -> snapbox::cmd::OutputAssert {
    sandbox.skiman().args(["linked", "add"]).args(args).assert()
}

#[test]
fn linked_add_registers_the_folder_and_a_second_run_changes_no_record() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    let before = contents(&[&company]);

    linked_add(&sandbox, &[home(&sandbox, "work/company")])
        .code(0)
        .stdout_eq(str![[r#"
registered linked folder company: ~/work/company
its skills: alpha
alpha has no `metadata.summary`; add these lines to the frontmatter of [..]/home/work/company/alpha/SKILL.md:
  metadata:
    summary: "Does alpha things"
linked [..]/canon/linked/company
linked [..]/home/.claude/skills/alpha
linked [..]/home/.agents/skills/alpha
linked [..]/home/.gemini/config/skills/alpha

"#]]);
    let manifest = read(sandbox.canon().join("skiman.toml"));
    assert!(manifest.ends_with(COMPANY), "{manifest}");

    std::fs::remove_file(sandbox.tool_dir(Tool::Codex).join("alpha")).unwrap();
    linked_add(&sandbox, &[&company])
        .code(0)
        .stdout_eq(str![[r#"
linked folder company is already registered: ~/work/company
its skills: alpha
alpha has no `metadata.summary`; add these lines to the frontmatter of [..]/home/work/company/alpha/SKILL.md:
  metadata:
    summary: "Does alpha things"
linked [..]/home/.agents/skills/alpha

"#]]);
    assert_eq!(read(sandbox.canon().join("skiman.toml")), manifest);
    assert_eq!(contents(&[&company]), before);
}

#[test]
fn the_same_id_with_another_path_is_refused() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/company", &["alpha"]);
    folder(&sandbox, "other/company", &["beta"]);
    linked_add(&sandbox, &[home(&sandbox, "work/company")]).code(0);
    let manifest = read(sandbox.canon().join("skiman.toml"));

    linked_add(&sandbox, &[home(&sandbox, "other/company")])
        .code(1)
        .stdout_eq("")
        .stderr_eq(str![[r#"
error: skiman.toml already has [linked.company] for another path; to move that folder, change its `path` line, or run `skiman linked remove company` first

"#]]);
    assert_eq!(read(sandbox.canon().join("skiman.toml")), manifest);
}

/// The same id in a table that is not well formed, with the same path or
/// none: the refusal says what is wrong with it, and nothing changes.
#[test]
fn the_same_id_in_a_malformed_table_is_refused_with_what_is_wrong() {
    for (table, why) in [
        (
            "[linked.company]\npath = \"~/work/company\"\nnote = \"x\"\n",
            "unknown key `note` in [linked.company] (allowed: path)",
        ),
        (
            "[linked.company]\n",
            "[linked.company] has no `path` (a string: an absolute path or one starting with `~/`)",
        ),
    ] {
        let sandbox = Sandbox::new();
        init(&sandbox);
        folder(&sandbox, "work/company", &["alpha"]);
        append_manifest(&sandbox, &format!("\n{table}"));
        let manifest = read(sandbox.canon().join("skiman.toml"));

        linked_add(&sandbox, &[home(&sandbox, "work/company")])
            .code(1)
            .stdout_eq("")
            .stderr_eq(format!(
                "error: skiman.toml already has [linked.company], which is not well formed: {why}; fix it in skiman.toml, then run the command again\n"
            ));
        assert_eq!(read(sandbox.canon().join("skiman.toml")), manifest);
    }
}

/// A folder called `skills` gets its parent's name, so `~/work/acme/skills`
/// is `acme`.
#[test]
fn a_folder_called_skills_is_named_after_its_parent() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    folder(&sandbox, "work/Acme Corp/skills", &[]);

    linked_add(&sandbox, &[home(&sandbox, "work/Acme Corp/skills")])
        .code(0)
        .stdout_eq(str![[r#"
registered linked folder acme-corp: ~/work/Acme Corp/skills
it holds no skill yet; the next sync links each skill added to it
linked [..]/canon/linked/acme-corp

"#]]);
}

/// The lines `linked add` prints for each linked skill without a usable
/// `metadata.summary` fit the frontmatter as it is: a `metadata` map that
/// is there gets one line, an empty `summary` is replaced in place, and a
/// shape where a line would make a duplicate key is named instead.
#[test]
fn linked_add_prints_the_summary_lines_each_skill_needs() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.home().join("work/company");
    let skill = |name: &str, extra: &str| {
        write(
            dir.join(name).join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: Does {name} \"quoted\" things. And more.\n{extra}---\n\n# {name}\n"
            ),
        );
    };
    skill("plain", "");
    skill("block", "metadata:\n    author: someone\n");
    skill("empty", "metadata:\n  summary: \"\"\n");
    skill("flow", "metadata: {author: someone}\n");
    skill("number", "metadata:\n  summary: 3\n");
    skill("scalar", "metadata: none\n");
    skill(
        "done",
        "metadata:\n  summary: Already says what it is for\n",
    );
    write(
        dir.join("broken/SKILL.md"),
        "---\nname: broken\ndescription: [unclosed\n---\n",
    );

    // snapbox shows each `\` as `/`; the raw output has the YAML escapes.
    let output = linked_add(&sandbox, &[home(&sandbox, "work/company")])
        .code(0)
        .stdout_eq(str![[r#"
registered linked folder company: ~/work/company
its skills: block, broken, done, empty, flow, number, plain, scalar
block has no `metadata.summary`; add this line under `metadata:` in the frontmatter of [..]/home/work/company/block/SKILL.md:
      summary: "Does block /"quoted/" things."
broken: the frontmatter of [..]/home/work/company/broken/SKILL.md does not parse; fix the frontmatter first, then add `metadata.summary`
empty has an empty `metadata.summary`; in the frontmatter of [..]/home/work/company/empty/SKILL.md, replace that line with:
    summary: "Does empty /"quoted/" things."
flow: `metadata` in [..]/home/work/company/flow/SKILL.md is written on one line (a flow map); write it as a block map, one key per line, and add `summary: "Does flow /"quoted/" things."` to it
number: `metadata.summary` in [..]/home/work/company/number/SKILL.md is not a string; make it one, such as `summary: "Does number /"quoted/" things."`
plain has no `metadata.summary`; add these lines to the frontmatter of [..]/home/work/company/plain/SKILL.md:
  metadata:
    summary: "Does plain /"quoted/" things."
scalar: `metadata` in [..]/home/work/company/scalar/SKILL.md is not a map; make it one that holds `summary: "Does scalar /"quoted/" things."`
...
"#]])
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(output).unwrap();
    assert!(
        stdout.contains("\n    summary: \"Does plain \\\"quoted\\\" things.\"\n"),
        "{stdout}"
    );
    assert!(!stdout.contains("/done/SKILL.md"), "{stdout}");
}

/// The lines `linked add` prints parse once pasted in: a description
/// with control characters gets YAML escapes, and a frontmatter written as
/// one flow map gets no block lines, which would not fit it.
#[test]
fn the_summary_lines_linked_add_prints_parse_once_applied() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let dir = sandbox.home().join("work/company");
    let bell = dir.join("bell/SKILL.md");
    write(
        &bell,
        "---\nname: bell\ndescription: \"A \\a bell, a \\e escape and a \\\\ slash.\"\n---\n\n# bell\n",
    );
    let flow = dir.join("flow/SKILL.md");
    write(
        &flow,
        "---\n{name: flow, description: A flow-style document.}\n---\n",
    );
    write(
        dir.join("done/SKILL.md"),
        "---\n{name: done, description: Done., metadata: {summary: Already there}}\n---\n",
    );

    let output = linked_add(&sandbox, &[&dir])
        .code(0)
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(output).unwrap();
    let mut lines = stdout.lines();
    lines
        .find(|l| l.starts_with("bell has no `metadata.summary`; add these lines"))
        .unwrap_or_else(|| panic!("{stdout}"));
    let added: String = lines
        .take_while(|l| l.starts_with("  "))
        .map(|l| format!("{}\n", &l[2..]))
        .collect();
    assert!(
        !added.chars().any(|c| c.is_control() && c != '\n'),
        "{added:?}"
    );
    let text = read(&bell).replacen("\n---\n", &format!("\n{added}---\n"), 1);
    std::fs::write(&bell, text).unwrap();
    assert!(
        stdout.contains(&format!(
            "flow: the frontmatter of {} is written on one line (a flow map); write it as a block map, one key per line, and add `metadata:` holding `summary: \"A flow-style document.\"`",
            flow.display()
        )),
        "{stdout}"
    );

    assert!(!stdout.contains("/done/SKILL.md"), "{stdout}");

    let output = sandbox.skiman().arg("check").output().unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert_eq!(output.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("linked skill flow has no `metadata.summary`"),
        "{stderr}"
    );
    assert!(!stderr.contains("bell"), "{stderr}");
}

/// `linked remove` of an id that is not registered changes nothing; when
/// `linked/<id>` is left from an earlier registration, it says that sync
/// removes it.
#[test]
fn linked_remove_refuses_an_id_that_is_not_registered() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    sandbox
        .skiman()
        .args(["linked", "remove", "company"])
        .assert()
        .code(1)
        .stderr_eq("error: skiman.toml has no [linked.company]\n");

    let leftover = sandbox.canon().join("linked/company");
    std::fs::create_dir_all(leftover.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("/nowhere", &leftover).unwrap();
    sandbox
        .skiman()
        .args(["linked", "remove", "company"])
        .assert()
        .code(1)
        .stderr_eq("error: skiman.toml has no [linked.company]; [..]/canon/linked/company is left from an earlier registration, and `skiman sync` removes it\n");
    sandbox.skiman().arg("sync").assert().code(0);
    assert!(std::fs::symlink_metadata(&leftover).is_err());
}

/// `linked remove` works when the folder is gone from this machine.
#[test]
fn linked_remove_works_when_the_folder_is_gone() {
    let sandbox = Sandbox::new();
    init(&sandbox);
    let company = folder(&sandbox, "work/company", &["alpha"]);
    linked_add(&sandbox, &[&company]).code(0);
    std::fs::remove_dir_all(&company).unwrap();

    sandbox
        .skiman()
        .args(["linked", "remove", "company"])
        .assert()
        .code(0);

    assert!(!read(sandbox.canon().join("skiman.toml")).contains("[linked"));
    assert!(std::fs::symlink_metadata(sandbox.tool_dir(Tool::ClaudeCode).join("alpha")).is_err());
}

/// A `linked add` killed right before it writes `skiman.toml` (after the
/// `.gitignore` line) is finished by running it again.
#[test]
#[cfg_attr(
    not(debug_assertions),
    ignore = "needs the test hook, which only debug builds have"
)]
fn a_linked_add_killed_before_its_record_finishes_when_run_again() {
    crate::support::assert_each_kill_finishes(
        |sandbox| {
            init(sandbox);
            std::fs::write(sandbox.canon().join(".gitignore"), ".skiman/\n").unwrap();
            let company = folder(sandbox, "work/company", &["alpha"]);
            vec![
                "linked".to_owned(),
                "add".to_owned(),
                company.display().to_string(),
            ]
        },
        &["linked:write:skiman.toml"],
    );
}
