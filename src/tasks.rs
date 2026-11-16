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
    Processed(Vec<(u32, Section, Vec<String>, Vec<processing::StepEntry>, f32, f32, Option<crate::model::GpsTrack>, crate::crs::CrsSpec)>),
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
                let mut ids = ids.into_iter();
                let Some(id) = ids.next() else { return };
                let rest: Vec<_> = ids.collect();
                if !rest.is_empty() { self.task_queue.push_front(Pending::Process(rest)); }
                let Some(p) = self.profiles.iter().find(|p| p.id == id) else { return };
                let (name, raw, steps, dx, freq_mhz, gps, crs) = (p.name.clone(), p.raw.clone(), p.steps.clone(), p.dx, p.freq_mhz, p.gps.clone(), self.project_crs.clone());
                let (topography, topography_error) = processing::topography_for_steps(p, &self.project_crs);
                let title = format!("Processing {name}");
                self.start_task(title, true, move |prog| {
                    prog.set_text(name.clone());
                    let ctx = Ctx { dx, freq_mhz, progress: Some(prog.clone()), label: name, topography, topography_error };
                    let (sec, log) = processing::run_all(&raw, &steps, &ctx);
                    prog.set(1.0);
                    Output::Processed(if prog.is_cancelled() { Vec::new() } else { vec![(id, sec, log, steps, dx, freq_mhz, gps, crs)] })
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
                if cancelled { self.task_queue.retain(|q| !matches!(q, Pending::Process(_))); }
                let n = list.len();
                let mut applied = 0;
                let mut errors = Vec::new();
                for (id, sec, log, steps, dx, freq_mhz, gps, crs) in list {
                    if let Some(i) = self.profile_index(id) {
                        let p = &self.profiles[i];
                        if p.steps == steps && p.dx.to_bits() == dx.to_bits() && p.freq_mhz.to_bits() == freq_mhz.to_bits() && p.gps == gps && self.project_crs == crs {
                            self.apply_processed(i, sec, log);
                            errors.extend(self.profiles[i].log.iter().cloned());
                            applied += 1;
                        }
                    }
                }
                if cancelled {
                    self.status = format!("Processing cancelled ({applied} profile(s) finished).");
                } else if errors.is_empty() {
                    self.status = format!("Processing finished: {applied}/{n} profile(s) in {secs:.1} s.");
                } else {
                    self.status = errors.join("; ");
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::processing::{Step, StepEntry};

    #[test]
    fn stale_processing_result_is_discarded() {
        let mut app = App::headless(vec![], None);
        let id = app.add_profile(Profile::new("test".into(), PathBuf::new(), "test", Section::new(2, 2, 1.0), 0.05));
        let old = vec![StepEntry { enabled: true, step: Step::Dewow { window_ns: 3.0 } }];
        app.profiles[0].steps = vec![StepEntry { enabled: true, step: Step::Dewow { window_ns: 4.0 } }];
        let mut stale = Section::new(2, 2, 1.0);
        stale.data.fill(99.0);
        app.finish_task(Output::Processed(vec![(id, stale, vec![], old, 0.05, 0.0, None, app.project_crs.clone())]), 0.0, false);
        assert_eq!(app.profiles[0].proc.data, vec![0.0; 4]);
    }

    #[test]
    fn changed_gps_discards_background_migration_result() {
        let mut app = App::headless(vec![], None);
        let id = app.add_profile(Profile::new("test".into(), PathBuf::new(), "test", Section::new(2, 2, 1.0), 0.05));
        let steps = app.profiles[0].steps.clone();
        let mut stale = Section::new(2, 2, 1.0);
        stale.data.fill(99.0);
        app.profiles[0].gps = Some(crate::model::GpsTrack { source: "new GPS".into(), crs: app.project_crs.clone(), fixes: vec![] });
        app.finish_task(Output::Processed(vec![(id, stale, vec![], steps, 0.05, 0.0, None, app.project_crs.clone())]), 0.0, false);
        assert_eq!(app.profiles[0].proc.data, vec![0.0; 4]);
    }

    #[test]
    fn cancelling_batch_drops_remaining_processing_requests() {
        let mut app = App::headless(vec![], None);
        app.task_queue.push_back(Pending::Process(vec![1, 2]));
        app.task_queue.push_back(Pending::Import(vec![PathBuf::from("later.sgy")]));
        app.finish_task(Output::Processed(vec![]), 0.0, true);
        assert_eq!(app.task_queue.len(), 1);
        assert!(matches!(app.task_queue.front(), Some(Pending::Import(_))));
    }

    #[test]
    #[ignore]
    fn synthetic_batch_memory() {
        let mut app = App::headless(vec![], None);
        let mut ids = Vec::new();
        for k in 0..3 {
            let mut raw = Section::new(1024, 2400, 0.25);
            for (i, v) in raw.data.iter_mut().enumerate() { *v = ((i + k) as f32 * 0.01).sin(); }
            let id = app.add_profile(Profile::new(format!("profile {k}"), PathBuf::new(), "test", raw, 0.05));
            app.profiles[k].steps.push(StepEntry { enabled: true, step: Step::Dewow { window_ns: 5.0 } });
            ids.push(id);
        }
        let ctx = egui::Context::default();
        let start = Instant::now();
        let mut first = None;
        app.request_process(ids);
        while app.busy() {
            app.pump_tasks(&ctx);
            if first.is_none() && app.profiles[0].version > 1 { first = Some(start.elapsed().as_secs_f64()); }
            std::thread::sleep(Duration::from_millis(5));
        }
        println!("BATCH 3 profiles 2400x1024 first {:.3} s, all {:.3} s", first.unwrap_or_default(), start.elapsed().as_secs_f64());
    }
}
