//! GeoPackage export (OGC GeoPackage 1.2): profiles, point markers, layer picks and trace points.

use crate::crs::CrsSpec;
use rusqlite::{params, Connection};
use std::path::Path;

pub struct ExportOptions {
    pub profiles: bool,
    pub markers: bool,
    pub layers: bool,
    pub trace_points: bool,
    pub trace_step: usize,
}

pub struct ProfileRow {
    pub name: String,
    pub file: String,
    pub format: String,
    pub traces: usize,
    pub samples: usize,
    pub dt_ns: f64,
    pub window_ns: f64,
    pub length_m: f64,
    pub freq_mhz: f64,
    pub velocity: f64,
    pub processing: String,
    pub coords: Vec<[f64; 3]>,
    pub dist: Vec<f64>,
}

pub struct MarkerRow {
    pub id: u32,
    pub profile: String,
    pub type_name: String,
    pub trace: f64,
    pub distance_m: f64,
    pub time_ns: f64,
    pub depth_m: f64,
    pub amplitude: f64,
    pub velocity: Option<f64>,
    pub comment: String,
    pub color: String,
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

pub struct LineRow {
    pub id: u32,
    pub profile: String,
    pub type_name: String,
    pub comment: String,
    pub color: String,
    /// (x, y, z, depth, time, distance)
    pub pts: Vec<[f64; 6]>,
}

fn geom_blob(srs: i32, wkb_type: u32, pts: &[[f64; 3]], z: bool) -> Vec<u8> {
    let mut b = Vec::with_capacity(64 + pts.len() * 24);
    b.extend_from_slice(b"GP");
    b.push(0); // version
    b.push(0b0000_0011); // little endian, xy envelope
    b.extend_from_slice(&srs.to_le_bytes());
    let (mut minx, mut maxx, mut miny, mut maxy) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
    for p in pts {
        minx = minx.min(p[0]);
        maxx = maxx.max(p[0]);
        miny = miny.min(p[1]);
        maxy = maxy.max(p[1]);
    }
    for v in [minx, maxx, miny, maxy] {
        b.extend_from_slice(&v.to_le_bytes());
    }
    b.push(1);
    let t = if z { wkb_type + 1000 } else { wkb_type };
    b.extend_from_slice(&t.to_le_bytes());
    if wkb_type == 2 {
        b.extend_from_slice(&(pts.len() as u32).to_le_bytes());
    }
    for p in pts {
        b.extend_from_slice(&p[0].to_le_bytes());
        b.extend_from_slice(&p[1].to_le_bytes());
        if z {
            b.extend_from_slice(&p[2].to_le_bytes());
        }
    }
    b
}

fn srs_id(crs: &CrsSpec) -> i32 {
    match crs {
        CrsSpec::Epsg(c) => *c as i32,
        CrsSpec::Proj(_) => 990001,
    }
}

pub fn export(path: &Path, crs: &CrsSpec, opt: &ExportOptions, profiles: &[ProfileRow], markers: &[MarkerRow], lines: &[LineRow]) -> Result<String, String> {
    if path.exists() {
        std::fs::remove_file(path).map_err(|e| format!("cannot replace {}: {e}", path.display()))?;
    }
    let mut c = Connection::open(path).map_err(|e| e.to_string())?;
    let e = |x: rusqlite::Error| x.to_string();
    c.execute_batch(
        "PRAGMA application_id = 1196444487; PRAGMA user_version = 10200;
        CREATE TABLE gpkg_spatial_ref_sys (srs_name TEXT NOT NULL, srs_id INTEGER NOT NULL PRIMARY KEY, organization TEXT NOT NULL, organization_coordsys_id INTEGER NOT NULL, definition TEXT NOT NULL, description TEXT);
        CREATE TABLE gpkg_contents (table_name TEXT NOT NULL PRIMARY KEY, data_type TEXT NOT NULL, identifier TEXT UNIQUE, description TEXT DEFAULT '', last_change DATETIME NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ','now')), min_x DOUBLE, min_y DOUBLE, max_x DOUBLE, max_y DOUBLE, srs_id INTEGER, CONSTRAINT fk_gc_r_srs_id FOREIGN KEY (srs_id) REFERENCES gpkg_spatial_ref_sys(srs_id));
        CREATE TABLE gpkg_geometry_columns (table_name TEXT NOT NULL, column_name TEXT NOT NULL, geometry_type_name TEXT NOT NULL, srs_id INTEGER NOT NULL, z TINYINT NOT NULL, m TINYINT NOT NULL, CONSTRAINT pk_geom_cols PRIMARY KEY (table_name, column_name));
        INSERT INTO gpkg_spatial_ref_sys VALUES ('Undefined cartesian SRS', -1, 'NONE', -1, 'undefined', 'undefined cartesian coordinate reference system');
        INSERT INTO gpkg_spatial_ref_sys VALUES ('Undefined geographic SRS', 0, 'NONE', 0, 'undefined', 'undefined geographic coordinate reference system');
        INSERT INTO gpkg_spatial_ref_sys VALUES ('WGS 84 geodetic', 4326, 'EPSG', 4326, 'GEOGCS[\"WGS 84\",DATUM[\"WGS_1984\",SPHEROID[\"WGS 84\",6378137,298.257223563,AUTHORITY[\"EPSG\",\"7030\"]],AUTHORITY[\"EPSG\",\"6326\"]],PRIMEM[\"Greenwich\",0,AUTHORITY[\"EPSG\",\"8901\"]],UNIT[\"degree\",0.0174532925199433,AUTHORITY[\"EPSG\",\"9122\"]],AUTHORITY[\"EPSG\",\"4326\"]]', 'longitude/latitude coordinates in decimal degrees on the WGS 84 spheroid');",
    )
    .map_err(e)?;
    let srs = srs_id(crs);
    if srs != 4326 {
        let (org, code, def) = match crs {
            CrsSpec::Epsg(cd) => ("EPSG", *cd as i32, crs.wkt().unwrap_or_else(|| "undefined".into())),
            CrsSpec::Proj(p) => ("NONE", srs, p.clone()),
        };
        c.execute("INSERT INTO gpkg_spatial_ref_sys VALUES (?1, ?2, ?3, ?4, ?5, ?6)", params![crs.label(), srs, org, code, def, crs.label()]).map_err(e)?;
    }
    let tx = c.transaction().map_err(e)?;
    let mut summary = Vec::new();
    let register = |tx: &rusqlite::Transaction, table: &str, gtype: &str, desc: &str, ext: [f64; 4]| -> rusqlite::Result<()> {
        tx.execute(
            "INSERT INTO gpkg_contents (table_name, data_type, identifier, description, min_x, min_y, max_x, max_y, srs_id) VALUES (?1, 'features', ?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![table, desc, ext[0], ext[1], ext[2], ext[3], srs],
        )?;
        tx.execute("INSERT INTO gpkg_geometry_columns VALUES (?1, 'geom', ?2, ?3, 1, 0)", params![table, gtype, srs])?;
        Ok(())
    };
    let mut extent = |pts: &[[f64; 3]], ext: &mut [f64; 4]| {
        for p in pts {
            ext[0] = ext[0].min(p[0]);
            ext[1] = ext[1].min(p[1]);
            ext[2] = ext[2].max(p[0]);
            ext[3] = ext[3].max(p[1]);
        }
    };
    let empty = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];

    if opt.profiles {
        tx.execute_batch("CREATE TABLE profiles (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom LINESTRING, name TEXT, file TEXT, format TEXT, traces INTEGER, samples INTEGER, dt_ns REAL, time_window_ns REAL, length_m REAL, frequency_mhz REAL, velocity_m_ns REAL, processing TEXT)").map_err(e)?;
        let mut ext = empty;
        let mut n = 0;
        for p in profiles.iter().filter(|p| p.coords.len() >= 2) {
            extent(&p.coords, &mut ext);
            tx.execute(
                "INSERT INTO profiles (geom, name, file, format, traces, samples, dt_ns, time_window_ns, length_m, frequency_mhz, velocity_m_ns, processing) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![geom_blob(srs, 2, &p.coords, true), p.name, p.file, p.format, p.traces as i64, p.samples as i64, p.dt_ns, p.window_ns, p.length_m, p.freq_mhz, p.velocity, p.processing],
            )
            .map_err(e)?;
            n += 1;
        }
        register(&tx, "profiles", "LINESTRING", "GPR profile tracks", ext).map_err(e)?;
        summary.push(format!("{n} profiles"));
    }
    if opt.trace_points {
        tx.execute_batch("CREATE TABLE trace_points (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom POINT, profile TEXT, trace INTEGER, distance_m REAL)").map_err(e)?;
        let mut ext = empty;
        let mut n = 0;
        for p in profiles.iter().filter(|p| !p.coords.is_empty()) {
            for (i, c) in p.coords.iter().enumerate().step_by(opt.trace_step.max(1)) {
                extent(&[*c], &mut ext);
                tx.execute("INSERT INTO trace_points (geom, profile, trace, distance_m) VALUES (?1,?2,?3,?4)", params![geom_blob(srs, 1, &[*c], true), p.name, (i + 1) as i64, p.dist.get(i).copied().unwrap_or(0.0)]).map_err(e)?;
                n += 1;
            }
        }
        register(&tx, "trace_points", "POINT", "GPR trace positions", ext).map_err(e)?;
        summary.push(format!("{n} trace points"));
    }
    if opt.markers {
        tx.execute_batch("CREATE TABLE markers (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom POINT, marker_id INTEGER, profile TEXT, type TEXT, trace REAL, distance_m REAL, time_ns REAL, depth_m REAL, elevation_m REAL, amplitude REAL, velocity_m_ns REAL, comment TEXT, color TEXT, x REAL, y REAL)").map_err(e)?;
        let mut ext = empty;
        let mut n = 0;
        for m in markers {
            let p = [m.x, m.y, m.z];
            let has = m.x.is_finite() && m.y.is_finite();
            if has {
                extent(&[p], &mut ext);
            }
            let g: Option<Vec<u8>> = if has { Some(geom_blob(srs, 1, &[p], true)) } else { None };
            tx.execute(
                "INSERT INTO markers (geom, marker_id, profile, type, trace, distance_m, time_ns, depth_m, elevation_m, amplitude, velocity_m_ns, comment, color, x, y) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)",
                params![g, m.id, m.profile, m.type_name, m.trace + 1.0, m.distance_m, m.time_ns, m.depth_m, if has { Some(m.z) } else { None }, m.amplitude, m.velocity, m.comment, m.color, if has { Some(m.x) } else { None }, if has { Some(m.y) } else { None }],
            )
            .map_err(e)?;
            n += 1;
        }
        register(&tx, "markers", "POINT", "GPR point markers", ext).map_err(e)?;
        summary.push(format!("{n} markers"));
    }
    if opt.layers {
        tx.execute_batch("CREATE TABLE layer_picks (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom LINESTRING, pick_id INTEGER, profile TEXT, type TEXT, points INTEGER, mean_depth_m REAL, min_depth_m REAL, max_depth_m REAL, comment TEXT, color TEXT)").map_err(e)?;
        tx.execute_batch("CREATE TABLE layer_pick_vertices (fid INTEGER PRIMARY KEY AUTOINCREMENT, geom POINT, pick_id INTEGER, profile TEXT, type TEXT, distance_m REAL, time_ns REAL, depth_m REAL, elevation_m REAL)").map_err(e)?;
        let mut ext = empty;
        let mut n = 0;
        for l in lines.iter().filter(|l| l.pts.len() >= 2 && l.pts.iter().all(|p| p[0].is_finite())) {
            let pts: Vec<[f64; 3]> = l.pts.iter().map(|p| [p[0], p[1], p[2]]).collect();
            extent(&pts, &mut ext);
            let depths: Vec<f64> = l.pts.iter().map(|p| p[3]).collect();
            let mean = depths.iter().sum::<f64>() / depths.len() as f64;
            let mn = depths.iter().cloned().fold(f64::MAX, f64::min);
            let mx = depths.iter().cloned().fold(f64::MIN, f64::max);
            tx.execute(
                "INSERT INTO layer_picks (geom, pick_id, profile, type, points, mean_depth_m, min_depth_m, max_depth_m, comment, color) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params![geom_blob(srs, 2, &pts, true), l.id, l.profile, l.type_name, l.pts.len() as i64, mean, mn, mx, l.comment, l.color],
            )
            .map_err(e)?;
            for p in &l.pts {
                tx.execute(
                    "INSERT INTO layer_pick_vertices (geom, pick_id, profile, type, distance_m, time_ns, depth_m, elevation_m) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![geom_blob(srs, 1, &[[p[0], p[1], p[2]]], true), l.id, l.profile, l.type_name, p[5], p[4], p[3], p[2]],
                )
                .map_err(e)?;
            }
            n += 1;
        }
        register(&tx, "layer_picks", "LINESTRING", "GPR layer picks (elevation = surface - depth)", ext).map_err(e)?;
        register(&tx, "layer_pick_vertices", "POINT", "GPR layer pick vertices", ext).map_err(e)?;
        summary.push(format!("{n} layer picks"));
    }
    tx.commit().map_err(e)?;
    Ok(summary.join(", "))
}
