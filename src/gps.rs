//! Readers for GPS files that go with GPR profiles.

use crate::crs::CrsSpec;
use crate::model::{GpsFix, GpsTrack};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum GpsFormat {
    Auto,
    /// MALA / ImpulseRadar .cor: trace date time lat N lon E alt M quality
    Cor,
    /// GSSI .dzg: $GSSIS,trace,... followed by NMEA sentences
    Dzg,
    /// Sensors & Software .gps: "Trace #n at position p" followed by NMEA
    PulseEkko,
    /// Plain NMEA, fixes spread evenly over the traces
    Nmea,
    /// Text table with trace number and coordinates
    Csv,
}

impl GpsFormat {
    pub const ALL: [GpsFormat; 6] = [GpsFormat::Auto, GpsFormat::Cor, GpsFormat::Dzg, GpsFormat::PulseEkko, GpsFormat::Nmea, GpsFormat::Csv];
    pub fn name(&self) -> &'static str {
        match self {
            GpsFormat::Auto => "Detect from file",
            GpsFormat::Cor => "MALA / ImpulseRadar (.cor)",
            GpsFormat::Dzg => "GSSI (.dzg)",
            GpsFormat::PulseEkko => "Sensors & Software (.gps)",
            GpsFormat::Nmea => "NMEA log (spread over traces)",
            GpsFormat::Csv => "Table: trace, x/lon, y/lat, z",
        }
    }
}

/// Look for a GPS file next to the data file.
pub fn find_candidate(data: &Path) -> Option<PathBuf> {
    let exts = ["cor", "COR", "dzg", "DZG", "gps", "GPS", "csv", "CSV", "txt", "TXT", "nmea", "NMEA", "pos", "POS"];
    for e in exts {
        let p = data.with_extension(e);
        if p.exists() {
            return Some(p);
        }
    }
    let files = list_gps_files(&search_dirs(data, None));
    best_match(data, &files).map(|b| b.0)
}

pub fn read(path: &Path, fmt: GpsFormat, ntr: usize, table_crs: &CrsSpec) -> Result<GpsTrack, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let fmt = if fmt == GpsFormat::Auto { detect(path, &text) } else { fmt };
    let (fixes, crs, clock_range) = match fmt {
        GpsFormat::Cor => { let (fixes, clock_range) = read_cor(&text); (fixes, CrsSpec::Epsg(4326), clock_range) },
        GpsFormat::Dzg => (read_tagged_nmea(&text, "$GSSIS"), CrsSpec::Epsg(4326), None),
        GpsFormat::PulseEkko => (read_pulseekko(&text), CrsSpec::Epsg(4326), None),
        GpsFormat::Nmea => (spread(read_nmea_all(&text), ntr), CrsSpec::Epsg(4326), None),
        GpsFormat::Csv | GpsFormat::Auto => { let (fixes, crs) = read_table(&text, table_crs); (fixes, crs, None) },
    };
    if fixes.is_empty() {
        return Err(format!("no positions found in {} (read as {})", path.display(), fmt.name()));
    }
    Ok(GpsTrack { source: format!("{} [{}]", path.file_name().and_then(|n| n.to_str()).unwrap_or(""), fmt.name()), crs, fixes, clock_range })
}

fn detect(path: &Path, text: &str) -> GpsFormat {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if text.contains("$GSSIS") {
        return GpsFormat::Dzg;
    }
    if text.contains("Trace #") {
        return GpsFormat::PulseEkko;
    }
    if ext == "cor" {
        return GpsFormat::Cor;
    }
    if text.contains("$GPGGA") || text.contains("$GNGGA") || text.contains("$GPRMC") {
        return GpsFormat::Nmea;
    }
    GpsFormat::Csv
}

fn cor_clock(token: &str) -> Option<String> {
    let fields: Vec<_> = token.split([':', '.']).collect();
    if !(3..=4).contains(&fields.len()) || (fields.len() == 4 && !fields[3].chars().all(|c| c.is_ascii_digit())) { return None; }
    let (h, m, s) = (fields[0].parse::<u8>().ok()?, fields[1].parse::<u8>().ok()?, fields[2].parse::<u8>().ok()?);
    (h < 24 && m < 60 && s < 60).then(|| format!("{h:02}.{m:02}.{s:02}"))
}

fn read_cor(text: &str) -> (Vec<GpsFix>, Option<(String, String)>) {
    let mut out = Vec::new();
    let (mut start, mut end) = (None, None);
    for line in text.lines() {
        let tok: Vec<&str> = line.split(|c: char| c == '\t' || c == ',' || c == ';' || c == ' ').filter(|s| !s.is_empty()).collect();
        if tok.len() < 6 {
            continue;
        }
        let Ok(trace) = tok[0].parse::<f64>() else { continue };
        // Find "N"/"S" and "E"/"W" markers; the number before each is the coordinate.
        let mut lat = None;
        let mut lon = None;
        let mut alt = None;
        for i in 1..tok.len() {
            match tok[i] {
                "N" | "S" if lat.is_none() => {
                    if let Ok(v) = tok[i - 1].parse::<f64>() {
                        lat = Some(if tok[i] == "S" { -dm_or_deg(v) } else { dm_or_deg(v) });
                    }
                }
                "E" | "W" if lon.is_none() => {
                    if let Ok(v) = tok[i - 1].parse::<f64>() {
                        lon = Some(if tok[i] == "W" { -dm_or_deg(v) } else { dm_or_deg(v) });
                    }
                    if let Some(a) = tok.get(i + 1).and_then(|s| s.parse::<f64>().ok()) {
                        alt = Some(a);
                    }
                }
                _ => {}
            }
        }
        if let (Some(la), Some(lo)) = (lat, lon) {
            // .cor trace numbers start at 1.
            out.push(GpsFix { trace: (trace - 1.0).max(0.0), x: lo, y: la, z: alt.unwrap_or(0.0), has_elevation: alt.is_some() });
            if let Some(time) = tok.get(2).and_then(|s| cor_clock(s)) {
                if start.is_none() { start = Some(time.clone()); }
                end = Some(time);
            }
        }
    }
    (out, start.zip(end))
}

pub fn cor_clock_range(track: &GpsTrack, data_path: &Path) -> Option<(String, String)> {
    if let Some(range) = &track.clock_range { return Some(range.clone()); }
    let name = track.source.split(" [").next()?;
    if !Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("cor")) { return None; }
    for path in search_dirs(data_path, None).into_iter().map(|dir| dir.join(name)) {
        let Ok(bytes) = std::fs::read(path) else { continue };
        let (fixes, range) = read_cor(&String::from_utf8_lossy(&bytes));
        if fixes.len() == track.fixes.len() && fixes.iter().zip(&track.fixes).all(|(a, b)| (a.trace, a.x, a.y, a.z) == (b.trace, b.x, b.y, b.z)) {
            return range;
        }
    }
    None
}

#[cfg(test)]
mod altitude_investigation {
    use super::*;
    use crate::model::{Profile, Section};

    #[test]
    fn cor_height_through_reprojection() {
        let lines = "1 2026-11-20 12:00:00 46.9511 N 7.4386 E 500.000 M 4\n3 2026-11-20 12:00:02 46.9513 N 7.4388 E 501.000 M 4\n";
        let path = std::env::temp_dir().join(format!("gpr-altitude-investigation-{}.cor", std::process::id()));
        std::fs::write(&path, lines).unwrap();
        let imported = read(&path, GpsFormat::Auto, 3, &CrsSpec::Epsg(2056)).unwrap();
        let mut old_project_track = imported.clone();
        old_project_track.clock_range = None;
        assert_eq!(cor_clock_range(&old_project_track, &path.with_extension("iprb")), Some(("12.00.00".into(), "12.00.02".into())));
        std::fs::remove_file(path).unwrap();
        assert_eq!(imported.crs, CrsSpec::Epsg(4326));
        assert_eq!(imported.clock_range, Some(("12.00.00".into(), "12.00.02".into())));
        let (fixes, range) = read_cor(lines);
        assert_eq!(range, Some(("12.00.00".into(), "12.00.02".into())));
        assert_eq!(imported.fixes, fixes);
        assert_eq!((fixes[0].trace, fixes[0].z, fixes[1].trace, fixes[1].z), (0.0, 500.0, 2.0, 501.0));
        for target in [CrsSpec::Epsg(4326), CrsSpec::Epsg(2056), CrsSpec::Epsg(32632)] {
            for f in &fixes {
                let mut xyz = [[f.x, f.y, f.z]];
                crate::crs::transform_spec(&CrsSpec::Epsg(4326), &target, &mut xyz).unwrap();
                let mut zero = [[f.x, f.y, 0.0]];
                crate::crs::transform_spec(&CrsSpec::Epsg(4326), &target, &mut zero).unwrap();
                println!("{:?} trace={} cor_z={:.3} transformed={:?} dz={:.6} xy_at_zero={:?}", target, f.trace, f.z, xyz[0], xyz[0][2] - f.z, &zero[0][..2]);
            }
            let mut p = Profile::new("synthetic".into(), "synthetic.cor".into(), "test", Section { ntr: 3, ns: 2, dt: 1.0, t0: 0.0, depth_axis: false, depth_datum_m: None, data: vec![0.0; 6] }, 0.0);
            p.gps = Some(imported.clone());
            p.update_coords(&target);
            println!("{:?} per_trace_z={:?} midpoint_coord={:?} picked_at_2m={:.6}", target, p.coords.iter().map(|c| c[2]).collect::<Vec<_>>(), p.coord_at(1.0).unwrap(), p.coord_at(1.0).unwrap()[2] - 2.0);
            assert_eq!(p.coords.iter().map(|c| c[2]).collect::<Vec<_>>(), [500.0, 500.5, 501.0]);
            if target == CrsSpec::Epsg(2056) {
                assert!((p.coords[0][0] - 2_599_997.526_851).abs() < 0.001);
                assert!((p.coords[0][1] - 1_200_001.904_897).abs() < 0.001);
                p.show_topo = true;
                let (datum, shifts) = p.topo_shifts().unwrap();
                println!("topographic_display datum={datum:.6} trace0_surface={:.6} trace0_shift_samples={:.6}", p.coords[0][2], shifts[0]);
                assert_eq!(datum, 501.0);
                assert!((shifts[0] - 20.0).abs() < 1e-5);
                let topo = p.topographic_geometry(&target, 100.0).unwrap();
                assert_eq!(topo.elevation_m, [500.0, 500.5, 501.0]);
                let out = std::env::temp_dir().join(format!("gpr-altitude-investigation-{}.sgy", std::process::id()));
                let meta = crate::segy_export::SegyMeta { name: "synthetic".into(), source: "synthetic.cor".into(), line_no: 1, coords: p.coords.clone(), dist: p.dist.clone(), crs_label: target.short(), geographic: false, freq_mhz: 400.0, velocity: 0.1, dx: 1.0 };
                crate::segy_export::write(&out, &p.proc, &meta, "RAW", &[]).unwrap();
                let bytes = std::fs::read(&out).unwrap();
                std::fs::remove_file(out).unwrap();
                let exported = i32::from_be_bytes(bytes[3600 + 40..3600 + 44].try_into().unwrap()) as f64 / 100.0;
                println!("SEG-Y trace0_elevation={exported:.3} delta_from_cor={:.3}", exported - fixes[0].z);
                assert_eq!(exported, 500.0);
            }
        }
        for (label, lon, lat) in [("Geneva", 6.1432, 46.2044), ("Zurich", 8.5417, 47.3769), ("Lugano", 8.9511, 46.0037)] {
            let mut xyz = [[lon, lat, 500.0]];
            crate::crs::transform_spec(&CrsSpec::Epsg(4326), &CrsSpec::Epsg(2056), &mut xyz).unwrap();
            println!("{label} source=[{lon},{lat},500] LV95={:?} dz={:.6}", xyz[0], xyz[0][2] - 500.0);
        }
        let json = serde_json::to_string(&imported).unwrap();
        let restored: GpsTrack = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.fixes, fixes);
        let mut p = Profile::new("synthetic".into(), "synthetic.cor".into(), "test", Section { ntr: 3, ns: 2, dt: 1.0, t0: 0.0, depth_axis: false, depth_datum_m: None, data: vec![0.0; 6] }, 0.0);
        p.gps = Some(restored);
        for target in [2056, 32632, 4326, 2056] {
            p.update_coords(&CrsSpec::Epsg(target));
            println!("reproject EPSG:{target} trace0_z={:.6} raw_z={:.6}", p.coords[0][2], p.gps.as_ref().unwrap().fixes[0].z);
            assert_eq!(p.coords[0][2], 500.0);
        }
        let mut app = crate::app::App::headless(vec![], None);
        app.remember_projects = false;
        let mut demo = crate::demo::make().remove(0);
        demo.gps = Some(imported);
        app.add_profile(demo);
        let before = app.profiles[0].coords[0][2];
        assert_eq!(before, 500.0);
        let project_path = std::env::temp_dir().join(format!("gpr-altitude-investigation-{}.gprproj", std::process::id()));
        crate::project::save(&app, &project_path).unwrap();
        let file = crate::project::parse(&project_path).unwrap();
        let loaded = crate::project::load_profiles(file, project_path.clone(), &crate::processing::Progress::new());
        let mut reopened = crate::app::App::headless(vec![], None);
        reopened.remember_projects = false;
        crate::project::apply(&mut reopened, loaded);
        std::fs::remove_file(project_path).unwrap();
        println!("project_reopen trace0_z_before={before:.6} after={:.6} raw_z={:.6}", reopened.profiles[0].coords[0][2], reopened.profiles[0].gps.as_ref().unwrap().fixes[0].z);
        assert!((reopened.profiles[0].coords[0][2] - before).abs() < 1e-9);
        assert_eq!(reopened.profiles[0].gps.as_ref().unwrap().fixes[0].z, 500.0);
        assert_eq!(reopened.profiles[0].gps.as_ref().unwrap().clock_range, Some(("12.00.00".into(), "12.00.02".into())));
    }
}

/// Values above 180 are ddmm.mmmm (NMEA style).
fn dm_or_deg(v: f64) -> f64 {
    if v.abs() > 180.0 {
        nmea_deg(v)
    } else {
        v
    }
}

fn nmea_deg(v: f64) -> f64 {
    let d = (v / 100.0).trunc();
    d + (v - d * 100.0) / 60.0
}

/// Parse a GGA (or RMC) sentence: (lon, lat, alt).
fn parse_nmea(line: &str) -> Option<(f64, f64, f64)> {
    let l = line.trim();
    let f: Vec<&str> = l.split('*').next()?.split(',').collect();
    let kind = f.first()?;
    if kind.ends_with("GGA") && f.len() > 9 {
        let lat = nmea_deg(f[2].parse().ok()?) * if f[3] == "S" { -1.0 } else { 1.0 };
        let lon = nmea_deg(f[4].parse().ok()?) * if f[5] == "W" { -1.0 } else { 1.0 };
        let q: i32 = f[6].parse().unwrap_or(1);
        if q == 0 {
            return None;
        }
        let alt = f[9].parse().ok()?;
        return Some((lon, lat, alt));
    }
    if kind.ends_with("RMC") && f.len() > 6 {
        if f[2] != "A" {
            return None;
        }
        let lat = nmea_deg(f[3].parse().ok()?) * if f[4] == "S" { -1.0 } else { 1.0 };
        let lon = nmea_deg(f[5].parse().ok()?) * if f[6] == "W" { -1.0 } else { 1.0 };
        return Some((lon, lat, 0.0));
    }
    None
}

fn read_tagged_nmea(text: &str, tag: &str) -> Vec<GpsFix> {
    let mut out = Vec::new();
    let mut cur: Option<f64> = None;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with(tag) {
            cur = l.split(',').nth(1).and_then(|s| s.trim().parse::<f64>().ok());
        } else if let Some(tr) = cur {
            if let Some((x, y, z)) = parse_nmea(l) {
                if l.contains("GGA") || !out.iter().any(|f: &GpsFix| f.trace == tr) {
                    out.retain(|f: &GpsFix| f.trace != tr);
                    out.push(GpsFix { trace: tr, x, y, z, has_elevation: l.contains("GGA") });
                }
            }
        }
    }
    out
}

fn read_pulseekko(text: &str) -> Vec<GpsFix> {
    let mut out = Vec::new();
    let mut cur: Option<f64> = None;
    for line in text.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Trace #") {
            cur = rest.split_whitespace().next().and_then(|s| s.parse::<f64>().ok()).map(|t| (t - 1.0).max(0.0));
        } else if let Some(tr) = cur {
            if let Some((x, y, z)) = parse_nmea(l) {
                if l.contains("GGA") || !out.iter().any(|f: &GpsFix| f.trace == tr) {
                    out.retain(|f: &GpsFix| f.trace != tr);
                    out.push(GpsFix { trace: tr, x, y, z, has_elevation: l.contains("GGA") });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod elevation_tests {
    use super::*;

    #[test]
    fn pulseekko_prefers_measured_gga_height_over_rmc_placeholder() {
        let text = "Trace #1\n$GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W\n$GPGGA,123519,4807.038,N,01131.000,E,1,08,0.9,545.4,M,46.9,M,,\n";
        let fixes = read_pulseekko(text);
        assert_eq!(fixes.len(), 1);
        assert!(fixes[0].has_elevation);
        assert_eq!(fixes[0].z, 545.4);
    }
}

fn read_nmea_all(text: &str) -> Vec<(f64, f64, f64)> {
    text.lines().filter(|l| l.contains("GGA")).filter_map(parse_nmea).collect()
}

fn spread(pts: Vec<(f64, f64, f64)>, ntr: usize) -> Vec<GpsFix> {
    let n = pts.len();
    if n == 0 {
        return vec![];
    }
    let span = ntr.saturating_sub(1).max(1) as f64;
    pts.into_iter()
        .enumerate()
        .map(|(i, (x, y, z))| GpsFix { trace: if n > 1 { i as f64 * span / (n - 1) as f64 } else { 0.0 }, x, y, z, has_elevation: true })
        .collect()
}

/// Text table. Header names (trace, x/east/lon, y/north/lat, z/elev/alt) are used when present;
/// otherwise columns are trace, x, y[, z]. Values that look like degrees are read as WGS 84.
fn read_table(text: &str, table_crs: &CrsSpec) -> (Vec<GpsFix>, CrsSpec) {
    let split = |l: &str| -> Vec<String> { l.split(|c: char| c == ',' || c == ';' || c == '\t' || c == ' ').filter(|s| !s.is_empty()).map(|s| s.trim().to_string()).collect() };
    let mut cols: Option<(usize, usize, usize, Option<usize>, bool)> = None; // trace, x, y, z, swapped-latlon
    let mut out = Vec::new();
    for line in text.lines() {
        let tok = split(line);
        if tok.is_empty() {
            continue;
        }
        let numeric = tok.iter().filter(|t| t.parse::<f64>().is_ok()).count();
        if cols.is_none() && numeric < tok.len().min(3) {
            // Header row.
            let find = |names: &[&str]| tok.iter().position(|t| names.iter().any(|n| t.to_ascii_lowercase().contains(n)));
            let tr = find(&["trace", "trc", "scan", "nr"]).unwrap_or(0);
            let lon = find(&["lon", "east", "x", "rechts"]);
            let lat = find(&["lat", "north", "y", "hoch"]);
            let z = find(&["elev", "alt", "height", "z", "hoehe"]);
            if let (Some(x), Some(y)) = (lon, lat) {
                cols = Some((tr, x, y, z, false));
            }
            continue;
        }
        let (tc, xc, yc, zc, _) = cols.unwrap_or((0, 1, 2, if tok.len() > 3 { Some(3) } else { None }, false));
        let get = |i: usize| tok.get(i).and_then(|s| s.parse::<f64>().ok());
        if let (Some(t), Some(x), Some(y)) = (get(tc), get(xc), get(yc)) {
            let height = zc.and_then(get);
            out.push(GpsFix { trace: t, x, y, z: height.unwrap_or(0.0), has_elevation: height.is_some() });
        }
    }
    // Trace numbers starting at 1 are moved to 0-based.
    if out.first().map(|f| f.trace >= 1.0).unwrap_or(false) && out.iter().all(|f| f.trace.fract() == 0.0) {
        out.iter_mut().for_each(|f| f.trace -= 1.0);
    }
    let geographic = !out.is_empty() && out.iter().all(|f| f.x.abs() <= 180.0 && f.y.abs() <= 90.0);
    if geographic {
        // A column order lat, lon is detected by latitude-like first column in Europe (|x| > |y| unlikely).
        (out, CrsSpec::Epsg(4326))
    } else {
        (out, table_crs.clone())
    }
}

// ------------------------------------------------------------------ automatic matching

pub const GPS_EXTS: [&str; 7] = ["cor", "dzg", "gps", "csv", "txt", "nmea", "pos"];

fn is_gps_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| GPS_EXTS.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

/// Remove a trailing channel / antenna suffix: "_0", "_1", "_A1", "_ch2", "-2", "_A001".
pub fn strip_channel(stem: &str) -> String {
    let s = stem.to_string();
    if let Some(i) = s.rfind(|c: char| c == '_' || c == '-') {
        let tail = &s[i + 1..];
        let t = tail.to_ascii_lowercase();
        let digits = t.trim_start_matches("ch").trim_start_matches('a').trim_start_matches('b');
        let has_prefix = digits.len() < t.len();
        // Pure digits: at most 2 ("_0", "_12"); with a letter prefix: "A1", "A001", "ch2".
        let short = if has_prefix { tail.len() <= 4 } else { tail.len() <= 2 };
        if short && !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && i > 0 {
            return s[..i].to_string();
        }
    }
    s
}

/// The last group of digits in a name ("..._0002_0" -> after stripping the channel, "0002").
fn last_number(s: &str) -> Option<(String, String)> {
    let b = s.as_bytes();
    let mut end = b.len();
    while end > 0 && !b[end - 1].is_ascii_digit() {
        end -= 1;
    }
    if end == 0 {
        return None;
    }
    let mut start = end;
    while start > 0 && b[start - 1].is_ascii_digit() {
        start -= 1;
    }
    Some((s[..start].to_string(), s[start..end].trim_start_matches('0').to_string()))
}

/// How well a GPS file name fits a data file name (0 = not at all).
pub fn match_score(data_stem: &str, gps_stem: &str) -> u32 {
    let d = data_stem.to_lowercase();
    let g = gps_stem.to_lowercase();
    if d == g {
        return 1000;
    }
    let d2 = strip_channel(&d);
    let g2 = strip_channel(&g);
    if d2 == g || d2 == g2 {
        return 900;
    }
    // Data name starts with the GPS name followed by a separator.
    if d.len() > g.len() && d.starts_with(&g) && matches!(d.as_bytes()[g.len()], b'_' | b'-' | b' ' | b'.') {
        return 700 + g.len().min(200) as u32;
    }
    if g.len() > d2.len() && g.starts_with(&d2) && matches!(g.as_bytes()[d2.len()], b'_' | b'-' | b' ' | b'.') {
        return 600 + d2.len().min(200) as u32;
    }
    // Same profile number with the same text before it, or at least the same number.
    if let (Some((dp, dn)), Some((gp, gn))) = (last_number(&d2), last_number(&g2)) {
        if dn == gn {
            if dp == gp {
                return 500;
            }
            let common = dp.chars().zip(gp.chars()).take_while(|(a, b)| a == b).count();
            if common >= 3 {
                return 200 + common.min(200) as u32;
            }
        }
    }
    0
}

pub fn list_gps_files(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in dirs {
        if let Ok(rd) = std::fs::read_dir(d) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_file() && is_gps_file(&p) && !out.contains(&p) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

/// Folders searched for a data file: the chosen GPS folder, then the data folder and its GPS sub-folders.
pub fn search_dirs(data: &Path, gps_folder: Option<&Path>) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(f) = gps_folder {
        v.push(f.to_path_buf());
    }
    if let Some(dir) = data.parent() {
        v.push(dir.to_path_buf());
        for sub in ["GPS", "gps", "Gps", "COR", "cor"] {
            let s = dir.join(sub);
            if s.is_dir() {
                v.push(s);
            }
        }
    }
    v
}

/// Best GPS file for a data file among `files` (with its score).
pub fn best_match(data: &Path, files: &[PathBuf]) -> Option<(PathBuf, u32)> {
    let stem = data.file_stem()?.to_str()?;
    let data_dir = data.parent();
    let mut best: Option<(PathBuf, u32)> = None;
    for f in files {
        let Some(gs) = f.file_stem().and_then(|s| s.to_str()) else { continue };
        let mut sc = match_score(stem, gs);
        if sc == 0 {
            continue;
        }
        // Prefer the data folder on equal names; prefer .cor/.dzg/.gps over tables.
        if f.parent() == data_dir {
            sc += 2;
        }
        let ext = f.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if matches!(ext.as_str(), "cor" | "dzg" | "gps") {
            sc += 5;
        }
        if best.as_ref().map(|b| sc > b.1).unwrap_or(true) {
            best = Some((f.clone(), sc));
        }
    }
    best
}
