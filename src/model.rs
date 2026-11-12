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
    pub data: Vec<f32>,
}

impl Section {
    pub fn new(ns: usize, ntr: usize, dt: f32) -> Self {
        Section { ns, ntr, dt, t0: 0.0, depth_axis: false, data: vec![0.0; ns * ntr] }
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpsFix {
    /// Trace index (0-based, may be fractional).
    pub trace: f64,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpsTrack {
    pub source: String,
    pub crs: CrsSpec,
    pub fixes: Vec<GpsFix>,
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
    /// Per-trace distance along the profile (m).
    pub dist: Vec<f64>,
    pub use_gps_distance: bool,
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
            freq_mhz: 0.0,
            velocity: 0.1,
            steps: Vec::new(),
            header: Vec::new(),
            gps: None,
            embedded_gps: None,
            coords: Vec::new(),
            dist: Vec::new(),
            use_gps_distance: true,
            log: Vec::new(),
            version: 1,
        };
        p.update_distance();
        p
    }

    pub fn has_coords(&self) -> bool {
        !self.coords.is_empty()
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
            .map(|(f, p)| (f.trace, p))
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
        if self.use_gps_distance && self.coords.len() >= 2 && self.coords.len() == n {
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
            if acc > 0.0 {
                self.dist = d;
                return;
            }
        }
        self.dist = (0..n).map(|i| i as f64 * self.dx as f64).collect();
    }

    pub fn length(&self) -> f64 {
        self.dist.last().copied().unwrap_or(0.0)
    }

    pub fn dist_at(&self, trace: f32) -> f64 {
        if self.dist.is_empty() {
            return trace as f64 * self.dx as f64;
        }
        let n = self.dist.len();
        let t = trace.clamp(0.0, (n - 1) as f32);
        let i = t.floor() as usize;
        let f = (t - i as f32) as f64;
        if i + 1 < n {
            self.dist[i] * (1.0 - f) + self.dist[i + 1] * f
        } else {
            self.dist[n - 1]
        }
    }

    pub fn trace_at_dist(&self, d: f64) -> f32 {
        if self.dist.len() < 2 {
            return (d / self.dx as f64) as f32;
        }
        let idx = self.dist.partition_point(|v| *v < d);
        if idx == 0 {
            return 0.0;
        }
        if idx >= self.dist.len() {
            return (self.dist.len() - 1) as f32;
        }
        let a = self.dist[idx - 1];
        let b = self.dist[idx];
        let f = if b > a { (d - a) / (b - a) } else { 0.0 };
        (idx - 1) as f32 + f as f32
    }

    pub fn coord_at(&self, trace: f32) -> Option<[f64; 3]> {
        if self.coords.is_empty() {
            return None;
        }
        let n = self.coords.len();
        let t = trace.clamp(0.0, (n - 1) as f32);
        let i = t.floor() as usize;
        let f = (t - i as f32) as f64;
        let a = self.coords[i];
        let b = self.coords[(i + 1).min(n - 1)];
        Some([a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f])
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
    let f = if span.abs() > 1e-12 { (t - a.0) / span } else { 0.0 };
    // Extrapolation beyond the ends is held at the end fix.
    let f = if idx == 0 || idx >= fixes.len() { f.clamp(0.0, 1.0) } else { f };
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
