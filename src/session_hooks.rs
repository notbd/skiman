//! Session-start hook entries in the agent tools' settings files (spec
//! "Hooks"): `init` adds one entry per file whose hook calls the recorded
//! binary with `sync --hook`, and `--session-hooks none` removes skiman's hooks.
//! Each tool's file has its own shape ([`HookShape`] in the tool table): a
//! group in `hooks.SessionStart`, or a named hook at the top level.
//!
//! A hook is skiman's when its command contains [`MARKER`], or is exactly the
//! command this run writes: a binary run through a renamed link (`sk`) has no
//! `skiman` in its command. A hook written under an earlier name is then not
//! recognised and has to be removed by hand; claiming every `… sync --hook`
//! would also claim, and later remove, another program's hook.
//!
//! A settings file is rewritten at its realpath (temporary file in that
//! directory, then rename), keeping every other key and entry but not its
//! formatting, and a number that 64 bits cannot hold loses digits. A file
//! that is not valid JSON, or whose shape around the entries is unexpected, is
//! never rewritten (exit 4, and the entry to add by hand is printed). A missing
//! settings file is created when the folder that holds it exists; when that
//! folder is missing the tool is not installed and nothing is written.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::Exit;
use crate::canon_lock::CanonLock;
use crate::commands;
use crate::failure::Failure;
use crate::tools::{AgentTool, HookShape, SessionHook, TOOLS};

/// Marks a hook command as skiman's.
pub const MARKER: &str = "skiman sync --hook";

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

/// Asked right before a settings file (the path in the tool table) is
/// written: holds the canon lock while the write happens, or says why the
/// file is left as it is (a settings file that resolves into a linked
/// folder, exit 3).
pub type Guard<'a> = &'a dyn Fn(&Path) -> Result<CanonLock, Failure>;

/// Applies `request`; `binary` is the recorded binary path. Every settings
/// file is handled on its own, and the exit is the highest of them.
pub fn apply(home: &Path, binary: &Path, request: Request, guard: Guard) -> Exit {
    let own = entry_command(binary);
    let hookable = TOOLS.iter().filter(|tool| tool.session_hook.is_some());
    match request {
        Request::Add(ids) => hookable
            .filter(|tool| ids.split(',').any(|id| id == tool.id))
            .map(|tool| add(home, tool, &own, guard))
            .fold(Exit::Success, Exit::combine),
        Request::Remove => hookable
            .map(|tool| remove(home, tool, &own, guard))
            .fold(Exit::Success, Exit::combine),
        Request::Ask => ask(home, hookable.collect(), &own, guard),
    }
}

/// Whether a settings file holds skiman's session-start entry, for `doctor`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Exactly the entry `init` adds for this binary, and no other skiman hook.
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
    let hook = session_hook(tool);
    let path = home.join(hook.file);
    let settings = match load(path.clone()) {
        Ok(settings) => settings,
        Err(Unusable::NotInstalled(_)) => return (path, Status::Missing),
        Err(Unusable::Broken(reason)) => return (path, Status::Unusable(reason)),
    };
    let own = entry_command(binary);
    let status = match ours(&settings.root, hook.shape, &own) {
        Err(what) => Status::Unusable(format!("{}: {what}", path.display())),
        Ok(Ours::None) => Status::Missing,
        Ok(Ours::Exactly(entry)) if entry == expected(hook.shape, &own) => Status::Installed,
        Ok(_) => Status::Different,
    };
    (path, status)
}

/// The handler skiman's entry holds.
fn handler(own: &str) -> Value {
    json!({
        "type": "command",
        "command": own,
        "timeout": TIMEOUT_SECONDS,
    })
}

/// The entry skiman adds: a group in `hooks.SessionStart`, or a named hook.
fn entry(shape: HookShape, own: &str) -> Value {
    match shape {
        HookShape::Grouped { matcher } => json!({ "matcher": matcher, "hooks": [handler(own)] }),
        HookShape::Named { .. } => json!({ "SessionStart": [handler(own)] }),
    }
}

/// The entry with where it sits: the named hook under its name.
fn expected(shape: HookShape, own: &str) -> Placed {
    let name = match shape {
        HookShape::Grouped { .. } => None,
        HookShape::Named { name } => Some(name.to_owned()),
    };
    Placed {
        name,
        entry: entry(shape, own),
    }
}

/// An entry and, for a named hook, its name.
#[derive(Debug, PartialEq)]
struct Placed {
    name: Option<String>,
    entry: Value,
}

/// skiman's hooks in a file.
enum Ours {
    None,
    /// One skiman handler, in this entry.
    Exactly(Placed),
    /// More than one.
    Several,
}

fn ours(root: &Value, shape: HookShape, own: &str) -> Result<Ours, String> {
    let mut found = entries(root, shape)?
        .into_iter()
        .map(|(name, entry)| (name, entry, hook_count(entry, shape, own)))
        .filter(|(_, _, count)| *count > 0);
    Ok(match (found.next(), found.next()) {
        (None, _) => Ours::None,
        (Some((name, entry, 1)), None) => Ours::Exactly(Placed {
            name: name.map(str::to_owned),
            entry: entry.clone(),
        }),
        _ => Ours::Several,
    })
}

/// The command of the hook skiman adds.
fn entry_command(binary: &Path) -> String {
    format!("{} sync --hook", shell_word(&binary.to_string_lossy()))
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

/// What saving a settings file through `serde_json` does to the rest of it.
const REWRITTEN: &str = "; skiman rewrote the whole file, so how it is written may have changed, and a number that 64 bits cannot hold may have lost digits";

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

fn session_hook(tool: &AgentTool) -> SessionHook {
    tool.session_hook.expect("tool has session-start hooks")
}

fn settings_path(home: &Path, tool: &AgentTool) -> PathBuf {
    home.join(session_hook(tool).file)
}

fn warn_unavailable(home: &Path, tool: &AgentTool, reason: &str) {
    err!(
        "warning: {reason}; skiman adds no entry to {}",
        settings_path(home, tool).display()
    );
}

/// Loads the settings file and inserts skiman's entry: `Ok(None)` when the
/// file already holds exactly that entry and no other skiman hook.
fn plan_addition(home: &Path, tool: &AgentTool, own: &str) -> Result<Option<Settings>, Unusable> {
    let shape = session_hook(tool).shape;
    let mut settings = load(settings_path(home, tool))?;
    let broken = |path: &Path, what| Unusable::Broken(format!("{}: {what}", path.display()));
    match ours(&settings.root, shape, own).map_err(|what| broken(&settings.path, what))? {
        Ours::Exactly(placed) if placed == expected(shape, own) => return Ok(None),
        _ => {}
    }
    strip(&mut settings.root, shape, own, false);
    insert(&mut settings.root, shape, own).map_err(|what| broken(&settings.path, what))?;
    Ok(Some(settings))
}

/// Adds skiman's entry to a file `strip` has cleared of skiman's hooks.
fn insert(root: &mut Value, shape: HookShape, own: &str) -> Result<(), String> {
    let Value::Object(root) = root else {
        unreachable!("checked by entries")
    };
    match shape {
        HookShape::Grouped { .. } => {
            let hooks = root
                .entry("hooks")
                .or_insert_with(|| Value::Object(Map::new()));
            let start = hooks
                .as_object_mut()
                .expect("checked by entries")
                .entry("SessionStart")
                .or_insert_with(|| Value::Array(Vec::new()));
            start
                .as_array_mut()
                .expect("checked by entries")
                .push(entry(shape, own));
        }
        HookShape::Named { name } => {
            if root.contains_key(name) {
                return Err(format!("the hook named `{name}` is not skiman's"));
            }
            root.insert(name.to_owned(), entry(shape, own));
        }
    }
    Ok(())
}

fn add(home: &Path, tool: &AgentTool, own: &str, guard: Guard) -> Exit {
    if let Some(reason) = tool.session_hook_unavailable {
        warn_unavailable(home, tool, reason);
        return Exit::Success;
    }
    let shape = session_hook(tool).shape;
    match plan_addition(home, tool, own) {
        Ok(None) => Exit::Success,
        Ok(Some(settings)) => write_addition(&settings, shape, own, guard),
        Err(Unusable::NotInstalled(dir)) => {
            err!(
                "warning: {} does not exist, so skiman adds no session-start hook entry for {}",
                dir.display(),
                tool.id
            );
            Exit::Success
        }
        Err(Unusable::Broken(reason)) => {
            print_by_hand(&reason, shape, own);
            Exit::OperationalError
        }
    }
}

fn write_addition(settings: &Settings, shape: HookShape, own: &str, guard: Guard) -> Exit {
    let _lock = match guard(&settings.path) {
        Ok(lock) => lock,
        Err(failure) => return failure.report(),
    };
    match settings.save() {
        Ok(()) => {
            out!(
                "added the session-start hook entry to {}{}",
                settings.path.display(),
                settings.permissions.as_ref().map_or("", |_| REWRITTEN)
            );
            Exit::Success
        }
        Err(e) => {
            print_by_hand(
                &format!("cannot write {}: {e}", settings.real.display()),
                shape,
                own,
            );
            Exit::OperationalError
        }
    }
}

/// Where skiman's entry goes in the file, and the entry as it is written there.
fn entry_to_add(shape: HookShape, own: &str) -> (&'static str, Value) {
    match shape {
        HookShape::Grouped { .. } => ("`hooks.SessionStart`", entry(shape, own)),
        HookShape::Named { name } => ("the top level", json!({ name: entry(shape, own) })),
    }
}

fn print_by_hand(reason: &str, shape: HookShape, own: &str) {
    let (place, entry) = entry_to_add(shape, own);
    err!(
        "error: {reason}, so skiman did not change it; add this entry to {place} by hand:\n{}",
        pretty(&entry)
    );
}

fn pretty(value: &Value) -> String {
    serde_json::to_string_pretty(value).expect("JSON value serialises")
}

fn remove(home: &Path, tool: &AgentTool, own: &str, guard: Guard) -> Exit {
    let path = settings_path(home, tool);
    let by_hand = |reason: String| {
        err!(
            "error: {reason}, so skiman did not change it; remove by hand every hook whose command contains `{MARKER}` or is `{own}`"
        );
        Exit::OperationalError
    };
    let mut settings = match load(path) {
        Ok(settings) if settings.permissions.is_none() => return Exit::Success,
        Ok(settings) => settings,
        Err(Unusable::NotInstalled(_)) => return Exit::Success,
        Err(Unusable::Broken(reason)) => return by_hand(reason),
    };
    if !strip(&mut settings.root, session_hook(tool).shape, own, true) {
        return Exit::Success;
    }
    let _lock = match guard(&settings.path) {
        Ok(lock) => lock,
        Err(failure) => return failure.report(),
    };
    match settings.save() {
        Ok(()) => {
            out!(
                "removed the session-start hook entries from {}{REWRITTEN}",
                settings.path.display()
            );
            Exit::Success
        }
        Err(e) => by_hand(format!("cannot write {}: {e}", settings.real.display())),
    }
}

/// Previews the entries `init` would add, asks, and adds them on "y". The
/// question can stay open for a long time, so on "y" each file is read again
/// and the entry is added to what it holds then.
fn ask(home: &Path, tools: Vec<&AgentTool>, own: &str, guard: Guard) -> Exit {
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
        match plan_addition(home, tool, own) {
            Ok(None) | Err(Unusable::NotInstalled(_)) => {}
            Ok(Some(settings)) => planned.push((tool, settings.path)),
            Err(Unusable::Broken(reason)) => {
                print_by_hand(&reason, session_hook(tool).shape, own);
                exit = Exit::OperationalError;
            }
        }
    }
    if planned.is_empty() {
        return exit;
    }
    for (tool, path) in &planned {
        let (place, entry) = entry_to_add(session_hook(tool).shape, own);
        err!(
            "skiman can add this session-start hook entry to {place} of {}, so starting a session refreshes distribution:\n{}",
            path.display(),
            pretty(&entry)
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
        for (tool, _) in &planned {
            exit = exit.combine(add(home, tool, own, guard));
        }
    } else {
        let ids: Vec<&str> = planned.iter().map(|(tool, _)| tool.id).collect();
        err!(
            "hint: no session-start hook entry added; add it later with `skiman init <path> --session-hooks {}`",
            ids.join(",")
        );
    }
    exit
}

/// Every entry that can hold session-start handlers, with its name for a
/// named hook, or what is wrong with the shape around them.
fn entries(root: &Value, shape: HookShape) -> Result<Vec<(Option<&str>, &Value)>, String> {
    let Value::Object(root) = root else {
        return Err("the top level is not a JSON object".to_owned());
    };
    match shape {
        HookShape::Grouped { .. } => {
            let Some(hooks) = root.get("hooks") else {
                return Ok(Vec::new());
            };
            let Value::Object(hooks) = hooks else {
                return Err("`hooks` is not a JSON object".to_owned());
            };
            match hooks.get("SessionStart") {
                None => Ok(Vec::new()),
                Some(Value::Array(groups)) => Ok(groups.iter().map(|g| (None, g)).collect()),
                Some(_) => Err("`hooks.SessionStart` is not a list".to_owned()),
            }
        }
        HookShape::Named { .. } => {
            let mut found = Vec::new();
            for (name, hook) in root {
                let Value::Object(fields) = hook else {
                    return Err(format!("the hook `{name}` is not a JSON object"));
                };
                match fields.get("SessionStart") {
                    None | Some(Value::Array(_)) => found.push((Some(name.as_str()), hook)),
                    Some(_) => return Err(format!("`{name}.SessionStart` is not a list")),
                }
            }
            Ok(found)
        }
    }
}

/// The handler list of an entry.
fn handlers(entry: &Value, shape: HookShape) -> Option<&Vec<Value>> {
    let key = match shape {
        HookShape::Grouped { .. } => "hooks",
        HookShape::Named { .. } => "SessionStart",
    };
    entry.get(key).and_then(Value::as_array)
}

/// How many of the entry's handlers are skiman's; `own` is the command this
/// run writes.
fn hook_count(entry: &Value, shape: HookShape, own: &str) -> usize {
    handlers(entry, shape).map_or(0, |list| {
        list.iter().filter(|h| is_skiman_hook(h, own)).count()
    })
}

fn is_skiman_hook(hook: &Value, own: &str) -> bool {
    hook.get("command")
        .and_then(Value::as_str)
        .is_some_and(|command| command.contains(MARKER) || command == own)
}

/// Removes skiman's handlers, and every entry they leave empty: a group with
/// no handler, or a named hook with no event left. With `prune`, also
/// removes `SessionStart` and `hooks` when this left them empty. Returns
/// whether anything was removed.
fn strip(root: &mut Value, shape: HookShape, own: &str, prune: bool) -> bool {
    let Value::Object(root) = root else {
        return false;
    };
    match shape {
        HookShape::Grouped { .. } => {
            let Some(Value::Object(hooks)) = root.get_mut("hooks") else {
                return false;
            };
            let Some(Value::Array(groups)) = hooks.get_mut("SessionStart") else {
                return false;
            };
            let mut removed = false;
            groups.retain_mut(|group| {
                if hook_count(group, shape, own) == 0 {
                    return true;
                }
                let list = group["hooks"].as_array_mut().expect("counted above");
                list.retain(|hook| !is_skiman_hook(hook, own));
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
        HookShape::Named { .. } => {
            let mut removed = false;
            root.retain(|_, hook| {
                if hook_count(hook, shape, own) == 0 {
                    return true;
                }
                let fields = hook.as_object_mut().expect("counted above");
                let list = fields["SessionStart"]
                    .as_array_mut()
                    .expect("counted above");
                list.retain(|h| !is_skiman_hook(h, own));
                if list.is_empty() {
                    fields.shift_remove("SessionStart");
                }
                removed = true;
                fields.keys().any(|key| key != "enabled")
            });
            removed
        }
    }
}
