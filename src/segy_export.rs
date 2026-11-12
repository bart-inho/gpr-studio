//! SEG-Y export for clients: SEG-Y rev 1, big endian, IEEE 32-bit float samples, EBCDIC text header
//! describing the profile, the processing and where the coordinates are, one trace per GPR trace with
//! its coordinates (cm), elevation (cm) and distance along the profile (cm, as the shot point number).
//! Sample interval in picoseconds (bytes 3217-3218 and 117-118), the usual GPR convention.

use crate::app::App;
use crate::model::Section;
use crate::processing::{Ctx, Progress, StepEntry};
use egui::{Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct SegyMeta {
    pub name: String,
    pub source: String,
    pub line_no: i32,
    pub coords: Vec<[f64; 3]>,
    pub dist: Vec<f64>,
    pub crs_label: String,
    pub geographic: bool,
    pub freq_mhz: f32,
    pub velocity: f32,
    pub dx: f32,
}

pub struct SegyJob {
    pub meta: SegyMeta,
    /// Data file to read again on the worker (saves memory with many profiles); None = use `raw`.
    pub reload: Option<(PathBuf, usize, crate::formats::ImportOptions)>,
    pub raw: Option<Section>,
    pub steps: Vec<StepEntry>,
    /// Stages to write: 0 = raw, k = after step k (1-based).
    pub stages: Vec<usize>,
    pub out_dir: PathBuf,
}

fn ebcdic(c: char) -> u8 {
    let c = c.to_ascii_uppercase();
    match c {
        'A'..='I' => 0xC1 + (c as u8 - b'A'),
        'J'..='R' => 0xD1 + (c as u8 - b'J'),
        'S'..='Z' => 0xE2 + (c as u8 - b'S'),
        '0'..='9' => 0xF0 + (c as u8 - b'0'),
        '.' => 0x4B,
        '<' => 0x4C,
        '(' => 0x4D,
        '+' => 0x4E,
        '|' => 0x4F,
        '&' => 0x50,
        '!' => 0x5A,
        '$' => 0x5B,
        '*' => 0x5C,
        ')' => 0x5D,
        ';' => 0x5E,
        '-' => 0x60,
        '/' => 0x61,
        ',' => 0x6B,
        '%' => 0x6C,
        '_' => 0x6D,
        '>' => 0x6E,
        '?' => 0x6F,
        ':' => 0x7A,
        '#' => 0x7B,
        '@' => 0x7C,
        '\'' => 0x7D,
        '=' => 0x7E,
        '"' => 0x7F,
        _ => 0x40,
    }
}

fn be16(b: &mut [u8], o: usize, v: i16) {
    b[o..o + 2].copy_from_slice(&v.to_be_bytes());
}
fn be32(b: &mut [u8], o: usize, v: i32) {
    b[o..o + 4].copy_from_slice(&v.to_be_bytes());
}

pub fn safe_name(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' };
        if c == '_' && out.ends_with('_') {
            continue;
        }
        out.push(c);
    }
    out.trim_matches('_').to_string()
}

/// Write one section as SEG-Y. `stage` describes the processing for the text header.
pub fn write(path: &Path, sec: &Section, m: &SegyMeta, stage: &str, steps: &[String]) -> Result<(), String> {
    let ns = sec.ns.min(65535);
    let ntr = sec.ntr;
    // Sample interval: picoseconds; for depth sections micrometres (same number as "ps").
    let dt_units = if sec.depth_axis { sec.dt * 1_000_000.0 } else { sec.dt * 1000.0 };
    let dt_u16 = dt_units.round().clamp(1.0, 65535.0) as u16;
    // Coordinates: projected in cm (scalar -100); geographic in arc seconds / 1000 (scalar -1000).
    let (cscale, cunits) = if m.geographic { (3_600_000.0, 2i16) } else { (100.0, 1i16) };
    let csc_i16: i16 = if m.geographic { -1000 } else { -100 };
    let mut lines: Vec<String> = vec![
        "GPR STUDIO SEG-Y EXPORT - SEG-Y REV 1, BIG ENDIAN, IEEE 32 BIT FLOAT".into(),
        format!("PROFILE: {}", m.name),
        format!("SOURCE FILE: {}", m.source),
        format!("CONTENT: {stage}"),
        format!("TRACES: {ntr}  SAMPLES PER TRACE: {ns}"),
        if sec.depth_axis {
            format!("DEPTH SECTION: SAMPLE INTERVAL {:.4} M (BYTES 3217/117 HOLD IT IN MICROMETRES)", sec.dt)
        } else {
            format!("SAMPLE INTERVAL: {:.4} NS (BYTES 3217/117 HOLD IT IN PICOSECONDS)", sec.dt)
        },
        format!("FIRST SAMPLE AT {:.3} {}", sec.t0, if sec.depth_axis { "M" } else { "NS (TIME ZERO = 0)" }),
        format!("ANTENNA: {:.0} MHZ  VELOCITY: {:.4} M/NS  TRACE SPACING: {:.4} M", m.freq_mhz, m.velocity, m.dx),
        format!("CRS: {}", m.crs_label),
        if m.geographic {
            "X/Y BYTES 73-80, 81-88, 181-188: ARC SECONDS, SCALAR BYTE 71 = -1000".into()
        } else {
            "X/Y BYTES 73-80, 81-88, 181-188: METRES, SCALAR BYTE 71 = -100 (CM)".into()
        },
        "ELEVATION BYTES 41-44 AND 45-48: METRES, SCALAR BYTE 69 = -100 (CM)".into(),
        "DISTANCE ALONG PROFILE: BYTES 197-200 (SHOT POINT), SCALAR BYTE 201 = -100".into(),
        "TRACE NUMBER: BYTES 1-4, 5-8, 9-12, 21-24 (CDP), 193-196 (CROSSLINE)".into(),
        "PROCESSING STEPS:".into(),
    ];
    if steps.is_empty() {
        lines.push("  NONE (RAW DATA)".into());
    }
    for s in steps {
        lines.push(format!("  {s}"));
    }
    let mut text = vec![0x40u8; 3200];
    for i in 0..40 {
        let body = if i == 39 { "END TEXTUAL HEADER".to_string() } else { lines.get(i).cloned().unwrap_or_default() };
        let l = format!("C{:>2} {}", i + 1, body);
        for (k, c) in l.chars().take(80).enumerate() {
            text[i * 80 + k] = ebcdic(c);
        }
    }
    let mut out = Vec::with_capacity(3600 + ntr * (240 + ns * 4));
    out.extend_from_slice(&text);
    let mut bin = vec![0u8; 400];
    be32(&mut bin, 0, 1);
    be32(&mut bin, 4, m.line_no);
    be32(&mut bin, 8, 1);
    be16(&mut bin, 12, 1);
    be16(&mut bin, 16, dt_u16 as i16);
    be16(&mut bin, 18, dt_u16 as i16);
    be16(&mut bin, 20, ns as u16 as i16);
    be16(&mut bin, 22, ns as u16 as i16);
    be16(&mut bin, 24, 5); // IEEE float
    be16(&mut bin, 26, 1); // fold
    be16(&mut bin, 28, 4); // horizontally stacked
    be16(&mut bin, 54, 1); // metres
    be16(&mut bin, 300, 0x0100); // revision 1.0
    be16(&mut bin, 302, 1); // fixed trace length
    be16(&mut bin, 304, 0); // no extended text headers
    out.extend_from_slice(&bin);
    let clamp = |v: f64| v.round().clamp(i32::MIN as f64, i32::MAX as f64) as i32;
    for t in 0..ntr {
        let mut h = vec![0u8; 240];
        let n = (t + 1) as i32;
        be32(&mut h, 0, n);
        be32(&mut h, 4, n);
        be32(&mut h, 8, n);
        be32(&mut h, 12, 1);
        be32(&mut h, 20, n);
        be32(&mut h, 24, 1);
        be16(&mut h, 28, 1);
        be16(&mut h, 34, 1);
        if let Some(c) = m.coords.get(t) {
            if c[0].is_finite() && c[1].is_finite() {
                let (x, y) = (clamp(c[0] * cscale), clamp(c[1] * cscale));
                for o in [72, 80, 180] {
                    be32(&mut h, o, x);
                    be32(&mut h, o + 4, y);
                }
                be32(&mut h, 40, clamp(c[2] * 100.0));
                be32(&mut h, 44, clamp(c[2] * 100.0));
            }
        }
        be16(&mut h, 68, -100);
        be16(&mut h, 70, csc_i16);
        be16(&mut h, 88, cunits);
        be16(&mut h, 114, ns as u16 as i16);
        be16(&mut h, 116, dt_u16 as i16);
        be32(&mut h, 188, m.line_no);
        be32(&mut h, 192, n);
        be32(&mut h, 196, clamp(m.dist.get(t).copied().unwrap_or(t as f64 * m.dx as f64) * 100.0));
        be16(&mut h, 200, -100);
        out.extend_from_slice(&h);
        for v in &sec.trace(t)[..ns] {
            out.extend_from_slice(&(if v.is_finite() { *v } else { 0.0 }).to_be_bytes());
        }
    }
    std::fs::write(path, out).map_err(|e| format!("{}: {e}", path.display()))
}

/// Run on the worker: process each profile once and write the chosen stages.
pub fn run(jobs: Vec<SegyJob>, prog: &Arc<Progress>) -> (Vec<String>, Vec<String>) {
    let mut written = Vec::new();
    let mut errors = Vec::new();
    let n = jobs.len().max(1);
    for (k, job) in jobs.into_iter().enumerate() {
        if prog.is_cancelled() {
            break;
        }
        prog.set_range(k as f32 / n as f32, (k + 1) as f32 / n as f32);
        prog.set_text(format!("profile {}/{n}: {} — reading", k + 1, job.meta.name));
        let raw = match (&job.raw, &job.reload) {
            (Some(r), _) => r.clone(),
            (None, Some((path, ch, opts))) => match crate::formats::load(path, opts).ok().and_then(|l| l.into_iter().nth(*ch)) {
                Some(p) => p.raw,
                None => {
                    errors.push(format!("{}: data file could not be read again", job.meta.name));
                    continue;
                }
            },
            _ => continue,
        };
        let last = job.stages.iter().copied().max().unwrap_or(0);
        let mut cur = raw;
        let mut done_steps: Vec<String> = Vec::new();
        let total = last.max(1) as f32;
        for stage in 0..=last {
            if prog.is_cancelled() {
                break;
            }
            if stage > 0 {
                let e = &job.steps[stage - 1];
                if e.enabled {
                    let base = k as f32 / n as f32;
                    let span = 1.0 / n as f32;
                    prog.set_range(base + span * (stage - 1) as f32 / total, base + span * stage as f32 / total);
                    prog.set_text(format!("profile {}/{n}: {} — {}", k + 1, job.meta.name, e.step.name()));
                    let ctx = Ctx { dx: job.meta.dx, freq_mhz: job.meta.freq_mhz, progress: Some(prog.clone()), label: String::new() };
                    match e.step.apply(&cur, &ctx) {
                        Ok(s) => cur = s,
                        Err(err) => errors.push(format!("{}: step {} skipped ({err})", job.meta.name, stage)),
                    }
                    prog.set_range(k as f32 / n as f32, (k + 1) as f32 / n as f32);
                    done_steps.push(format!("{}. {} ({})", stage, e.step.name(), e.step.summary()));
                } else {
                    done_steps.push(format!("{}. {} - OFF", stage, e.step.name()));
                }
            }
            if job.stages.contains(&stage) {
                let (label, tag) = if stage == 0 {
                    ("RAW DATA".to_string(), "00_raw".to_string())
                } else {
                    let e = &job.steps[stage - 1];
                    (format!("PROCESSED UP TO STEP {stage}: {}", e.step.name()), format!("{:02}_{}", stage, safe_name(e.step.name())))
                };
                let file = job.out_dir.join(format!("{}_{}.sgy", safe_name(&job.meta.name), tag));
                prog.set_text(format!("profile {}/{n}: {} — writing {}", k + 1, job.meta.name, file.file_name().and_then(|s| s.to_str()).unwrap_or("")));
                match write(&file, &cur, &job.meta, &label, &done_steps) {
                    Ok(()) => written.push(file.display().to_string()),
                    Err(e) => errors.push(e),
                }
            }
        }
    }
    prog.set(1.0);
    (written, errors)
}

impl App {
    pub fn segy_window(&mut self, ctx: &egui::Context) {
        if !self.show_segy {
            return;
        }
        let mut open = true;
        let mut go = false;
        let Some(a) = self.active else {
            self.show_segy = false;
            return;
        };
        let steps: Vec<(String, bool)> = self.profiles[a].steps.iter().map(|e| (format!("{} ({})", e.step.name(), e.step.summary()), e.enabled)).collect();
        if self.segy_sel.len() != steps.len() + 1 {
            self.segy_sel = vec![false; steps.len() + 1];
            if let Some(l) = self.segy_sel.last_mut() {
                *l = true;
            }
        }
        egui::Window::new("Export SEG-Y").open(&mut open).resizable(false).show(ctx, |ui| {
            ui.label("One .sgy file per profile and per checked step. Each file holds the data processed up to and including that step (all earlier steps applied).");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.segy_all, false, format!("Active profile ({})", self.profiles[a].name));
                ui.radio_value(&mut self.segy_all, true, format!("All {} profiles", self.profiles.len()));
            });
            if self.segy_all {
                ui.weak("Every profile uses its own processing sequence, cut at the same step number.");
            }
            ui.separator();
            ui.label(RichText::new("Export after step:").strong());
            ui.checkbox(&mut self.segy_sel[0], "0. Raw data (no processing)");
            for (i, (name, enabled)) in steps.iter().enumerate() {
                let label = if *enabled { format!("{}. {}", i + 1, name) } else { format!("{}. {}  (switched off)", i + 1, name) };
                ui.checkbox(&mut self.segy_sel[i + 1], label);
            }
            if steps.is_empty() {
                ui.weak("This profile has no processing steps yet.");
            }
            ui.separator();
            ui.weak(format!(
                "SEG-Y rev 1, IEEE float, big endian; sample interval in picoseconds; coordinates ({}) in cm and elevation in every trace header; distance along the profile as shot point; text header describes the processing.",
                self.project_crs.short()
            ));
            let n_files = self.segy_sel.iter().filter(|b| **b).count() * if self.segy_all { self.profiles.len() } else { 1 };
            ui.add_space(4.0);
            if ui.add_enabled(n_files > 0, egui::Button::new(RichText::new(format!("Choose folder and export {n_files} file(s)…")).strong())).clicked() {
                go = true;
            }
            if self.busy() {
                ui.colored_label(Color32::from_rgb(170, 90, 0), "Another job is running; the export will start after it.");
            }
        });
        if go {
            if let Some(dir) = rfd::FileDialog::new().set_title("Folder for the SEG-Y files").pick_folder() {
                let stages: Vec<usize> = self.segy_sel.iter().enumerate().filter(|(_, b)| **b).map(|(i, _)| i).collect();
                let idx: Vec<usize> = if self.segy_all { (0..self.profiles.len()).collect() } else { vec![a] };
                let geographic = self.is_geographic();
                let mut jobs = Vec::new();
                let mut skipped = 0;
                for i in idx {
                    let p = &self.profiles[i];
                    let st: Vec<usize> = stages.iter().copied().filter(|s| *s <= p.steps.len()).collect();
                    skipped += stages.len() - st.len();
                    if st.is_empty() {
                        continue;
                    }
                    let real_file = p.path.exists() && !p.format.starts_with("Demo");
                    jobs.push(SegyJob {
                        meta: SegyMeta {
                            name: p.name.clone(),
                            source: p.path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string(),
                            line_no: (i + 1) as i32,
                            coords: p.coords.clone(),
                            dist: p.dist.clone(),
                            crs_label: self.project_crs.label(),
                            geographic,
                            freq_mhz: p.freq_mhz,
                            velocity: p.velocity,
                            dx: p.dx,
                        },
                        reload: if real_file { Some((p.path.clone(), p.channel, self.import_opts.clone())) } else { None },
                        raw: if real_file { None } else { Some(p.raw.clone()) },
                        steps: p.steps.clone(),
                        stages: st,
                        out_dir: dir.clone(),
                    });
                }
                if skipped > 0 {
                    self.status = format!("{skipped} file(s) skipped: those profiles have fewer processing steps.");
                }
                self.task_queue.push_back(crate::tasks::Pending::Segy(jobs));
                self.show_segy = false;
            }
        }
        if !open {
            self.show_segy = false;
        }
    }
}

pub fn run_and_report(jobs: Vec<SegyJob>, prog: Arc<Progress>) -> crate::tasks::Output {
    let (w, e) = run(jobs, &prog);
    crate::tasks::Output::Exported(w, e)
}

