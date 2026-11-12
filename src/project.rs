//! Project files (.gprproj, JSON): data file paths, processing, GPS, picks, CRS and basemaps.

use crate::app::App;
use crate::crs::CrsSpec;
use crate::model::{GpsTrack, LinePick, Marker, MarkerType};
use crate::model::Profile;
use crate::processing::{Ctx, Progress, StepEntry};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize)]
struct ProjProfile {
    id: u32,
    name: String,
    path: PathBuf,
    channel: usize,
    dx: f32,
    velocity: f32,
    freq_mhz: f32,
    use_gps_distance: bool,
    steps: Vec<StepEntry>,
    gps: Option<GpsTrack>,
}

#[derive(Serialize, Deserialize)]
struct ProjBasemap {
    path: PathBuf,
    crs: CrsSpec,
    visible: bool,
    opacity: f32,
}

#[derive(Serialize, Deserialize)]
pub struct ProjFile {
    version: u32,
    crs: CrsSpec,
    profiles: Vec<ProjProfile>,
    marker_types: Vec<MarkerType>,
    markers: Vec<Marker>,
    lines: Vec<LinePick>,
    basemaps: Vec<ProjBasemap>,
    next_pick_id: u32,
    import: crate::formats::ImportOptions,
}

pub fn save(app: &App, path: &Path) -> Result<(), String> {
    let f = ProjFile {
        version: 1,
        crs: app.project_crs.clone(),
        profiles: app
            .profiles
            .iter()
            .map(|p| ProjProfile {
                id: p.id,
                name: p.name.clone(),
                path: p.path.clone(),
                channel: p.channel,
                dx: p.dx,
                velocity: p.velocity,
                freq_mhz: p.freq_mhz,
                use_gps_distance: p.use_gps_distance,
                steps: p.steps.clone(),
                gps: p.gps.clone(),
            })
            .collect(),
        marker_types: app.marker_types.clone(),
        markers: app.markers.clone(),
        lines: app.lines.clone(),
        basemaps: app.basemaps.iter().map(|b| ProjBasemap { path: b.path.clone(), crs: b.crs.clone(), visible: b.visible, opacity: b.opacity }).collect(),
        next_pick_id: app.next_pick_id,
        import: app.import_opts.clone(),
    };
    let s = serde_json::to_string_pretty(&f).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| e.to_string())
}

pub fn parse(path: &Path) -> Result<ProjFile, String> {
    let s = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&s).map_err(|e| e.to_string())
}

/// Everything read for a project, ready to be put into the app.
pub struct Loaded {
    pub file: ProjFile,
    pub path: PathBuf,
    /// (id in the project file, profile with its processing already done)
    pub profiles: Vec<(u32, Profile)>,
    pub missing: Vec<String>,
}

/// Read every data file and run its processing (on a background thread).
pub fn load_profiles(mut file: ProjFile, path: PathBuf, prog: &std::sync::Arc<Progress>) -> Loaded {
    let mut profiles = Vec::new();
    let mut missing = Vec::new();
    let list = std::mem::take(&mut file.profiles);
    let n = list.len().max(1);
    for (k, pp) in list.into_iter().enumerate() {
        if prog.is_cancelled() {
            missing.push(pp.name.clone());
            continue;
        }
        prog.set_range(k as f32 / n as f32, (k + 1) as f32 / n as f32);
        let label = format!("profile {}/{n}: {}", k + 1, pp.name);
        prog.set_text(format!("{label} — reading"));
        let loaded = if pp.path.extension().and_then(|e| e.to_str()) == Some("demo") {
            crate::demo::make().into_iter().find(|p| p.name == pp.name).map(|p| vec![p]).ok_or_else(|| "demo".to_string())
        } else {
            crate::formats::load(&pp.path, &file.import)
        };
        match loaded {
            Ok(list) => {
                let Some(mut p) = list.into_iter().nth(pp.channel) else {
                    missing.push(pp.name.clone());
                    continue;
                };
                p.name = pp.name;
                p.dx = pp.dx;
                p.velocity = pp.velocity;
                p.freq_mhz = pp.freq_mhz;
                p.use_gps_distance = pp.use_gps_distance;
                p.steps = pp.steps;
                p.gps = pp.gps;
                if p.steps.iter().any(|e| e.enabled) {
                    let ctx = Ctx { dx: p.dx, freq_mhz: p.freq_mhz, progress: Some(prog.clone()), label };
                    let (sec, log) = crate::processing::run_all(&p.raw, &p.steps, &ctx);
                    p.proc = sec;
                    p.log = log;
                }
                profiles.push((pp.id, p));
            }
            Err(_) => missing.push(pp.path.display().to_string()),
        }
    }
    prog.set(1.0);
    Loaded { file, path, profiles, missing }
}

/// Replace the app's contents with a loaded project.
pub fn apply(app: &mut App, l: Loaded) -> String {
    let f = l.file;
    app.profiles.clear();
    app.views.clear();
    app.markers.clear();
    app.lines.clear();
    app.basemaps.clear();
    app.project_crs = f.crs;
    app.crs_text = match &app.project_crs {
        CrsSpec::Epsg(c) => c.to_string(),
        CrsSpec::Proj(s) => s.clone(),
    };
    app.import_opts = f.import;
    app.marker_types = f.marker_types;
    let mut id_map = std::collections::HashMap::new();
    for (old, p) in l.profiles {
        let new_id = app.add_profile(p);
        id_map.insert(old, new_id);
        if let Some(i) = app.profile_index(new_id) {
            // Keep the velocity a time-depth step defines and fit the view to the processed data.
            let (sec, log) = (app.profiles[i].proc.clone(), app.profiles[i].log.clone());
            app.apply_processed(i, sec, log);
            let p = &app.profiles[i];
            let (id, ntr, ns) = (p.id, p.proc.ntr as f32, p.proc.ns as f32);
            app.views.insert(id, crate::app::ViewState { tr0: 0.0, tr1: ntr, s0: 0.0, s1: ns });
        }
    }
    for mut m in f.markers {
        if let Some(n) = id_map.get(&m.profile_id) {
            m.profile_id = *n;
            app.markers.push(m);
        }
    }
    for mut ln in f.lines {
        if let Some(n) = id_map.get(&ln.profile_id) {
            ln.profile_id = *n;
            app.lines.push(ln);
        }
    }
    app.next_pick_id = f.next_pick_id.max(app.markers.iter().map(|m| m.id).chain(app.lines.iter().map(|l| l.id)).max().unwrap_or(0) + 1);
    for b in f.basemaps {
        if b.path.exists() {
            app.add_basemap(b.path);
        }
    }
    app.reproject_all();
    app.active = if app.profiles.is_empty() { None } else { Some(0) };
    app.project_path = Some(l.path.clone());
    let name = l.path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    if l.missing.is_empty() {
        format!("Project {name} opened: {} profiles.", app.profiles.len())
    } else {
        format!("Project {name} opened; not loaded: {}", l.missing.join(", "))
    }
}
