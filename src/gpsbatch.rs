//! One dialog for the GPS files of all imported profiles, matched automatically by file name.

use crate::app::{App, GpsBatch, GpsRow};
use crate::crs::CrsSpec;
use crate::gps::{self, GpsFormat};
use egui::{Color32, RichText};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

fn match_label(score: u32) -> &'static str {
    match score {
        0 => "",
        s if s >= 1000 => "same name",
        s if s >= 900 => "name w/o channel",
        s if s >= 600 => "name prefix",
        s if s >= 500 => "same number",
        _ => "similar number",
    }
}

impl App {
    /// Find the best GPS file for every row (same name, name without the channel ending,
    /// name prefix, or same profile number) in the chosen folder, the data folder and GPS sub-folders.
    pub fn rematch_gps(&self, b: &mut GpsBatch) {
        let folder = if b.folder.trim().is_empty() { None } else { Some(PathBuf::from(b.folder.trim())) };
        let mut cache: HashMap<Vec<PathBuf>, Vec<PathBuf>> = HashMap::new();
        for r in b.rows.iter_mut() {
            let Some(p) = self.profiles.iter().find(|p| p.id == r.profile_id) else { continue };
            let dirs = gps::search_dirs(&p.path, folder.as_deref());
            let files = cache.entry(dirs.clone()).or_insert_with(|| gps::list_gps_files(&dirs));
            match gps::best_match(&p.path, files) {
                Some((f, sc)) => {
                    r.path = f.to_string_lossy().to_string();
                    r.score = sc;
                    r.include = true;
                }
                None => {
                    r.path.clear();
                    r.score = 0;
                    r.include = r.has_embedded && b.use_embedded;
                }
            }
            r.status = None;
        }
    }

    pub fn gps_dialog_ui(&mut self, ctx: &egui::Context) {
        // Gather every queued profile into one batch.
        if !self.gps_queue.is_empty() {
            let mut b = self.gps_dialog.take().unwrap_or(GpsBatch { asked: false, rows: Vec::new(), folder: String::new(), format: GpsFormat::Auto, table_crs: "2056".into(), use_embedded: true });
            while let Some(id) = self.gps_queue.pop_front() {
                if b.rows.iter().any(|r| r.profile_id == id) {
                    continue;
                }
                if let Some(p) = self.profiles.iter().find(|p| p.id == id) {
                    b.rows.push(GpsRow { profile_id: id, name: p.name.clone(), path: String::new(), score: 0, include: false, has_embedded: p.embedded_gps.is_some(), status: None });
                }
            }
            self.rematch_gps(&mut b);
            self.gps_dialog = Some(b);
        }
        let Some(mut b) = self.gps_dialog.take() else { return };
        b.rows.retain(|r| self.profiles.iter().any(|p| p.id == r.profile_id));
        if b.rows.is_empty() {
            return;
        }
        let n = b.rows.len();
        let found = b.rows.iter().filter(|r| !r.path.is_empty()).count();
        let embedded = b.rows.iter().filter(|r| r.has_embedded).count();
        let mut close = false;
        let mut load = false;
        let mut rematch = false;
        if !b.asked {
            egui::Window::new("GPS for imported profiles").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                let what = if n == 1 { format!("profile \"{}\"", b.rows[0].name) } else { format!("the {n} imported profiles") };
                ui.label(RichText::new(format!("Are there associated GPS files for {what}?")).strong());
                let emb = if embedded > 0 { format!(" ({embedded} carry coordinates in the data file)") } else { String::new() };
                ui.label(format!("GPS file found automatically for {found} of {n}{emb}."));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let yes = ui.add(egui::Button::new(RichText::new("  Yes  ").strong()).fill(Color32::from_rgb(200, 225, 255)));
                    if yes.clicked() || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        b.asked = true;
                    }
                    if ui.button("  No  ").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        close = true;
                    }
                });
                ui.weak("Default: Yes (Enter). The matched files are listed before anything is loaded.");
            });
        } else {
            egui::Window::new("GPS files for imported profiles").collapsible(false).resizable(true).default_size([880.0, 520.0]).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label("GPS folder:");
                    ui.add(egui::TextEdit::singleline(&mut b.folder).desired_width(420.0).hint_text("next to the data files (and in a GPS sub-folder)"));
                    if ui.button("Browse…").clicked() {
                        if let Some(d) = rfd::FileDialog::new().pick_folder() {
                            b.folder = d.to_string_lossy().to_string();
                            rematch = true;
                        }
                    }
                    if ui.button("Match again").clicked() {
                        rematch = true;
                    }
                });
                ui.horizontal(|ui| {
                    egui::ComboBox::from_label("format").selected_text(b.format.name()).show_ui(ui, |ui| {
                        for f in GpsFormat::ALL {
                            ui.selectable_value(&mut b.format, f, f.name());
                        }
                    });
                    ui.label("Table CRS (EPSG):");
                    ui.add(egui::TextEdit::singleline(&mut b.table_crs).desired_width(60.0));
                    if embedded > 0 {
                        ui.checkbox(&mut b.use_embedded, "Use coordinates stored in the data files when no GPS file matches");
                    }
                });
                ui.weak("Matching by file name: the same name; the same name without the channel ending (_0, _1, _A1…); a GPS name that starts the data name; or the same profile number. NMEA, .cor, .dzg and .gps are read as WGS 84.");
                ui.horizontal(|ui| {
                    let sel = b.rows.iter().filter(|r| r.include).count();
                    ui.label(RichText::new(format!("Matched {found} of {n} · {sel} checked")).strong());
                    if ui.small_button("Check all matched").clicked() {
                        let ue = b.use_embedded;
                        for r in b.rows.iter_mut() {
                            r.include = !r.path.is_empty() || (r.has_embedded && ue);
                        }
                    }
                    if ui.small_button("Uncheck all").clicked() {
                        for r in b.rows.iter_mut() {
                            r.include = false;
                        }
                    }
                });
                ui.separator();
                let folder = b.folder.clone();
                let use_emb = b.use_embedded;
                egui::ScrollArea::vertical().max_height((ui.available_height() - 40.0).max(120.0)).show(ui, |ui| {
                    egui::Grid::new("gpsrows").striped(true).num_columns(6).show(ui, |ui| {
                        for h in ["", "Profile", "GPS file", "Match", "", "Result"] {
                            ui.strong(h);
                        }
                        ui.end_row();
                        for r in b.rows.iter_mut() {
                            ui.checkbox(&mut r.include, "");
                            ui.label(&r.name);
                            if r.path.is_empty() {
                                ui.weak(if r.has_embedded && use_emb { "(coordinates in the data file)" } else { "— none found —" });
                            } else {
                                let fname = Path::new(&r.path).file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                                ui.label(fname).on_hover_text(&r.path);
                            }
                            ui.weak(match_label(r.score));
                            if ui.small_button("…").on_hover_text("Choose the GPS file for this profile").clicked() {
                                let mut dlg = rfd::FileDialog::new().add_filter("GPS files", &gps::GPS_EXTS).add_filter("All files", &["*"]);
                                let start = if !r.path.is_empty() {
                                    Path::new(&r.path).parent().map(|p| p.to_path_buf())
                                } else if !folder.is_empty() {
                                    Some(PathBuf::from(&folder))
                                } else {
                                    None
                                };
                                if let Some(d) = start {
                                    dlg = dlg.set_directory(d);
                                }
                                if let Some(f) = dlg.pick_file() {
                                    r.path = f.to_string_lossy().to_string();
                                    r.score = 1000;
                                    r.include = true;
                                    r.status = None;
                                }
                            }
                            match &r.status {
                                Some(Ok(m)) => {
                                    ui.colored_label(Color32::from_rgb(0, 130, 0), m);
                                }
                                Some(Err(e)) => {
                                    ui.colored_label(Color32::RED, e.chars().take(60).collect::<String>()).on_hover_text(e);
                                }
                                None => {
                                    ui.label("");
                                }
                            }
                            ui.end_row();
                        }
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    let sel = b.rows.iter().filter(|r| r.include).count();
                    if ui.add_enabled(sel > 0, egui::Button::new(RichText::new(format!("Load GPS for {sel} profile(s)")).strong())).clicked() {
                        load = true;
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
            });
        }
        if rematch {
            self.rematch_gps(&mut b);
        }
        if load {
            let (ok, bad) = self.load_gps_batch(&mut b);
            self.status = format!("GPS loaded for {ok} profile(s){}.", if bad > 0 { format!(", {bad} failed (see the list)") } else { String::new() });
            if bad == 0 {
                close = true;
            }
        }
        if !close {
            self.gps_dialog = Some(b);
        }
    }

    /// Read the checked rows' GPS files; returns (loaded, failed).
    pub fn load_gps_batch(&mut self, b: &mut GpsBatch) -> (usize, usize) {
        let tcrs = CrsSpec::parse(&b.table_crs).unwrap_or(CrsSpec::Epsg(2056));
        let crs = self.project_crs.clone();
        let (mut ok, mut bad) = (0, 0);
        for r in b.rows.iter_mut().filter(|r| r.include) {
            let Some(i) = self.profiles.iter().position(|p| p.id == r.profile_id) else { continue };
            let ntr = self.profiles[i].raw.ntr;
            let res = if !r.path.is_empty() {
                gps::read(Path::new(r.path.trim()), b.format, ntr, &tcrs)
            } else if r.has_embedded && b.use_embedded {
                self.profiles[i].embedded_gps.clone().ok_or_else(|| "no coordinates in the file".to_string())
            } else {
                Err("no GPS file chosen".to_string())
            };
            match res {
                Ok(t) => {
                    let nf = t.fixes.len();
                    let p = &mut self.profiles[i];
                    p.gps = Some(t);
                    p.update_coords(&crs);
                    r.status = Some(Ok(format!("{nf} positions, {:.1} m", p.length())));
                    r.include = false;
                    ok += 1;
                }
                Err(e) => {
                    r.status = Some(Err(e));
                    bad += 1;
                }
            }
        }
        self.map.fitted = false;
        (ok, bad)
    }
}
