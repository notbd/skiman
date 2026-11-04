//! Machine state: the items reported at the last sync, per tool directory and
//! skill repo (spec Data formats, "Machine state"). `sync --hook` prints only
//! items missing from this set. Stored in `<state dir>/reported.toml`:
//! one key per scope (a realpath), holding the item ids as an array.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use toml_edit::{Array, DocumentMut, value};

use crate::fsutil;
use crate::paths::Paths;

/// Item ids per scope.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reported {
    scopes: BTreeMap<String, BTreeSet<String>>,
}

impl Reported {
    fn file(paths: &Paths) -> PathBuf {
        paths.state_dir().join("reported.toml")
    }

    /// The recorded set; a missing or unreadable file is an empty set (every item is new).
    pub fn load(paths: &Paths) -> Reported {
        let Ok(text) = std::fs::read_to_string(Reported::file(paths)) else {
            return Reported::default();
        };
        let Ok(doc) = text.parse::<DocumentMut>() else {
            return Reported::default();
        };
        let scopes = doc
            .iter()
            .filter_map(|(scope, item)| {
                let ids = item.as_array()?.iter().filter_map(|v| v.as_str());
                Some((scope.to_owned(), ids.map(str::to_owned).collect()))
            })
            .collect();
        Reported { scopes }
    }

    pub fn insert(&mut self, scope: &str, id: String) {
        self.scopes.entry(scope.to_owned()).or_default().insert(id);
    }

    pub fn contains(&self, scope: &str, id: &str) -> bool {
        self.scopes.get(scope).is_some_and(|ids| ids.contains(id))
    }

    /// Writes the set (temporary file, then rename).
    pub fn save(&self, paths: &Paths) -> std::io::Result<()> {
        let mut doc = DocumentMut::new();
        for (scope, ids) in &self.scopes {
            doc[scope.as_str()] = value(ids.iter().map(String::as_str).collect::<Array>());
        }
        let file = Reported::file(paths);
        std::fs::create_dir_all(paths.state_dir())?;
        fsutil::write_atomic(&file, doc.to_string().as_bytes())
    }
}
