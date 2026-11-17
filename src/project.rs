//! Project files (.gprproj, JSON): data file paths, processing, GPS, picks, CRS and basemaps.

use crate::app::App;
use crate::crs::CrsSpec;
use crate::model::{GpsTrack, LinePick, Marker, MarkerType};
use crate::model::Profile;
use crate::processing::{Ctx, Progress, StepEntry};
use serde::{Deserialize, Serialize};
use std::io::Write;
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
    save_file(&build(app, path.parent()), path)
}

fn save_file(project: &ProjFile, path: &Path) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(project).map_err(|e| e.to_string())?;
    let name = path.file_name().ok_or("project path has no file name")?.to_string_lossy();
    for n in 0..100 {
        let temp = path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), n));
        let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&temp) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        };
        let result = file.write_all(&bytes).and_then(|_| file.sync_all());
        drop(file);
        let result = result.and_then(|_| std::fs::rename(&temp, path));
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        return result.map_err(|e| e.to_string());
    }
    Err("could not create a temporary project file".into())
}

/// The project as it would be saved (compared with the last save to know about unsaved changes).
pub fn snapshot(app: &App) -> String {
    serde_json::to_string(&build(app, None)).unwrap_or_default()
}

/// A path inside `base` is written relative to it with '/' (opens on any system); others stay absolute.
fn rel(base: Option<&Path>, p: &Path) -> PathBuf {
    match base.filter(|b| !b.as_os_str().is_empty()).and_then(|b| p.strip_prefix(b).ok()) {
        Some(r) => PathBuf::from(r.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")),
        None => p.to_path_buf(),
    }
}

/// A path from a project file: relative ones are inside the project's folder. A missing file is also
/// looked for by name in the project's data folders (project moved to another computer).
fn resolve(base: &Path, p: &Path) -> PathBuf {
    let direct = if p.is_absolute() { p.to_path_buf() } else { base.join(p) };
    if direct.exists() {
        return direct;
    }
    let s = p.to_string_lossy();
    let name = s.rsplit(['/', '\\']).next().unwrap_or("");
    ["data", "data/basemaps"].iter().map(|d| base.join(d).join(name)).find(|c| !name.is_empty() && c.is_file()).unwrap_or(direct)
}

fn build(app: &App, base: Option<&Path>) -> ProjFile {
    ProjFile {
        version: 2,
        crs: app.project_crs.clone(),
        profiles: app
            .profiles
            .iter()
            .map(|p| ProjProfile {
                id: p.id,
                name: p.name.clone(),
                path: rel(base, &p.path),
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
        basemaps: app.basemaps.iter().map(|b| ProjBasemap { path: rel(base, &b.path), crs: b.crs.clone(), visible: b.visible, opacity: b.opacity }).collect(),
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
    let mut list = std::mem::take(&mut file.profiles);
    let base = path.parent().unwrap_or(Path::new("")).to_path_buf();
    for pp in list.iter_mut() {
        pp.path = resolve(&base, &pp.path);
    }
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
    let base = l.path.parent().unwrap_or(Path::new("")).to_path_buf();
    for b in f.basemaps {
        let path = resolve(&base, &b.path);
        if path.exists() {
            app.add_basemap(path);
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

// ------------------------------------------------------------ Collect and save

/// Everything to copy for "Collect and save", planned on the UI thread (only quick file checks).
pub struct CollectJob {
    /// The new project file, `<folder>/<name>.gprproj`.
    pub project: PathBuf,
    /// (source, destination) for every file, once each.
    pub copies: Vec<(PathBuf, PathBuf)>,
    /// Profile and basemap files: original path → collected path.
    pub moved: Vec<(PathBuf, PathBuf)>,
    /// GPS files that had to be renamed: (old file name, new file name).
    pub gps_renamed: Vec<(String, String)>,
    pub skipped: Vec<String>,
    pub missing: Vec<String>,
}

pub struct CollectResult {
    pub copied: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
}

/// Files a reader in formats.rs opens next to the data file (with the extension case it looks for).
fn data_companions(p: &Path) -> Vec<PathBuf> {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
    let lists: Vec<Vec<String>> = match ext.to_ascii_lowercase().as_str() {
        "rd3" | "rd7" => vec![vec!["rad".into(), "RAD".into()]],
        "iprb" => vec![vec!["iprh".into(), "IPRH".into()]],
        "dt1" => vec![vec!["hd".into(), "HD".into()]],
        // REFLEXW: data .??T with header .??R of the same case (read_reflexw).
        e if e.len() == 3 && e.as_bytes()[0].is_ascii_digit() && e.as_bytes()[1].is_ascii_digit() && e.ends_with('t') => {
            vec![vec![format!("{}{}", &ext[..2], if ext.ends_with('T') { 'R' } else { 'r' })]]
        }
        _ => vec![],
    };
    lists.iter().filter_map(|l| l.iter().map(|e| p.with_extension(e)).find(|c| c.is_file())).collect()
}

/// World and aux files next to a basemap.
fn basemap_companions(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = ["tfw", "TFW", "tifw", "TIFW", "wld", "WLD", "prj", "PRJ", "aux.xml", "AUX.XML"].iter().map(|e| p.with_extension(e)).collect();
    v.extend(p.file_name().map(|n| p.with_file_name(format!("{}.aux.xml", n.to_string_lossy()))));
    let mut out: Vec<PathBuf> = Vec::new();
    for c in v {
        // Case-insensitive file systems report both spellings: keep one.
        let key = c.to_string_lossy().to_lowercase();
        if c.is_file() && !out.iter().any(|o| o.to_string_lossy().to_lowercase() == key) {
            out.push(c);
        }
    }
    out
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y)
}

/// Pick destination names for a file and its companions in `dir` (`stem`, then `stem_2`, …), so that
/// pairs keep one stem and nothing already there is overwritten. Returns (source, destination) pairs.
fn place(main: &Path, companions: &[PathBuf], dir: &Path, used: &mut std::collections::HashSet<String>) -> Vec<(PathBuf, PathBuf)> {
    let stem = main.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    let suffix = |f: &Path| f.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default().get(stem.len()..).unwrap_or("").to_string();
    let files: Vec<(PathBuf, String)> = std::iter::once(main.to_path_buf()).chain(companions.iter().cloned()).map(|f| { let s = suffix(&f); (f, s) }).collect();
    for k in 1.. {
        let st = if k == 1 { stem.clone() } else { format!("{stem}_{k}") };
        let pairs: Vec<(PathBuf, PathBuf)> = files.iter().map(|(f, sfx)| (f.clone(), dir.join(format!("{st}{sfx}")))).collect();
        let free = pairs.iter().all(|(src, d)| !used.contains(&d.to_string_lossy().to_lowercase()) && (!d.exists() || same_file(src, d)));
        if free {
            for (_, d) in &pairs {
                used.insert(d.to_string_lossy().to_lowercase());
            }
            return pairs;
        }
    }
    unreachable!()
}

/// Plan the copy of every file the project uses into `<folder>/data`.
pub fn plan_collect(app: &App, folder: &Path, name: &str) -> CollectJob {
    let (data, gps_dir, maps) = (folder.join("data"), folder.join("data").join("gps"), folder.join("data").join("basemaps"));
    let mut job = CollectJob { project: folder.join(format!("{name}.gprproj")), copies: vec![], moved: vec![], gps_renamed: vec![], skipped: vec![], missing: vec![] };
    let mut used = std::collections::HashSet::new();
    let mut gps_done = std::collections::HashSet::new();
    let mut gps_lists: std::collections::HashMap<Vec<PathBuf>, Vec<PathBuf>> = Default::default();
    for p in &app.profiles {
        if p.path.extension().and_then(|e| e.to_str()) == Some("demo") {
            job.skipped.push(format!("{} (demo profile, no file)", p.name));
            continue;
        }
        if job.moved.iter().any(|(src, _)| *src == p.path) {
            continue; // another channel of the same file
        }
        if !p.path.is_file() {
            job.missing.push(p.path.display().to_string());
            continue;
        }
        let pairs = place(&p.path, &data_companions(&p.path), &data, &mut used);
        job.moved.push((p.path.clone(), pairs[0].1.clone()));
        job.copies.extend(pairs);
        // GPS files: the one named in the track, and any with the data file's name.
        let dirs = crate::gps::search_dirs(&p.path, None);
        let mut found: Vec<PathBuf> = Vec::new();
        if let Some(g) = &p.gps {
            let fname = g.source.split(" [").next().unwrap_or("");
            found.extend(dirs.iter().map(|d| d.join(fname)).find(|c| !fname.is_empty() && c.is_file()));
        }
        let stem = p.path.file_stem().map(|s| s.to_string_lossy().to_lowercase());
        let list = gps_lists.entry(dirs.clone()).or_insert_with(|| crate::gps::list_gps_files(&dirs));
        found.extend(list.iter().filter(|f| f.file_stem().map(|s| s.to_string_lossy().to_lowercase()) == stem).cloned());
        for f in found {
            if !gps_done.insert(f.clone()) {
                continue;
            }
            let pair = place(&f, &[], &gps_dir, &mut used).remove(0);
            let (old, new) = (f.file_name().unwrap_or_default().to_string_lossy().to_string(), pair.1.file_name().unwrap_or_default().to_string_lossy().to_string());
            if old != new {
                job.gps_renamed.push((old, new));
            }
            job.copies.push(pair);
        }
    }
    for b in &app.basemaps {
        if job.moved.iter().any(|(src, _)| *src == b.path) {
            continue;
        }
        if !b.path.is_file() {
            job.missing.push(b.path.display().to_string());
            continue;
        }
        let pairs = place(&b.path, &basemap_companions(&b.path), &maps, &mut used);
        job.moved.push((b.path.clone(), pairs[0].1.clone()));
        job.copies.extend(pairs);
    }
    // A file already in place (collecting into the same folder again) is not copied onto itself.
    job.copies.retain(|(src, dst)| !same_file(src, dst));
    job
}

/// Copy the files (background thread). Nothing is ever deleted; a cancel stops after the current block.
pub fn run_collect(job: &CollectJob, prog: &Progress) -> CollectResult {
    use std::io::{Read, Write};
    let total: u64 = job.copies.iter().map(|(s, _)| s.metadata().map(|m| m.len()).unwrap_or(0)).sum::<u64>().max(1);
    let mut r = CollectResult { copied: 0, bytes: 0, errors: vec![] };
    let mut buf = vec![0u8; 8 << 20];
    for (src, dst) in &job.copies {
        if prog.is_cancelled() {
            break;
        }
        prog.set_text(src.file_name().unwrap_or_default().to_string_lossy().to_string());
        let res = (|| -> std::io::Result<()> {
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d)?;
            }
            let mut i = std::fs::File::open(src)?;
            let mut o = std::fs::File::create(dst)?;
            loop {
                let n = i.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                o.write_all(&buf[..n])?;
                r.bytes += n as u64;
                prog.set(r.bytes as f32 / total as f32);
                if prog.is_cancelled() {
                    return Err(std::io::Error::other("cancelled"));
                }
            }
            o.sync_all()
        })();
        match res {
            Ok(()) => r.copied += 1,
            Err(e) => r.errors.push(format!("{}: {e}", src.display())),
        }
    }
    prog.set(1.0);
    r
}

fn size_text(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{:.0} kB", b as f64 / 1024.0),
    }
}

/// After the copy: point the session at the collected files and save the project there.
/// Returns the summary (first line for the status bar).
pub fn finish_collect(app: &mut App, job: CollectJob, r: CollectResult, cancelled: bool) -> String {
    let mut lines = vec![];
    let ok = !cancelled && r.errors.is_empty();
    if ok {
        let remap_path = |path: &Path| job.moved.iter().find(|(src, _)| src == path).map(|(_, dst)| dst.clone()).unwrap_or_else(|| path.to_path_buf());
        let remap_gps = |source: &str| job.gps_renamed.iter().find(|(old, _)| source.starts_with(&format!("{old} [")))
            .map(|(old, new)| source.replacen(old, new, 1)).unwrap_or_else(|| source.to_string());
        let mut file = build(app, job.project.parent());
        for (saved, live) in file.profiles.iter_mut().zip(&app.profiles) {
            saved.path = rel(job.project.parent(), &remap_path(&live.path));
            if let Some(gps) = &mut saved.gps { gps.source = remap_gps(&gps.source); }
        }
        for (saved, live) in file.basemaps.iter_mut().zip(&app.basemaps) {
            saved.path = rel(job.project.parent(), &remap_path(&live.path));
        }
        match save_file(&file, &job.project) {
            Ok(()) => {
                for p in &mut app.profiles {
                    p.path = remap_path(&p.path);
                    if let Some(gps) = &mut p.gps { gps.source = remap_gps(&gps.source); }
                }
                for b in &mut app.basemaps { b.path = remap_path(&b.path); }
                app.saved_snapshot = Some(snapshot(app));
                if app.remember_projects {
                    crate::settings::set_last_project(&job.project);
                }
                app.project_path = Some(job.project.clone());
                lines.push(format!("Project collected: {} ({} files copied, {}).", job.project.display(), r.copied, size_text(r.bytes)));
            }
            Err(e) => lines.push(format!("Files copied, but the project could not be written: {e}")),
        }
    } else {
        let why = if cancelled { "cancelled" } else { "failed" };
        lines.push(format!("Collect {why}: {} files copied ({}); the project file was not written and the session is unchanged.", r.copied, size_text(r.bytes)));
    }
    for e in &r.errors {
        lines.push(format!("Error: {e}"));
    }
    for m in &job.missing {
        lines.push(format!("Missing (not copied): {m}"));
    }
    for s in &job.skipped {
        lines.push(format!("Skipped: {s}"));
    }
    lines.join("\n")
}

impl App {
    /// File › Collect and save project…: ask for the folder and name.
    pub fn collect_dialog(&mut self) {
        let name = self.project_path.as_ref().and_then(|p| p.file_stem()).map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "survey".into());
        let mut dlg = rfd::FileDialog::new().set_title("Collect and save project: folder and project name").set_file_name(&name);
        if let Some(d) = self.project_path.as_ref().and_then(|p| p.parent()) {
            dlg = dlg.set_directory(d);
        }
        let Some(f) = dlg.save_file() else { return };
        let name = f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(name);
        let folder = f.with_file_name(&name);
        let not_empty = std::fs::read_dir(&folder).map(|mut d| d.next().is_some()).unwrap_or(false);
        if not_empty {
            self.collect_confirm = Some(folder);
        } else {
            self.start_collect(folder);
        }
    }

    pub fn start_collect(&mut self, folder: PathBuf) {
        let name = folder.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "survey".into());
        let job = plan_collect(self, &folder, &name);
        self.task_queue.push_back(crate::tasks::Pending::Collect(Box::new(job)));
    }

    /// The question for a folder that is not empty, and the summary afterwards.
    pub fn collect_windows(&mut self, ctx: &egui::Context) {
        if let Some(folder) = self.collect_confirm.clone() {
            let mut choice = 0;
            egui::Modal::new(egui::Id::new("collect_confirm")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.heading("Folder is not empty");
                ui.label(format!("{} already contains files. Write the project into it? Existing files are kept; new files with the same name get a _2 ending.", folder.display()));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(egui::RichText::new("  Continue  ").strong()).fill(ui.visuals().selection.bg_fill)).clicked() {
                        choice = 1;
                    }
                    if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        choice = 2;
                    }
                });
            });
            if choice > 0 {
                self.collect_confirm = None;
            }
            if choice == 1 {
                self.start_collect(folder);
            }
        }
        let mut open = self.collect_report.is_some();
        if let Some(text) = &self.collect_report {
            egui::Window::new("Collect and save project").open(&mut open).resizable(false).show(ctx, |ui| {
                for l in text.lines() {
                    ui.label(l);
                }
            });
        }
        if !open {
            self.collect_report = None;
        }
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
mod collect_tests {
    use super::*;

    #[test]
    fn save_replaces_project_without_leaving_temporary_file() {
        let root = PathBuf::from("target/project-save-test");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("survey.gprproj");
        std::fs::write(&path, "old project").unwrap();
        let app = App::headless(vec![], None);
        save(&app, &path).unwrap();
        assert_eq!(parse(&path).unwrap().profiles.len(), 0);
        assert!(!root.join(format!(".survey.gprproj.{}.0.tmp", std::process::id())).exists());
    }

    #[test]
    fn failed_save_cleans_up_temporary_file() {
        let root = PathBuf::from("target/project-save-failure");
        std::fs::create_dir_all(root.join("survey.gprproj")).unwrap();
        let app = App::headless(vec![], None);
        assert!(save(&app, &root.join("survey.gprproj")).is_err());
        assert!(root.join("survey.gprproj").is_dir());
        assert!(!root.join(format!(".survey.gprproj.{}.0.tmp", std::process::id())).exists());
    }

    #[test]
    fn failed_collect_keeps_the_open_session_paths() {
        let root = PathBuf::from("target/collect-save-failure");
        let _ = std::fs::remove_dir_all(&root);
        let mut app = App::headless(vec![], None);
        app.remember_projects = false;
        app.add_profile(crate::demo::make().remove(0));
        let original = app.profiles[0].path.clone();
        let snapshot_before = snapshot(&app);
        app.saved_snapshot = Some(snapshot_before.clone());
        let job = CollectJob {
            project: root.join("missing/survey.gprproj"),
            copies: vec![],
            moved: vec![(original.clone(), root.join("data/line.dzt"))],
            gps_renamed: vec![],
            skipped: vec![],
            missing: vec![],
        };
        let result = CollectResult { copied: 1, bytes: 100, errors: vec![] };
        let msg = finish_collect(&mut app, job, result, false);
        assert!(msg.starts_with("Files copied, but the project could not be written"), "{msg}");
        assert_eq!(app.profiles[0].path, original);
        assert_eq!(app.saved_snapshot.as_deref(), Some(snapshot_before.as_str()));
        assert!(app.project_path.is_none());
    }

    fn write_dzt(path: &Path, ntr: usize, ns: usize, channels: usize) {
        let mut b = vec![0u8; 1024 + ntr * ns * channels * 2];
        b[0..2].copy_from_slice(&255u16.to_le_bytes());
        b[2..4].copy_from_slice(&1u16.to_le_bytes());
        b[4..6].copy_from_slice(&(ns as u16).to_le_bytes());
        b[6..8].copy_from_slice(&16u16.to_le_bytes());
        b[14..18].copy_from_slice(&20f32.to_le_bytes());
        b[26..30].copy_from_slice(&64f32.to_le_bytes());
        b[52..54].copy_from_slice(&(channels as u16).to_le_bytes());
        std::fs::write(path, b).unwrap();
    }

    fn write_mala(dir: &Path, stem: &str, ns: usize) {
        std::fs::write(dir.join(format!("{stem}.rad")), format!("SAMPLES:{ns}\nFREQUENCY:5000\nDISTANCE INTERVAL:0.05\n")).unwrap();
        std::fs::write(dir.join(format!("{stem}.rd3")), vec![1u8; ns * 2 * 30]).unwrap();
    }

    #[test]
    fn collected_project_opens_after_the_folder_is_moved() {
        let root = PathBuf::from("target/collect-test");
        let _ = std::fs::remove_dir_all(&root);
        let (a, b) = (root.join("src/a"), root.join("src/b"));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        // Two different header/data pairs with the same name, a two-channel file and its GPS file.
        write_mala(&a, "line", 64);
        write_mala(&b, "line", 32);
        write_dzt(&a.join("multi.dzt"), 20, 64, 2);
        std::fs::write(a.join("multi.dzg"), "$GSSIS,0\n").unwrap();
        // A small TIFF basemap with a world file.
        let tif = root.join("src/map.tif");
        let mut enc = tiff::encoder::TiffEncoder::new(std::fs::File::create(&tif).unwrap()).unwrap();
        enc.write_image::<tiff::encoder::colortype::Gray8>(4, 4, &[128u8; 16]).unwrap();
        std::fs::write(root.join("src/map.tfw"), "1\n0\n0\n-1\n2600000\n1200004\n").unwrap();

        let mut app = App::headless(vec![], None);
        app.remember_projects = false;
        app.import_files_now(vec![a.join("line.rd3"), b.join("line.rd3"), a.join("multi.dzt")]);
        assert_eq!(app.profiles.len(), 4, "{}", app.status);
        app.add_basemap(tif);
        app.wait_jobs();
        assert_eq!(app.basemaps.len(), 1, "{}", app.status);

        let job = plan_collect(&app, &root.join("proj"), "proj");
        let r = run_collect(&job, &Progress::new());
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(r.copied, 8); // 2×(rd3+rad), dzt, dzg, tif, tfw — the shared dzt once
        let msg = finish_collect(&mut app, job, r, false);
        assert!(msg.starts_with("Project collected"), "{msg}");
        let data = root.join("proj/data");
        for f in ["line.rd3", "line.rad", "line_2.rd3", "line_2.rad", "multi.dzt", "gps/multi.dzg", "basemaps/map.tif", "basemaps/map.tfw"] {
            assert!(data.join(f).is_file(), "{f} missing");
        }
        let text = std::fs::read_to_string(root.join("proj/proj.gprproj")).unwrap();
        assert!(text.contains("\"data/line_2.rd3\"") && text.contains("\"data/basemaps/map.tif\""), "{text}");

        // Move the folder (as if sent to another computer) and delete the originals.
        std::fs::rename(root.join("proj"), root.join("moved")).unwrap();
        std::fs::remove_dir_all(root.join("src")).unwrap();
        let path = root.join("moved/proj.gprproj");
        let loaded = load_profiles(parse(&path).unwrap(), path, &Progress::new());
        assert!(loaded.missing.is_empty(), "{:?}", loaded.missing);
        let mut reopened = App::headless(vec![], None);
        reopened.remember_projects = false;
        apply(&mut reopened, loaded);
        reopened.wait_jobs();
        assert_eq!(reopened.profiles.len(), 4);
        assert_eq!(reopened.basemaps.len(), 1, "{}", reopened.status);
        assert!(reopened.profiles.iter().all(|p| p.path.starts_with(root.join("moved/data"))));
        assert_eq!(reopened.profiles.iter().filter(|p| p.raw.ns == 32).count(), 1); // the renamed pair kept its own header
    }

    #[test]
    fn missing_absolute_path_falls_back_to_the_data_folder() {
        let root = PathBuf::from("target/collect-fallback");
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/x.dzt"), b"").unwrap();
        assert_eq!(resolve(&root, Path::new("/nowhere/else/x.dzt")), root.join("data/x.dzt"));
        assert_eq!(resolve(&root, Path::new("C:\\survey\\x.dzt")), root.join("data/x.dzt"));
        assert_eq!(resolve(&root, Path::new("data/x.dzt")), root.join("data/x.dzt"));
        assert_eq!(rel(Some(&root), &root.join("data").join("x.dzt")), PathBuf::from("data/x.dzt"));
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
