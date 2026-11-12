//! Application state, menus, side panels and dialogs.

use crate::basemap::{self, Basemap};
use crate::crs::{self, CrsSpec};
use crate::formats::{self, ImportOptions, SegyTimeUnit};
use crate::gps::{self, GpsFormat};
use crate::model::*;
use crate::processing::{self, BgMode, Ctx, Divergence, Snap, Step, StepEntry, Taper, TimeZeroMode};
use crate::render::ColorMap;
use egui::{Color32, RichText};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PickMode {
    Navigate,
    Point,
    Hyperbola,
    Layer,
    Delete,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum YAxis {
    Time,
    Depth,
}

#[derive(Clone, Debug)]
pub struct ViewState {
    pub tr0: f32,
    pub tr1: f32,
    pub s0: f32,
    pub s1: f32,
}

#[derive(Clone, Debug)]
pub struct HyperbolaFit {
    pub trace: f32,
    pub time_ns: f32,
    pub velocity: f32,
}

pub struct GpsRow {
    pub profile_id: u32,
    pub name: String,
    pub path: String,
    pub score: u32,
    pub include: bool,
    pub has_embedded: bool,
    pub status: Option<Result<String, String>>,
}

pub struct GpsBatch {
    pub asked: bool,
    pub rows: Vec<GpsRow>,
    pub folder: String,
    pub format: GpsFormat,
    pub table_crs: String,
    pub use_embedded: bool,
}

pub struct StepEditor {
    pub step: Step,
    pub index: Option<usize>,
    pub spectrum: Vec<(f32, f32)>,
}

pub struct MapState {
    pub center: [f64; 2],
    pub scale: f64,
    pub fitted: bool,
    pub hover_world: Option<[f64; 2]>,
    pub hover_trace: Option<(u32, f32)>,
}

pub struct PdfOptions {
    pub a3: bool,
    pub range: PdfRange,
    pub chunk_m: f32,
    pub all_profiles: bool,
    pub basemap: bool,
    pub markers: bool,
    pub processing: bool,
    pub title: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PdfRange {
    Visible,
    Whole,
    Chunks,
}

pub struct BasemapJob {
    pub name: String,
    pub path: PathBuf,
    pub progress: Arc<AtomicU32>,
    pub rx: Receiver<Result<basemap::Loaded, String>>,
}

pub struct App {
    pub profiles: Vec<Profile>,
    pub active: Option<usize>,
    pub next_id: u32,
    pub project_crs: CrsSpec,
    pub crs_text: String,
    pub import_opts: ImportOptions,
    pub gps_queue: VecDeque<u32>,
    pub gps_dialog: Option<GpsBatch>,
    pub views: HashMap<u32, ViewState>,
    pub colormap: ColorMap,
    pub contrast: f32,
    pub y_axis: YAxis,
    pub wiggle: bool,
    pub section_tex: Option<egui::TextureHandle>,
    pub section_key: String,
    pub clip_cache: HashMap<(u32, u64), f32>,
    pub mode: PickMode,
    pub cur_type: usize,
    pub snap: Snap,
    pub snap_win: usize,
    pub auto_track: bool,
    pub marker_types: Vec<MarkerType>,
    pub markers: Vec<Marker>,
    pub lines: Vec<LinePick>,
    pub next_pick_id: u32,
    pub open_line: Option<LinePick>,
    pub hyperbola: Option<HyperbolaFit>,
    pub selected_pick: Option<u32>,
    pub hover: Option<(f32, f32)>,
    pub map: MapState,
    pub basemaps: Vec<Basemap>,
    pub basemap_jobs: Vec<BasemapJob>,
    pub show_map: bool,
    pub map_detached: bool,
    pub step_editor: Option<StepEditor>,
    pub show_marker_table: bool,
    pub show_marker_types: bool,
    pub show_crs: bool,
    pub show_import_opts: bool,
    pub show_gpkg: bool,
    pub show_pdf: bool,
    pub show_about: bool,
    pub show_header: bool,
    pub gpkg_opts: crate::gpkg::ExportOptions,
    pub pdf_opts: PdfOptions,
    pub status: String,
    pub screenshot: Option<PathBuf>,
    pub frame: u64,
    pub shot_sent: bool,
    pub project_path: Option<PathBuf>,
    pub task: Option<crate::tasks::Task>,
    pub task_queue: VecDeque<crate::tasks::Pending>,
    pub select_rect: Option<(egui::Pos2, egui::Pos2)>,
    pub show_segy: bool,
    pub segy_sel: Vec<bool>,
    pub segy_all: bool,
    pub confirm_close: bool,
    pub allow_close: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext, files: Vec<PathBuf>, screenshot: Option<PathBuf>) -> Self {
        cc.egui_ctx.set_visuals(egui::Visuals::light());
        let mut style = (*cc.egui_ctx.style()).clone();
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        cc.egui_ctx.set_style(style);
        Self::headless(files, screenshot)
    }

    pub fn headless(files: Vec<PathBuf>, screenshot: Option<PathBuf>) -> Self {
        let mut app = App {
            profiles: Vec::new(),
            active: None,
            next_id: 1,
            project_crs: CrsSpec::Epsg(2056),
            crs_text: "2056".into(),
            import_opts: ImportOptions::default(),
            gps_queue: VecDeque::new(),
            gps_dialog: None,
            views: HashMap::new(),
            colormap: ColorMap::Gray,
            contrast: 1.0,
            y_axis: YAxis::Time,
            wiggle: false,
            section_tex: None,
            section_key: String::new(),
            clip_cache: HashMap::new(),
            mode: PickMode::Navigate,
            cur_type: 0,
            snap: Snap::AbsMaximum,
            snap_win: 6,
            auto_track: true,
            marker_types: default_marker_types(),
            markers: Vec::new(),
            lines: Vec::new(),
            next_pick_id: 1,
            open_line: None,
            hyperbola: None,
            selected_pick: None,
            hover: None,
            map: MapState { center: [0.0, 0.0], scale: 1.0, fitted: false, hover_world: None, hover_trace: None },
            basemaps: Vec::new(),
            basemap_jobs: Vec::new(),
            show_map: true,
            map_detached: false,
            step_editor: None,
            show_marker_table: false,
            show_marker_types: false,
            show_crs: false,
            show_import_opts: false,
            show_gpkg: false,
            show_pdf: false,
            show_about: false,
            show_header: false,
            gpkg_opts: crate::gpkg::ExportOptions { profiles: true, markers: true, layers: true, trace_points: false, trace_step: 10 },
            pdf_opts: PdfOptions { a3: false, range: PdfRange::Visible, chunk_m: 20.0, all_profiles: false, basemap: true, markers: true, processing: true, title: String::new() },
            status: "Open GPR files with File > Open, or load the demo survey.".into(),
            screenshot,
            frame: 0,
            shot_sent: false,
            project_path: None,
            task: None,
            task_queue: VecDeque::new(),
            select_rect: None,
            show_segy: false,
            segy_sel: Vec::new(),
            segy_all: false,
            confirm_close: false,
            allow_close: false,
        };
        if app.screenshot.is_some() {
            app.load_demo(true);
        }
        let (tifs, data): (Vec<PathBuf>, Vec<PathBuf>) = files.into_iter().partition(|f| f.extension().map(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff")).unwrap_or(false));
        for t in tifs {
            app.add_basemap(t);
        }
        if !data.is_empty() {
            app.import_files_now(data);
        }
        app
    }

    // ------------------------------------------------------------ profiles

    pub fn active_profile(&self) -> Option<&Profile> {
        self.active.and_then(|i| self.profiles.get(i))
    }

    pub fn profile_index(&self, id: u32) -> Option<usize> {
        self.profiles.iter().position(|p| p.id == id)
    }

    pub fn add_profile(&mut self, mut p: Profile) -> u32 {
        p.id = self.next_id;
        self.next_id += 1;
        let id = p.id;
        p.update_coords(&self.project_crs);
        self.views.insert(id, ViewState { tr0: 0.0, tr1: p.proc.ntr as f32, s0: 0.0, s1: p.proc.ns as f32 });
        self.profiles.push(p);
        self.active = Some(self.profiles.len() - 1);
        id
    }

    pub fn load_demo(&mut self, with_processing: bool) {
        let first_id = self.next_id;
        for p in crate::demo::make() {
            let id = self.add_profile(p);
            if with_processing {
                let i = self.profile_index(id).unwrap();
                self.profiles[i].steps = vec![
                    StepEntry { enabled: true, step: Step::TimeZero { mode: TimeZeroMode::FirstBreak, shift_ns: 0.0, threshold_pct: 20.0 } },
                    StepEntry { enabled: true, step: Step::Dewow { window_ns: 3.0 } },
                    StepEntry { enabled: true, step: Step::BackgroundRemoval { mode: BgMode::WholeProfile, t_start: 0.0, t_end: 1e9 } },
                    StepEntry { enabled: true, step: Step::GainFunction { linear: 1.0, exponent_db_m: 4.0, t_start: 0.0, max_gain: 100.0 } },
                ];
                self.reprocess(i);
            }
        }
        if with_processing {
            if let Some(i) = self.profile_index(first_id) {
                let pid = self.profiles[i].id;
                self.markers.push(Marker { id: self.next_pick_id, profile_id: pid, type_idx: 0, trace: 120.0, time_ns: 12.0, amplitude: 0.0, velocity: Some(0.1), comment: "pipe".into() });
                self.next_pick_id += 1;
                self.markers.push(Marker { id: self.next_pick_id, profile_id: pid, type_idx: 1, trace: 440.0, time_ns: 9.0, amplitude: 0.0, velocity: None, comment: String::new() });
                self.next_pick_id += 1;
                let pts: Vec<(f32, f32)> = (0..600)
                    .step_by(20)
                    .map(|t| {
                        let x = t as f32 * 0.05;
                        let z = 1.45 + 0.18 * (x * 0.35).sin() + 0.04 * (x * 1.7).cos();
                        (t as f32, 2.0 * z / 0.1)
                    })
                    .collect();
                self.lines.push(LinePick { id: self.next_pick_id, profile_id: pid, type_idx: 6, points: pts, comment: "layer".into() });
                self.next_pick_id += 1;
                self.active = Some(i);
                if let Some(v) = self.views.get_mut(&pid) {
                    v.tr0 = 0.0;
                    v.tr1 = 400.0;
                }
            }
        }
        self.map.fitted = false;
        self.status = "Demo survey loaded (synthetic data, LV95 coordinates).".into();
    }

    /// Import on the calling thread (start-up and self-test); the window uses `import_files`.
    pub fn import_files_now(&mut self, files: Vec<PathBuf>) {
        let mut errors = Vec::new();
        let mut n = 0;
        let mut seen = std::collections::HashSet::new();
        for f in files {
            // A header and its data file picked together load once.
            let key = f.with_extension("").to_string_lossy().to_lowercase();
            if !seen.insert(key) {
                continue;
            }
            match formats::load(&f, &self.import_opts) {
                Ok(list) => {
                    for p in list {
                        let id = self.add_profile(p);
                        self.gps_queue.push_back(id);
                        n += 1;
                    }
                }
                Err(e) => errors.push(format!("{}: {e}", f.file_name().and_then(|s| s.to_str()).unwrap_or(""))),
            }
        }
        self.map.fitted = false;
        self.status = if errors.is_empty() { format!("Imported {n} profile(s).") } else { format!("Imported {n} profile(s). Problems: {}", errors.join(" | ")) };
    }

    /// Process on the calling thread (small jobs, start-up and self-test).
    pub fn reprocess(&mut self, i: usize) {
        let p = &self.profiles[i];
        let ctx = Ctx { dx: p.dx, freq_mhz: p.freq_mhz, ..Default::default() };
        let (proc, log) = processing::run_all(&p.raw, &p.steps, &ctx);
        self.apply_processed(i, proc, log);
    }

    pub fn apply_processed(&mut self, i: usize, proc: crate::model::Section, log: Vec<String>) {
        let p = &mut self.profiles[i];
        p.proc = proc;
        p.log = log;
        p.version += 1;
        // A time-depth step defines the velocity used for depth.
        if let Some(v) = p.steps.iter().rev().find_map(|e| if let (true, Step::TimeDepth { velocity, .. }) = (e.enabled, &e.step) { Some(*velocity) } else { None }) {
            p.velocity = v;
        }
        let (ns, ntr) = (p.proc.ns as f32, p.proc.ntr as f32);
        if let Some(v) = self.views.get_mut(&p.id) {
            v.s0 = v.s0.clamp(0.0, (ns - 1.0).max(0.0));
            v.s1 = v.s1.clamp(v.s0 + 1.0, ns.max(v.s0 + 1.0));
            v.tr1 = v.tr1.min(ntr.max(1.0));
            if v.s1 - v.s0 < 2.0 {
                v.s0 = 0.0;
                v.s1 = ns;
            }
        }
        if !self.profiles[i].log.is_empty() {
            self.status = self.profiles[i].log.join("; ");
        }
    }

    pub fn reproject_all(&mut self) {
        let crs = self.project_crs.clone();
        for p in self.profiles.iter_mut() {
            p.update_coords(&crs);
        }
        for b in self.basemaps.iter_mut() {
            b.mesh_cache = None;
            if let Some(d) = &mut b.detail {
                d.mesh = None;
            }
        }
        self.map.fitted = false;
    }

    // ------------------------------------------------------------ picks helpers

    pub fn pick_time_to_y(&self, p: &Profile, t: f32) -> f32 {
        if p.proc.depth_axis {
            p.depth_of_time(t)
        } else {
            t
        }
    }
    pub fn y_to_pick_time(&self, p: &Profile, y: f32) -> f32 {
        if p.proc.depth_axis {
            p.time_of_depth(y)
        } else {
            y
        }
    }

    pub fn delete_pick(&mut self, id: u32) {
        self.markers.retain(|m| m.id != id);
        self.lines.retain(|l| l.id != id);
        if self.selected_pick == Some(id) {
            self.selected_pick = None;
        }
    }

    pub fn finish_line(&mut self) {
        if let Some(l) = self.open_line.take() {
            if l.points.len() >= 2 {
                self.status = format!("Layer pick #{} saved ({} points).", l.id, l.points.len());
                self.lines.push(l);
            }
        }
    }

    // ------------------------------------------------------------ basemaps

    pub fn add_basemap(&mut self, path: PathBuf) {
        let progress = Arc::new(AtomicU32::new(0));
        let (tx, rx) = std::sync::mpsc::channel();
        let pr = progress.clone();
        let p2 = path.clone();
        std::thread::spawn(move || {
            let r = basemap::load(&p2, &|f| pr.store((f * 1000.0) as u32, Ordering::Relaxed));
            let _ = tx.send(r);
        });
        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("basemap").to_string();
        self.status = format!("Reading basemap {name}…");
        self.basemap_jobs.push(BasemapJob { name, path, progress, rx });
    }

    /// Wait for basemaps still being read (used without a window).
    pub fn wait_jobs(&mut self) {
        while !self.basemap_jobs.is_empty() {
            std::thread::sleep(std::time::Duration::from_millis(50));
            self.poll_jobs_inner();
        }
    }

    fn poll_jobs(&mut self, ctx: &egui::Context) {
        self.poll_jobs_inner();
        if !self.basemap_jobs.is_empty() {
            ctx.request_repaint();
        }
    }

    fn poll_jobs_inner(&mut self) {
        let mut done = Vec::new();
        for (i, j) in self.basemap_jobs.iter().enumerate() {
            if let Ok(r) = j.rx.try_recv() {
                done.push((i, r));
            }
        }
        for (i, r) in done.into_iter().rev() {
            let j = self.basemap_jobs.remove(i);
            match r {
                Ok(l) => {
                    let crs = l.info.crs.clone().unwrap_or_else(|| self.project_crs.clone());
                    let note = if l.info.crs.is_none() { format!(" (no CRS in file, assumed {})", self.project_crs.short()) } else { String::new() };
                    self.status = format!("Basemap {} loaded: {} x {} px, {}{note}.", j.name, l.info.width, l.info.height, crs.short());
                    self.basemaps.push(Basemap {
                        name: j.name,
                        path: j.path,
                        info: l.info,
                        crs,
                        overview: l.overview,
                        ov_factor: l.ov_factor,
                        style: l.style,
                        visible: true,
                        opacity: 1.0,
                        tex: None,
                        mesh_cache: None,
                        detail: None,
                        detail_pending: None,
                    });
                    if self.profiles.is_empty() {
                        self.map.fitted = false;
                    }
                }
                Err(e) => self.status = format!("Basemap {} could not be read: {e}", j.name),
            }
        }
    }

    // ------------------------------------------------------------ dialogs

    fn step_editor_ui(&mut self, ctx: &egui::Context) {
        let Some(mut ed) = self.step_editor.take() else { return };
        let mut open = true;
        let mut apply_active = false;
        let mut apply_all = false;
        let is_depth = self.active_profile().map(|p| p.proc.depth_axis).unwrap_or(false);
        let prof_v = self.active_profile().map(|p| p.velocity);
        egui::Window::new(format!("Processing: {}", ed.step.name())).open(&mut open).resizable(false).show(ctx, |ui| {
            if is_depth && ed.step.needs_time_axis() && ed.index.is_none() {
                ui.colored_label(Color32::from_rgb(180, 90, 0), "Note: the section is already in depth; this step will be skipped unless placed before the time-depth step.");
            }
            egui::Grid::new("params").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                let dv = |ui: &mut egui::Ui, label: &str, v: &mut f32, speed: f64, suffix: &str| {
                    ui.label(label);
                    ui.add(egui::DragValue::new(v).speed(speed).suffix(suffix));
                    ui.end_row();
                };
                match &mut ed.step {
                    Step::TimeZero { mode, shift_ns, threshold_pct } => {
                        ui.label("Method");
                        let label = match mode {
                            TimeZeroMode::FirstBreak => "First break (mean trace)",
                            TimeZeroMode::FirstBreakPerTrace => "First break (each trace)",
                            TimeZeroMode::Manual => "Manual shift",
                        };
                        egui::ComboBox::from_id_salt("tzmode").selected_text(label).show_ui(ui, |ui| {
                            ui.selectable_value(mode, TimeZeroMode::FirstBreak, "First break (mean trace)");
                            ui.selectable_value(mode, TimeZeroMode::FirstBreakPerTrace, "First break (each trace)");
                            ui.selectable_value(mode, TimeZeroMode::Manual, "Manual shift");
                        });
                        ui.end_row();
                        if *mode == TimeZeroMode::Manual {
                            dv(ui, "Move start time by", shift_ns, 0.05, " ns");
                        } else {
                            dv(ui, "Threshold (of max amplitude)", threshold_pct, 0.5, " %");
                        }
                    }
                    Step::Dewow { window_ns } => dv(ui, "Running mean window", window_ns, 0.1, " ns"),
                    Step::DcShift { t1, t2 } => {
                        dv(ui, "1. time", t1, 0.5, " ns");
                        dv(ui, "2. time", t2, 0.5, " ns");
                    }
                    Step::BackgroundRemoval { mode, t_start, t_end } => {
                        ui.label("Average trace from");
                        let mut sliding = matches!(mode, BgMode::Sliding(_));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut sliding, false, "whole profile");
                            ui.radio_value(&mut sliding, true, "running window");
                        });
                        ui.end_row();
                        if sliding {
                            let mut n = if let BgMode::Sliding(n) = mode { *n } else { 50 };
                            ui.label("Window (traces)");
                            ui.add(egui::DragValue::new(&mut n).range(2..=100000));
                            ui.end_row();
                            *mode = BgMode::Sliding(n);
                        } else {
                            *mode = BgMode::WholeProfile;
                        }
                        dv(ui, "Start time", t_start, 0.5, " ns");
                        dv(ui, "End time", t_end, 0.5, " ns");
                    }
                    Step::BandpassFrequency { f1, f2, f3, f4, taper } => {
                        dv(ui, "Low cut", f1, 5.0, " MHz");
                        dv(ui, "Lower plateau", f2, 5.0, " MHz");
                        dv(ui, "Upper plateau", f3, 5.0, " MHz");
                        dv(ui, "High cut", f4, 5.0, " MHz");
                        ui.label("Taper");
                        ui.horizontal(|ui| {
                            ui.radio_value(taper, Taper::Cos, "cosine");
                            ui.radio_value(taper, Taper::Cos2, "cosine²");
                        });
                        ui.end_row();
                    }
                    Step::BandpassButterworth { f_low, f_high, order } => {
                        dv(ui, "Lower cutoff", f_low, 5.0, " MHz");
                        dv(ui, "Upper cutoff", f_high, 5.0, " MHz");
                        ui.label("Order");
                        ui.add(egui::DragValue::new(order).range(1..=12));
                        ui.end_row();
                    }
                    Step::AgcGain { window_ns, scale, max_gain } => {
                        dv(ui, "Window length", window_ns, 0.5, " ns");
                        dv(ui, "Scaling factor", scale, 0.05, "");
                        dv(ui, "Max. gain", max_gain, 5.0, "");
                    }
                    Step::EnergyDecay { scale } => dv(ui, "Scaling factor", scale, 0.05, ""),
                    Step::GainFunction { linear, exponent_db_m, t_start, max_gain } => {
                        dv(ui, "Linear gain a'", linear, 0.05, "");
                        dv(ui, "Exponent b'", exponent_db_m, 0.1, " dB/m");
                        dv(ui, "Start time", t_start, 0.5, " ns");
                        dv(ui, "Max. gain", max_gain, 5.0, "");
                    }
                    Step::Divergence { kind } => {
                        ui.label("Losses");
                        ui.horizontal(|ui| {
                            ui.radio_value(kind, Divergence::Spherical, "spherical (t)");
                            ui.radio_value(kind, Divergence::Cylindrical, "line source (√t)");
                        });
                        ui.end_row();
                    }
                    Step::TraceNormalize => {
                        ui.label("Every trace is scaled to a maximum of 1.");
                        ui.end_row();
                    }
                    Step::RunningAverage { traces } => {
                        ui.label("Traces");
                        ui.add(egui::DragValue::new(traces).range(1..=999));
                        ui.end_row();
                    }
                    Step::DiffractionStack { velocity, width } => {
                        dv(ui, "Velocity", velocity, 0.001, " m/ns");
                        ui.label("Summation width (traces)");
                        ui.add(egui::DragValue::new(width).range(2..=4000));
                        ui.end_row();
                    }
                    Step::KirchhoffMigration { velocity, width, t_start, t_end } => {
                        dv(ui, "Velocity", velocity, 0.001, " m/ns");
                        ui.label("Summation width (traces)");
                        ui.add(egui::DragValue::new(width).range(2..=4000));
                        ui.end_row();
                        dv(ui, "Start time", t_start, 0.5, " ns");
                        dv(ui, "End time", t_end, 0.5, " ns");
                    }
                    Step::FkMigration { velocity, scale } => {
                        dv(ui, "Velocity", velocity, 0.001, " m/ns");
                        dv(ui, "Time window factor", scale, 0.1, "");
                    }
                    Step::TimeDepth { velocity, max_depth } => {
                        dv(ui, "Constant velocity", velocity, 0.001, " m/ns");
                        dv(ui, "Max. depth (0 = all)", max_depth, 0.1, " m");
                    }
                }
            });
            if let Some(pv) = prof_v {
                match &mut ed.step {
                    Step::DiffractionStack { velocity, .. } | Step::KirchhoffMigration { velocity, .. } | Step::FkMigration { velocity, .. } | Step::TimeDepth { velocity, .. } => {
                        if ui.button(format!("Use profile velocity ({pv:.3} m/ns)")).clicked() {
                            *velocity = pv;
                        }
                        ui.weak("Fit a hyperbola (toolbar: Hyperbola) to measure the velocity.");
                    }
                    _ => {}
                }
            }
            if matches!(ed.step, Step::BandpassFrequency { .. } | Step::BandpassButterworth { .. }) && !ed.spectrum.is_empty() {
                ui.separator();
                ui.label("Mean amplitude spectrum of the current data:");
                spectrum_plot(ui, &ed.spectrum, &ed.step);
            }
            ui.separator();
            ui.horizontal(|ui| {
                let label = if ed.index.is_some() { "Update step" } else { "Add to active profile" };
                if ui.button(RichText::new(label).strong()).clicked() {
                    apply_active = true;
                }
                if ed.index.is_none() && ui.button("Add to all profiles").clicked() {
                    apply_all = true;
                }
            });
        });
        if apply_active || apply_all {
            if let Some(a) = self.active {
                if let Some(idx) = ed.index {
                    if let Some(e) = self.profiles[a].steps.get_mut(idx) {
                        e.step = ed.step.clone();
                    }
                    { let id = self.profiles[a].id; self.request_process(vec![id]); }
                } else if apply_all {
                    for i in 0..self.profiles.len() {
                        self.profiles[i].steps.push(StepEntry { enabled: true, step: ed.step.clone() });
                        { let id = self.profiles[i].id; self.request_process(vec![id]); }
                    }
                } else {
                    self.profiles[a].steps.push(StepEntry { enabled: true, step: ed.step.clone() });
                    { let id = self.profiles[a].id; self.request_process(vec![id]); }
                }
            }
            self.section_key.clear();
            open = false;
        }
        if open {
            self.step_editor = Some(ed);
        }
    }

    pub fn open_step(&mut self, step: Step) {
        let spectrum = self.active_profile().map(|p| if !p.proc.depth_axis { processing::mean_spectrum(&p.proc) } else { vec![] }).unwrap_or_default();
        // Sensible defaults from the active profile.
        let mut step = step;
        if let Some(p) = self.active_profile() {
            let f = if p.freq_mhz > 0.0 { p.freq_mhz } else { 400.0 };
            match &mut step {
                Step::BandpassFrequency { f1, f2, f3, f4, .. } => {
                    *f1 = f * 0.25;
                    *f2 = f * 0.5;
                    *f3 = f * 1.5;
                    *f4 = f * 2.5;
                }
                Step::BandpassButterworth { f_low, f_high, .. } => {
                    *f_low = f * 0.35;
                    *f_high = f * 2.0;
                }
                Step::Dewow { window_ns } => *window_ns = (1000.0 / f).max(p.proc.dt),
                Step::DiffractionStack { velocity, .. } | Step::KirchhoffMigration { velocity, .. } | Step::FkMigration { velocity, .. } | Step::TimeDepth { velocity, .. } => *velocity = p.velocity,
                _ => {}
            }
        }
        self.step_editor = Some(StepEditor { step, index: None, spectrum });
    }

    fn crs_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_crs;
        let mut apply: Option<CrsSpec> = None;
        egui::Window::new("Coordinate system (map and exports)").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(format!("Current: {}", self.project_crs.label()));
            ui.separator();
            ui.label("Common systems:");
            egui::Grid::new("crslist").num_columns(2).show(ui, |ui| {
                for (k, (c, n)) in crs::COMMON.iter().enumerate() {
                    if ui.selectable_label(self.project_crs == CrsSpec::Epsg(*c), format!("EPSG:{c}  {n}")).clicked() {
                        apply = Some(CrsSpec::Epsg(*c));
                    }
                    if k % 2 == 1 {
                        ui.end_row();
                    }
                }
            });
            ui.separator();
            ui.label("Any EPSG code (about 6000 are built in) or a PROJ string (+proj=...):");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.crs_text).desired_width(300.0));
                if ui.button("Apply").clicked() {
                    match CrsSpec::parse(&self.crs_text) {
                        Some(c) => apply = Some(c),
                        None => self.status = format!("'{}' is not a known EPSG code or PROJ string.", self.crs_text),
                    }
                }
            });
            if let Some(w) = self.project_crs.proj4() {
                ui.weak(w);
            }
        });
        if let Some(c) = apply {
            match c.build() {
                Ok(_) => {
                    self.project_crs = c.clone();
                    self.crs_text = match &c {
                        CrsSpec::Epsg(e) => e.to_string(),
                        CrsSpec::Proj(s) => s.clone(),
                    };
                    self.reproject_all();
                    self.status = format!("Coordinate system set to {}.", c.label());
                }
                Err(e) => self.status = e,
            }
        }
        self.show_crs = open;
    }

    fn import_options_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_import_opts;
        egui::Window::new("Import options").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(RichText::new("SEG-Y").strong());
            ui.label("Sample interval in the binary header is stored in:");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.import_opts.segy_time_unit, SegyTimeUnit::Picoseconds, "picoseconds (GPR)");
                ui.radio_value(&mut self.import_opts.segy_time_unit, SegyTimeUnit::Nanoseconds, "nanoseconds");
                ui.radio_value(&mut self.import_opts.segy_time_unit, SegyTimeUnit::Microseconds, "microseconds (standard)");
            });
            ui.checkbox(&mut self.import_opts.segy_force_ibm, "Read 32-bit float samples as IBM floats");
            let mut t = match &self.import_opts.segy_coord_crs {
                CrsSpec::Epsg(c) => c.to_string(),
                CrsSpec::Proj(s) => s.clone(),
            };
            ui.horizontal(|ui| {
                ui.label("CRS of projected trace-header coordinates (EPSG):");
                if ui.add(egui::TextEdit::singleline(&mut t).desired_width(70.0)).changed() {
                    if let Some(c) = CrsSpec::parse(&t) {
                        self.import_opts.segy_coord_crs = c;
                    }
                }
            });
            ui.separator();
            ui.label(RichText::new("All formats").strong());
            ui.horizontal(|ui| {
                ui.label("Trace spacing when the file has none:");
                ui.add(egui::DragValue::new(&mut self.import_opts.default_dx).speed(0.005).range(0.001..=100.0).suffix(" m"));
            });
            ui.weak("Settings apply to files imported from now on.");
        });
        self.show_import_opts = open;
    }

    fn marker_types_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_marker_types;
        let mut remove = None;
        egui::Window::new("Marker types").open(&mut open).show(ctx, |ui| {
            egui::Grid::new("mtypes").striped(true).num_columns(5).show(ui, |ui| {
                ui.strong("Name");
                ui.strong("Colour");
                ui.strong("Symbol");
                ui.strong("Kind");
                ui.label("");
                ui.end_row();
                for (i, t) in self.marker_types.iter_mut().enumerate() {
                    ui.add(egui::TextEdit::singleline(&mut t.name).desired_width(140.0));
                    ui.color_edit_button_srgb(&mut t.color);
                    egui::ComboBox::from_id_salt(("sym", i)).selected_text(t.symbol.name()).width(80.0).show_ui(ui, |ui| {
                        for s in Symbol::ALL {
                            ui.selectable_value(&mut t.symbol, s, s.name());
                        }
                    });
                    egui::ComboBox::from_id_salt(("kind", i)).selected_text(if t.kind == MarkerKind::Point { "point" } else { "line" }).width(60.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut t.kind, MarkerKind::Point, "point");
                        ui.selectable_value(&mut t.kind, MarkerKind::Line, "line");
                    });
                    if ui.small_button("✖").on_hover_text("Remove type").clicked() {
                        remove = Some(i);
                    }
                    ui.end_row();
                }
            });
            if ui.button("➕ Add type").clicked() {
                self.marker_types.push(MarkerType { name: format!("Type {}", self.marker_types.len() + 1), color: [80, 80, 200], symbol: Symbol::Circle, kind: MarkerKind::Point });
            }
        });
        if let Some(i) = remove {
            let used = self.markers.iter().any(|m| m.type_idx == i) || self.lines.iter().any(|l| l.type_idx == i);
            if used || self.marker_types.len() <= 1 {
                self.status = "That marker type is in use and was kept.".into();
            } else {
                self.marker_types.remove(i);
                for m in self.markers.iter_mut() {
                    if m.type_idx > i {
                        m.type_idx -= 1;
                    }
                }
                for l in self.lines.iter_mut() {
                    if l.type_idx > i {
                        l.type_idx -= 1;
                    }
                }
                self.cur_type = self.cur_type.min(self.marker_types.len() - 1);
            }
        }
        self.show_marker_types = open;
    }

    fn marker_table_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_marker_table;
        let mut delete = None;
        let mut goto: Option<(u32, f32)> = None;
        let types = self.marker_types.clone();
        let profiles = &self.profiles;
        let selected = self.selected_pick;
        let mut new_sel = None;
        egui::Window::new("Picks").open(&mut open).default_size([760.0, 360.0]).show(ctx, |ui| {
            ui.label(format!("{} point markers, {} layer picks", self.markers.len(), self.lines.len()));
            egui::ScrollArea::both().show(ui, |ui| {
                egui::Grid::new("mtable").striped(true).num_columns(10).show(ui, |ui| {
                    for h in ["#", "Profile", "Type", "Distance m", "Time ns", "Depth m", "X / E", "Y / N", "Comment", ""] {
                        ui.strong(h);
                    }
                    ui.end_row();
                    for m in self.markers.iter_mut() {
                        let Some(p) = profiles.iter().find(|p| p.id == m.profile_id) else { continue };
                        if ui.selectable_label(selected == Some(m.id), m.id.to_string()).clicked() {
                            new_sel = Some(m.id);
                            goto = Some((m.profile_id, m.trace));
                        }
                        ui.label(&p.name);
                        egui::ComboBox::from_id_salt(("mt", m.id)).selected_text(types.get(m.type_idx).map(|t| t.name.clone()).unwrap_or_default()).width(110.0).show_ui(ui, |ui| {
                            for (i, t) in types.iter().enumerate().filter(|(_, t)| t.kind == MarkerKind::Point) {
                                ui.selectable_value(&mut m.type_idx, i, &t.name);
                            }
                        });
                        ui.label(format!("{:.2}", p.dist_at(m.trace)));
                        ui.label(format!("{:.2}", m.time_ns));
                        let v = m.velocity.unwrap_or(p.velocity);
                        ui.label(format!("{:.3}", v * m.time_ns / 2.0));
                        match p.coord_at(m.trace) {
                            Some(c) => {
                                ui.label(format!("{:.2}", c[0]));
                                ui.label(format!("{:.2}", c[1]));
                            }
                            None => {
                                ui.label("-");
                                ui.label("-");
                            }
                        }
                        ui.add(egui::TextEdit::singleline(&mut m.comment).desired_width(140.0));
                        if ui.small_button("🗑").clicked() {
                            delete = Some(m.id);
                        }
                        ui.end_row();
                    }
                    for l in self.lines.iter_mut() {
                        let Some(p) = profiles.iter().find(|p| p.id == l.profile_id) else { continue };
                        if ui.selectable_label(selected == Some(l.id), l.id.to_string()).clicked() {
                            new_sel = Some(l.id);
                            goto = Some((l.profile_id, l.points.first().map(|q| q.0).unwrap_or(0.0)));
                        }
                        ui.label(&p.name);
                        ui.label(types.get(l.type_idx).map(|t| t.name.clone()).unwrap_or_default());
                        let a = l.points.first().map(|q| q.0).unwrap_or(0.0);
                        let b = l.points.last().map(|q| q.0).unwrap_or(0.0);
                        ui.label(format!("{:.1}-{:.1}", p.dist_at(a), p.dist_at(b)));
                        let mean_t = l.points.iter().map(|q| q.1).sum::<f32>() / l.points.len().max(1) as f32;
                        ui.label(format!("ø {:.2}", mean_t));
                        ui.label(format!("ø {:.3}", p.velocity * mean_t / 2.0));
                        ui.label(format!("{} pts", l.points.len()));
                        ui.label("line");
                        ui.add(egui::TextEdit::singleline(&mut l.comment).desired_width(140.0));
                        if ui.small_button("🗑").clicked() {
                            delete = Some(l.id);
                        }
                        ui.end_row();
                    }
                });
            });
        });
        if new_sel.is_some() {
            self.selected_pick = new_sel;
        }
        if let Some(id) = delete {
            self.delete_pick(id);
        }
        if let Some((pid, tr)) = goto {
            self.focus_trace(pid, tr);
        }
        self.show_marker_table = open;
    }

    pub fn focus_trace(&mut self, pid: u32, tr: f32) {
        if let Some(i) = self.profile_index(pid) {
            self.active = Some(i);
            if let Some(v) = self.views.get_mut(&pid) {
                let w = v.tr1 - v.tr0;
                let ntr = self.profiles[i].proc.ntr as f32;
                v.tr0 = (tr - w / 2.0).clamp(0.0, (ntr - w).max(0.0));
                v.tr1 = v.tr0 + w;
            }
        }
    }

    fn header_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_header;
        egui::Window::new("File header").open(&mut open).show(ctx, |ui| {
            if let Some(p) = self.active_profile() {
                egui::Grid::new("hdr").striped(true).show(ui, |ui| {
                    let rows: Vec<(String, String)> = vec![
                        ("name".into(), p.name.clone()),
                        ("file".into(), p.path.display().to_string()),
                        ("format".into(), p.format.clone()),
                        ("traces".into(), p.raw.ntr.to_string()),
                        ("samples".into(), p.raw.ns.to_string()),
                        ("sample interval".into(), format!("{:.4} ns", p.raw.dt)),
                        ("time window".into(), format!("{:.2} ns", p.raw.dt * p.raw.ns as f32)),
                        ("trace spacing".into(), format!("{:.4} m", p.dx)),
                        ("antenna frequency".into(), format!("{:.0} MHz", p.freq_mhz)),
                        ("GPS".into(), p.gps.as_ref().map(|g| format!("{} ({} fixes, {})", g.source, g.fixes.len(), g.crs.short())).unwrap_or("none".into())),
                    ];
                    for (k, v) in rows.iter().chain(p.header.iter()) {
                        ui.label(k);
                        ui.label(v);
                        ui.end_row();
                    }
                });
            }
        });
        self.show_header = open;
    }

    fn about_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_about;
        egui::Window::new("Supported files").open(&mut open).show(ctx, |ui| {
            ui.label(RichText::new("GPR data").strong());
            egui::Grid::new("fmts").striped(true).show(ui, |ui| {
                for (n, e) in formats::SUPPORTED {
                    ui.label(*n);
                    ui.label(*e);
                    ui.end_row();
                }
            });
            ui.weak("IDS native .dt: export SEG-Y from the IDS software. Proceq GP8000/GP8800: export SEG-Y from the Proceq app.");
            ui.separator();
            ui.label(RichText::new("GPS").strong());
            for f in GpsFormat::ALL.iter().skip(1) {
                ui.label(format!("• {}", f.name()));
            }
            ui.label("• SEG-Y trace-header coordinates");
            ui.separator();
            ui.label(RichText::new("Basemaps").strong());
            ui.label("GeoTIFF (any size; stripped or tiled; 8/16-bit, float, palette) or TIFF + world file.");
        });
        self.show_about = open;
    }

    fn gpkg_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_gpkg;
        let mut go = false;
        let no_gps = self.profiles.iter().filter(|p| !p.has_coords()).count();
        egui::Window::new("Export GeoPackage").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label(format!("Coordinate system: {}", self.project_crs.label()));
            ui.checkbox(&mut self.gpkg_opts.profiles, "Profile tracks (lines with acquisition attributes)");
            ui.checkbox(&mut self.gpkg_opts.markers, "Point markers (type, depth, elevation, velocity, comment)");
            ui.checkbox(&mut self.gpkg_opts.layers, "Layer picks (lines and their vertices with depth)");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.gpkg_opts.trace_points, "Trace positions, every");
                ui.add(egui::DragValue::new(&mut self.gpkg_opts.trace_step).range(1..=10000));
                ui.label("traces");
            });
            if no_gps > 0 {
                ui.colored_label(Color32::from_rgb(180, 90, 0), format!("{no_gps} profile(s) have no GPS and are left out of the map layers."));
            }
            if ui.button(RichText::new("Save .gpkg…").strong()).clicked() {
                go = true;
            }
        });
        if go {
            if let Some(path) = rfd::FileDialog::new().add_filter("GeoPackage", &["gpkg"]).set_file_name("gpr_survey.gpkg").save_file() {
                match self.export_gpkg(&path) {
                    Ok(s) => {
                        self.status = format!("GeoPackage written: {} ({s}).", path.display());
                        open = false;
                    }
                    Err(e) => self.status = format!("GeoPackage export failed: {e}"),
                }
            }
        }
        self.show_gpkg = open;
    }

    pub fn export_gpkg(&self, path: &std::path::Path) -> Result<String, String> {
        use crate::gpkg::*;
        let hex = |c: [u8; 3]| format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2]);
        let profiles: Vec<ProfileRow> = self
            .profiles
            .iter()
            .map(|p| ProfileRow {
                name: p.name.clone(),
                file: p.path.display().to_string(),
                format: p.format.clone(),
                traces: p.raw.ntr,
                samples: p.raw.ns,
                dt_ns: p.raw.dt as f64,
                window_ns: (p.raw.dt * p.raw.ns as f32) as f64,
                length_m: p.length(),
                freq_mhz: p.freq_mhz as f64,
                velocity: p.velocity as f64,
                processing: p.steps.iter().filter(|e| e.enabled).map(|e| format!("{} ({})", e.step.name(), e.step.summary())).collect::<Vec<_>>().join("; "),
                coords: p.coords.clone(),
                dist: p.dist.clone(),
            })
            .collect();
        let markers: Vec<MarkerRow> = self
            .markers
            .iter()
            .filter_map(|m| {
                let p = self.profiles.iter().find(|p| p.id == m.profile_id)?;
                let t = self.marker_types.get(m.type_idx)?;
                let v = m.velocity.unwrap_or(p.velocity);
                let depth = (v * m.time_ns / 2.0) as f64;
                let c = p.coord_at(m.trace).unwrap_or([f64::NAN, f64::NAN, 0.0]);
                Some(MarkerRow {
                    id: m.id,
                    profile: p.name.clone(),
                    type_name: t.name.clone(),
                    trace: m.trace as f64,
                    distance_m: p.dist_at(m.trace),
                    time_ns: m.time_ns as f64,
                    depth_m: depth,
                    amplitude: m.amplitude as f64,
                    velocity: m.velocity.map(|v| v as f64),
                    comment: m.comment.clone(),
                    color: hex(t.color),
                    x: c[0],
                    y: c[1],
                    z: c[2] - depth,
                })
            })
            .collect();
        let lines: Vec<LineRow> = self
            .lines
            .iter()
            .filter_map(|l| {
                let p = self.profiles.iter().find(|p| p.id == l.profile_id)?;
                let t = self.marker_types.get(l.type_idx)?;
                let pts = l
                    .points
                    .iter()
                    .map(|(tr, time)| {
                        let depth = (p.velocity * time / 2.0) as f64;
                        let c = p.coord_at(*tr).unwrap_or([f64::NAN, f64::NAN, 0.0]);
                        [c[0], c[1], c[2] - depth, depth, *time as f64, p.dist_at(*tr)]
                    })
                    .collect();
                Some(LineRow { id: l.id, profile: p.name.clone(), type_name: t.name.clone(), comment: l.comment.clone(), color: hex(t.color), pts })
            })
            .collect();
        crate::gpkg::export(path, &self.project_crs, &self.gpkg_opts, &profiles, &markers, &lines)
    }

    fn pdf_window(&mut self, ctx: &egui::Context) {
        let mut open = self.show_pdf;
        let mut go = false;
        egui::Window::new("Export PDF report").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Title:");
                ui.text_edit_singleline(&mut self.pdf_opts.title);
            });
            ui.horizontal(|ui| {
                ui.label("Paper:");
                ui.radio_value(&mut self.pdf_opts.a3, false, "A4 landscape");
                ui.radio_value(&mut self.pdf_opts.a3, true, "A3 landscape");
            });
            ui.label("Section of the profile on each page:");
            ui.radio_value(&mut self.pdf_opts.range, PdfRange::Visible, "the part visible in the data view");
            ui.radio_value(&mut self.pdf_opts.range, PdfRange::Whole, "the whole profile on one page");
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.pdf_opts.range, PdfRange::Chunks, "the whole profile, one page every");
                ui.add(egui::DragValue::new(&mut self.pdf_opts.chunk_m).range(1.0..=10000.0).suffix(" m"));
            });
            ui.checkbox(&mut self.pdf_opts.all_profiles, "All profiles (otherwise only the active one)");
            ui.checkbox(&mut self.pdf_opts.basemap, "Basemap under the GPS tracks");
            ui.checkbox(&mut self.pdf_opts.markers, "Markers and layer picks (data view and map)");
            ui.checkbox(&mut self.pdf_opts.processing, "Processing sequence and pick table");
            ui.weak("Each page shows the radargram with its axes, the GPS map with the same section highlighted, a scale bar, north arrow and the profile information.");
            if ui.button(RichText::new("Save PDF…").strong()).clicked() {
                go = true;
            }
        });
        if go {
            if let Some(path) = rfd::FileDialog::new().add_filter("PDF", &["pdf"]).set_file_name("gpr_report.pdf").save_file() {
                match crate::report::export(self, &path) {
                    Ok(n) => {
                        self.status = format!("PDF written: {} ({n} pages).", path.display());
                        open = false;
                    }
                    Err(e) => self.status = format!("PDF export failed: {e}"),
                }
            }
        }
        self.show_pdf = open;
    }

    // ------------------------------------------------------------ panels

    fn open_files_dialog(&mut self) {
        let exts = formats::file_filter_exts();
        if let Some(files) = rfd::FileDialog::new().add_filter("GPR data", &exts).add_filter("All files", &["*"]).pick_files() {
            self.import_files(files);
        }
    }

    fn menu_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            egui::menu::bar(ui, |ui| {
                ui.menu_button("File", |ui| {
                    if ui.button("Open GPR files…").clicked() {
                        ui.close_menu();
                        self.open_files_dialog();
                    }
                    if ui.button("Import options…").clicked() {
                        self.show_import_opts = true;
                        ui.close_menu();
                    }
                    if ui.button("Load demo survey").clicked() {
                        self.load_demo(false);
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Open project…").clicked() {
                        ui.close_menu();
                        if let Some(f) = rfd::FileDialog::new().add_filter("GPR project", &["gprproj"]).pick_file() {
                            self.open_project(f);
                        }
                    }
                    if ui.button("Save project…").clicked() {
                        ui.close_menu();
                        if let Some(f) = rfd::FileDialog::new().add_filter("GPR project", &["gprproj"]).set_file_name("survey.gprproj").save_file() {
                            match crate::project::save(self, &f) {
                                Ok(()) => {
                                    self.status = format!("Project saved: {}", f.display());
                                    self.project_path = Some(f);
                                }
                                Err(e) => self.status = format!("Project could not be saved: {e}"),
                            }
                        }
                    }
                    ui.separator();
                    if ui.button("File header…").clicked() {
                        self.show_header = true;
                        ui.close_menu();
                    }
                    if ui.button("Supported files…").clicked() {
                        self.show_about = true;
                        ui.close_menu();
                    }
                    ui.separator();
                    if ui.button("Quit").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
                ui.menu_button("Processing", |ui| {
                    let groups: [(&str, &[usize]); 5] = [("Static", &[0]), ("1D filter", &[1, 2, 4, 5]), ("Gain", &[6, 7, 8, 9, 10]), ("2D filter", &[3, 11]), ("Migration / depth", &[12, 13, 14, 15])];
                    let all = Step::defaults();
                    for (g, idx) in groups {
                        ui.label(RichText::new(g).weak());
                        for i in idx {
                            if ui.button(all[*i].name()).clicked() {
                                self.open_step(all[*i].clone());
                                ui.close_menu();
                            }
                        }
                    }
                });
                ui.menu_button("Picking", |ui| {
                    for (m, n) in [(PickMode::Navigate, "Navigate / select"), (PickMode::Point, "Point marker"), (PickMode::Hyperbola, "Hyperbola (velocity)"), (PickMode::Layer, "Layer line"), (PickMode::Delete, "Delete pick")] {
                        if ui.radio(self.mode == m, n).clicked() {
                            self.mode = m;
                            ui.close_menu();
                        }
                    }
                    ui.separator();
                    if ui.button("Marker types…").clicked() {
                        self.show_marker_types = true;
                        ui.close_menu();
                    }
                    if ui.button("Pick table…").clicked() {
                        self.show_marker_table = true;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Map", |ui| {
                    ui.checkbox(&mut self.show_map, "Show GPS map");
                    ui.checkbox(&mut self.map_detached, "Map in its own window");
                    if ui.button("Add GeoTIFF basemap…").clicked() {
                        ui.close_menu();
                        if let Some(fs) = rfd::FileDialog::new().add_filter("GeoTIFF", &["tif", "tiff", "TIF", "TIFF"]).pick_files() {
                            for f in fs {
                                self.add_basemap(f);
                            }
                        }
                    }
                    if ui.button("Coordinate system…").clicked() {
                        self.show_crs = true;
                        ui.close_menu();
                    }
                    if ui.button("Fit map").clicked() {
                        self.map.fitted = false;
                        ui.close_menu();
                    }
                });
                ui.menu_button("Export", |ui| {
                    if ui.button("GeoPackage (profiles and markers)…").clicked() {
                        self.show_gpkg = true;
                        ui.close_menu();
                    }
                    if ui.add_enabled(self.active.is_some(), egui::Button::new("SEG-Y (.sgy) of chosen processing steps…")).clicked() {
                        self.show_segy = true;
                        ui.close_menu();
                    }
                    if ui.button("PDF report (data view + GPS map)…").clicked() {
                        self.show_pdf = true;
                        ui.close_menu();
                    }
                    if ui.button("Picks as CSV…").clicked() {
                        ui.close_menu();
                        if let Some(f) = rfd::FileDialog::new().add_filter("CSV", &["csv"]).set_file_name("picks.csv").save_file() {
                            match self.export_csv(&f) {
                                Ok(n) => self.status = format!("{n} picks written to {}", f.display()),
                                Err(e) => self.status = e,
                            }
                        }
                    }
                });
            });
        });
    }

    pub fn export_csv(&self, path: &std::path::Path) -> Result<usize, String> {
        let mut s = String::from("id,profile,type,kind,trace,distance_m,time_ns,depth_m,x,y,elevation_m,velocity_m_ns,comment\n");
        let mut n = 0;
        let f3 = |v: Option<f64>| v.map(|x| format!("{x:.3}")).unwrap_or_default();
        for m in &self.markers {
            let Some(p) = self.profiles.iter().find(|p| p.id == m.profile_id) else { continue };
            let v = m.velocity.unwrap_or(p.velocity);
            let d = v * m.time_ns / 2.0;
            let c = p.coord_at(m.trace);
            let tn = self.marker_types.get(m.type_idx).map(|t| t.name.as_str()).unwrap_or("");
            s.push_str(&format!(
                "{},{},{},point,{:.2},{:.3},{:.3},{:.3},{},{},{},{},\"{}\"\n",
                m.id,
                p.name,
                tn,
                m.trace + 1.0,
                p.dist_at(m.trace),
                m.time_ns,
                d,
                f3(c.map(|c| c[0])),
                f3(c.map(|c| c[1])),
                f3(c.map(|c| c[2] - d as f64)),
                m.velocity.map(|v| format!("{v:.4}")).unwrap_or_default(),
                m.comment.replace('"', "'")
            ));
            n += 1;
        }
        for l in &self.lines {
            let Some(p) = self.profiles.iter().find(|p| p.id == l.profile_id) else { continue };
            let tn = self.marker_types.get(l.type_idx).map(|t| t.name.as_str()).unwrap_or("");
            for (tr, t) in &l.points {
                let d = p.velocity * t / 2.0;
                let c = p.coord_at(*tr);
                s.push_str(&format!(
                    "{},{},{},line,{:.2},{:.3},{:.3},{:.3},{},{},{},,\"{}\"\n",
                    l.id,
                    p.name,
                    tn,
                    tr + 1.0,
                    p.dist_at(*tr),
                    t,
                    d,
                    f3(c.map(|c| c[0])),
                    f3(c.map(|c| c[1])),
                    f3(c.map(|c| c[2] - d as f64)),
                    l.comment.replace('"', "'")
                ));
            }
            n += 1;
        }
        std::fs::write(path, s).map_err(|e| e.to_string())?;
        Ok(n)
    }

    fn toolbar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("toolbar").show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (m, icon, tip) in [
                    (PickMode::Navigate, "✋ Navigate", "Drag to pan, wheel to zoom (Ctrl+wheel: time axis), double-click to fit"),
                    (PickMode::Point, "+ Point", "Click to place a point marker.\nRight-click a pick to delete it; Shift + right-drag deletes all picks in a box.\nOnly picks of the type selected in the toolbar are deleted."),
                    (PickMode::Hyperbola, "^ Hyperbola", "Click the apex, drag a flank to fit the velocity, Enter saves.\nRight-click a hyperbola to delete it."),
                    (PickMode::Layer, "~ Layer", "Click along a reflector; double-click (or Enter) ends the line and the next click starts a new one.\nBackspace removes the last point; right-click deletes a pick; Shift + right-drag deletes picks in a box."),
                    (PickMode::Delete, "✖ Delete", "Click a pick to delete it; Shift + right-drag deletes all picks in a box.\nOnly picks of the type selected in the toolbar are deleted."),
                ] {
                    if ui.selectable_label(self.mode == m, icon).on_hover_text(tip).clicked() {
                        if self.mode == PickMode::Layer && m != PickMode::Layer {
                            self.finish_line();
                        }
                        self.mode = m;
                    }
                }
                ui.separator();
                let want_kind = if self.mode == PickMode::Layer { MarkerKind::Line } else { MarkerKind::Point };
                if matches!(self.mode, PickMode::Point | PickMode::Layer | PickMode::Hyperbola) && self.marker_types.get(self.cur_type).map(|t| t.kind != want_kind).unwrap_or(true) {
                    if let Some(i) = self.marker_types.iter().position(|t| t.kind == want_kind) {
                        self.cur_type = i;
                    }
                }
                let cur = self.marker_types.get(self.cur_type).cloned();
                egui::ComboBox::from_id_salt("curtype")
                    .width(150.0)
                    .selected_text(cur.as_ref().map(|t| RichText::new(&t.name).color(Color32::from_rgb(t.color[0], t.color[1], t.color[2]))).unwrap_or_default())
                    .show_ui(ui, |ui| {
                        for (i, t) in self.marker_types.iter().enumerate() {
                            let txt = RichText::new(format!("{} ({})", t.name, if t.kind == MarkerKind::Point { "point" } else { "line" })).color(Color32::from_rgb(t.color[0], t.color[1], t.color[2]));
                            ui.selectable_value(&mut self.cur_type, i, txt);
                        }
                    });
                egui::ComboBox::from_id_salt("snap").width(120.0).selected_text(format!("Snap: {:?}", self.snap)).show_ui(ui, |ui| {
                    for s in [Snap::None, Snap::AbsMaximum, Snap::Maximum, Snap::Minimum, Snap::ZeroCrossing] {
                        ui.selectable_value(&mut self.snap, s, format!("{s:?}"));
                    }
                });
                ui.add(egui::DragValue::new(&mut self.snap_win).range(1..=100).prefix("±").suffix(" smp")).on_hover_text("Snap window");
                ui.checkbox(&mut self.auto_track, "Phase follow").on_hover_text("Layer picks follow the phase between clicks");
                ui.separator();
                egui::ComboBox::from_id_salt("cmap").width(110.0).selected_text(self.colormap.name()).show_ui(ui, |ui| {
                    for c in ColorMap::ALL {
                        ui.selectable_value(&mut self.colormap, c, c.name());
                    }
                });
                ui.add(egui::Slider::new(&mut self.contrast, 0.2..=20.0).logarithmic(true).text("contrast"));
                let depth_sec = self.active_profile().map(|p| p.proc.depth_axis).unwrap_or(false);
                if !depth_sec {
                    ui.selectable_value(&mut self.y_axis, YAxis::Time, "Time");
                    ui.selectable_value(&mut self.y_axis, YAxis::Depth, "Depth");
                }
                if let Some(a) = self.active {
                    let p = &mut self.profiles[a];
                    let r = ui.add(egui::DragValue::new(&mut p.velocity).speed(0.001).range(0.01..=0.3).prefix("v=").suffix(" m/ns")).on_hover_text("Velocity for depth (0.3 = air, 0.1 = moist soil)");
                    if r.changed() {
                        p.version += 1;
                    }
                }
                if ui.button("⛶ Fit").clicked() {
                    if let Some(p) = self.active_profile() {
                        let (id, ntr, ns) = (p.id, p.proc.ntr as f32, p.proc.ns as f32);
                        self.views.insert(id, ViewState { tr0: 0.0, tr1: ntr, s0: 0.0, s1: ns });
                    }
                }
            });
        });
    }

    fn left_panel(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("left").resizable(true).default_width(270.0).show(ctx, |ui| {
            ui.heading("Profiles");
            let mut remove = None;
            let mut gps_req = None;
            let mut open_files = false;
            let mut demo = false;
            egui::ScrollArea::vertical().id_salt("plist").max_height(200.0).show(ui, |ui| {
                for (i, p) in self.profiles.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let gps = if p.has_coords() { "•" } else { " " };
                        // Long names show their end, where the profile number and channel are.
                        let max = ((ui.available_width() - 70.0) / 6.5).max(10.0) as usize;
                        let chars: Vec<char> = p.name.chars().collect();
                        let shown = if chars.len() > max { format!("…{}", chars[chars.len() - (max - 1)..].iter().collect::<String>()) } else { p.name.clone() };
                        let r = ui.selectable_label(self.active == Some(i), format!("{gps} {shown}")).on_hover_text(format!("{}\n{}\n{} traces x {} samples\n{:.1} m\nGPS: {}", p.name, p.format, p.raw.ntr, p.raw.ns, p.length(), p.gps.as_ref().map(|g| g.source.clone()).unwrap_or("none".into())));
                        if r.clicked() {
                            self.active = Some(i);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("✖").on_hover_text("Close profile").clicked() {
                                remove = Some(i);
                            }
                            if ui.small_button("GPS").on_hover_text("Associate a GPS file").clicked() {
                                gps_req = Some(p.id);
                            }
                        });
                    });
                }
                if self.profiles.is_empty() {
                    ui.weak("No profiles yet.");
                    open_files = ui.button("Open GPR files…").clicked();
                    demo = ui.button("Load demo survey").clicked();
                }
            });
            if open_files {
                self.open_files_dialog();
            }
            if demo {
                self.load_demo(false);
            }
            if let Some(id) = gps_req {
                self.gps_queue.push_front(id);
            }
            if let Some(i) = remove {
                let id = self.profiles[i].id;
                self.profiles.remove(i);
                self.markers.retain(|m| m.profile_id != id);
                self.lines.retain(|l| l.profile_id != id);
                self.active = if self.profiles.is_empty() { None } else { Some(i.min(self.profiles.len() - 1)) };
            }
            if let Some(a) = self.active {
                let p = &mut self.profiles[a];
                ui.horizontal(|ui| {
                    if ui.checkbox(&mut p.use_gps_distance, "Distance from GPS").on_hover_text("Otherwise traces are spaced by the nominal trace spacing").changed() {
                        p.update_distance();
                    }
                    let r = ui.add(egui::DragValue::new(&mut p.dx).speed(0.001).range(0.001..=100.0).prefix("dx=").suffix(" m"));
                    if r.changed() {
                        p.update_distance();
                    }
                });
                ui.weak(format!("{} · {} tr · {:.3} ns · {:.1} m", p.format, p.raw.ntr, p.raw.dt, p.length()));
            }
            ui.separator();
            ui.heading("Processing sequence");
            let all = Step::defaults();
            ui.horizontal_wrapped(|ui| {
                for (i, short) in [(0, "Time 0"), (1, "Dewow"), (3, "Background"), (6, "AGC"), (8, "Gain f(t)"), (7, "Energy decay"), (4, "Bandpass"), (5, "Butterworth"), (13, "Kirchhoff"), (14, "FK Stolt"), (15, "Time-depth")] {
                    if ui.small_button(short).on_hover_text(all[i].name()).clicked() {
                        self.open_step(all[i].clone());
                    }
                }
            });
            ui.add_space(4.0);
            let mut changed = false;
            let mut edit = None;
            let mut del = None;
            let mut up = None;
            if let Some(a) = self.active {
                let n = self.profiles[a].steps.len();
                if n == 0 {
                    ui.weak("Raw data. Add steps with the buttons above or the Processing menu.");
                }
                egui::ScrollArea::vertical().id_salt("steps").max_height(260.0).show(ui, |ui| {
                    for (k, e) in self.profiles[a].steps.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.checkbox(&mut e.enabled, "").changed() {
                                changed = true;
                            }
                            ui.label(RichText::new(format!("{}. {}", k + 1, e.step.name())).strong()).on_hover_text(e.step.summary());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("✖").clicked() {
                                    del = Some(k);
                                }
                                if ui.small_button("✏").clicked() {
                                    edit = Some(k);
                                }
                                if k > 0 && ui.small_button("⏶").clicked() {
                                    up = Some(k);
                                }
                            });
                        });
                        ui.weak(format!("     {}", e.step.summary()));
                    }
                });
                if let Some(k) = del {
                    self.profiles[a].steps.remove(k);
                    changed = true;
                }
                if let Some(k) = up {
                    self.profiles[a].steps.swap(k, k - 1);
                    changed = true;
                }
                if changed {
                    { let id = self.profiles[a].id; self.request_process(vec![id]); }
                }
                if let Some(k) = edit {
                    let step = self.profiles[a].steps[k].step.clone();
                    let spectrum = processing::mean_spectrum(&self.profiles[a].raw);
                    self.step_editor = Some(StepEditor { step, index: Some(k), spectrum });
                }
                ui.horizontal(|ui| {
                    if ui.button("Apply sequence to all profiles").on_hover_text("Copy this processing sequence to every loaded profile").clicked() {
                        let steps = self.profiles[a].steps.clone();
                        for i in 0..self.profiles.len() {
                            if i != a {
                                self.profiles[i].steps = steps.clone();
                                { let id = self.profiles[i].id; self.request_process(vec![id]); }
                            }
                        }
                        self.status = format!("Sequence of {} steps applied to {} profiles.", steps.len(), self.profiles.len());
                    }
                    if n > 0 && ui.button("Raw").on_hover_text("Remove all steps").clicked() {
                        self.profiles[a].steps.clear();
                        { let id = self.profiles[a].id; self.request_process(vec![id]); }
                    }
                });
                if let Some(a) = self.active {
                    for l in &self.profiles[a].log {
                        ui.colored_label(Color32::from_rgb(170, 80, 0), l);
                    }
                }
            }
            ui.separator();
            ui.heading("Picks");
            ui.label(format!("{} markers · {} layer picks", self.markers.len(), self.lines.len()));
            ui.horizontal(|ui| {
                if ui.button("Pick table").clicked() {
                    self.show_marker_table = true;
                }
                if ui.button("Marker types").clicked() {
                    self.show_marker_types = true;
                }
            });
            if let Some(h) = self.hyperbola.clone() {
                ui.separator();
                ui.label(RichText::new(format!("Hyperbola: v = {:.4} m/ns", h.velocity)).strong());
                ui.label(format!("ε = {:.1}, apex depth {:.2} m", (0.2998 / h.velocity).powi(2), h.velocity * h.time_ns / 2.0));
                let mut save = false;
                let mut apply = false;
                let mut cancel = false;
                ui.horizontal(|ui| {
                    save = ui.button("Save marker").clicked();
                    apply = ui.button("Use v for profile").clicked();
                    cancel = ui.button("Cancel").clicked();
                });
                if apply {
                    if let Some(a) = self.active {
                        self.profiles[a].velocity = h.velocity;
                        self.profiles[a].version += 1;
                    }
                }
                if save {
                    self.save_hyperbola();
                }
                if cancel {
                    self.hyperbola = None;
                }
            }
        });
    }

    pub fn save_hyperbola(&mut self) {
        let (Some(a), Some(h)) = (self.active, self.hyperbola.take()) else { return };
        let p = &self.profiles[a];
        let idx = if self.marker_types.get(self.cur_type).map(|t| t.kind == MarkerKind::Point).unwrap_or(false) { self.cur_type } else { 0 };
        let y = self.pick_time_to_y(p, h.time_ns);
        let amp = p.proc.value(h.trace.round() as usize, p.proc.sample_of(y).round().max(0.0) as usize);
        let pid = p.id;
        self.markers.push(Marker { id: self.next_pick_id, profile_id: pid, type_idx: idx, trace: h.trace, time_ns: h.time_ns, amplitude: amp, velocity: Some(h.velocity), comment: String::new() });
        self.status = format!("Marker #{} saved with v = {:.4} m/ns.", self.next_pick_id, h.velocity);
        self.next_pick_id += 1;
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(self.project_crs.short()).strong()).on_hover_text(self.project_crs.label());
                ui.separator();
                self.task_status_ui(ui);
                if let (Some(p), Some((tr, y))) = (self.active_profile(), self.hover) {
                    let t = self.y_to_pick_time(p, y);
                    let mut s = format!("trace {:.0}  x {:.2} m  ", tr + 1.0, p.dist_at(tr));
                    if p.proc.depth_axis {
                        s.push_str(&format!("depth {:.3} m  ", y));
                    } else {
                        s.push_str(&format!("t {:.2} ns  depth {:.3} m  ", t, p.depth_of_time(t)));
                    }
                    let si = p.proc.sample_of(y).round();
                    if si >= 0.0 {
                        s.push_str(&format!("amp {:.1}  ", p.proc.value(tr as usize, si as usize)));
                    }
                    if let Some(c) = p.coord_at(tr) {
                        s.push_str(&format!("E {:.2}  N {:.2}  H {:.2}", c[0], c[1], c[2]));
                    }
                    ui.monospace(s);
                    ui.separator();
                }
                for j in &self.basemap_jobs {
                    ui.add(egui::Spinner::new());
                    ui.label(format!("{} {:.0}%", j.name, j.progress.load(Ordering::Relaxed) as f32 / 10.0));
                }
                ui.label(&self.status);
            });
        });
    }

    /// Ask to save the project when the window is closed.
    fn close_question_ui(&mut self, ctx: &egui::Context) {
        let close_req = ctx.input(|i| i.viewport().close_requested());
        if close_req && !self.allow_close && self.screenshot.is_none() && !self.profiles.is_empty() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }
        if !self.confirm_close {
            return;
        }
        let mut choice = 0; // 1 save, 2 don't save, 3 cancel
        egui::Modal::new(egui::Id::new("close_question")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.heading("Save project?");
            ui.add_space(4.0);
            let name = self.project_path.as_ref().and_then(|p| p.file_name()).and_then(|s| s.to_str()).map(|s| format!(" ({s})")).unwrap_or_default();
            ui.label(format!("Do you want to save the project{name} before closing GPR Studio?"));
            ui.weak("The project keeps the file list, processing, GPS, picks, coordinate system and basemaps.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("  Save…  ").strong()).fill(Color32::from_rgb(200, 225, 255))).clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    choice = 1;
                }
                if ui.button("Don't save").clicked() {
                    choice = 2;
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    choice = 3;
                }
            });
        });
        match choice {
            1 => {
                let mut dlg = rfd::FileDialog::new().add_filter("GPR project", &["gprproj"]).set_title("Save project");
                if let Some(p) = &self.project_path {
                    if let Some(d) = p.parent() {
                        dlg = dlg.set_directory(d);
                    }
                    if let Some(f) = p.file_name().and_then(|s| s.to_str()) {
                        dlg = dlg.set_file_name(f);
                    }
                } else {
                    dlg = dlg.set_file_name("survey.gprproj");
                }
                if let Some(f) = dlg.save_file() {
                    match crate::project::save(self, &f) {
                        Ok(()) => {
                            self.project_path = Some(f);
                            self.allow_close = true;
                            self.confirm_close = false;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        Err(e) => self.status = format!("Project could not be saved: {e}"),
                    }
                }
            }
            2 => {
                self.allow_close = true;
                self.confirm_close = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            3 => self.confirm_close = false,
            _ => {}
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        ctx.request_repaint();
        if self.frame >= 120 && !self.shot_sent && self.basemap_jobs.is_empty() && !self.busy() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
            self.shot_sent = true;
        }
        let shot = ctx.input(|i| i.raw.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None }));
        if let Some(img) = shot {
            let [w, h] = img.size;
            let mut bytes = Vec::with_capacity(w * h * 4);
            for c in &img.pixels {
                bytes.extend_from_slice(&c.to_srgba_unmultiplied());
            }
            if let Some(d) = path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            if let Err(e) = image::save_buffer(&path, &bytes, w as u32, h as u32, image::ExtendedColorType::Rgba8) {
                eprintln!("screenshot failed: {e}");
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.frame > 900 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}

fn spectrum_plot(ui: &mut egui::Ui, spec: &[(f32, f32)], step: &Step) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(380.0, 130.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, Color32::from_gray(250));
    let amax = spec.iter().map(|s| s.1).fold(0.0, f32::max).max(1e-20);
    let fmax = spec.iter().rev().find(|(_, a)| *a > amax * 0.01).map(|s| s.0 * 1.3).unwrap_or(1000.0).max(50.0);
    let pts: Vec<egui::Pos2> = spec.iter().filter(|(f, _)| *f <= fmax).map(|(f, a)| egui::pos2(rect.left() + f / fmax * rect.width(), rect.bottom() - a / amax * (rect.height() - 14.0))).collect();
    painter.add(egui::Shape::line(pts, egui::Stroke::new(1.5, Color32::from_rgb(30, 80, 200))));
    let mut fp = Vec::new();
    for k in 0..200 {
        let f = fmax * k as f32 / 199.0;
        let w = match step {
            Step::BandpassFrequency { f1, f2, f3, f4, taper } => processing::bandpass_weight(f, [*f1, *f2, *f3, *f4], *taper),
            Step::BandpassButterworth { f_low, f_high, order } => {
                let n = *order as i32;
                let hp = 1.0 / (1.0 + (f_low / f.max(1e-3)).powi(2 * n));
                let lp = 1.0 / (1.0 + (f / f_high.max(1e-3)).powi(2 * n));
                hp * lp
            }
            _ => 0.0,
        };
        fp.push(egui::pos2(rect.left() + f / fmax * rect.width(), rect.bottom() - w * (rect.height() - 14.0)));
    }
    painter.add(egui::Shape::line(fp, egui::Stroke::new(1.5, Color32::from_rgb(220, 60, 30))));
    painter.text(rect.left_top() + egui::vec2(4.0, 2.0), egui::Align2::LEFT_TOP, "spectrum (blue), filter (red)", egui::FontId::proportional(11.0), Color32::DARK_GRAY);
    painter.text(rect.right_bottom() - egui::vec2(4.0, 2.0), egui::Align2::RIGHT_BOTTOM, format!("{fmax:.0} MHz"), egui::FontId::proportional(11.0), Color32::DARK_GRAY);
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.frame += 1;
        self.poll_jobs(ctx);
        self.pump_tasks(ctx);
        self.menu_bar(ctx);
        self.toolbar(ctx);
        self.status_bar(ctx);
        self.left_panel(ctx);
        if self.show_map {
            if self.map_detached {
                let vid = egui::ViewportId::from_hash_of("map_window");
                let mut close = false;
                ctx.show_viewport_immediate(vid, egui::ViewportBuilder::default().with_title("GPS tracks and map").with_inner_size([700.0, 600.0]), |ctx, _class| {
                    egui::CentralPanel::default().show(ctx, |ui| self.map_ui(ui));
                    if ctx.input(|i| i.viewport().close_requested()) {
                        close = true;
                    }
                });
                if close {
                    self.map_detached = false;
                }
            } else {
                egui::SidePanel::right("map").resizable(true).default_width(470.0).min_width(260.0).show(ctx, |ui| self.map_ui(ui));
            }
        }
        egui::CentralPanel::default().show(ctx, |ui| self.dataview_ui(ui));
        self.gps_dialog_ui(ctx);
        self.step_editor_ui(ctx);
        self.crs_window(ctx);
        self.import_options_window(ctx);
        self.marker_types_window(ctx);
        self.marker_table_window(ctx);
        self.header_window(ctx);
        self.about_window(ctx);
        self.gpkg_window(ctx);
        self.pdf_window(ctx);
        self.segy_window(ctx);
        self.task_popup_ui(ctx);
        self.close_question_ui(ctx);
        self.handle_screenshot(ctx);
    }
}
