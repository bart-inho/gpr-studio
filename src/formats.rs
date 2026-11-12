//! Readers for GPR system files.

use crate::crs::CrsSpec;
use crate::model::{GpsFix, GpsTrack, Profile, Section};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum SegyTimeUnit {
    Picoseconds,
    Nanoseconds,
    Microseconds,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ImportOptions {
    pub segy_time_unit: SegyTimeUnit,
    pub segy_force_ibm: bool,
    /// Coordinates stored in SEG-Y trace headers; their CRS when projected.
    pub segy_coord_crs: CrsSpec,
    pub default_dx: f32,
}

impl Default for ImportOptions {
    fn default() -> Self {
        ImportOptions {
            segy_time_unit: SegyTimeUnit::Picoseconds,
            segy_force_ibm: false,
            segy_coord_crs: CrsSpec::Epsg(2056),
            default_dx: 0.05,
        }
    }
}

pub const SUPPORTED: &[(&str, &str)] = &[
    ("GSSI (SIR, StructureScan, UtilityScan)", "dzt"),
    ("MALA / RAMAC (16 bit)", "rd3 + rad"),
    ("MALA (32 bit)", "rd7 + rad"),
    ("ImpulseRadar (Raptor, CrossOver, PinPointR)", "iprb + iprh"),
    ("Sensors & Software pulseEKKO / Noggin", "dt1 + hd"),
    ("SEG-Y (IDS, Proceq GP8000/GP8800, generic)", "sgy, segy, seg"),
    ("REFLEXW processed data", "??t + ??r (e.g. .00t/.00r)"),
];

pub fn file_filter_exts() -> Vec<&'static str> {
    vec!["dzt", "DZT", "rd3", "RD3", "rd7", "RD7", "iprb", "IPRB", "dt1", "DT1", "sgy", "SGY", "segy", "SEGY", "seg", "SEG", "00t", "00T", "01t", "01T", "02t", "02T", "03t", "03T"]
}

pub fn load(path: &Path, opt: &ImportOptions) -> Result<Vec<Profile>, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let mut out = match ext.as_str() {
        "dzt" => read_dzt(path)?,
        "rd3" | "rd7" | "rad" => vec![read_mala(path)?],
        "iprb" | "iprh" => vec![read_impulse(path)?],
        "dt1" | "hd" => vec![read_dt1(path)?],
        "sgy" | "segy" | "seg" => vec![read_segy(path, opt)?],
        "dt" => return Err("IDS native .dt files are not readable yet. Export the line as SEG-Y from the IDS software (GRED / OneVision) and import that file.".into()),
        e if e.len() == 3 && e.as_bytes()[0].is_ascii_digit() && e.as_bytes()[1].is_ascii_digit() && (e.ends_with('t') || e.ends_with('r')) => {
            vec![read_reflexw(path)?]
        }
        _ => {
            // Try by content.
            let head = std::fs::read(path).map_err(|e| e.to_string())?;
            if head.len() > 3600 && looks_like_segy(&head) {
                vec![read_segy(path, opt)?]
            } else {
                return Err(format!("unknown file type .{ext}"));
            }
        }
    };
    for p in out.iter_mut() {
        if p.dx <= 0.0 || !p.dx.is_finite() {
            p.dx = opt.default_dx;
        }
        p.update_distance();
    }
    Ok(out)
}

fn stem_name(path: &Path) -> String {
    path.file_stem().and_then(|s| s.to_str()).unwrap_or("profile").to_string()
}

fn sibling(path: &Path, exts: &[&str]) -> Option<PathBuf> {
    for e in exts {
        let p = path.with_extension(e);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn u16le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn i16le(b: &[u8], o: usize) -> i16 {
    i16::from_le_bytes([b[o], b[o + 1]])
}
fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn i32le(b: &[u8], o: usize) -> i32 {
    i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32le(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

// ------------------------------------------------------------------ GSSI DZT

pub fn read_dzt(path: &Path) -> Result<Vec<Profile>, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() < 1024 {
        return Err("file too small for a DZT header".into());
    }
    let tag = u16le(&b, 0);
    if tag & 0xff != 0xff {
        return Err("not a GSSI DZT file (bad header tag)".into());
    }
    let rh_data = u16le(&b, 2) as usize;
    let nsamp = u16le(&b, 4) as usize;
    let bits = u16le(&b, 6) as usize;
    let sps = f32le(&b, 10);
    let spm = f32le(&b, 14);
    let position = f32le(&b, 22);
    let range = f32le(&b, 26);
    let nchan = (u16le(&b, 52) as usize).max(1);
    let epsr = f32le(&b, 54);
    let antname: String = b[98..112].iter().take_while(|c| **c != 0).map(|c| *c as char).collect();
    let data_off = if rh_data < 1024 { rh_data.max(1) * 1024 } else { rh_data };
    if nsamp == 0 || !(bits == 8 || bits == 16 || bits == 32) {
        return Err(format!("unsupported DZT layout (samples {nsamp}, bits {bits})"));
    }
    let bps = bits / 8;
    let body = &b[data_off.min(b.len())..];
    let scan_bytes = nsamp * bps * nchan;
    let nscans = body.len() / scan_bytes;
    if nscans == 0 {
        return Err("DZT file holds no traces".into());
    }
    let dt = if range > 0.0 { range / nsamp as f32 } else { 0.1 };
    let freq = parse_mhz(&antname).unwrap_or(0.0);
    let mut out = Vec::new();
    for ch in 0..nchan {
        let mut sec = Section::new(nsamp, nscans, dt);
        for t in 0..nscans {
            let base = t * scan_bytes + ch * nsamp * bps;
            let tr = sec.trace_mut(t);
            for s in 0..nsamp {
                let o = base + s * bps;
                tr[s] = match bits {
                    8 => body[o] as f32 - 128.0,
                    16 => u16le(body, o) as f32 - 32768.0,
                    _ => i32le(body, o) as f32,
                };
            }
            // The first two samples hold scan numbers and marks, not signal.
            tr[0] = 0.0;
            if nsamp > 1 {
                tr[1] = 0.0;
            }
        }
        let dx = if spm > 0.0 { 1.0 / spm } else { 0.0 };
        let name = if nchan > 1 { format!("{} ch{}", stem_name(path), ch + 1) } else { stem_name(path) };
        let mut p = Profile::new(name, path.to_path_buf(), "GSSI DZT", sec, dx);
        p.channel = ch;
        p.freq_mhz = freq;
        if epsr > 1.0 {
            p.velocity = 0.2998 / epsr.sqrt();
        }
        p.header = vec![
            ("samples".into(), nsamp.to_string()),
            ("bits".into(), bits.to_string()),
            ("channels".into(), nchan.to_string()),
            ("range (ns)".into(), format!("{range:.3}")),
            ("position (ns)".into(), format!("{position:.3}")),
            ("scans/s".into(), format!("{sps:.2}")),
            ("scans/m".into(), format!("{spm:.3}")),
            ("dielectric".into(), format!("{epsr:.2}")),
            ("antenna".into(), antname.clone()),
        ];
        if spm <= 0.0 {
            p.log.push("No distance calibration in the file (time-based survey); trace spacing set from import options or GPS.".into());
        }
        out.push(p);
    }
    Ok(out)
}

fn parse_mhz(s: &str) -> Option<f32> {
    let lower = s.to_ascii_lowercase();
    if let Some(i) = lower.find("mhz") {
        let num: String = lower[..i].trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
        return num.parse().ok();
    }
    if let Some(i) = lower.find("ghz") {
        let num: String = lower[..i].trim_end().chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<Vec<_>>().into_iter().rev().collect();
        return num.parse::<f32>().ok().map(|g| g * 1000.0);
    }
    None
}

// ------------------------------------------------------------------ MALA / ImpulseRadar (key: value headers)

fn read_kv(path: &Path) -> Result<HashMap<String, String>, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&raw);
    let mut m = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once(':').or_else(|| line.split_once('=')) {
            m.insert(k.trim().to_ascii_uppercase(), v.trim().to_string());
        }
    }
    Ok(m)
}

fn kv_f(m: &HashMap<String, String>, keys: &[&str]) -> Option<f32> {
    for k in keys {
        if let Some(v) = m.get(*k) {
            if let Ok(x) = v.split_whitespace().next().unwrap_or("").parse::<f32>() {
                return Some(x);
            }
        }
    }
    None
}

fn read_binary_traces(data: &[u8], ns: usize, bytes: usize) -> Section {
    let ntr = data.len() / (ns * bytes);
    let mut sec = Section::new(ns, ntr, 1.0);
    for t in 0..ntr {
        let tr = sec.trace_mut(t);
        for s in 0..ns {
            let o = (t * ns + s) * bytes;
            tr[s] = if bytes == 2 { i16le(data, o) as f32 } else { i32le(data, o) as f32 };
        }
    }
    sec
}

pub fn read_mala(path: &Path) -> Result<Profile, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let data_path = if ext == "rad" { sibling(path, &["rd3", "RD3", "rd7", "RD7"]).ok_or("no .rd3/.rd7 data file next to the .rad header")? } else { path.to_path_buf() };
    let rad = sibling(&data_path, &["rad", "RAD"]).ok_or("the .rad header file is missing")?;
    let h = read_kv(&rad)?;
    let ns = kv_f(&h, &["SAMPLES"]).ok_or("SAMPLES missing in .rad")? as usize;
    let fs = kv_f(&h, &["FREQUENCY"]).unwrap_or(1000.0); // sampling frequency, MHz
    let dx = kv_f(&h, &["DISTANCE INTERVAL"]).unwrap_or(0.0);
    let ant = h.get("ANTENNAS").cloned().unwrap_or_default();
    let de = data_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let bytes = if de == "rd7" { 4 } else { 2 };
    let data = std::fs::read(&data_path).map_err(|e| e.to_string())?;
    if ns == 0 {
        return Err("SAMPLES is zero".into());
    }
    let mut sec = read_binary_traces(&data, ns, bytes);
    sec.dt = 1000.0 / fs;
    let mut p = Profile::new(stem_name(&data_path), data_path.clone(), if bytes == 2 { "MALA RD3" } else { "MALA RD7" }, sec, dx);
    p.freq_mhz = parse_mhz(&ant).unwrap_or(0.0);
    p.header = vec![
        ("samples".into(), ns.to_string()),
        ("sampling frequency (MHz)".into(), format!("{fs:.3}")),
        ("time window (ns)".into(), h.get("TIMEWINDOW").cloned().unwrap_or_default()),
        ("distance interval (m)".into(), format!("{dx}")),
        ("antenna".into(), ant),
        ("traces".into(), p.raw.ntr.to_string()),
    ];
    Ok(p)
}

pub fn read_impulse(path: &Path) -> Result<Profile, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let data_path = if ext == "iprh" { sibling(path, &["iprb", "IPRB"]).ok_or("no .iprb data file next to the .iprh header")? } else { path.to_path_buf() };
    let hdr = sibling(&data_path, &["iprh", "IPRH"]).ok_or("the .iprh header file is missing")?;
    let h = read_kv(&hdr)?;
    let ns = kv_f(&h, &["SAMPLES"]).ok_or("SAMPLES missing in .iprh")? as usize;
    let fs = kv_f(&h, &["FREQUENCY"]).unwrap_or(1000.0);
    let dx = kv_f(&h, &["USER DISTANCE INTERVAL", "DISTANCE INTERVAL"]).unwrap_or(0.0);
    let bits = kv_f(&h, &["DATA VERSION"]).unwrap_or(16.0) as usize;
    let bytes = if bits >= 32 { 4 } else { 2 };
    let data = std::fs::read(&data_path).map_err(|e| e.to_string())?;
    if ns == 0 {
        return Err("SAMPLES is zero".into());
    }
    let mut sec = read_binary_traces(&data, ns, bytes);
    sec.dt = 1000.0 / fs;
    let mut p = Profile::new(stem_name(&data_path), data_path.clone(), "ImpulseRadar IPRB", sec, dx);
    let ant = h.get("ANTENNA").cloned().unwrap_or_default();
    p.freq_mhz = parse_mhz(&ant).or_else(|| ant.trim().parse().ok()).unwrap_or(0.0);
    p.header = vec![
        ("samples".into(), ns.to_string()),
        ("data bits".into(), bits.to_string()),
        ("sampling frequency (MHz)".into(), format!("{fs:.3}")),
        ("time window (ns)".into(), h.get("TIMEWINDOW").cloned().unwrap_or_default()),
        ("distance interval (m)".into(), format!("{dx}")),
        ("antenna".into(), ant),
        ("zero level".into(), h.get("ZERO LEVEL").cloned().unwrap_or_default()),
    ];
    Ok(p)
}

// ------------------------------------------------------------------ Sensors & Software DT1

pub fn read_dt1(path: &Path) -> Result<Profile, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let data_path = if ext == "hd" { sibling(path, &["dt1", "DT1"]).ok_or("no .dt1 next to the .hd header")? } else { path.to_path_buf() };
    let hd = sibling(&data_path, &["hd", "HD"]).ok_or("the .hd header file is missing")?;
    let h = read_kv(&hd)?;
    let ns = kv_f(&h, &["NUMBER OF PTS/TRC"]).ok_or("NUMBER OF PTS/TRC missing in .hd")? as usize;
    let window = kv_f(&h, &["TOTAL TIME WINDOW"]).unwrap_or(ns as f32 * 0.1);
    let dx = kv_f(&h, &["STEP SIZE USED"]).unwrap_or(0.0);
    let freq = kv_f(&h, &["NOMINAL FREQUENCY"]).unwrap_or(0.0);
    let tz = kv_f(&h, &["TIMEZERO AT POINT"]).unwrap_or(0.0);
    let data = std::fs::read(&data_path).map_err(|e| e.to_string())?;
    let tbytes = 128 + ns * 2;
    let ntr = data.len() / tbytes;
    let mut sec = Section::new(ns, ntr, if ns > 1 { window / (ns as f32 - 1.0) } else { 0.1 });
    for t in 0..ntr {
        let base = t * tbytes + 128;
        let tr = sec.trace_mut(t);
        for s in 0..ns {
            tr[s] = i16le(&data, base + s * 2) as f32;
        }
    }
    sec.t0 = -tz * sec.dt;
    let mut p = Profile::new(stem_name(&data_path), data_path.clone(), "Sensors & Software DT1", sec, dx);
    p.freq_mhz = freq;
    p.header = vec![
        ("samples".into(), ns.to_string()),
        ("time window (ns)".into(), format!("{window}")),
        ("step size (m)".into(), format!("{dx}")),
        ("nominal frequency (MHz)".into(), format!("{freq}")),
        ("time zero at point".into(), format!("{tz}")),
    ];
    Ok(p)
}

// ------------------------------------------------------------------ SEG-Y

fn be16(b: &[u8], o: usize, le: bool) -> i16 {
    if le {
        i16::from_le_bytes([b[o], b[o + 1]])
    } else {
        i16::from_be_bytes([b[o], b[o + 1]])
    }
}
fn be32(b: &[u8], o: usize, le: bool) -> i32 {
    let a = [b[o], b[o + 1], b[o + 2], b[o + 3]];
    if le {
        i32::from_le_bytes(a)
    } else {
        i32::from_be_bytes(a)
    }
}
fn ibm_to_f32(u: u32) -> f32 {
    if u & 0x7fff_ffff == 0 {
        return 0.0;
    }
    let sign = if u >> 31 == 1 { -1.0f64 } else { 1.0 };
    let exp = ((u >> 24) & 0x7f) as i32 - 64;
    let frac = (u & 0x00ff_ffff) as f64 / 16777216.0;
    (sign * frac * 16f64.powi(exp)) as f32
}

fn looks_like_segy(b: &[u8]) -> bool {
    let f = |le| be16(b, 3224, le);
    (1..=16).contains(&f(false)) || (1..=16).contains(&f(true))
}

pub fn read_segy(path: &Path, opt: &ImportOptions) -> Result<Profile, String> {
    let b = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if b.len() < 3600 {
        return Err("file too small for SEG-Y".into());
    }
    let fmt_be = be16(&b, 3224, false);
    let le = !(1..=16).contains(&fmt_be) && (1..=16).contains(&be16(&b, 3224, true));
    let fmt = be16(&b, 3224, le);
    let mut ns = be16(&b, 3220, le) as u16 as usize;
    let dt_raw = be16(&b, 3216, le) as u16 as f32;
    let ext_headers = be16(&b, 3504, le).max(0) as usize;
    let first = 3600 + ext_headers * 3200;
    if ns == 0 && b.len() > first + 240 {
        ns = be16(&b, first + 114, le) as u16 as usize;
    }
    let bps = match fmt {
        1 | 2 | 4 | 5 => 4,
        3 => 2,
        8 => 1,
        _ => return Err(format!("SEG-Y sample format {fmt} is not supported")),
    };
    if ns == 0 {
        return Err("SEG-Y file does not say how many samples a trace has".into());
    }
    let tb = 240 + ns * bps;
    let ntr = (b.len() - first) / tb;
    if ntr == 0 {
        return Err("SEG-Y file holds no traces".into());
    }
    let mut dt_val = dt_raw;
    if dt_val <= 0.0 {
        dt_val = be16(&b, first + 116, le) as u16 as f32;
    }
    let dt = match opt.segy_time_unit {
        SegyTimeUnit::Picoseconds => dt_val / 1000.0,
        SegyTimeUnit::Nanoseconds => dt_val,
        SegyTimeUnit::Microseconds => dt_val * 1000.0,
    };
    let mut sec = Section::new(ns, ntr, if dt > 0.0 { dt } else { 0.1 });
    let mut fixes = Vec::new();
    let mut coord_units = 1;
    for t in 0..ntr {
        let th = first + t * tb;
        let tr = sec.trace_mut(t);
        for s in 0..ns {
            let o = th + 240 + s * bps;
            tr[s] = match fmt {
                1 => {
                    let u = be32(&b, o, le) as u32;
                    ibm_to_f32(u)
                }
                5 => {
                    let u = be32(&b, o, le) as u32;
                    if opt.segy_force_ibm {
                        ibm_to_f32(u)
                    } else {
                        f32::from_bits(u)
                    }
                }
                2 | 4 => be32(&b, o, le) as f32,
                3 => be16(&b, o, le) as f32,
                _ => b[o] as i8 as f32,
            };
            if !tr[s].is_finite() {
                tr[s] = 0.0;
            }
        }
        let scal = be16(&b, th + 70, le) as f64;
        let sc = if scal > 0.0 { scal } else if scal < 0.0 { -1.0 / scal } else { 1.0 };
        let escal = be16(&b, th + 68, le) as f64;
        let esc = if escal > 0.0 { escal } else if escal < 0.0 { -1.0 / escal } else { 1.0 };
        let mut x = be32(&b, th + 72, le) as f64 * sc;
        let mut y = be32(&b, th + 76, le) as f64 * sc;
        if x == 0.0 && y == 0.0 {
            x = be32(&b, th + 80, le) as f64 * sc;
            y = be32(&b, th + 84, le) as f64 * sc;
        }
        let z = be32(&b, th + 40, le) as f64 * esc;
        coord_units = be16(&b, th + 88, le);
        if x != 0.0 || y != 0.0 {
            fixes.push(GpsFix { trace: t as f64, x, y, z });
        }
    }
    let mut p = Profile::new(stem_name(path), path.to_path_buf(), "SEG-Y", sec, 0.0);
    if fixes.len() >= 2 {
        let geographic = coord_units == 2 || coord_units == 3 || fixes.iter().all(|f| f.x.abs() <= 180.0 && f.y.abs() <= 90.0);
        if coord_units == 2 {
            for f in fixes.iter_mut() {
                f.x /= 3600.0;
                f.y /= 3600.0;
            }
        }
        let crs = if geographic { CrsSpec::Epsg(4326) } else { opt.segy_coord_crs.clone() };
        // Spacing from coordinates when projected.
        if !geographic {
            let a = &fixes[0];
            let z = &fixes[fixes.len() - 1];
            let d = ((z.x - a.x).powi(2) + (z.y - a.y).powi(2)).sqrt();
            if d > 0.0 {
                p.dx = (d / (z.trace - a.trace).max(1.0)) as f32;
            }
        }
        p.embedded_gps = Some(GpsTrack { source: "SEG-Y trace headers".into(), crs, fixes });
    }
    p.header = vec![
        ("byte order".into(), if le { "little endian".into() } else { "big endian".into() }),
        ("sample format".into(), fmt.to_string()),
        ("samples".into(), ns.to_string()),
        ("sample interval (raw)".into(), format!("{dt_raw}")),
        ("sample interval (ns)".into(), format!("{:.4}", p.raw.dt)),
        ("traces".into(), ntr.to_string()),
        ("coordinates in headers".into(), p.embedded_gps.as_ref().map(|g| g.fixes.len()).unwrap_or(0).to_string()),
    ];
    Ok(p)
}

// ------------------------------------------------------------------ REFLEXW .??R / .??T

pub fn read_reflexw(path: &Path) -> Result<Profile, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();
    let base = &ext[..2];
    let up = ext.chars().last().map(|c| c.is_ascii_uppercase()).unwrap_or(false);
    let (r, t) = if up { (format!("{base}R"), format!("{base}T")) } else { (format!("{base}r"), format!("{base}t")) };
    let hp = path.with_extension(&r);
    let dp = path.with_extension(&t);
    let h = std::fs::read(&hp).map_err(|e| format!("header {}: {e}", hp.display()))?;
    let d = std::fs::read(&dp).map_err(|e| format!("data {}: {e}", dp.display()))?;
    if h.len() < 0x220 {
        return Err("REFLEXW header too small".into());
    }
    let ns = u16le(&h, 0x1a4) as usize;
    let mut ntr = u32le(&h, 0x1c4) as usize;
    if ntr == 0 {
        ntr = u16le(&h, 0x1a6) as usize;
    }
    let dx = f32le(&h, 0x1cc);
    let dt = f32le(&h, 0x1d0);
    if ns == 0 || ntr == 0 {
        return Err("REFLEXW header does not give samples/traces".into());
    }
    let per = d.len() / ntr;
    let bytes = if per >= ns * 4 + 156 && (per - 156) / ns == 4 { 4 } else { 2 };
    let hdr = per.saturating_sub(ns * bytes);
    let mut sec = Section::new(ns, ntr, if dt > 0.0 { dt } else { 0.1 });
    for tr in 0..ntr {
        let base = tr * per + hdr;
        let out = sec.trace_mut(tr);
        for s in 0..ns {
            let o = base + s * bytes;
            if o + bytes > d.len() {
                break;
            }
            out[s] = if bytes == 2 { i16le(&d, o) as f32 } else { f32le(&d, o) };
        }
    }
    let mut p = Profile::new(stem_name(path), dp.clone(), "REFLEXW", sec, dx);
    p.header = vec![
        ("samples".into(), ns.to_string()),
        ("traces".into(), ntr.to_string()),
        ("trace increment".into(), format!("{dx}")),
        ("time increment (ns)".into(), format!("{dt}")),
        ("bytes per sample".into(), bytes.to_string()),
        ("trace header bytes".into(), hdr.to_string()),
    ];
    Ok(p)
}
