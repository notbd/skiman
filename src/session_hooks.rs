//! Session-start hook entries in the agent tools' settings files (spec
//! "Hooks"): `init` adds one group per file whose hook calls the recorded
//! binary with `sync --hook`, and `--session-hooks none` removes skiman's hooks.
//!
//! A hook is skiman's when its command contains [`MARKER`]. A settings file is
//! rewritten at its realpath (temporary file in that directory, then rename),
//! keeping every other key and entry; a file that is not valid JSON, or whose
//! `hooks.SessionStart` is not a list, is never rewritten (exit 4, and the
//! entry to add by hand is printed). A missing settings file is created when
//! the folder that holds it exists; when that folder is missing the tool is not
//! installed and nothing is written.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::Exit;
use crate::commands;
use crate::tools::{AgentTool, TOOLS};

/// Marks a hook command as skiman's.
pub const MARKER: &str = "skiman sync --hook";

/// Session sources the entry fires on (Phase 1 experiment 5: not `clear` or `compact`).
pub const MATCHER: &str = "startup|resume";

/// Seconds the agent tool waits for the hook.
const TIMEOUT_SECONDS: u64 = 10;

/// What `init` does with session-start hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    /// `--session-hooks <ids>`: add entries for these comma-separated tool ids.
    Add(&'a str),
    /// `--session-hooks none`: remove skiman's hooks from every settings file.
    Remove,
    /// No flag, with a terminal: preview the entries and ask.
    Ask,
}

/// Applies `request`; `binary` is the recorded binary path. Every settings
/// file is handled on its own, and the exit is the highest of them.
pub fn apply(home: &Path, binary: &Path, request: Request) -> Exit {
    let group = entry_group(binary);
    let hookable = TOOLS.iter().filter(|tool| tool.session_hook_file.is_some());
    match request {
        Request::Add(ids) => hookable
            .filter(|tool| ids.split(',').any(|id| id == tool.id))
            .map(|tool| add(home, tool, &group))
            .fold(Exit::Success, Exit::combine),
        Request::Remove => hookable
            .map(|tool| remove(home, tool))
            .fold(Exit::Success, Exit::combine),
        Request::Ask => ask(home, hookable.collect(), &group),
    }
}

/// Whether a settings file holds skiman's session-start entry, for `doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Exactly the group `init` adds for this binary, and no other skiman hook.
    Installed,
    /// skiman hooks that differ from what `init` adds now (another binary or matcher).
    Different,
    /// No skiman hook (or no settings file).
    Missing,
    /// The file cannot be read or has an unexpected shape.
    Unusable(String),
}

/// The settings file of `tool` (which must have one) and its [`Status`]. Read-only.
pub fn status(home: &Path, tool: &AgentTool, binary: &Path) -> (PathBuf, Status) {
    let path = settings_path(home, tool);
    let settings = match load(path.clone()) {
        Ok(settings) => settings,
        Err(Unusable::NotInstalled(_)) => return (path, Status::Missing),
        Err(Unusable::Broken(reason)) => return (path, Status::Unusable(reason)),
    };
    let groups = match session_start(&settings.root) {
        Ok(groups) => groups.map_or(&[][..], Vec::as_slice),
        Err(what) => {
            return (
                path.clone(),
                Status::Unusable(format!("{}: {what}", path.display())),
            );
        }
    };
    let ours: Vec<&Value> = groups.iter().filter(|g| hook_count(g) > 0).collect();
    let total: usize = groups.iter().map(hook_count).sum();
    let status = if total == 0 {
        Status::Missing
    } else if total == 1 && ours == [&entry_group(binary)] {
        Status::Installed
    } else {
        Status::Different
    };
    (path, status)
}

/// The `hooks.SessionStart` group skiman adds.
pub fn entry_group(binary: &Path) -> Value {
    json!({
        "matcher": MATCHER,
        "hooks": [{
            "type": "command",
            "command": format!("{} sync --hook", shell_word(&binary.to_string_lossy())),
            "timeout": TIMEOUT_SECONDS,
        }],
    })
}

/// `path` for a shell command line: every ASCII character outside a safe set
/// gets a backslash, so the command still contains [`MARKER`].
fn shell_word(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii() && !(c.is_ascii_alphanumeric() || "/._-+,:@%".contains(c)) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// A settings file as read from disk.
struct Settings {
    /// The path in the tool table, under `HOME` (used in messages).
    path: PathBuf,
    /// Where the file is written: its realpath, or `path` when it does not exist yet.
    real: PathBuf,
    root: Value,
    /// The file's permissions; `None` when it does not exist yet.
    permissions: Option<std::fs::Permissions>,
}

enum Unusable {
    /// The folder that holds the settings file is missing: the tool is not installed.
    NotInstalled(PathBuf),
    /// The file cannot be read, is not valid JSON, or has an unexpected shape.
    Broken(String),
}

fn load(path: PathBuf) -> Result<Settings, Unusable> {
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e)
            if e.kind() == std::io::ErrorKind::NotFound
                && std::fs::symlink_metadata(&path).is_err() =>
        {
            let parent = path.parent().expect("settings file has a parent");
            if !parent.is_dir() {
                return Err(Unusable::NotInstalled(parent.to_owned()));
            }
            return Ok(Settings {
                real: path.clone(),
                path,
                root: Value::Object(Map::new()),
                permissions: None,
            });
        }
        Err(e) => {
            return Err(Unusable::Broken(format!(
                "cannot read {}: {e}",
                path.display()
            )));
        }
    };
    let root: Value = serde_json::from_str(&text)
        .map_err(|e| Unusable::Broken(format!("{} is not valid JSON ({e})", path.display())))?;
    let broken =
        |e: std::io::Error| Unusable::Broken(format!("cannot read {}: {e}", path.display()));
    let real = path.canonicalize().map_err(broken)?;
    let permissions = std::fs::metadata(&real).map_err(broken)?.permissions();
    Ok(Settings {
        path,
        real,
        root,
        permissions: Some(permissions),
    })
}

impl Settings {
    fn save(&self) -> std::io::Result<()> {
        let mut text = serde_json::to_string_pretty(&self.root).expect("JSON value serialises");
        text.push('\n');
        match &self.permissions {
            Some(permissions) => {
                crate::fsutil::rewrite_atomic(&self.real, text.as_bytes(), permissions.clone())
            }
            None => crate::fsutil::write_atomic(&self.real, text.as_bytes()),
        }
    }
}

fn settings_path(home: &Path, tool: &AgentTool) -> PathBuf {
    home.join(
        tool.session_hook_file
            .expect("tool has a session-start hook file"),
    )
}

fn warn_unavailable(home: &Path, tool: &AgentTool, reason: &str) {
    eprintln!(
        "warning: {reason}; skiman adds no entry to {}",
        settings_path(home, tool).display()
    );
}

/// Loads the settings file and inserts `group`: `Ok(None)` when the file
/// already holds exactly that entry and no other skiman hook.
fn plan_addition(
    home: &Path,
    tool: &AgentTool,
    group: &Value,
) -> Result<Option<Settings>, Unusable> {
    let mut settings = load(settings_path(home, tool))?;
    let shape_error = |what: &str| Unusable::Broken(format!("{}: {what}", settings.path.display()));
    let groups = session_start(&settings.root)
        .map_err(shape_error)?
        .map_or(&[][..], Vec::as_slice);
    let ours: Vec<&Value> = groups.iter().filter(|g| hook_count(g) > 0).collect();
    let total: usize = groups.iter().map(hook_count).sum();
    if total == 1 && ours == [group] {
        return Ok(None);
    }
    strip(&mut settings.root, false);
    let Value::Object(root) = &mut settings.root else {
        unreachable!("checked by session_start")
    };
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let start = hooks
        .as_object_mut()
        .expect("checked by session_start")
        .entry("SessionStart")
        .or_insert_with(|| Value::Array(Vec::new()));
    start
        .as_array_mut()
        .expect("checked by session_start")
        .push(group.clone());
    Ok(Some(settings))
}

fn add(home: &Path, tool: &AgentTool, group: &Value) -> Exit {
    if let Some(reason) = tool.session_hook_unavailable {
        warn_unavailable(home, tool, reason);
        return Exit::Success;
    }
    match plan_addition(home, tool, group) {
        Ok(None) => Exit::Success,
        Ok(Some(settings)) => write_addition(&settings, group),
        Err(Unusable::NotInstalled(dir)) => {
            eprintln!(
                "warning: {} does not exist, so skiman adds no session-start hook entry for {}",
                dir.display(),
                tool.id
            );
            Exit::Success
        }
        Err(Unusable::Broken(reason)) => {
            print_by_hand(&reason, group);
            Exit::OperationalError
        }
    }
}

fn write_addition(settings: &Settings, group: &Value) -> Exit {
    match settings.save() {
        Ok(()) => {
            println!(
                "added the session-start hook entry to {}",
                settings.path.display()
            );
            Exit::Success
        }
        Err(e) => {
            print_by_hand(
                &format!("cannot write {}: {e}", settings.real.display()),
                group,
            );
            Exit::OperationalError
        }
    }
}

fn print_by_hand(reason: &str, group: &Value) {
    eprintln!(
        "error: {reason}, so skiman did not change it; add this entry to `hooks.SessionStart` by hand:\n{}",
        pretty(group)
    );
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON value serialises")
}

fn remove(home: &Path, tool: &AgentTool) -> Exit {
    let path = settings_path(home, tool);
    let by_hand = |reason: String| {
        eprintln!(
            "error: {reason}, so skiman did not change it; remove by hand every hook whose command contains `{MARKER}`"
        );
        Exit::OperationalError
    };
    let mut settings = match load(path) {
        Ok(settings) if settings.permissions.is_none() => return Exit::Success,
        Ok(settings) => settings,
        Err(Unusable::NotInstalled(_)) => return Exit::Success,
        Err(Unusable::Broken(reason)) => return by_hand(reason),
    };
    if !strip(&mut settings.root, true) {
        return Exit::Success;
    }
    match settings.save() {
        Ok(()) => {
            println!(
                "removed the session-start hook entries from {}",
                settings.path.display()
            );
            Exit::Success
        }
        Err(e) => by_hand(format!("cannot write {}: {e}", settings.real.display())),
    }
}

/// Previews the entries `init` would add, asks, and adds them on "y".
fn ask(home: &Path, tools: Vec<&AgentTool>, group: &Value) -> Exit {
    let mut exit = Exit::Success;
    let mut planned = Vec::new();
    for tool in tools {
        let file = settings_path(home, tool);
        if !file.parent().is_some_and(Path::is_dir) {
            continue;
        }
        if let Some(reason) = tool.session_hook_unavailable {
            warn_unavailable(home, tool, reason);
            continue;
        }
        match plan_addition(home, tool, group) {
            Ok(None) | Err(Unusable::NotInstalled(_)) => {}
            Ok(Some(settings)) => planned.push((tool.id, settings)),
            Err(Unusable::Broken(reason)) => {
                print_by_hand(&reason, group);
                exit = Exit::OperationalError;
            }
        }
    }
    if planned.is_empty() {
        return exit;
    }
    for (_, settings) in &planned {
        eprintln!(
            "skiman can add this session-start hook entry to `hooks.SessionStart` in {}, so a new or resumed session refreshes distribution:\n{}",
            settings.path.display(),
            pretty(group)
        );
    }
    let question = if planned.len() == 1 {
        "Add this entry? [y/N] "
    } else {
        "Add these entries? [y/N] "
    };
    let agreed = match commands::confirm(question) {
        Ok(agreed) => agreed,
        Err(failure) => return exit.combine(failure.report()),
    };
    if agreed {
        for (_, settings) in &planned {
            exit = exit.combine(write_addition(settings, group));
        }
    } else {
        let ids: Vec<&str> = planned.iter().map(|(id, _)| *id).collect();
        eprintln!(
            "hint: no session-start hook entry added; add it later with `skiman init <path> --session-hooks {}`",
            ids.join(",")
        );
    }
    exit
}

/// The `hooks.SessionStart` groups (`None` when absent), or what is wrong
/// with the shape around them.
fn session_start(root: &Value) -> Result<Option<&Vec<Value>>, &'static str> {
    let Value::Object(root) = root else {
        return Err("the top level is not a JSON object");
    };
    let Some(hooks) = root.get("hooks") else {
        return Ok(None);
    };
    let Value::Object(hooks) = hooks else {
        return Err("`hooks` is not a JSON object");
    };
    match hooks.get("SessionStart") {
        None => Ok(None),
        Some(Value::Array(groups)) => Ok(Some(groups)),
        Some(_) => Err("`hooks.SessionStart` is not a list"),
    }
}

/// How many of the group's hooks are skiman's.
fn hook_count(group: &Value) -> usize {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .map_or(0, |hooks| {
            hooks.iter().filter(|h| is_skiman_hook(h)).count()
        })
}

fn is_skiman_hook(hook: &Value) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(MARKER))
}

/// Removes skiman's hooks from `hooks.SessionStart`, and every group they
/// leave empty. With `prune`, also removes `SessionStart` and `hooks` when
/// this left them empty. Returns whether anything was removed.
fn strip(root: &mut Value, prune: bool) -> bool {
    let Value::Object(root) = root else {
        return false;
    };
    let Some(Value::Object(hooks)) = root.get_mut("hooks") else {
        return false;
    };
    let Some(Value::Array(groups)) = hooks.get_mut("SessionStart") else {
        return false;
    };
    let mut removed = false;
    groups.retain_mut(|group| {
        if hook_count(group) == 0 {
            return true;
        }
        let list = group["hooks"].as_array_mut().expect("counted above");
        list.retain(|hook| !is_skiman_hook(hook));
        removed = true;
        !list.is_empty()
    });
    if prune && removed && groups.is_empty() {
        hooks.shift_remove("SessionStart");
        if hooks.is_empty() {
            root.shift_remove("hooks");
        }
    }
    removed
}
