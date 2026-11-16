//! Small per-user settings file (the last project), kept in the user's config folder:
//! %APPDATA%\GPRStudio\settings.json on Windows, ~/.config/GPRStudio/settings.json elsewhere.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Default, Serialize, Deserialize)]
struct Settings {
    #[serde(default)]
    last_project: Option<PathBuf>,
}

fn file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("GPRStudio").join("settings.json"))
}

fn read() -> Settings {
    file().and_then(|f| std::fs::read_to_string(f).ok()).and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

pub fn last_project() -> Option<PathBuf> {
    read().last_project
}

pub fn set_last_project(p: &Path) {
    let Some(f) = file() else { return };
    let mut s = read();
    s.last_project = Some(p.to_path_buf());
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    if let Ok(t) = serde_json::to_string_pretty(&s) {
        let _ = std::fs::write(f, t);
    }
}
