//! Long work (processing, file import, opening a project) runs on a background thread so the
//! window stays responsive. Progress shows in the status bar, and in a popup with a progress bar
//! and a Cancel button when it takes longer than POPUP_AFTER.

use crate::app::App;
use crate::model::{Profile, Section};
use crate::processing::{self, Ctx, Progress};
use egui::RichText;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const POPUP_AFTER: Duration = Duration::from_secs(2);

pub enum Pending {
    Process(Vec<u32>),
    Import(Vec<PathBuf>),
    Project(PathBuf),
    Segy(Vec<crate::segy_export::SegyJob>),
}

pub enum Output {
    Processed(Vec<(u32, Section, Vec<String>)>),
    Imported(Vec<Profile>, Vec<String>),
    Project(Box<crate::project::Loaded>),
    Exported(Vec<String>, Vec<String>),
}

pub struct Task {
    pub title: String,
    pub progress: Arc<Progress>,
    pub started: Instant,
    pub rx: Receiver<Output>,
    pub cancellable: bool,
}

impl App {
    pub fn busy(&self) -> bool {
        self.task.is_some() || !self.task_queue.is_empty()
    }

    /// Reprocess these profiles in the background (merged with a waiting request).
    pub fn request_process(&mut self, ids: Vec<u32>) {
        for q in self.task_queue.iter_mut() {
            if let Pending::Process(v) = q {
                for id in ids {
                    if !v.contains(&id) {
                        v.push(id);
                    }
                }
                return;
            }
        }
        self.task_queue.push_back(Pending::Process(ids));
    }

    pub fn import_files(&mut self, files: Vec<PathBuf>) {
        self.task_queue.push_back(Pending::Import(files));
    }

    pub fn open_project(&mut self, path: PathBuf) {
        self.task_queue.push_back(Pending::Project(path));
    }

    fn start_task(&mut self, title: String, cancellable: bool, work: impl FnOnce(Arc<Progress>) -> Output + Send + 'static) {
        let progress = Progress::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let p2 = progress.clone();
        std::thread::spawn(move || {
            let out = work(p2);
            let _ = tx.send(out);
        });
        self.task = Some(Task { title, progress, started: Instant::now(), rx, cancellable });
    }

    pub fn pump_tasks(&mut self, ctx: &egui::Context) {
        if let Some(t) = &self.task {
            match t.rx.try_recv() {
                Ok(out) => {
                    let secs = t.started.elapsed().as_secs_f32();
                    let cancelled = t.progress.is_cancelled();
                    self.task = None;
                    self.finish_task(out, secs, cancelled);
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.status = format!("{} stopped unexpectedly.", t.title);
                    self.task = None;
                }
                Err(_) => {}
            }
        }
        if self.task.is_none() {
            if let Some(next) = self.task_queue.pop_front() {
                self.begin(next);
            }
        }
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(80));
        }
    }

    fn begin(&mut self, job: Pending) {
        match job {
            Pending::Process(ids) => {
                let jobs: Vec<(u32, String, Section, Vec<processing::StepEntry>, f32, f32)> = ids
                    .iter()
                    .filter_map(|id| self.profiles.iter().find(|p| p.id == *id))
                    .map(|p| (p.id, p.name.clone(), p.raw.clone(), p.steps.clone(), p.dx, p.freq_mhz))
                    .collect();
                if jobs.is_empty() {
                    return;
                }
                let n = jobs.len();
                let title = if n == 1 { format!("Processing {}", jobs[0].1) } else { format!("Processing {n} profiles") };
                self.start_task(title, true, move |prog| {
                    let mut out = Vec::new();
                    for (k, (id, name, raw, steps, dx, f)) in jobs.into_iter().enumerate() {
                        if prog.is_cancelled() {
                            break;
                        }
                        prog.set_range(k as f32 / n as f32, (k + 1) as f32 / n as f32);
                        let label = if n > 1 { format!("profile {}/{n}: {name}", k + 1) } else { name.clone() };
                        prog.set_text(label.clone());
                        let ctx = Ctx { dx, freq_mhz: f, progress: Some(prog.clone()), label };
                        let (sec, log) = processing::run_all(&raw, &steps, &ctx);
                        if prog.is_cancelled() {
                            break;
                        }
                        out.push((id, sec, log));
                    }
                    prog.set(1.0);
                    Output::Processed(out)
                });
            }
            Pending::Import(files) => {
                let opts = self.import_opts.clone();
                let n = files.len();
                self.start_task(format!("Importing {n} file(s)"), true, move |prog| {
                    let mut profiles = Vec::new();
                    let mut errors = Vec::new();
                    let mut seen = std::collections::HashSet::new();
                    for (k, f) in files.into_iter().enumerate() {
                        if prog.is_cancelled() {
                            break;
                        }
                        prog.set(k as f32 / n as f32);
                        prog.set_text(format!("{}/{n}: {}", k + 1, f.file_name().and_then(|s| s.to_str()).unwrap_or("")));
                        // A header and its data file picked together load once.
                        let key = f.with_extension("").to_string_lossy().to_lowercase();
                        if !seen.insert(key) {
                            continue;
                        }
                        match crate::formats::load(&f, &opts) {
                            Ok(list) => profiles.extend(list),
                            Err(e) => errors.push(format!("{}: {e}", f.file_name().and_then(|s| s.to_str()).unwrap_or(""))),
                        }
                    }
                    Output::Imported(profiles, errors)
                });
            }
            Pending::Segy(jobs) => {
                if jobs.is_empty() {
                    return;
                }
                let n = jobs.len();
                self.start_task(format!("Exporting SEG-Y ({n} profile(s))"), true, move |prog| crate::segy_export::run_and_report(jobs, prog));
            }
            Pending::Project(path) => {
                let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("project").to_string();
                match crate::project::parse(&path) {
                    Ok(file) => {
                        self.start_task(format!("Opening project {name}"), true, move |prog| Output::Project(Box::new(crate::project::load_profiles(file, path, &prog))));
                    }
                    Err(e) => self.status = format!("Project could not be opened: {e}"),
                }
            }
        }
    }

    fn finish_task(&mut self, out: Output, secs: f32, cancelled: bool) {
        match out {
            Output::Processed(list) => {
                let n = list.len();
                for (id, sec, log) in list {
                    if let Some(i) = self.profile_index(id) {
                        self.apply_processed(i, sec, log);
                    }
                }
                self.status = if cancelled { format!("Processing cancelled ({n} profile(s) finished).") } else { format!("Processing finished: {n} profile(s) in {secs:.1} s.") };
            }
            Output::Imported(profiles, errors) => {
                let n = profiles.len();
                for p in profiles {
                    let id = self.add_profile(p);
                    self.gps_queue.push_back(id);
                }
                self.map.fitted = false;
                let c = if cancelled { " (cancelled)" } else { "" };
                self.status = if errors.is_empty() { format!("Imported {n} profile(s){c}.") } else { format!("Imported {n} profile(s){c}. Problems: {}", errors.join(" | ")) };
            }
            Output::Exported(files, errors) => {
                let dir = files.first().and_then(|f| std::path::Path::new(f).parent().map(|d| d.display().to_string())).unwrap_or_default();
                let c = if cancelled { " (cancelled)" } else { "" };
                self.status = if errors.is_empty() {
                    format!("SEG-Y export{c}: {} file(s) written to {dir} in {secs:.1} s.", files.len())
                } else {
                    format!("SEG-Y export{c}: {} file(s) written to {dir}; problems: {}", files.len(), errors.join(" | "))
                };
            }
            Output::Project(loaded) => {
                let msg = crate::project::apply(self, *loaded);
                self.status = if cancelled { format!("{msg} (opening cancelled – some profiles not loaded)") } else { msg };
            }
        }
    }

    /// Status bar part: a progress bar while a task runs.
    pub fn task_status_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = &self.task {
            let f = t.progress.fraction();
            ui.add(egui::ProgressBar::new(f).desired_width(220.0).show_percentage().animate(true));
            ui.label(RichText::new(format!("{} · {}", t.title, t.progress.text())).strong());
            ui.separator();
        } else if !self.task_queue.is_empty() {
            ui.spinner();
            ui.separator();
        }
    }

    /// Popup with a progress bar once a task has run for a while.
    pub fn task_popup_ui(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.task else { return };
        if t.started.elapsed() < POPUP_AFTER {
            return;
        }
        let f = t.progress.fraction();
        let el = t.started.elapsed().as_secs_f32();
        let eta = if f > 0.02 { Some(el / f - el) } else { None };
        let mut cancel = false;
        egui::Modal::new(egui::Id::new("task_popup")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.heading(format!("{}…", t.title));
            ui.add_space(4.0);
            ui.label(t.progress.text());
            ui.add_space(6.0);
            ui.add(egui::ProgressBar::new(f).show_percentage().animate(true));
            ui.add_space(4.0);
            ui.weak(match eta {
                Some(e) => format!("{:.0} s elapsed, about {:.0} s left", el, e.max(0.0)),
                None => format!("{el:.0} s elapsed"),
            });
            if !self.task_queue.is_empty() {
                ui.weak(format!("{} more job(s) waiting", self.task_queue.len()));
            }
            ui.add_space(6.0);
            if t.cancellable {
                if t.progress.is_cancelled() {
                    ui.weak("Cancelling…");
                } else if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            }
        });
        if cancel {
            t.progress.cancel();
        }
    }
}
