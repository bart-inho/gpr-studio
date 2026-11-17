//! Small per-user settings file (last project, theme, export folder, map placement), kept in the user's config folder:
//! %APPDATA%\GPRStudio\settings.json on Windows, ~/.config/GPRStudio/settings.json elsewhere.

use crate::app::MapPlacement;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub last_project: Option<PathBuf>,
    #[serde(default)]
    pub dark_mode: bool,
    #[serde(default)]
    pub export_dir: Option<PathBuf>,
    #[serde(default)]
    pub map_placement: MapPlacement,
}

impl Settings {
    /// The default export folder, when it is set and still exists.
    pub fn export_dir(&self) -> Option<&Path> {
        self.export_dir.as_deref().filter(|d| d.is_dir())
    }
}

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("GPRStudio").join("settings.json"))
}

pub fn load() -> Settings {
    file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn save(s: &Settings) {
    let Some(f) = file() else { return };
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(f, t);
    }
}

pub fn last_project() -> Option<PathBuf> {
    load().last_project
}

pub fn set_last_project(p: &Path) {
    let mut s = load();
    s.last_project = Some(p.to_path_buf());
    save(&s);
}
