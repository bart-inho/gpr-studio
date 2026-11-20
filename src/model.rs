//! Core data: radargram sections, profiles, GPS tracks and picks.

use crate::crs::CrsSpec;
use crate::processing::StepEntry;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// One 2D data block, trace-major: data[trace * ns + sample].
#[derive(Clone, Debug, Default)]
pub struct Section {
    pub ns: usize,
    pub ntr: usize,
    /// Sample interval in ns (or in m when `depth_axis`).
    pub dt: f32,
    /// y value of the first sample (ns or m).
    pub t0: f32,
    pub depth_axis: bool,
    /// For topographic migration only: elevation of depth zero (metres in project CRS).
    pub depth_datum_m: Option<f64>,
    pub data: Vec<f32>,
}

impl Section {
    pub fn new(ns: usize, ntr: usize, dt: f32) -> Self {
        Section { ns, ntr, dt, t0: 0.0, depth_axis: false, depth_datum_m: None, data: vec![0.0; ns * ntr] }
    }
    pub fn trace(&self, i: usize) -> &[f32] {
        &self.data[i * self.ns..(i + 1) * self.ns]
    }
    pub fn trace_mut(&mut self, i: usize) -> &mut [f32] {
        let ns = self.ns;
        &mut self.data[i * ns..(i + 1) * ns]
    }
    pub fn y_of(&self, sample: f32) -> f32 {
        self.t0 + sample * self.dt
    }
    pub fn sample_of(&self, y: f32) -> f32 {
        (y - self.t0) / self.dt
    }
    pub fn y_max(&self) -> f32 {
        self.y_of(self.ns.saturating_sub(1) as f32)
    }
    pub fn value(&self, tr: usize, s: usize) -> f32 {
        if tr < self.ntr && s < self.ns {
            self.data[tr * self.ns + s]
        } else {
            0.0
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpsFix {
    /// Trace index (0-based, may be fractional).
    pub trace: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// False for missing/implicit heights and older projects whose height provenance is unknown.
    #[serde(default)]
    pub has_elevation: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GpsTrack {
    pub source: String,
    pub crs: CrsSpec,
    pub fixes: Vec<GpsFix>,
    #[serde(default)]
    pub clock_range: Option<(String, String)>,
}

pub fn point_at(coords: &[[f64; 3]], trace: f32) -> Option<[f64; 3]> {
    if coords.is_empty() { return None; }
    let n = coords.len();
    let t = trace.clamp(0.0, (n - 1) as f32);
    let i = t.floor() as usize;
    let f = (t - i as f32) as f64;
    let a = coords[i];
    let b = coords[(i + 1).min(n - 1)];
    Some([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f])
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManualLine {
    pub number: u32,
    pub length_m: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ManualGrid {
    pub source_file: String,
    pub mode: String,
    pub size_x_m: f64,
    pub size_y_m: f64,
    pub spacing_x_m: f64,
    pub spacing_y_m: f64,
    pub lines: Vec<ManualLine>,
}

pub fn decode_proceq_text(bytes: &[u8]) -> Result<String, String> {
    if let Some((body, little_endian)) = bytes.strip_prefix(&[0xff, 0xfe]).map(|b| (b, true))
        .or_else(|| bytes.strip_prefix(&[0xfe, 0xff]).map(|b| (b, false))) {
        let mut chunks = body.chunks_exact(2);
        let units: Vec<u16> = chunks.by_ref().map(|pair| {
            if little_endian { u16::from_le_bytes([pair[0], pair[1]]) }
            else { u16::from_be_bytes([pair[0], pair[1]]) }
        }).collect();
        if !chunks.remainder().is_empty() {
            return Err("Proceq CSV has an incomplete UTF-16 character".into());
        }
        return String::from_utf16(&units).map_err(|_| "Proceq CSV has invalid UTF-16 text".into());
    }
    let body = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(bytes);
    std::str::from_utf8(body).map(str::to_owned).map_err(|_| "Proceq CSV is neither UTF-8 nor BOM-marked UTF-16".into())
}

impl ManualGrid {
    pub fn parse(text: &str, source_file: &str) -> Result<Self, String> {
        let mut grid = Self { source_file: source_file.into(), mode: String::new(), size_x_m: 0.0, size_y_m: 0.0, spacing_x_m: 0.0, spacing_y_m: 0.0, lines: Vec::new() };
        let mut scan = false;
        let mut user_info = false;
        for row in text.lines() {
            let fields: Vec<&str> = row.trim().split('\t').map(str::trim).filter(|s| !s.is_empty()).collect();
            if fields.is_empty() { continue; }
            let key = fields[0].to_ascii_lowercase();
            if key == "user info" { user_info = true; scan = false; continue; }
            if key == "settings" { user_info = false; scan = false; continue; }
            if key.contains("scan distance") { user_info = false; scan = true; continue; }
            if user_info { continue; }
            let value = fields.get(1).copied().unwrap_or("");
            let decimal = || value.replace(',', ".").parse::<f64>().ok().filter(|v| v.is_finite());
            if key == "mode" { grid.mode = value.into(); }
            else if key.contains("grid size") && key.contains('x') { grid.size_x_m = decimal().unwrap_or(0.0) / 100.0; }
            else if key.contains("grid size") && key.contains('y') { grid.size_y_m = decimal().unwrap_or(0.0) / 100.0; }
            else if key.contains("grid spacing") && key.contains('x') { grid.spacing_x_m = decimal().unwrap_or(0.0) / 100.0; }
            else if key.contains("grid spacing") && key.contains('y') { grid.spacing_y_m = decimal().unwrap_or(0.0) / 100.0; }
            else if scan && key.starts_with("line ") {
                let words: Vec<&str> = fields[0].split_whitespace().collect();
                let number = words.get(1).and_then(|s| s.parse::<u32>().ok());
                let length = fields.get(1).copied().or_else(|| words.get(2).copied()).unwrap_or("").replace(',', ".").parse::<f64>().ok();
                if let (Some(number), Some(length_m)) = (number, length.filter(|v| v.is_finite() && *v >= 0.0)) {
                    grid.lines.push(ManualLine { number, length_m });
                }
            }
        }
        for (label, value) in [("Grid Size X", grid.size_x_m), ("Grid Size Y", grid.size_y_m), ("Grid Spacing X", grid.spacing_x_m), ("Grid Spacing Y", grid.spacing_y_m)] {
            if value <= 0.0 { return Err(format!("Not a Proceq area-scan CSV: {label} is missing or invalid")); }
        }
        if !grid.mode.is_empty() && !grid.mode.to_ascii_lowercase().contains("area") {
            return Err("Not a Proceq area-scan CSV: Mode is not Area Scan".into());
        }
        if grid.size_x_m / grid.spacing_x_m > 2000.0 || grid.size_y_m / grid.spacing_y_m > 2000.0 {
            return Err("Not a Proceq area-scan CSV: grid spacing is too fine".into());
        }
        Ok(grid)
    }

    pub fn coords_for(&self, number: u32, ntr: usize) -> Option<Vec<[f64; 3]>> {
        // shortcut: scans run along X; add the transposed case when a Proceq file supplies Y direction.
        let (order, line) = self.lines.iter().enumerate().find(|(_, line)| line.number == number && line.length_m > 0.0)?;
        Some((0..ntr).map(|i| [i as f64 * line.length_m / ntr.saturating_sub(1).max(1) as f64, order as f64 * self.spacing_y_m, 0.0]).collect())
    }
}

pub struct Profile {
    pub id: u32,
    pub name: String,
    pub path: PathBuf,
    pub format: String,
    pub channel: usize,
    pub raw: Section,
    pub proc: Section,
    /// Nominal trace spacing (m).
    pub dx: f32,
    pub freq_mhz: f32,
    /// Wave velocity for depth (m/ns).
    pub velocity: f32,
    pub steps: Vec<StepEntry>,
    pub header: Vec<(String, String)>,
    pub gps: Option<GpsTrack>,
    /// Coordinates found inside the data file itself (SEG-Y trace headers).
    pub embedded_gps: Option<GpsTrack>,
    /// Per-trace coordinates in the project CRS (empty without GPS).
    pub coords: Vec<[f64; 3]>,
    pub manual_coords: Option<Vec<[f64; 3]>>,
    pub manual_line: Option<u32>,
    pub manual_active: bool,
    /// Per-trace distance along the profile (m).
    pub dist: Vec<f64>,
    /// Horizontal GPS chainage, independent of the chosen display distance calibration.
    pub horizontal_dist: Vec<f64>,
    pub use_gps_distance: bool,
    /// The trace spacing was read from the data file's header.
    pub dx_from_file: bool,
    /// The trace spacing was typed in by the person (kept over the file's value).
    pub dx_user_set: bool,
    /// Show the section with topography (time zero follows the GPS elevation).
    pub show_topo: bool,
    /// Show traces in reverse order without changing acquisition data or pick indices.
    pub reverse_display: bool,
    pub log: Vec<String>,
    pub version: u64,
}

impl Profile {
    pub fn new(name: String, path: PathBuf, format: &str, raw: Section, dx: f32) -> Self {
        let mut p = Profile {
            id: 0,
            name,
            path,
            format: format.to_string(),
            channel: 0,
            proc: raw.clone(),
            raw,
            dx: if dx > 0.0 && dx.is_finite() { dx } else { 0.05 },
            dx_from_file: dx > 0.0 && dx.is_finite(),
            dx_user_set: false,
            show_topo: false,
            reverse_display: false,
            freq_mhz: 0.0,
            velocity: 0.1,
            steps: Vec::new(),
            header: Vec::new(),
            gps: None,
            embedded_gps: None,
            coords: Vec::new(),
            manual_coords: None,
            manual_line: None,
            manual_active: false,
            dist: Vec::new(),
            horizontal_dist: Vec::new(),
            use_gps_distance: true,
            log: Vec::new(),
            version: 1,
        };
        p.update_distance();
        p
    }

    pub fn has_coords(&self) -> bool {
        !self.coords.is_empty() || self.manual_coords.as_ref().is_some_and(|c| !c.is_empty())
    }

    pub fn map_coords(&self, manual: bool) -> &[[f64; 3]] {
        if manual { self.manual_coords.as_deref().unwrap_or(&[]) } else { &self.coords }
    }

    /// Orient the dominant track direction west-to-east or north-to-south.
    pub fn auto_orientation(&self, project_crs: &CrsSpec) -> Option<bool> {
        if self.coords.len() < 2 {
            return None;
        }
        let mut ends = [self.coords[0], *self.coords.last()?];
        crate::crs::transform_spec(project_crs, &CrsSpec::Epsg(4326), &mut ends).ok()?;
        if ends.iter().any(|p| !p[0].is_finite() || !p[1].is_finite()) {
            return None;
        }
        let lat = ((ends[0][1] + ends[1][1]) * 0.5).to_radians();
        let east = (ends[1][0] - ends[0][0]) * lat.cos();
        let north = ends[1][1] - ends[0][1];
        if east.abs().max(north.abs()) < 1e-9 {
            return None;
        }
        Some(if east.abs() >= north.abs() { east < 0.0 } else { north > 0.0 })
    }

    /// Recompute per-trace project coordinates from the GPS fixes.
    pub fn update_coords(&mut self, project_crs: &CrsSpec) {
        self.coords.clear();
        let track = match &self.gps {
            Some(t) if !t.fixes.is_empty() => t.clone(),
            _ => {
                self.update_distance();
                return;
            }
        };
        let mut pts: Vec<[f64; 3]> = track.fixes.iter().map(|f| [f.x, f.y, f.z]).collect();
        if let Err(e) = crate::crs::transform_spec(&track.crs, project_crs, &mut pts) {
            self.log.push(format!("GPS transform failed: {e}"));
            self.update_distance();
            return;
        }
        let mut fixes: Vec<(f64, [f64; 3])> = track
            .fixes
            .iter()
            .zip(pts)
            .filter(|(_, p)| p[0].is_finite() && p[1].is_finite())
            // The GPS height has no declared vertical CRS; reproject x/y but keep its recorded altitude.
            .map(|(f, mut p)| { p[2] = f.z; (f.trace, p) })
            .collect();
        fixes.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        fixes.dedup_by(|a, b| (a.0 - b.0).abs() < 1e-9);
        if fixes.is_empty() {
            self.update_distance();
            return;
        }
        let n = self.raw.ntr;
        self.coords = (0..n).map(|i| interp_fix(&fixes, i as f64)).collect();
        self.update_distance();
    }

    pub fn update_distance(&mut self) {
        let n = self.proc.ntr.max(self.raw.ntr);
        self.horizontal_dist.clear();
        if self.coords.len() >= 2 && self.coords.len() == n {
            let mut d = Vec::with_capacity(n);
            let mut acc = 0.0;
            d.push(0.0);
            for i in 1..n {
                let a = self.coords[i - 1];
                let b = self.coords[i];
                acc += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                d.push(acc);
            }
            // A degenerate track (no movement) falls back to the nominal spacing.
            if acc.is_finite() && acc > 0.0 { self.horizontal_dist = d; }
        }
        if self.use_gps_distance {
            let manual = self.manual_coords.as_deref().filter(|c| c.len() == n);
            let chosen = if self.manual_active || self.coords.is_empty() { manual } else { None };
            if let Some(coords) = chosen {
                let mut d = Vec::with_capacity(n);
                d.push(0.0);
                for i in 1..n {
                    let a = coords[i - 1];
                    let b = coords[i];
                    d.push(d[i - 1] + (b[0] - a[0]).hypot(b[1] - a[1]));
                }
                if d.last().is_some_and(|v| v.is_finite() && *v > 0.0) { self.dist = d; return; }
            }
            if !self.horizontal_dist.is_empty() { self.dist = self.horizontal_dist.clone(); return; }
        }
        self.dist = (0..n).map(|i| i as f64 * self.dx as f64).collect();
    }

    pub fn topographic_geometry(&self, project_crs: &CrsSpec, max_gap_m: f32) -> Result<crate::processing::TopographicGeometry, String> {
        let track = self.gps.as_ref().ok_or("Topographic Kirchhoff needs associated GPS positions with elevations.")?;
        if self.raw.ntr < 2 || self.coords.len() != self.raw.ntr || self.horizontal_dist.len() != self.raw.ntr {
            return Err("Topographic Kirchhoff needs valid positions for every trace.".into());
        }
        let crs = project_crs.build()?;
        let units = project_crs.proj4().unwrap_or_default();
        if crs.geographic || !units.split_whitespace().any(|s| s == "+units=m" || s == "+to_meter=1") {
            return Err("Topographic Kirchhoff requires a projected coordinate system in metres (for example EPSG:2056).".into());
        }
        if !max_gap_m.is_finite() || max_gap_m <= 0.0 {
            return Err("Maximum supported GPS gap must be positive metres.".into());
        }
        let mut fixes = track.fixes.clone();
        fixes.sort_by(|a, b| a.trace.total_cmp(&b.trace));
        if fixes.len() < 2 {
            return Err("Topographic Kirchhoff needs at least two GPS positions with elevations.".into());
        }
        if fixes.iter().any(|f| !f.has_elevation || !f.trace.is_finite() || !f.x.is_finite() || !f.y.is_finite() || !f.z.is_finite()) {
            return Err("GPS heights are missing or unverified. Re-associate a GPS file containing measured elevations.".into());
        }
        let mut pts: Vec<_> = fixes.iter().map(|f| [f.x, f.y, f.z]).collect();
        crate::crs::transform_spec(&track.crs, project_crs, &mut pts)?;
        // Traces before the first / after the last fix are extrapolated; bound them like any other gap.
        for (c, p, end) in [(self.coords[0], pts[0], "start"), (self.coords[self.raw.ntr - 1], pts[pts.len() - 1], "end")] {
            let d = (c[0] - p[0]).hypot(c[1] - p[1]);
            if !d.is_finite() || d > max_gap_m as f64 {
                return Err(format!("GPS stops {d:.2} m before the profile {end} (limit {max_gap_m:.2} m); add measured positions or increase the stated gap limit."));
            }
        }
        for (i, pair) in fixes.windows(2).enumerate() {
            let trace_gap = pair[1].trace - pair[0].trace;
            let dx = pts[i + 1][0] - pts[i][0];
            let dy = pts[i + 1][1] - pts[i][1];
            let gap = dx.hypot(dy);
            if trace_gap <= 0.0 || !gap.is_finite() || gap > max_gap_m as f64 {
                return Err(format!("GPS gap exceeds {max_gap_m:.2} m or contains duplicate trace positions; add measured positions or increase the stated gap limit."));
            }
        }
        if self.coords.iter().any(|c| c.iter().any(|v| !v.is_finite())) || self.horizontal_dist.windows(2).any(|w| !w[1].is_finite() || w[1] <= w[0]) {
            return Err("GPS coordinates contain invalid or repeated trace positions.".into());
        }
        Ok(crate::processing::TopographicGeometry {
            x_m: self.horizontal_dist.clone(),
            elevation_m: self.coords.iter().map(|c| c[2]).collect(),
        })
    }

    pub fn axis_distance(&self) -> &[f64] {
        if self.proc.depth_datum_m.is_some() && !self.horizontal_dist.is_empty() { &self.horizontal_dist } else { &self.dist }
    }

    pub fn antenna_height_correction(&self) -> f64 {
        self.steps.iter().find_map(|e| if e.enabled {
            if let crate::processing::Step::TopographicKirchhoff { gps_above_antenna_m, .. } = &e.step { Some(*gps_above_antenna_m as f64) } else { None }
        } else { None }).unwrap_or(0.0)
    }

    pub fn length(&self) -> f64 {
        self.dist.last().copied().unwrap_or(0.0)
    }

    pub fn axis_length(&self) -> f64 {
        self.axis_distance().last().copied().unwrap_or(0.0)
    }

    pub fn dist_at(&self, trace: f32) -> f64 {
        self.distance_at(&self.dist, trace)
    }

    pub fn axis_dist_at(&self, trace: f32) -> f64 {
        self.distance_at(self.axis_distance(), trace)
    }

    fn distance_at(&self, distances: &[f64], trace: f32) -> f64 {
        if distances.is_empty() {
            return trace as f64 * self.dx as f64;
        }
        let n = distances.len();
        let t = trace.clamp(0.0, (n - 1) as f32);
        let i = t.floor() as usize;
        let f = (t - i as f32) as f64;
        if i + 1 < n {
            distances[i] * (1.0 - f) + distances[i + 1] * f
        } else {
            distances[n - 1]
        }
    }

    pub fn trace_at_dist(&self, d: f64) -> f32 {
        self.trace_at_distance(&self.dist, d)
    }

    pub fn axis_trace_at_dist(&self, d: f64) -> f32 {
        self.trace_at_distance(self.axis_distance(), d)
    }

    fn trace_at_distance(&self, distances: &[f64], d: f64) -> f32 {
        if distances.len() < 2 {
            return (d / self.dx as f64) as f32;
        }
        let idx = distances.partition_point(|v| *v < d);
        if idx == 0 {
            return 0.0;
        }
        if idx >= distances.len() {
            return (distances.len() - 1) as f32;
        }
        let a = distances[idx - 1];
        let b = distances[idx];
        let f = if b > a { (d - a) / (b - a) } else { 0.0 };
        (idx - 1) as f32 + f as f32
    }

    pub fn coord_at(&self, trace: f32) -> Option<[f64; 3]> {
        point_at(&self.coords, trace)
    }

    /// Elevations usable for a topographic display (GPS heights that vary).
    pub fn has_topo(&self) -> bool {
        if self.coords.len() != self.proc.ntr || self.coords.is_empty() {
            return false;
        }
        let (mn, mx) = self.coords.iter().fold((f64::MAX, f64::MIN), |(a, b), c| (a.min(c[2]), b.max(c[2])));
        mx > mn && mx.is_finite() && mn.is_finite()
    }

    /// Highest surface elevation and, per trace, the shift down in samples so that time zero
    /// follows the topography: 2 (zmax - z) / v as time, or zmax - z for a depth section.
    pub fn topo_shifts(&self) -> Option<(f64, Vec<f32>)> {
        if !self.show_topo || !self.has_topo() {
            return None;
        }
        let zmax = self.coords.iter().map(|c| c[2]).fold(f64::MIN, f64::max);
        let v = self.velocity.max(1e-4) as f64;
        let sh = self
            .coords
            .iter()
            .map(|c| {
                let dz = zmax - c[2];
                let y = if self.proc.depth_axis { dz } else { 2.0 * dz / v };
                (y / self.proc.dt as f64) as f32
            })
            .collect();
        Some((zmax, sh))
    }

    /// Two-way time (ns, relative to time zero) to depth (m).
    pub fn depth_of_time(&self, t: f32) -> f32 {
        self.velocity * t / 2.0
    }
    pub fn time_of_depth(&self, z: f32) -> f32 {
        if self.velocity > 0.0 {
            2.0 * z / self.velocity
        } else {
            0.0
        }
    }

    /// Pick time (acquisition ns) at a trace → y value of the processed section, and back.
    pub fn y_of_pick_time(&self, tr: f32, t: f32) -> f32 {
        match self.topo_pick_frame(tr) {
            Some((surface, v)) => surface + v * t / 2.0,
            None => if self.proc.depth_axis { self.depth_of_time(t) } else { t },
        }
    }
    pub fn pick_time_of_y(&self, tr: f32, y: f32) -> f32 {
        match self.topo_pick_frame(tr) {
            Some((surface, v)) => (2.0 * (y - surface) / v).max(0.0),
            None => if self.proc.depth_axis { self.time_of_depth(y) } else { y },
        }
    }
    /// Topographic migration: (surface depth below datum at this trace, migration velocity), as in `topographic_kirchhoff`.
    fn topo_pick_frame(&self, tr: f32) -> Option<(f32, f32)> {
        let datum = self.proc.depth_datum_m?;
        let v = self.steps.iter().find_map(|e| match &e.step {
            crate::processing::Step::TopographicKirchhoff { velocity, .. } if e.enabled && *velocity > 0.0 => Some(*velocity),
            _ => None,
        })?;
        let z = self.coord_at(tr)?[2];
        Some(((datum - (z - self.antenna_height_correction())) as f32, v))
    }

    /// Visible sample range height for a 1:1 metre scale at this horizontal trace span and plot size.
    pub fn one_to_one_samples(&self, traces: f32, plot: [f32; 2]) -> Option<f32> {
        let n = self.proc.ntr.max(2) as f64;
        let m_per_tr = self.axis_length() / (n - 1.0);
        let m_per_s = if self.proc.depth_axis { self.proc.dt } else { self.depth_of_time(self.proc.dt) } as f64;
        let s = traces as f64 * m_per_tr / m_per_s * (plot[1] / plot[0]) as f64;
        (s.is_finite() && s > 0.0).then_some(s as f32)
    }

    /// Trace span that shows `samples` samples at 1:1 in a plot of `plot` points (inverse of `one_to_one_samples`).
    pub fn one_to_one_traces(&self, samples: f32, plot: [f32; 2]) -> Option<f32> {
        let per_trace = self.one_to_one_samples(1.0, plot)?;
        let t = samples / per_trace;
        (t.is_finite() && t > 0.0).then_some(t)
    }
}

fn interp_fix(fixes: &[(f64, [f64; 3])], t: f64) -> [f64; 3] {
    if fixes.len() == 1 {
        return fixes[0].1;
    }
    let idx = fixes.partition_point(|f| f.0 < t);
    let (a, b) = if idx == 0 {
        (&fixes[0], &fixes[1])
    } else if idx >= fixes.len() {
        (&fixes[fixes.len() - 2], &fixes[fixes.len() - 1])
    } else {
        (&fixes[idx - 1], &fixes[idx])
    };
    let span = b.0 - a.0;
    // Beyond the ends this extrapolates linearly along the end segment (loggers often stop a few traces early).
    let f = if span.abs() > 1e-12 { (t - a.0) / span } else { 0.0 };
    [
        a.1[0] + (b.1[0] - a.1[0]) * f,
        a.1[1] + (b.1[1] - a.1[1]) * f,
        a.1[2] + (b.1[2] - a.1[2]) * f,
    ]
}

// ---------------------------------------------------------------- picks

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Symbol {
    Circle,
    Square,
    Triangle,
    Diamond,
    Cross,
    Star,
}

impl Symbol {
    pub const ALL: [Symbol; 6] = [Symbol::Circle, Symbol::Square, Symbol::Triangle, Symbol::Diamond, Symbol::Cross, Symbol::Star];
    pub fn name(&self) -> &'static str {
        match self {
            Symbol::Circle => "circle",
            Symbol::Square => "square",
            Symbol::Triangle => "triangle",
            Symbol::Diamond => "diamond",
            Symbol::Cross => "cross",
            Symbol::Star => "star",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MarkerKind {
    /// A single point (target, hyperbola apex).
    Point,
    /// A polyline along a reflector (layer, water table...).
    Line,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MarkerType {
    pub name: String,
    pub color: [u8; 3],
    pub symbol: Symbol,
    pub kind: MarkerKind,
}

pub fn default_marker_types() -> Vec<MarkerType> {
    let t = |n: &str, c: [u8; 3], s: Symbol, k: MarkerKind| MarkerType { name: n.into(), color: c, symbol: s, kind: k };
    vec![
        t("Pipe / utility", [230, 40, 40], Symbol::Circle, MarkerKind::Point),
        t("Cable", [255, 150, 0], Symbol::Diamond, MarkerKind::Point),
        t("Rebar", [40, 110, 255], Symbol::Cross, MarkerKind::Point),
        t("Void / cavity", [200, 0, 200], Symbol::Square, MarkerKind::Point),
        t("Anomaly", [250, 220, 0], Symbol::Triangle, MarkerKind::Point),
        t("Unknown object", [150, 150, 150], Symbol::Star, MarkerKind::Point),
        t("Layer boundary", [0, 200, 80], Symbol::Circle, MarkerKind::Line),
        t("Water table", [0, 200, 230], Symbol::Circle, MarkerKind::Line),
        t("Bedrock", [160, 100, 40], Symbol::Circle, MarkerKind::Line),
    ]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Marker {
    pub id: u32,
    pub profile_id: u32,
    pub type_idx: usize,
    pub trace: f32,
    /// Two-way time relative to time zero (ns).
    pub time_ns: f32,
    pub amplitude: f32,
    /// Velocity from a hyperbola fit (m/ns), if one was made.
    pub velocity: Option<f32>,
    pub comment: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinePick {
    pub id: u32,
    pub profile_id: u32,
    pub type_idx: usize,
    /// (trace, time ns)
    pub points: Vec<(f32, f32)>,
    pub comment: String,
}

/// The profile number for map labels: the last group of digits once a channel ending is removed
/// ("..._2026-10-14_0002_1" -> "0002", "DEMO_003" -> "003"); the name itself when it has no digits.
pub fn short_label(name: &str) -> String {
    let s = crate::gps::strip_channel(name);
    let chars: Vec<char> = s.chars().collect();
    let mut end = chars.len();
    while end > 0 && !chars[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == 0 {
        return if chars.len() > 16 { chars[..16].iter().collect() } else { s };
    }
    let mut start = end;
    while start > 0 && chars[start - 1].is_ascii_digit() {
        start -= 1;
    }
    chars[start..end].iter().collect()
}

#[cfg(test)]
mod manual_grid_tests {
    use super::*;

    // Matches the Proceq Export Format Version 2 layout. Personal values are fictitious.
    const CSV: &str = "Export Format Version\t2\r\n\
Export App Version\t6.0.0\r\n\
USER INFO\r\n\
User\tPrivate Example\r\n\
Email\tprivate@example.invalid\r\n\
Probe S/N\tTEST-0001\r\n\
Hardware Version\tB0\r\n\
Firmware Version\t5.7.3\r\n\
App Version\t6.0.0\r\n\
Contract ID\tTEST-ID\r\n\
MAC Address\t00:00:00:00:00:00\r\n\
\r\n\
\r\n\
SETTINGS\r\n\
Measuring Presets\r\n\
File Name\tdoor3d10cmgrid001\r\n\
File ID\tTEST-FILE-ID\r\n\
Folder\tst.Christophe\r\n\
Mode\tArea Scan\r\n\
Grid Size - X [cm]\t160.0\r\n\
Grid Size - Y [cm]\t120.0\r\n\
Grid Spacing - X [cm]\t10.0\r\n\
Grid Spacing - Y [cm]\t10.0\r\n\
Scan\t14\r\n\
Units\tMetric\r\n\
Resolution\tMax Depth\r\n\
Repetition Rate [scans /cm]\t1.0\r\n\
Scan Length [samples / scan]\t409.000000\r\n\
Time Window [ns]\t28\r\n\
Sampling Rate [GHz]\t20.48\r\n\
Antenna Spacing [cm]\t7.2\r\n\
\r\n\
\r\n\
SCAN DISTANCE [m]\r\n\
Line 1\t0.00\r\n\
Line 18\t1.60\r\n\
Line 19\t1.60\r\n\
Line 20\t1.60\r\n\
Line 21\t1.60\r\n\
Line 22\t1.60\r\n\
Line 23\t1.60\r\n\
Line 24\t1.60\r\n\
Line 25\t1.60\r\n\
Line 26\t1.60\r\n\
Line 27\t1.60\r\n\
Line 28\t1.60\r\n\
Line 29\t1.60\r\n\
Line 30\t1.60\r\n";

    #[test]
    fn parses_proceq_area_scan_without_user_info() {
        let g = ManualGrid::parse(CSV, "scan.csv").unwrap();
        assert_eq!(g.source_file, "scan.csv");
        assert_eq!(g.mode, "Area Scan");
        assert_eq!((g.size_x_m, g.size_y_m), (1.6, 1.2));
        assert!((g.spacing_x_m - 0.1).abs() < 1e-9);
        assert!((g.spacing_y_m - 0.1).abs() < 1e-9);
        assert_eq!(g.lines.len(), 14);
        assert_eq!(g.lines.iter().map(|line| line.number).collect::<Vec<_>>(), [1, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30]);
        assert_eq!(g.lines[0].length_m, 0.0);
        assert!(g.lines[1..].iter().all(|line| line.length_m == 1.6));
        assert!(!serde_json::to_string(&g).unwrap().contains("Private Example"));
        assert!(!serde_json::to_string(&g).unwrap().contains("private@example.invalid"));
        assert!(g.coords_for(1, 17).is_none());
    }

    #[test]
    fn placed_profile_uses_file_order_and_scan_length() {
        let g = ManualGrid::parse(CSV, "scan.csv").unwrap();
        let mut p = Profile::new("Profile_018".into(), PathBuf::new(), "demo", Section::new(5, 17, 0.1), 0.05);
        p.manual_coords = g.coords_for(18, p.raw.ntr);
        p.manual_active = true;
        p.update_distance();
        let c = p.manual_coords.as_ref().unwrap();
        assert_eq!(c.len(), 17);
        assert_eq!(c[0], [0.0, 0.1, 0.0]);
        assert_eq!(c[16], [1.6, 0.1, 0.0]);
        assert!((p.dist[16] - 1.6).abs() < 1e-9);
    }

    #[test]
    fn rejects_missing_grid_size() {
        let csv = "Mode\tArea Scan\nGrid Spacing - X [cm]\t10\nGrid Spacing - Y [cm]\t10\n";
        assert!(ManualGrid::parse(csv, "bad.csv").unwrap_err().contains("Grid Size X"));
    }

    #[test]
    fn accepts_comma_decimals_in_export() {
        let csv = CSV.replace("160.0", "160,0").replace("1.60", "1,60");
        let g = ManualGrid::parse(&csv, "scan.csv").unwrap();
        assert_eq!(g.size_x_m, 1.6);
        assert_eq!(g.lines[1].length_m, 1.6);
    }

    #[test]
    fn reads_utf16_le_proceq_export() {
        let mut bytes = vec![0xff, 0xfe];
        for unit in CSV.encode_utf16() { bytes.extend_from_slice(&unit.to_le_bytes()); }
        let text = decode_proceq_text(&bytes).unwrap();
        let g = ManualGrid::parse(&text, "survey.csv").unwrap();
        assert_eq!((g.size_x_m, g.size_y_m), (1.6, 1.2));
        assert_eq!(g.lines.len(), 14);
        assert_eq!(g.lines[1].number, 18);
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;

    #[test]
    fn orients_by_dominant_compass_direction() {
        let mut p = Profile::new("test".into(), PathBuf::new(), "test", Section::new(1, 2, 1.0), 1.0);
        let crs = CrsSpec::Epsg(4326);
        p.coords = vec![[8.0, 47.0, 0.0], [7.99, 47.001, 0.0]];
        assert_eq!(p.auto_orientation(&crs), Some(true)); // westbound
        p.coords[1] = [8.01, 47.001, 0.0];
        assert_eq!(p.auto_orientation(&crs), Some(false)); // eastbound
        p.coords[1] = [8.001, 47.01, 0.0];
        assert_eq!(p.auto_orientation(&crs), Some(true)); // northbound
        p.coords[1] = [8.001, 46.99, 0.0];
        assert_eq!(p.auto_orientation(&crs), Some(false)); // southbound
        p.coords[1] = p.coords[0];
        assert_eq!(p.auto_orientation(&crs), None);
    }

    #[test]
    fn one_to_one_traces_inverts_samples() {
        // 101 traces over 5 m (0.05 m/trace); dt 0.2 ns at v 0.1 m/ns = 0.01 m/sample.
        let mut p = Profile::new("test".into(), PathBuf::new(), "test", Section::new(200, 101, 0.2), 0.05);
        p.use_gps_distance = false;
        p.update_distance();
        let plot = [800.0, 400.0];
        let wtr = p.one_to_one_traces(200.0, plot).unwrap();
        // 200 samples = 2 m over 400 px -> 0.005 m/px; 800 px wide = 4 m = 80 traces.
        assert!((wtr - 80.0).abs() < 1e-3, "{wtr}");
        assert!((p.one_to_one_samples(wtr, plot).unwrap() - 200.0).abs() < 1e-3);
        p.dx = 0.0;
        p.update_distance();
        assert!(p.one_to_one_traces(200.0, plot).is_none());
    }

    #[test]
    fn topographic_geometry_requires_measured_height_coverage_and_horizontal_spacing() {
        let crs = CrsSpec::Epsg(2056);
        let mut p = Profile::new("test".into(), PathBuf::new(), "test", Section::new(10, 5, 0.2), 0.5);
        p.use_gps_distance = false;
        p.gps = Some(GpsTrack { source: "measured".into(), crs: crs.clone(), fixes: (0..5).map(|i| GpsFix { trace: i as f64, x: 2_600_000.0 + i as f64, y: 1_200_000.0, z: 500.0 + i as f64 * 0.1, has_elevation: true }).collect(), clock_range: None });
        p.update_coords(&crs);
        let g = p.topographic_geometry(&crs, 1.1).unwrap();
        assert_eq!(g.x_m, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert_eq!(p.dist_at(4.0), 2.0); // Display calibration remains nominal.
        p.gps.as_mut().unwrap().fixes[2].has_elevation = false;
        assert!(p.topographic_geometry(&crs, 1.1).is_err());
        p.gps.as_mut().unwrap().fixes[2].has_elevation = true;
        p.gps.as_mut().unwrap().fixes.remove(2);
        assert!(p.topographic_geometry(&crs, 1.1).is_err()); // Unsupported 2 m GPS gap.
        p.gps.as_mut().unwrap().fixes.remove(0);
        p.update_coords(&crs);
        assert!(p.topographic_geometry(&crs, 0.5).is_err()); // First trace 1 m before the first fix.
        let g = p.topographic_geometry(&crs, 3.0).unwrap(); // Short head gap is extrapolated.
        assert_eq!(g.x_m, vec![0.0, 1.0, 2.0, 3.0, 4.0]);
        assert!((g.elevation_m[0] - 500.0).abs() < 1e-9);
    }
}
