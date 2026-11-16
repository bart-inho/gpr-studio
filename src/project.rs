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
    #[serde(default)]
    dx_user_set: bool,
    #[serde(default)]
    show_topo: bool,
    #[serde(default)]
    reverse_display: bool,
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
    let s = serde_json::to_string_pretty(&build(app)).map_err(|e| e.to_string())?;
    std::fs::write(path, s).map_err(|e| e.to_string())
}

/// The project as it would be saved (compared with the last save to know about unsaved changes).
pub fn snapshot(app: &App) -> String {
    serde_json::to_string(&build(app)).unwrap_or_default()
}

fn build(app: &App) -> ProjFile {
    ProjFile {
        version: 2,
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
                dx_user_set: p.dx_user_set,
                show_topo: p.show_topo,
                reverse_display: p.reverse_display,
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
    }
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
    let mut remaining = std::collections::HashMap::<PathBuf, usize>::new();
    for pp in &list { *remaining.entry(pp.path.clone()).or_default() += 1; }
    let mut sources = std::collections::HashMap::<PathBuf, Vec<Option<Profile>>>::new();
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
            if pp.channel == 0 { crate::demo::make().into_iter().find(|p| p.name == pp.name).ok_or_else(|| "demo".to_string()) }
            else { Err("channel missing".to_string()) }
        } else {
            if remaining[&pp.path] > 1 && !sources.contains_key(&pp.path) {
                match crate::formats::load(&pp.path, &file.import) {
                    Ok(list) => { sources.insert(pp.path.clone(), list.into_iter().map(Some).collect()); }
                    Err(e) => { missing.push(format!("{}: {e}", pp.path.display())); continue; }
                }
            }
            sources.get_mut(&pp.path).and_then(|list| list.get_mut(pp.channel)).and_then(Option::take)
                .map(Ok)
                // A project may list the same channel twice; reload only that exceptional case.
                .unwrap_or_else(|| crate::formats::load(&pp.path, &file.import).and_then(|list| list.into_iter().nth(pp.channel).ok_or_else(|| "channel missing".to_string())))
        };
        match loaded {
            Ok(mut p) => {
                p.name = pp.name;
                // The file's own distance interval wins unless the spacing was typed in by hand.
                if pp.dx_user_set || !p.dx_from_file {
                    p.dx = pp.dx;
                    p.dx_user_set = pp.dx_user_set;
                }
                p.velocity = pp.velocity;
                p.freq_mhz = pp.freq_mhz;
                p.show_topo = pp.show_topo;
                p.reverse_display = pp.reverse_display;
                // Projects saved before version 2 always used GPS distance; now the file spacing comes first.
                p.use_gps_distance = if file.version >= 2 { pp.use_gps_distance } else { !p.dx_from_file };
                p.steps = pp.steps;
                p.gps = pp.gps;
                if p.steps.iter().any(|e| e.enabled) {
                    p.update_coords(&file.crs);
                    let (topography, topography_error) = crate::processing::topography_for_steps(&p, &file.crs);
                    p.proc = crate::model::Section::default();
                    let ctx = Ctx { dx: p.dx, freq_mhz: p.freq_mhz, progress: Some(prog.clone()), label, topography, topography_error };
                    let (sec, log) = crate::processing::run_all(&p.raw, &p.steps, &ctx);
                    p.proc = sec;
                    p.log = log;
                }
                profiles.push((pp.id, p));
            }
            Err(_) => missing.push(pp.path.display().to_string()),
        }
        if let Some(count) = remaining.get_mut(&pp.path) {
            *count -= 1;
            if *count == 0 { sources.remove(&pp.path); }
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
            let (sec, log) = {
                let p = &mut app.profiles[i];
                (std::mem::take(&mut p.proc), std::mem::take(&mut p.log))
            };
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
    app.show_topo = app.profiles.iter().any(|p| p.show_topo);
    for p in app.profiles.iter_mut() {
        p.show_topo = app.show_topo;
    }
    app.saved_snapshot = Some(snapshot(app));
    if app.remember_projects {
        crate::settings::set_last_project(&l.path);
    }
    let name = l.path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
    if l.missing.is_empty() {
        format!("Project {name} opened: {} profiles.", app.profiles.len())
    } else {
        format!("Project {name} opened; not loaded: {}", l.missing.join(", "))
    }
}

#[cfg(test)]
mod topographic_project_tests {
    use super::*;
    use crate::processing::Step;

    #[test]
    fn reopens_topographic_processing_and_preserves_acquisition_pick() {
        let mut app = App::headless(vec![], None);
        app.remember_projects = false;
        let profile = crate::demo::make().remove(0);
        let id = app.add_profile(profile);
        app.profiles[0].steps.push(StepEntry { enabled: true, step: Step::TopographicKirchhoff { velocity: 0.1, width: 20, t_start: 0.0, t_end: 1e9, gps_above_antenna_m: 0.2, max_gap_m: 2.0 } });
        app.reprocess(0);
        assert!(app.profiles[0].proc.depth_datum_m.is_some(), "{:?}", app.profiles[0].log);
        // Picking on the migrated view: time ↔ depth below datum round-trips, time 0 sits on the local surface.
        let p = &app.profiles[0];
        let (datum, z) = (p.proc.depth_datum_m.unwrap(), p.coord_at(10.0).unwrap()[2]);
        assert!((p.y_of_pick_time(10.0, 0.0) as f64 - (datum - (z - 0.2))).abs() < 1e-3);
        assert!((p.pick_time_of_y(10.0, p.y_of_pick_time(10.0, 15.0)) - 15.0).abs() < 1e-3);
        app.markers.push(Marker { id: 1, profile_id: id, type_idx: 0, trace: 10.0, time_ns: 15.0, amplitude: 1.0, velocity: None, comment: "original pick".into() });
        let path = PathBuf::from("target/topographic-reopen.gprproj");
        save(&app, &path).unwrap();
        let file = parse(&path).unwrap();
        let loaded = load_profiles(file, path, &Progress::new());
        let mut reopened = App::headless(vec![], None);
        reopened.remember_projects = false;
        apply(&mut reopened, loaded);
        assert_eq!(reopened.markers.len(), 1);
        assert_eq!(reopened.markers[0].time_ns, 15.0);
        assert!(reopened.profiles[0].proc.depth_datum_m.is_some(), "{:?}", reopened.profiles[0].log);
        let max_diff = reopened.profiles[0].proc.data.iter().zip(&app.profiles[0].proc.data)
            .filter(|(a, b)| a.is_finite() && b.is_finite())
            .map(|(a, b)| (a-b).abs()).fold(0.0f32, f32::max);
        assert!(max_diff < 1e-3, "reopened migration differs by {max_diff}");
        assert!(reopened.profiles[0].proc.data.iter().zip(&app.profiles[0].proc.data).all(|(a,b)| a.is_nan() == b.is_nan()));
        let pdf = PathBuf::from("target/topographic-report-test.pdf");
        assert!(crate::report::export(&reopened, &pdf).is_ok());
    }
}

#[cfg(test)]
mod performance_measurements {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn synthetic_multichannel_project() {
        let dir = PathBuf::from("target/perf-project");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("three_channels.dzt");
        let (ntr, ns, channels) = if std::env::var_os("GPR_PERF_LARGE").is_some() {
            (6000usize, 1024usize, 3usize)
        } else {
            (1200usize, 512usize, 3usize)
        };
        let mut bytes = vec![0u8; 1024 + ntr * ns * channels * 2];
        bytes[0..2].copy_from_slice(&255u16.to_le_bytes());
        bytes[2..4].copy_from_slice(&1u16.to_le_bytes());
        bytes[4..6].copy_from_slice(&(ns as u16).to_le_bytes());
        bytes[6..8].copy_from_slice(&16u16.to_le_bytes());
        bytes[14..18].copy_from_slice(&20f32.to_le_bytes());
        bytes[26..30].copy_from_slice(&128f32.to_le_bytes());
        bytes[52..54].copy_from_slice(&(channels as u16).to_le_bytes());
        for (i, pair) in bytes[1024..].chunks_exact_mut(2).enumerate() {
            pair.copy_from_slice(&((i * 37 % 65536) as u16).to_le_bytes());
        }
        std::fs::write(&path, &bytes).unwrap();
        let import = crate::formats::ImportOptions::default();
        let t = Instant::now();
        let source = crate::formats::load(&path, &import).unwrap();
        println!("SOURCE 3-channel DZT {ntr}x{ns} file {:.1} MiB, load {:.3} s", bytes.len() as f64 / 1_048_576.0, t.elapsed().as_secs_f64());
        let expected: Vec<_> = source.iter().map(|p| p.raw.trace(0)[2]).collect();
        std::hint::black_box(source);
        let profiles = (0..channels).map(|channel| ProjProfile {
            id: channel as u32 + 1, name: format!("channel {channel}"), path: path.clone(), channel,
            dx: 0.05, velocity: 0.1, freq_mhz: 500.0, use_gps_distance: false,
            dx_user_set: false, show_topo: false, reverse_display: false,
            steps: vec![StepEntry { enabled: true, step: crate::processing::Step::Dewow { window_ns: 5.0 } }], gps: None,
        }).collect();
        let file = ProjFile {
            version: 2, crs: CrsSpec::Epsg(2056), profiles,
            marker_types: crate::model::default_marker_types(), markers: vec![], lines: vec![], basemaps: vec![],
            next_pick_id: 1, import,
        };
        let t = Instant::now();
        let loaded = load_profiles(file, dir.join("project.gprproj"), &Progress::new());
        println!("PROJECT 3 profiles ready {:.3} s, loaded {}", t.elapsed().as_secs_f64(), loaded.profiles.len());
        assert_eq!(loaded.profiles.len(), channels);
        let actual: Vec<_> = loaded.profiles.iter().map(|(_, p)| p.raw.trace(0)[2]).collect();
        assert_eq!(actual, expected);
    }
}
