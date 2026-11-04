//! The record of an import in progress, in machine state (spec Import):
//! `<state dir>/import/record.toml`, plus the temporary area
//! `<state dir>/import/area/` holding the built `upstream/` and
//! `skiman.lock` until the import finishes.
//!
//! `skiman import` writes the record once the user confirms, before it
//! writes anything into the skill repo, and updates each location's state
//! as it takes the location over. A finished import keeps its record with
//! `status = "finished"`; its area is removed. Resuming an import that
//! stopped part-way reads this record (#20).
//!
//! Hashes: a skill or snapshot folder is its tree hash; a single file is
//! `sha256:<hex of its content>`; an absent key means the file was absent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::failure::Failure;
use crate::fsutil::write_atomic;
use crate::paths::Paths;

pub const FORMAT_VERSION: i64 = 1;

/// `<state dir>/import/`.
pub fn dir(paths: &Paths) -> PathBuf {
    paths.state_dir().join("import")
}

/// `<state dir>/import/record.toml`.
pub fn file(paths: &Paths) -> PathBuf {
    dir(paths).join("record.toml")
}

/// `<state dir>/import/area/`: `upstream/<name>/` and `skiman.lock` as fetched.
pub fn area(paths: &Paths) -> PathBuf {
    dir(paths).join("area")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// Confirmed and not finished: the import stopped part-way, or is running.
    InProgress,
    Finished,
}

impl Status {
    fn as_str(self) -> &'static str {
        match self {
            Status::InProgress => "in-progress",
            Status::Finished => "finished",
        }
    }
}

/// The three files of the skill repo an import replaces, by hash.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoFiles {
    pub shared_body: Option<String>,
    pub manifest: Option<String>,
    pub lock: Option<String>,
}

/// One skill the import writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillHashes {
    /// Tree hash of `skills/<name>` as the import writes it.
    pub skill: String,
    /// Tree hash of `upstream/<name>`, for a third-party skill.
    pub upstream: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocationState {
    /// Not touched yet.
    Planned,
    /// The original is in the backup; the owned replacement is not created yet.
    BackedUp,
    /// The original is in the backup and the owned link (if any) is in place.
    TakenOver,
}

impl LocationState {
    fn as_str(self) -> &'static str {
        match self {
            LocationState::Planned => "planned",
            LocationState::BackedUp => "backed-up",
            LocationState::TakenOver => "taken-over",
        }
    }
}

/// A tool-directory entry or instruction location the import takes over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    pub tool: String,
    /// The entry or instruction location (realpath of its parent joined with its name).
    pub path: PathBuf,
    /// Where the original goes, relative to the backup folder.
    pub backup: PathBuf,
    /// The owned link created in its place; `None` when nothing replaces it
    /// (a foreign skill the import folder does not hold).
    pub link: Option<PathBuf>,
    pub state: LocationState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportRecord {
    pub status: Status,
    /// Realpath of the skill repo imported into.
    pub repo: PathBuf,
    /// Realpath of the import folder.
    pub folder: PathBuf,
    /// Tree hash of the whole import folder.
    pub folder_hash: String,
    /// The backup folder of this import.
    pub backup: PathBuf,
    /// The skill repo's `instructions/shared.md`, `skiman.toml` and `skiman.lock` before the import.
    pub pre_import: RepoFiles,
    /// The same three files as the import writes them (`skiman.toml` with the
    /// repo's `[tools]` merged in when the folder has none).
    pub candidate: RepoFiles,
    /// The `skiman.lock` text the import writes.
    pub lock_content: String,
    /// Every skill, by name.
    pub skills: BTreeMap<String, SkillHashes>,
    /// Every location to take over, in the order the import takes them over.
    pub locations: Vec<Location>,
}

impl ImportRecord {
    /// Writes the record (temporary file, then rename).
    pub fn save(&self, paths: &Paths) -> Result<(), Failure> {
        let file = file(paths);
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(dir(paths))?;
            write_atomic(&file, self.to_toml().as_bytes())
        };
        write().map_err(|e| {
            Failure::operational(format!(
                "cannot write the import record {}: {e}",
                file.display()
            ))
        })
    }

    fn to_toml(&self) -> String {
        let path = |p: &Path| value(p.display().to_string());
        let mut doc = DocumentMut::new();
        doc.insert("version", value(FORMAT_VERSION));
        doc.insert("status", value(self.status.as_str()));
        doc.insert("repo", path(&self.repo));
        doc.insert("folder", path(&self.folder));
        doc.insert("folder_hash", value(&self.folder_hash));
        doc.insert("backup", path(&self.backup));
        doc.insert("lock_content", value(&self.lock_content));
        for (key, files) in [
            ("pre_import", &self.pre_import),
            ("candidate", &self.candidate),
        ] {
            let mut table = Table::new();
            for (name, hash) in [
                ("shared_body", &files.shared_body),
                ("manifest", &files.manifest),
                ("lock", &files.lock),
            ] {
                if let Some(hash) = hash {
                    table.insert(name, value(hash));
                }
            }
            doc.insert(key, Item::Table(table));
        }
        let mut skills = Table::new();
        skills.set_implicit(true);
        for (name, hashes) in &self.skills {
            let mut table = Table::new();
            table.insert("skill", value(&hashes.skill));
            if let Some(upstream) = &hashes.upstream {
                table.insert("upstream", value(upstream));
            }
            skills.insert(name, Item::Table(table));
        }
        doc.insert("skills", Item::Table(skills));
        let mut locations = ArrayOfTables::new();
        for location in &self.locations {
            let mut table = Table::new();
            table.insert("tool", value(&location.tool));
            table.insert("path", path(&location.path));
            table.insert("backup", path(&location.backup));
            if let Some(link) = &location.link {
                table.insert("link", path(link));
            }
            table.insert("state", value(location.state.as_str()));
            locations.push(table);
        }
        doc.insert("locations", Item::ArrayOfTables(locations));
        doc.to_string()
    }

    /// The recorded import; `None` when there is no record.
    pub fn load(paths: &Paths) -> Result<Option<ImportRecord>, Failure> {
        let file = file(paths);
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => {
                return Err(Failure::operational(format!(
                    "cannot read the import record {}: {e}",
                    file.display()
                )));
            }
        };
        ImportRecord::parse(&text).map(Some).map_err(|e| {
            Failure::operational(format!(
                "the import record {} is damaged ({e}); move it away by hand",
                file.display()
            ))
        })
    }

    fn parse(text: &str) -> Result<ImportRecord, String> {
        let doc = text.parse::<DocumentMut>().map_err(|e| e.to_string())?;
        let root = doc.as_table();
        if root.get("version").and_then(Item::as_integer) != Some(FORMAT_VERSION) {
            return Err("unknown version".to_owned());
        }
        let status = match string(root, "status")?.as_str() {
            "in-progress" => Status::InProgress,
            "finished" => Status::Finished,
            other => return Err(format!("unknown status `{other}`")),
        };
        let files = |key: &str| -> Result<RepoFiles, String> {
            let table = table(root, key)?;
            let hash = |name| table.get(name).and_then(Item::as_str).map(str::to_owned);
            Ok(RepoFiles {
                shared_body: hash("shared_body"),
                manifest: hash("manifest"),
                lock: hash("lock"),
            })
        };
        let mut skills = BTreeMap::new();
        if let Some(table) = root.get("skills").and_then(Item::as_table) {
            for (name, item) in table {
                let skill = item.as_table().ok_or("`skills` holds a non-table")?;
                skills.insert(
                    name.to_owned(),
                    SkillHashes {
                        skill: string(skill, "skill")?,
                        upstream: skill
                            .get("upstream")
                            .and_then(Item::as_str)
                            .map(str::to_owned),
                    },
                );
            }
        }
        let mut locations = Vec::new();
        if let Some(array) = root.get("locations").and_then(Item::as_array_of_tables) {
            for table in array {
                let state = match string(table, "state")?.as_str() {
                    "planned" => LocationState::Planned,
                    "backed-up" => LocationState::BackedUp,
                    "taken-over" => LocationState::TakenOver,
                    other => return Err(format!("unknown location state `{other}`")),
                };
                locations.push(Location {
                    tool: string(table, "tool")?,
                    path: string(table, "path")?.into(),
                    backup: string(table, "backup")?.into(),
                    link: table.get("link").and_then(Item::as_str).map(PathBuf::from),
                    state,
                });
            }
        }
        Ok(ImportRecord {
            status,
            repo: string(root, "repo")?.into(),
            folder: string(root, "folder")?.into(),
            folder_hash: string(root, "folder_hash")?,
            backup: string(root, "backup")?.into(),
            pre_import: files("pre_import")?,
            candidate: files("candidate")?,
            lock_content: string(root, "lock_content")?,
            skills,
            locations,
        })
    }
}

fn string(table: &Table, key: &str) -> Result<String, String> {
    table
        .get(key)
        .and_then(Item::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("`{key}` is missing"))
}

fn table<'a>(table: &'a Table, key: &str) -> Result<&'a Table, String> {
    table
        .get(key)
        .and_then(Item::as_table)
        .ok_or_else(|| format!("`{key}` is missing"))
}
