//! Processing steps. Each step works on a Section and returns a new one.
//! Formulas follow the original program's manual (see MECHANICS.md).

use crate::model::Section;
use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum TimeZeroMode {
    /// Shift by a fixed time (ns).
    Manual,
    /// First break of the mean trace: first sample whose |amplitude| exceeds threshold % of the maximum.
    FirstBreak,
    /// Like FirstBreak but each trace is aligned on its own first break.
    FirstBreakPerTrace,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Taper {
    Cos,
    Cos2,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum BgMode {
    WholeProfile,
    /// Running mean over this many traces.
    Sliding(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Divergence {
    /// Spherical (proportional to t).
    Spherical,
    /// Line source (proportional to sqrt(t)).
    Cylindrical,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Step {
    TimeZero { mode: TimeZeroMode, shift_ns: f32, threshold_pct: f32 },
    Dewow { window_ns: f32 },
    DcShift { t1: f32, t2: f32 },
    BackgroundRemoval { mode: BgMode, t_start: f32, t_end: f32 },
    BandpassFrequency { f1: f32, f2: f32, f3: f32, f4: f32, taper: Taper },
    BandpassButterworth { f_low: f32, f_high: f32, order: usize },
    AgcGain { window_ns: f32, scale: f32, max_gain: f32 },
    EnergyDecay { scale: f32 },
    GainFunction { linear: f32, exponent_db_m: f32, t_start: f32, max_gain: f32 },
    Divergence { kind: Divergence },
    TraceNormalize,
    RunningAverage { traces: usize },
    DiffractionStack { velocity: f32, width: usize },
    KirchhoffMigration { velocity: f32, width: usize, t_start: f32, t_end: f32 },
    FkMigration { velocity: f32, scale: f32 },
    TimeDepth { velocity: f32, max_depth: f32 },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StepEntry {
    pub enabled: bool,
    pub step: Step,
}

#[derive(Default)]
pub struct Ctx {
    pub dx: f32,
    pub freq_mhz: f32,
    /// Progress and cancel shared with the window while processing runs in the background.
    pub progress: Option<std::sync::Arc<Progress>>,
    /// Shown before the step name in the progress text.
    pub label: String,
}

impl Ctx {
    pub fn tick(&self, f: f32) {
        if let Some(p) = &self.progress {
            p.sub(f);
        }
    }
    pub fn cancelled(&self) -> bool {
        self.progress.as_ref().map(|p| p.is_cancelled()).unwrap_or(false)
    }
}

/// Progress of a background task: overall fraction, a text line and a cancel flag.
/// `set_range` maps the next sub-task's own 0..1 onto a part of the overall bar.
#[derive(Default)]
pub struct Progress {
    frac: std::sync::atomic::AtomicU32,
    range: std::sync::Mutex<(f32, f32)>,
    text: std::sync::Mutex<String>,
    cancel: std::sync::atomic::AtomicBool,
}

impl Progress {
    pub fn new() -> std::sync::Arc<Self> {
        let p = Progress::default();
        *p.range.lock().unwrap() = (0.0, 1.0);
        std::sync::Arc::new(p)
    }
    pub fn set_range(&self, a: f32, b: f32) {
        *self.range.lock().unwrap() = (a, b);
        self.set(a);
    }
    pub fn set(&self, f: f32) {
        self.frac.store((f.clamp(0.0, 1.0) * 1_000_000.0) as u32, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn sub(&self, f: f32) {
        let (a, b) = *self.range.lock().unwrap();
        self.set(a + (b - a) * f.clamp(0.0, 1.0));
    }
    pub fn fraction(&self) -> f32 {
        self.frac.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1_000_000.0
    }
    pub fn set_text(&self, t: impl Into<String>) {
        *self.text.lock().unwrap() = t.into();
    }
    pub fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl Step {
    pub fn name(&self) -> &'static str {
        match self {
            Step::TimeZero { .. } => "Set time zero",
            Step::Dewow { .. } => "Subtract-mean (dewow)",
            Step::DcShift { .. } => "Subtract DC shift",
            Step::BackgroundRemoval { .. } => "Background removal",
            Step::BandpassFrequency { .. } => "Bandpass (frequency)",
            Step::BandpassButterworth { .. } => "Bandpass Butterworth",
            Step::AgcGain { .. } => "AGC gain",
            Step::EnergyDecay { .. } => "Energy decay gain",
            Step::GainFunction { .. } => "Gain function",
            Step::Divergence { .. } => "Divergence compensation",
            Step::TraceNormalize => "Trace normalisation",
            Step::RunningAverage { .. } => "Running average (x)",
            Step::DiffractionStack { .. } => "Diffraction stack",
            Step::KirchhoffMigration { .. } => "Kirchhoff migration",
            Step::FkMigration { .. } => "FK migration (Stolt)",
            Step::TimeDepth { .. } => "Time-depth conversion",
        }
    }

    pub fn summary(&self) -> String {
        match self {
            Step::TimeZero { mode, shift_ns, threshold_pct } => match mode {
                TimeZeroMode::Manual => format!("shift {shift_ns:.2} ns"),
                TimeZeroMode::FirstBreak => format!("first break {threshold_pct:.0}%"),
                TimeZeroMode::FirstBreakPerTrace => format!("per trace {threshold_pct:.0}%"),
            },
            Step::Dewow { window_ns } => format!("window {window_ns:.2} ns"),
            Step::DcShift { t1, t2 } => trange(*t1, *t2),
            Step::BackgroundRemoval { mode, t_start, t_end } => match mode {
                BgMode::WholeProfile => format!("whole profile, {}", trange(*t_start, *t_end)),
                BgMode::Sliding(n) => format!("{n} traces, {}", trange(*t_start, *t_end)),
            },
            Step::BandpassFrequency { f1, f2, f3, f4, .. } => format!("{f1:.0}/{f2:.0}/{f3:.0}/{f4:.0} MHz"),
            Step::BandpassButterworth { f_low, f_high, order } => format!("{f_low:.0}-{f_high:.0} MHz, order {order}"),
            Step::AgcGain { window_ns, .. } => format!("window {window_ns:.1} ns"),
            Step::EnergyDecay { scale } => format!("scale {scale:.2}"),
            Step::GainFunction { linear, exponent_db_m, .. } => format!("a'={linear:.2}, b'={exponent_db_m:.2} dB/m"),
            Step::Divergence { kind } => match kind {
                Divergence::Spherical => "spherical (t)".into(),
                Divergence::Cylindrical => "line source (sqrt t)".into(),
            },
            Step::TraceNormalize => String::new(),
            Step::RunningAverage { traces } => format!("{traces} traces"),
            Step::DiffractionStack { velocity, width } => format!("v={velocity:.3} m/ns, {width} traces"),
            Step::KirchhoffMigration { velocity, width, t_start, t_end } => format!("v={velocity:.3} m/ns, {width} traces, {}", trange(*t_start, *t_end)),
            Step::FkMigration { velocity, scale } => format!("v={velocity:.3} m/ns, x{scale:.1}"),
            Step::TimeDepth { velocity, .. } => format!("v={velocity:.3} m/ns"),
        }
    }

    pub fn defaults() -> Vec<Step> {
        vec![
            Step::TimeZero { mode: TimeZeroMode::FirstBreak, shift_ns: 0.0, threshold_pct: 20.0 },
            Step::Dewow { window_ns: 5.0 },
            Step::DcShift { t1: 0.0, t2: 1e9 },
            Step::BackgroundRemoval { mode: BgMode::WholeProfile, t_start: 0.0, t_end: 1e9 },
            Step::BandpassFrequency { f1: 100.0, f2: 200.0, f3: 600.0, f4: 900.0, taper: Taper::Cos },
            Step::BandpassButterworth { f_low: 150.0, f_high: 800.0, order: 4 },
            Step::AgcGain { window_ns: 20.0, scale: 1.0, max_gain: 1000.0 },
            Step::EnergyDecay { scale: 1.0 },
            Step::GainFunction { linear: 1.0, exponent_db_m: 2.0, t_start: 0.0, max_gain: 1000.0 },
            Step::Divergence { kind: Divergence::Spherical },
            Step::TraceNormalize,
            Step::RunningAverage { traces: 3 },
            Step::DiffractionStack { velocity: 0.1, width: 40 },
            Step::KirchhoffMigration { velocity: 0.1, width: 40, t_start: 0.0, t_end: 1e9 },
            Step::FkMigration { velocity: 0.1, scale: 2.0 },
            Step::TimeDepth { velocity: 0.1, max_depth: 0.0 },
        ]
    }

    pub fn needs_time_axis(&self) -> bool {
        !matches!(self, Step::TraceNormalize | Step::RunningAverage { .. })
    }

    pub fn apply(&self, s: &Section, ctx: &Ctx) -> Result<Section, String> {
        if s.depth_axis && self.needs_time_axis() {
            return Err("the section is already converted to depth".into());
        }
        let mut o = s.clone();
        match *self {
            Step::TimeZero { mode, shift_ns, threshold_pct } => return Ok(time_zero(s, mode, shift_ns, threshold_pct)),
            Step::Dewow { window_ns } => {
                let w = ((window_ns / s.dt).round() as usize).max(1);
                for t in 0..s.ntr {
                    running_mean_subtract(o.trace_mut(t), w);
                }
            }
            Step::DcShift { t1, t2 } => {
                let (a, b) = sample_range(s, t1, t2);
                for t in 0..s.ntr {
                    let tr = o.trace_mut(t);
                    let n = (b - a).max(1) as f32;
                    let m: f32 = tr[a..b].iter().sum::<f32>() / n;
                    tr.iter_mut().for_each(|v| *v -= m);
                }
            }
            Step::BackgroundRemoval { mode, t_start, t_end } => background_removal(&mut o, mode, t_start, t_end),
            Step::BandpassFrequency { f1, f2, f3, f4, taper } => bandpass_frequency(&mut o, [f1, f2, f3, f4], taper),
            Step::BandpassButterworth { f_low, f_high, order } => butterworth(&mut o, f_low, f_high, order.clamp(1, 12)),
            Step::AgcGain { window_ns, scale, max_gain } => agc(&mut o, window_ns, scale, max_gain),
            Step::EnergyDecay { scale } => energy_decay(&mut o, scale),
            Step::GainFunction { linear, exponent_db_m, t_start, max_gain } => {
                gain_function(&mut o, linear, exponent_db_m, t_start, max_gain, ctx.freq_mhz)
            }
            Step::Divergence { kind } => {
                for t in 0..s.ntr {
                    for i in 0..s.ns {
                        let tt = s.y_of(i as f32).max(0.0);
                        let g = match kind {
                            Divergence::Spherical => tt,
                            Divergence::Cylindrical => tt.sqrt(),
                        };
                        o.data[t * s.ns + i] *= g.max(1e-3);
                    }
                }
            }
            Step::TraceNormalize => {
                for t in 0..s.ntr {
                    let tr = o.trace_mut(t);
                    let m = tr.iter().fold(0f32, |a, v| a.max(v.abs()));
                    if m > 0.0 {
                        tr.iter_mut().for_each(|v| *v /= m);
                    }
                }
            }
            Step::RunningAverage { traces } => {
                let h = (traces.max(1) / 2) as isize;
                for t in 0..s.ntr as isize {
                    let a = (t - h).max(0) as usize;
                    let b = ((t + h) as usize).min(s.ntr - 1);
                    let n = (b - a + 1) as f32;
                    for i in 0..s.ns {
                        let mut acc = 0.0;
                        for k in a..=b {
                            acc += s.data[k * s.ns + i];
                        }
                        o.data[t as usize * s.ns + i] = acc / n;
                    }
                }
            }
            Step::DiffractionStack { velocity, width } => return Ok(kirchhoff(s, ctx, velocity, width, 0.0, 1e9, false)),
            Step::KirchhoffMigration { velocity, width, t_start, t_end } => {
                return Ok(kirchhoff(s, ctx, velocity, width, t_start, t_end, true))
            }
            Step::FkMigration { velocity, scale } => return Ok(stolt(s, ctx, velocity, scale)),
            Step::TimeDepth { velocity, max_depth } => {
                // Constant velocity: depth = v * t / 2 for every sample (t relative to time zero).
                o.depth_axis = true;
                o.dt = s.dt * velocity / 2.0;
                o.t0 = s.t0 * velocity / 2.0;
                if max_depth > 0.0 {
                    let keep = ((max_depth - o.t0) / o.dt).ceil().max(1.0) as usize;
                    if keep < s.ns {
                        o = crop_samples(&o, 0, keep);
                    }
                }
            }
        }
        Ok(o)
    }
}

fn trange(a: f32, b: f32) -> String {
    if b >= 1e8 {
        format!("{a:.1} ns to end")
    } else {
        format!("{a:.1}-{b:.1} ns")
    }
}

/// Rough relative cost of a step, used to share the progress bar between steps.
fn step_cost(s: &Step) -> f32 {
    match s {
        Step::KirchhoffMigration { .. } | Step::DiffractionStack { .. } => 40.0,
        Step::FkMigration { .. } => 15.0,
        Step::BandpassFrequency { .. } | Step::BandpassButterworth { .. } => 2.0,
        _ => 1.0,
    }
}

pub fn run_all(raw: &Section, steps: &[StepEntry], ctx: &Ctx) -> (Section, Vec<String>) {
    let mut cur = raw.clone();
    let mut log = Vec::new();
    let total: f32 = steps.iter().filter(|e| e.enabled).map(|e| step_cost(&e.step)).sum::<f32>().max(1e-6);
    let mut done = 0.0;
    let outer = ctx.progress.as_ref().map(|p| *p.range.lock().unwrap());
    for (i, e) in steps.iter().enumerate() {
        if !e.enabled {
            continue;
        }
        if ctx.cancelled() {
            log.push("cancelled".into());
            break;
        }
        let c = step_cost(&e.step);
        if let (Some(p), Some((a, b))) = (&ctx.progress, outer) {
            p.set_range(a + (b - a) * done / total, a + (b - a) * (done + c) / total);
        }
        done += c;
        if let Some(p) = &ctx.progress {
            p.set_text(if ctx.label.is_empty() { e.step.name().to_string() } else { format!("{} — {}", ctx.label, e.step.name()) });
        }
        match e.step.apply(&cur, ctx) {
            Ok(s) => cur = s,
            Err(err) => log.push(format!("step {} ({}) skipped: {err}", i + 1, e.step.name())),
        }
    }
    (cur, log)
}

fn sample_range(s: &Section, t1: f32, t2: f32) -> (usize, usize) {
    let a = s.sample_of(t1.min(t2)).floor().max(0.0) as usize;
    let b = (s.sample_of(t1.max(t2)).ceil().max(0.0) as usize + 1).min(s.ns);
    (a.min(s.ns.saturating_sub(1)), b.max(a.min(s.ns.saturating_sub(1)) + 1).min(s.ns))
}

fn crop_samples(s: &Section, first: usize, count: usize) -> Section {
    let count = count.min(s.ns - first.min(s.ns));
    let mut o = Section { ns: count, ntr: s.ntr, dt: s.dt, t0: s.y_of(first as f32), depth_axis: s.depth_axis, data: vec![0.0; count * s.ntr] };
    for t in 0..s.ntr {
        o.trace_mut(t).copy_from_slice(&s.trace(t)[first..first + count]);
    }
    o
}

pub fn mean_abs_trace(s: &Section) -> Vec<f32> {
    let mut m = vec![0f32; s.ns];
    for t in 0..s.ntr {
        for (i, v) in s.trace(t).iter().enumerate() {
            m[i] += v.abs();
        }
    }
    let n = s.ntr.max(1) as f32;
    m.iter_mut().for_each(|v| *v /= n);
    m
}

fn first_break(tr: &[f32], threshold_pct: f32) -> usize {
    let max = tr.iter().fold(0f32, |a, v| a.max(v.abs()));
    if max <= 0.0 {
        return 0;
    }
    let th = max * threshold_pct.clamp(0.1, 100.0) / 100.0;
    tr.iter().position(|v| v.abs() >= th).unwrap_or(0)
}

/// Remove everything before time zero and set the time of the new first sample to 0.
fn time_zero(s: &Section, mode: TimeZeroMode, shift_ns: f32, threshold_pct: f32) -> Section {
    match mode {
        TimeZeroMode::Manual => {
            let k = s.sample_of(s.t0 + shift_ns).round();
            if k >= 0.0 {
                let mut o = crop_samples(s, (k as usize).min(s.ns - 1), s.ns);
                o.t0 = 0.0;
                o
            } else {
                // Negative shift: pad with zeros at the top.
                let pad = (-k) as usize;
                let mut o = Section::new(s.ns + pad, s.ntr, s.dt);
                for t in 0..s.ntr {
                    o.trace_mut(t)[pad..].copy_from_slice(s.trace(t));
                }
                o
            }
        }
        TimeZeroMode::FirstBreak => {
            let m = mean_abs_trace(s);
            let k = first_break(&m, threshold_pct);
            let mut o = crop_samples(s, k, s.ns);
            o.t0 = 0.0;
            o
        }
        TimeZeroMode::FirstBreakPerTrace => {
            let ks: Vec<usize> = (0..s.ntr).map(|t| first_break(s.trace(t), threshold_pct)).collect();
            let kmax = ks.iter().copied().max().unwrap_or(0);
            let n = s.ns - kmax;
            let mut o = Section::new(n.max(1), s.ntr, s.dt);
            for t in 0..s.ntr {
                let k = ks[t];
                let src = &s.trace(t)[k..];
                let len = src.len().min(o.ns);
                o.trace_mut(t)[..len].copy_from_slice(&src[..len]);
            }
            o
        }
    }
}

fn running_mean_subtract(tr: &mut [f32], w: usize) {
    let n = tr.len();
    if n == 0 {
        return;
    }
    let h = w / 2;
    let mut pre = vec![0f64; n + 1];
    for i in 0..n {
        pre[i + 1] = pre[i] + tr[i] as f64;
    }
    let orig: Vec<f32> = tr.to_vec();
    for i in 0..n {
        let a = i.saturating_sub(h);
        let b = (i + h + 1).min(n);
        let m = (pre[b] - pre[a]) / (b - a) as f64;
        tr[i] = orig[i] - m as f32;
    }
}

fn background_removal(o: &mut Section, mode: BgMode, t_start: f32, t_end: f32) {
    let (a, b) = sample_range(o, t_start, t_end);
    let ns = o.ns;
    let ntr = o.ntr;
    match mode {
        BgMode::WholeProfile => {
            let mut mean = vec![0f64; ns];
            for t in 0..ntr {
                for i in a..b {
                    mean[i] += o.data[t * ns + i] as f64;
                }
            }
            for v in mean.iter_mut() {
                *v /= ntr.max(1) as f64;
            }
            for t in 0..ntr {
                for i in a..b {
                    o.data[t * ns + i] -= mean[i] as f32;
                }
            }
        }
        BgMode::Sliding(n) => {
            let src = o.data.clone();
            let h = (n.max(2) / 2) as isize;
            for i in a..b {
                let mut pre = vec![0f64; ntr + 1];
                for t in 0..ntr {
                    pre[t + 1] = pre[t] + src[t * ns + i] as f64;
                }
                for t in 0..ntr as isize {
                    let lo = (t - h).max(0) as usize;
                    let hi = ((t + h + 1) as usize).min(ntr);
                    let m = (pre[hi] - pre[lo]) / (hi - lo) as f64;
                    o.data[t as usize * ns + i] = src[t as usize * ns + i] - m as f32;
                }
            }
        }
    }
}

/// Amplitude spectrum (MHz, amplitude) of the mean of all traces' spectra.
pub fn mean_spectrum(s: &Section) -> Vec<(f32, f32)> {
    let n = s.ns.next_power_of_two().max(2);
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(n);
    let mut acc = vec![0f32; n / 2 + 1];
    let step = (s.ntr / 200).max(1);
    let mut count = 0;
    let mut buf = vec![Complex32::new(0.0, 0.0); n];
    for t in (0..s.ntr).step_by(step) {
        buf.iter_mut().for_each(|c| *c = Complex32::new(0.0, 0.0));
        for (i, v) in s.trace(t).iter().enumerate() {
            buf[i].re = *v;
        }
        fft.process(&mut buf);
        for k in 0..=n / 2 {
            acc[k] += buf[k].norm();
        }
        count += 1;
    }
    let fs = 1000.0 / s.dt; // MHz
    acc.iter().enumerate().map(|(k, a)| (k as f32 * fs / n as f32, a / count.max(1) as f32)).collect()
}

fn bandpass_frequency(o: &mut Section, f: [f32; 4], taper: Taper) {
    let n = (o.ns * 2).next_power_of_two();
    let fs = 1000.0 / o.dt;
    let mut planner = FftPlanner::<f32>::new();
    let fwd = planner.plan_fft_forward(n);
    let inv = planner.plan_fft_inverse(n);
    let weights: Vec<f32> = (0..n)
        .map(|k| {
            let kk = if k <= n / 2 { k } else { n - k };
            let fr = kk as f32 * fs / n as f32;
            bandpass_weight(fr, f, taper)
        })
        .collect();
    let mut buf = vec![Complex32::new(0.0, 0.0); n];
    for t in 0..o.ntr {
        buf.iter_mut().for_each(|c| *c = Complex32::new(0.0, 0.0));
        let tr = o.trace_mut(t);
        for (i, v) in tr.iter().enumerate() {
            buf[i].re = *v;
        }
        fwd.process(&mut buf);
        for (c, w) in buf.iter_mut().zip(&weights) {
            *c *= *w;
        }
        inv.process(&mut buf);
        for (i, v) in tr.iter_mut().enumerate() {
            *v = buf[i].re / n as f32;
        }
    }
}

pub fn bandpass_weight(fr: f32, f: [f32; 4], taper: Taper) -> f32 {
    let [f1, f2, f3, f4] = f;
    let ramp = |x: f32| -> f32 {
        let c = 0.5 - 0.5 * (std::f32::consts::PI * x.clamp(0.0, 1.0)).cos();
        match taper {
            Taper::Cos => c,
            Taper::Cos2 => c * c,
        }
    };
    if fr < f1 || fr > f4 {
        0.0
    } else if fr < f2 {
        ramp((fr - f1) / (f2 - f1).max(1e-6))
    } else if fr <= f3 {
        1.0
    } else {
        ramp((f4 - fr) / (f4 - f3).max(1e-6))
    }
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f32; 3],
    a: [f32; 3],
}

impl Biquad {
    fn run(&self, x: &mut [f32]) {
        let (mut x1, mut x2, mut y1, mut y2) = (0f32, 0f32, 0f32, 0f32);
        for v in x.iter_mut() {
            let x0 = *v;
            let y0 = self.b[0] * x0 + self.b[1] * x1 + self.b[2] * x2 - self.a[1] * y1 - self.a[2] * y2;
            x2 = x1;
            x1 = x0;
            y2 = y1;
            y1 = y0;
            *v = y0;
        }
    }
}

/// Butterworth sections (bilinear transform) for one low- or high-pass of the given order.
fn butter_sections(fc: f32, fs: f32, order: usize, high: bool) -> Vec<Biquad> {
    let mut out = Vec::new();
    let w0 = 2.0 * std::f32::consts::PI * (fc / fs).clamp(1e-4, 0.499);
    let cosw = w0.cos();
    let sinw = w0.sin();
    for k in 0..order / 2 {
        let theta = std::f32::consts::PI * (2 * k + 1) as f32 / (2 * order) as f32;
        let q = 1.0 / (2.0 * theta.sin());
        let alpha = sinw / (2.0 * q);
        let a0 = 1.0 + alpha;
        let (b0, b1, b2) = if high { ((1.0 + cosw) / 2.0, -(1.0 + cosw), (1.0 + cosw) / 2.0) } else { ((1.0 - cosw) / 2.0, 1.0 - cosw, (1.0 - cosw) / 2.0) };
        out.push(Biquad { b: [b0 / a0, b1 / a0, b2 / a0], a: [1.0, -2.0 * cosw / a0, (1.0 - alpha) / a0] });
    }
    if order % 2 == 1 {
        // First-order section.
        let k = (w0 / 2.0).tan();
        let a0 = k + 1.0;
        let (b0, b1) = if high { (1.0 / a0, -1.0 / a0) } else { (k / a0, k / a0) };
        out.push(Biquad { b: [b0, b1, 0.0], a: [1.0, (k - 1.0) / a0, 0.0] });
    }
    out
}

/// Recursive Butterworth bandpass in the time domain, run forward and backward (zero phase).
fn butterworth(o: &mut Section, f_low: f32, f_high: f32, order: usize) {
    let fs = 1000.0 / o.dt;
    let mut secs = Vec::new();
    if f_low > 0.0 {
        secs.extend(butter_sections(f_low, fs, order, true));
    }
    if f_high > 0.0 && f_high < fs / 2.0 {
        secs.extend(butter_sections(f_high, fs, order, false));
    }
    for t in 0..o.ntr {
        let tr = o.trace_mut(t);
        for s in &secs {
            s.run(tr);
        }
        tr.reverse();
        for s in &secs {
            s.run(tr);
        }
        tr.reverse();
    }
}

fn agc(o: &mut Section, window_ns: f32, scale: f32, max_gain: f32) {
    let w = ((window_ns / o.dt).round() as usize).max(1);
    let h = w / 2;
    for t in 0..o.ntr {
        let tr = o.trace_mut(t);
        let n = tr.len();
        let total = tr.iter().map(|v| v.abs() as f64).sum::<f64>() / n.max(1) as f64;
        let mut pre = vec![0f64; n + 1];
        for i in 0..n {
            pre[i + 1] = pre[i] + tr[i].abs() as f64;
        }
        for i in 0..n {
            let a = i.saturating_sub(h);
            let b = (i + h + 1).min(n);
            let m = (pre[b] - pre[a]) / (b - a) as f64;
            let g = if m > 0.0 { (total / m) as f32 } else { max_gain };
            tr[i] *= g.min(max_gain) * scale;
        }
    }
}

fn energy_decay(o: &mut Section, scale: f32) {
    let m = mean_abs_trace(o);
    // Median filter over 11 samples (window length estimated).
    let h = 5usize;
    let n = m.len();
    let mut curve = vec![0f32; n];
    for i in 0..n {
        let a = i.saturating_sub(h);
        let b = (i + h + 1).min(n);
        let mut w: Vec<f32> = m[a..b].to_vec();
        w.sort_by(|x, y| x.partial_cmp(y).unwrap_or(std::cmp::Ordering::Equal));
        curve[i] = w[w.len() / 2];
    }
    let max = curve.iter().fold(0f32, |a, v| a.max(*v));
    let floor = max * 1e-4;
    for t in 0..o.ntr {
        let tr = o.trace_mut(t);
        for i in 0..n {
            tr[i] = tr[i] / curve[i].max(floor).max(1e-20) * max * scale;
        }
    }
}

/// g(t) = (1 + a t) e^(b t), a = a'/pulse width, b = b' v / 8.69 with v = 0.1 m/ns.
fn gain_function(o: &mut Section, linear: f32, exponent_db_m: f32, t_start: f32, max_gain: f32, freq_mhz: f32) {
    let pulse_ns = if freq_mhz > 0.0 { 1000.0 / freq_mhz } else { 1.0 };
    let a = linear / pulse_ns;
    let b = exponent_db_m * 0.1 / 8.69;
    for i in 0..o.ns {
        let t = (o.y_of(i as f32) - t_start).max(0.0);
        let g = ((1.0 + a * t) * (b * t).exp()).min(max_gain);
        for tr in 0..o.ntr {
            o.data[tr * o.ns + i] *= g;
        }
    }
}

/// Constant-velocity zero-offset time migration by summation along diffraction hyperbolas.
/// `weighted` = Kirchhoff (time derivative, obliquity weight), otherwise plain diffraction stack.
fn kirchhoff(s: &Section, ctx: &Ctx, v: f32, width: usize, t_start: f32, t_end: f32, weighted: bool) -> Section {
    let dx = ctx.dx;
    let ns = s.ns;
    let ntr = s.ntr;
    let mut input = s.clone();
    if weighted {
        for t in 0..ntr {
            let tr = input.trace_mut(t);
            let orig = tr.to_vec();
            for i in 0..ns {
                let a = if i > 0 { orig[i - 1] } else { orig[i] };
                let b = if i + 1 < ns { orig[i + 1] } else { orig[i] };
                tr[i] = (b - a) * 0.5;
            }
        }
    }
    let half = (width.max(1) / 2) as isize;
    let (a, b) = sample_range(s, t_start, t_end);
    let mut out = s.clone();
    let k = 2.0 * dx / v.max(1e-4); // two-way time per trace offset (ns)
    for x0 in 0..ntr as isize {
        if x0 % 16 == 0 {
            ctx.tick(x0 as f32 / ntr as f32);
            if ctx.cancelled() {
                break;
            }
        }
        for i in a..b {
            let t0 = s.y_of(i as f32).max(0.0);
            let mut acc = 0f32;
            let mut wsum = 0f32;
            for dxi in -half..=half {
                let x = x0 + dxi;
                if x < 0 || x >= ntr as isize {
                    continue;
                }
                let off = k * dxi as f32;
                let t = (t0 * t0 + off * off).sqrt();
                let si = s.sample_of(t);
                if si < 0.0 || si >= (ns - 1) as f32 {
                    continue;
                }
                let j = si.floor() as usize;
                let f = si - j as f32;
                let tr = input.trace(x as usize);
                let val = tr[j] * (1.0 - f) + tr[j + 1] * f;
                let w = if weighted { if t > 0.0 { t0 / t } else { 1.0 } } else { 1.0 };
                acc += val * w;
                wsum += 1.0;
            }
            out.data[x0 as usize * ns + i] = if wsum > 0.0 { acc / wsum } else { 0.0 };
        }
    }
    out
}

/// Stolt FK migration for constant velocity (exploding reflector, v/2).
fn stolt(s: &Section, ctx: &Ctx, v: f32, scale: f32) -> Section {
    let dx = ctx.dx;
    let nt = ((s.ns as f32 * scale.max(1.0)).ceil() as usize).next_power_of_two();
    let nx = (s.ntr * 2).next_power_of_two();
    let mut planner = FftPlanner::<f32>::new();
    let ft = planner.plan_fft_forward(nt);
    let ftinv = planner.plan_fft_inverse(nt);
    let fx = planner.plan_fft_forward(nx);
    let fxinv = planner.plan_fft_inverse(nx);
    // grid[kx][w]
    let zero = Complex32::new(0.0, 0.0);
    let mut grid = vec![zero; nx * nt];
    for x in 0..s.ntr {
        let row = &mut grid[x * nt..(x + 1) * nt];
        for (i, val) in s.trace(x).iter().enumerate() {
            row[i].re = *val;
        }
        ft.process(row);
    }
    // FFT along x for each frequency.
    let mut col = vec![zero; nx];
    for w in 0..nt {
        for x in 0..nx {
            col[x] = grid[x * nt + w];
        }
        fx.process(&mut col);
        for x in 0..nx {
            grid[x * nt + w] = col[x];
        }
    }
    let dw = 2.0 * std::f32::consts::PI / (nt as f32 * s.dt); // rad/ns
    let dk = 2.0 * std::f32::consts::PI / (nx as f32 * dx); // rad/m
    let vh = v / 2.0;
    let mut out = vec![zero; nx * nt];
    ctx.tick(0.4);
    for kxi in 0..nx {
        let kx = if kxi <= nx / 2 { kxi as f32 } else { kxi as f32 - nx as f32 } * dk;
        let row = &grid[kxi * nt..(kxi + 1) * nt];
        let orow = &mut out[kxi * nt..(kxi + 1) * nt];
        for wi in 0..nt {
            let wo = if wi <= nt / 2 { wi as f32 } else { wi as f32 - nt as f32 } * dw;
            if wo == 0.0 {
                continue;
            }
            let w = wo.signum() * (wo * wo + (vh * kx) * (vh * kx)).sqrt();
            let fidx = w / dw;
            if fidx.abs() >= (nt / 2 - 1) as f32 {
                continue;
            }
            let fi = if fidx < 0.0 { fidx + nt as f32 } else { fidx };
            let j = fi.floor() as usize;
            let f = fi - j as f32;
            let a = row[j % nt];
            let b = row[(j + 1) % nt];
            let val = a * (1.0 - f) + b * f;
            orow[wi] = val * (wo.abs() / w.abs());
        }
    }
    ctx.tick(0.6);
    // Inverse transforms.
    for w in 0..nt {
        for x in 0..nx {
            col[x] = out[x * nt + w];
        }
        fxinv.process(&mut col);
        for x in 0..nx {
            out[x * nt + w] = col[x];
        }
    }
    let mut o = s.clone();
    let norm = (nt * nx) as f32;
    for x in 0..s.ntr {
        let row = &mut out[x * nt..(x + 1) * nt];
        ftinv.process(row);
        let tr = o.trace_mut(x);
        for i in 0..s.ns {
            tr[i] = row[i].re / norm;
        }
    }
    o
}

/// Snap a pick to the nearest extremum / zero crossing within ±`win` samples.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Snap {
    None,
    Maximum,
    Minimum,
    AbsMaximum,
    ZeroCrossing,
}

pub fn snap_sample(tr: &[f32], s: f32, win: usize, mode: Snap) -> f32 {
    let n = tr.len();
    if n == 0 || mode == Snap::None {
        return s;
    }
    let c = s.round().clamp(0.0, (n - 1) as f32) as usize;
    let a = c.saturating_sub(win);
    let b = (c + win + 1).min(n);
    match mode {
        Snap::None => s,
        Snap::Maximum => (a..b).max_by(|x, y| tr[*x].partial_cmp(&tr[*y]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(c) as f32,
        Snap::Minimum => (a..b).min_by(|x, y| tr[*x].partial_cmp(&tr[*y]).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(c) as f32,
        Snap::AbsMaximum => (a..b).max_by(|x, y| tr[*x].abs().partial_cmp(&tr[*y].abs()).unwrap_or(std::cmp::Ordering::Equal)).unwrap_or(c) as f32,
        Snap::ZeroCrossing => {
            let mut best = s;
            let mut bd = f32::MAX;
            for i in a..b.saturating_sub(1) {
                if (tr[i] <= 0.0) != (tr[i + 1] <= 0.0) {
                    let f = tr[i] / (tr[i] - tr[i + 1]);
                    let pos = i as f32 + f;
                    if (pos - s).abs() < bd {
                        bd = (pos - s).abs();
                        best = pos;
                    }
                }
            }
            best
        }
    }
}
