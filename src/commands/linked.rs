//! `skiman linked add|remove` (ADR 0010): register a folder of skills kept
//! outside the canon, or end a registration, then sync. Both run under the
//! canon lock and exit with sync's status.

use std::path::Path;

use saphyr::{AnnotatedMapping, LoadableYamlNode, MarkedYaml, Scalar, YamlData};

use crate::Exit;
use crate::canon::Canon;
use crate::canon_lock::CanonLock;
use crate::failure::{Failure, finish};
use crate::fsutil::write_atomic;
use crate::linked::{self, Folder, Linked, State, gitignore};
use crate::manifest::{self, Manifest};
use crate::paths::Paths;
use crate::project::root;
use crate::rules::frontmatter::split;
use crate::rules::{self, Scope};
use crate::sync::{self, Output};
use crate::tree::{Overlay, WorkingTree};

pub fn add(path: &Path, id: Option<String>) -> Exit {
    finish(run_add(path, id))
}

pub fn remove(id: &str) -> Exit {
    finish(run_remove(id))
}

fn run_add(path: &Path, id: Option<String>) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let home = paths.home();
    let cwd = std::env::current_dir()
        .map_err(|e| Failure::operational(format!("cannot read the current directory: {e}")))?;
    // Recorded as given, not as its realpath: `linked/<id>` points there.
    let absolute = root::given(home, &cwd, path);
    let written = root::written_form(home, &absolute);
    let given = id.is_some();
    let id = id.unwrap_or_else(|| default_id(&absolute));
    if let Some(why) = linked::invalid_id(&id) {
        let hint = if given { "" } else { "; give one with --id" };
        return Err(Failure::refused(format!("{why}{hint}")));
    }
    let lock = CanonLock::wait(&paths, &repo)?;
    let file = repo.manifest_file();
    let text = crate::fsutil::read_text(&file)?.unwrap_or_default();
    let manifest = Manifest::parse(&text).map_err(Failure::refused_in(&file))?;
    let registered = manifest.linked().into_iter().find(|e| e.id == id);
    // A recorded `path` names the same folder when it is written the same
    // way once expanded (`/Users/me/company` is `~/company`).
    let same = |recorded: &str| {
        root::expand(home, recorded).is_some_and(|p| root::written_form(home, &p) == written)
    };
    let recorded = match registered.map(|e| e.path) {
        None => None,
        Some(Ok(path)) if same(&path) => Some(path),
        // Not another path: the table is what needs fixing.
        Some(Err(malformed)) if malformed.path.as_deref().is_none_or(same) => {
            return Err(Failure::refused(format!(
                "skiman.toml already has [linked.{id}], which is not well formed: {}; fix it in skiman.toml, then run the command again",
                malformed.why
            )));
        }
        Some(_) => {
            return Err(Failure::refused(format!(
                "skiman.toml already has [linked.{id}] for another path; to move that folder, change its `path` line, or run `skiman linked remove {id}` first"
            )));
        }
    };
    let new = recorded.is_none();
    let text = if new {
        manifest::with_linked(&text, &id, &written).map_err(Failure::refused_in(&file))?
    } else {
        text
    };
    // The rules every command applies, on the registrations as they will be.
    let candidate = Manifest::parse(&text).map_err(Failure::refused_in(&file))?;
    let linked = Linked::read(home, &repo, &candidate);
    let folder = linked
        .folders
        .iter()
        .find(|f| f.id == id)
        .expect("the registration is in the candidate");
    // Only a new registration needs an existing folder that passes the
    // rules: a second run changes no record, so sync reports the folder's
    // state and its links follow it (removed when it is gone, kept while
    // it cannot be read).
    if new {
        match &folder.state {
            State::Read(_) => {}
            State::Missing => {
                return Err(Failure::refused(format!(
                    "{} is not an existing folder",
                    absolute.display()
                )));
            }
            State::Broken(why) => return Err(Failure::refused(why.clone())),
            State::Unreadable(why) => return Err(Failure::operational(why.clone())),
        }
        // Nor may it take the place of a registration that works now.
        let current = Linked::read(home, &repo, &manifest);
        for was in current
            .folders
            .iter()
            .filter(|f| !matches!(f.state, State::Broken(_)))
        {
            if let Some(now) = linked.folders.iter().find(|f| f.id == was.id)
                && let State::Broken(why) = &now.state
            {
                return Err(Failure::refused(format!(
                    "{} cannot be registered as `{id}`: it would break linked folder `{}` ({why})",
                    absolute.display(),
                    now.id
                )));
            }
        }
    }
    let gitignore = repo.root().join(".gitignore");
    match gitignore::ignore(repo.root())? {
        gitignore::Outcome::Link => {
            return Err(Failure::refused(format!(
                "{} is a link, so skiman cannot add `/linked/` to it; replace the link with a file holding the same lines, then run this again",
                gitignore.display()
            )));
        }
        gitignore::Outcome::Added => out!("added /linked/ to {}", gitignore.display()),
        gitignore::Outcome::Present => {}
    }
    if let Some(recorded) = &recorded {
        out!("linked folder {id} is already registered: {recorded}");
    } else {
        let tree = WorkingTree::new(repo.root());
        let overlay = Overlay::new(
            &tree,
            vec![(rules::MANIFEST.to_owned(), text.clone().into_bytes())],
        );
        rules::validate_write(
            &overlay,
            &Scope {
                manifest: true,
                ..Scope::default()
            },
        )?;
        crate::test_hook::at("linked", "write:skiman.toml");
        write_atomic(&file, text.as_bytes())
            .map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))?;
        out!("registered linked folder {id}: {written}");
    }
    describe(folder);
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}

/// The id `linked add` proposes: the folder's name made into a valid name,
/// as for a project root, or its parent's name for a folder called `skills`.
fn default_id(path: &Path) -> String {
    match path.parent() {
        Some(parent) if path.file_name().is_some_and(|n| n == "skills") => root::default_id(parent),
        _ => root::default_id(path),
    }
}

/// Prints the linked skills `folder` holds, read as sync reads them, and
/// for each without a usable `metadata.summary` what to add to its
/// `SKILL.md`.
fn describe(folder: &Folder) {
    let (State::Read(_), Some(path)) = (&folder.state, &folder.path) else {
        return;
    };
    let skills = if Linked::is_single(folder) {
        out!("it is one skill: {}", folder.id);
        vec![(folder.id.clone(), path.clone())]
    } else {
        let names = Linked::skill_names(folder);
        if names.is_empty() {
            out!("it holds no skill yet; the next sync links each skill added to it");
            return;
        }
        out!("its skills: {}", names.join(", "));
        names
            .into_iter()
            .map(|name| {
                let dir = folder.skill_dir(&name);
                (name, dir)
            })
            .collect()
    };
    for (name, dir) in skills {
        let file = dir.join("SKILL.md");
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                if let Some(advice) = summary_advice(&name, &file, &text) {
                    out!("{advice}");
                }
            }
            Err(e) => out!("{name}: cannot read {}: {e}", file.display()),
        }
    }
}

/// What to add to the `SKILL.md` at `file` (holding `text`) so that it has
/// a usable `metadata.summary` (ADR 0011); `None` when it has one. Never
/// prints lines that would create a duplicate key: where `metadata` has a
/// shape such lines would not fit, it names what to fix.
fn summary_advice(name: &str, file: &Path, text: &str) -> Option<String> {
    let at = file.display();
    let broken = || {
        format!(
            "{name}: the frontmatter of {at} does not parse; fix the frontmatter first, then add `metadata.summary`"
        )
    };
    let Some(yaml) = split(text) else {
        return Some(broken());
    };
    let Ok(docs) = MarkedYaml::load_from_str(yaml) else {
        return Some(broken());
    };
    let Some(doc) = docs.first() else {
        return Some(broken());
    };
    let YamlData::Mapping(fields) = &doc.data else {
        return Some(broken());
    };
    let description = match field(fields, "description").map(|(_, v)| &v.data) {
        Some(YamlData::Value(Scalar::String(d))) => d.to_string(),
        _ => String::new(),
    };
    let line = format!("summary: {}", quoted(&linked::stand_in(&description)));
    let Some((_, metadata)) = field(fields, "metadata") else {
        // Block lines do not fit a frontmatter written as one flow map.
        if is_flow(yaml, doc) {
            return Some(format!(
                "{name}: the frontmatter of {at} is written on one line (a flow map); write it as a block map, one key per line, and add `metadata:` holding `{line}`"
            ));
        }
        return Some(format!(
            "{name} has no `metadata.summary`; add these lines to the frontmatter of {at}:\n  metadata:\n    {line}"
        ));
    };
    let YamlData::Mapping(map) = &metadata.data else {
        return Some(format!(
            "{name}: `metadata` in {at} is not a map; make it one that holds `{line}`"
        ));
    };
    match field(map, "summary") {
        Some((key, value)) => match &value.data {
            YamlData::Value(Scalar::String(s)) if !s.trim().is_empty() => None,
            YamlData::Value(Scalar::String(_)) if !is_flow(yaml, metadata) => Some(format!(
                "{name} has an empty `metadata.summary`; in the frontmatter of {at}, replace that line with:\n  {}{line}",
                " ".repeat(key.span.start.col())
            )),
            YamlData::Value(Scalar::String(_)) => Some(format!(
                "{name} has an empty `metadata.summary`; in {at}, give `summary` in `metadata` a value, such as `{line}`"
            )),
            _ => Some(format!(
                "{name}: `metadata.summary` in {at} is not a string; make it one, such as `{line}`"
            )),
        },
        None if is_flow(yaml, metadata) => Some(format!(
            "{name}: `metadata` in {at} is written on one line (a flow map); write it as a block map, one key per line, and add `{line}` to it"
        )),
        None => {
            let indent = map.keys().next().map_or(2, |k| k.span.start.col());
            Some(format!(
                "{name} has no `metadata.summary`; add this line under `metadata:` in the frontmatter of {at}:\n  {}{line}",
                " ".repeat(indent)
            ))
        }
    }
}

/// The key and value of the field `key` of `map`.
fn field<'m, 'a>(
    map: &'m AnnotatedMapping<'a, MarkedYaml<'a>>,
    key: &str,
) -> Option<(&'m MarkedYaml<'a>, &'m MarkedYaml<'a>)> {
    map.iter()
        .find(|(k, _)| matches!(&k.data, YamlData::Value(Scalar::String(s)) if s == key))
}

/// Whether the node `value` of `yaml` is written in flow style (`{...}`),
/// after any node properties (an anchor `&a`, a tag `!t`) that may start
/// its span.
fn is_flow(yaml: &str, value: &MarkedYaml) -> bool {
    let Some((at, _)) = yaml.char_indices().nth(value.span.start.index()) else {
        return false;
    };
    let mut rest = yaml[at..].trim_start();
    // A property runs to the next space or `{`.
    while let Some(property) = rest.strip_prefix(['&', '!']) {
        rest = property
            .trim_start_matches(|c: char| !c.is_whitespace() && c != '{')
            .trim_start();
    }
    rest.starts_with('{')
}

/// `text` as a double-quoted YAML string: `\`, `"` and every character
/// YAML does not print as is (control characters, line and paragraph
/// separators, the byte order mark) are escaped.
fn quoted(text: &str) -> String {
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                out.push_str(&format!("\\u{:04X}", u32::from(c)));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn run_remove(id: &str) -> Result<Exit, Failure> {
    let paths = Paths::from_env()?;
    let repo = Canon::from_config(&paths)?;
    let lock = CanonLock::wait(&paths, &repo)?;
    let file = repo.manifest_file();
    let text = crate::fsutil::read_text(&file)?.unwrap_or_default();
    let manifest = Manifest::parse(&text).map_err(Failure::refused_in(&file))?;
    let Some(entry) = manifest.linked().into_iter().find(|e| e.id == id) else {
        let link = repo.linked_dir().join(id);
        let hint = if std::fs::symlink_metadata(&link).is_ok() {
            format!(
                "; {} is left from an earlier registration, and `skiman sync` removes it",
                link.display()
            )
        } else {
            String::new()
        };
        return Err(Failure::refused(format!(
            "skiman.toml has no [linked.{id}]{hint}"
        )));
    };
    let new = manifest::without_linked(&text, id).map_err(Failure::refused_in(&file))?;
    write_atomic(&file, new.as_bytes())
        .map_err(|e| Failure::operational(format!("cannot write {}: {e}", file.display())))?;
    match entry.path {
        Ok(path) => out!("unregistered linked folder {id}: {path}"),
        Err(_) => out!("unregistered linked folder {id}"),
    }
    Ok(sync::run(&paths, &repo, &lock, Output::Normal))
}
