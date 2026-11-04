//! Every path skiman uses derives from `$HOME` and the XDG variables, never
//! from platform directories (spec: Language and packaging).

use std::fmt;
use std::path::{Path, PathBuf};

/// Base directories read from the environment once, at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    home: PathBuf,
    config_home: PathBuf,
    state_home: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathsError {
    HomeNotSet,
    HomeNotAbsolute(PathBuf),
}

impl fmt::Display for PathsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathsError::HomeNotSet => write!(f, "HOME is not set"),
            PathsError::HomeNotAbsolute(home) => {
                write!(f, "HOME is not an absolute path: {}", home.display())
            }
        }
    }
}

impl std::error::Error for PathsError {}

impl Paths {
    /// Reads `HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME` from the process
    /// environment. An XDG variable that is unset, empty or relative falls back
    /// to its default under `HOME`, as the XDG Base Directory spec requires.
    pub fn from_env() -> Result<Self, PathsError> {
        let home = PathBuf::from(
            std::env::var_os("HOME")
                .filter(|h| !h.is_empty())
                .ok_or(PathsError::HomeNotSet)?,
        );
        if !home.is_absolute() {
            return Err(PathsError::HomeNotAbsolute(home));
        }
        let xdg = |name: &str, default: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(default))
        };
        Ok(Paths {
            config_home: xdg("XDG_CONFIG_HOME", ".config"),
            state_home: xdg("XDG_STATE_HOME", ".local/state"),
            home,
        })
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// `${XDG_CONFIG_HOME:-$HOME/.config}/skiman/config.toml`.
    pub fn config_file(&self) -> PathBuf {
        self.config_home.join("skiman/config.toml")
    }

    /// `${XDG_STATE_HOME:-$HOME/.local/state}/skiman`: machine state.
    pub fn state_dir(&self) -> PathBuf {
        self.state_home.join("skiman")
    }
}
