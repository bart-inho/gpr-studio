//! GeoPackage export (OGC GeoPackage 1.2): profiles, point markers, layer picks, trace points and interpolations.

use crate::crs::CrsSpec;
use crate::interp::{Grid, Zone};
use rusqlite::{params, Connection};
use std::path::Path;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RasterValue {
    #[default]
    Depth,
    Time,
    Altitude,
}

impl RasterValue {
    pub fn label(self) -> &'static str { match self { Self::Depth => "Depth (m)", Self::Time => "Time (ns)", Self::Altitude => "Altitude (m)" } }
    fn field(self) -> &'static str { match self { Self::Depth => "depth_m", Self::Time => "time_ns", Self::Altitude => "altitude_m" } }
    fn unit(self) -> &'static str { if self == Self::Time { "ns" } else { "m" } }
    fn quantity(self) -> &'static str { match self { Self::Depth => "depth", Self::Time => "time", Self::Altitude => "altitude" } }
    fn samples(self, grid: &Grid) -> &[f32] { match self { Self::Depth => &grid.depth_m, Self::Time => &grid.time_ns, Self::Altitude => &grid.altitude_m } }
}

pub struct ExportOptions {
    pub profiles: bool,
    pub markers: bool,
    pub layers: bool,
    pub trace_points: bool,
    pub trace_step: usize,
    pub interpolations: bool,
    pub raster_value: RasterValue,
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
    if wkb_type == 3 {
        b.extend_from_slice(&1u32.to_le_bytes()); // one ring
    }
    if wkb_type == 2 || wkb_type == 3 {
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

pub fn export(path: &Path, crs: &CrsSpec, opt: &ExportOptions, profiles: &[ProfileRow], markers: &[MarkerRow], lines: &[LineRow], zones: &[Zone]) -> Result<String, String> {
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
    let extent = |pts: &[[f64; 3]], ext: &mut [f64; 4]| {
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
    if opt.interpolations && zones.iter().any(|z| z.grid.is_some()) {
        if !crs.is_projected_metres()? {
            summary.push("Interpolation raster needs a projected CRS in metres".into());
        } else if let Some(n) = export_raster(&tx, srs, opt.raster_value, zones)? {
            summary.push(format!("interpolations raster ({n} tiles, {})", opt.raster_value.label()));
        }
    }
    tx.commit().map_err(e)?;
    Ok(summary.join(", "))
}

fn export_raster(tx: &rusqlite::Transaction, srs: i32, value: RasterValue, zones: &[Zone]) -> Result<Option<usize>, String> {
    let grids: Vec<_> = zones.iter().filter_map(|z| z.grid.as_ref().map(|g| (z.name.as_str(), g)))
        .filter(|(_, g)| value.samples(g).iter().any(|v| v.is_finite())).collect();
    if grids.is_empty() { return Ok(None); }
    let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut cell = f64::MAX;
    for (_, g) in &grids {
        if g.nx == 0 || g.ny == 0 || !g.cell_m.is_finite() || g.cell_m <= 0.0
            || !g.origin.iter().all(|v| v.is_finite()) || g.nx.checked_mul(g.ny) != Some(value.samples(g).len()) {
            return Err("Invalid interpolation grid in project".into());
        }
        cell = cell.min(g.cell_m);
        bounds[0] = bounds[0].min(g.origin[0]);
        bounds[1] = bounds[1].min(g.origin[1]);
        bounds[2] = bounds[2].max(g.origin[0] + g.nx as f64 * g.cell_m);
        bounds[3] = bounds[3].max(g.origin[1] + g.ny as f64 * g.cell_m);
    }
    let extent = [(bounds[0] / cell).floor() * cell, (bounds[1] / cell).floor() * cell,
        (bounds[2] / cell).ceil() * cell, (bounds[3] / cell).ceil() * cell];
    let (nx, ny) = (((extent[2] - extent[0]) / cell).round(), ((extent[3] - extent[1]) / cell).round());
    if !nx.is_finite() || !ny.is_finite() || nx < 1.0 || ny < 1.0 || nx * ny > 50_000_000.0 {
        return Err("Interpolation mosaic is too large; use a larger cell size or smaller zones".into());
    }
    let (nx, ny) = (nx as usize, ny as usize);
    let (mw, mh) = (nx.div_ceil(256), ny.div_ceil(256));
    let description = format!("{} ({}); {}", value.quantity(), value.unit(), grids.iter().map(|(name, g)|
        format!("{name}: Max radius {:.3} m, cell size {:.3} m", g.max_radius_m, g.cell_m)).collect::<Vec<_>>().join("; "));
    let e = |x: rusqlite::Error| x.to_string();
    tx.execute_batch("CREATE TABLE gpkg_tile_matrix_set (table_name TEXT NOT NULL PRIMARY KEY, srs_id INTEGER NOT NULL, min_x DOUBLE NOT NULL, min_y DOUBLE NOT NULL, max_x DOUBLE NOT NULL, max_y DOUBLE NOT NULL);
        CREATE TABLE gpkg_tile_matrix (table_name TEXT NOT NULL, zoom_level INTEGER NOT NULL, matrix_width INTEGER NOT NULL, matrix_height INTEGER NOT NULL, tile_width INTEGER NOT NULL, tile_height INTEGER NOT NULL, pixel_x_size DOUBLE NOT NULL, pixel_y_size DOUBLE NOT NULL, PRIMARY KEY (table_name, zoom_level));
        CREATE TABLE gpkg_extensions (table_name TEXT, column_name TEXT, extension_name TEXT NOT NULL, definition TEXT NOT NULL, scope TEXT NOT NULL, UNIQUE (table_name, column_name, extension_name));
        CREATE TABLE gpkg_2d_gridded_coverage_ancillary (id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, tile_matrix_set_name TEXT NOT NULL UNIQUE, datatype TEXT NOT NULL DEFAULT 'integer', scale REAL NOT NULL DEFAULT 1.0, offset REAL NOT NULL DEFAULT 0.0, precision REAL DEFAULT 1.0, data_null REAL, grid_cell_encoding TEXT DEFAULT 'grid-value-is-center', uom TEXT, field_name TEXT DEFAULT 'Height', quantity_definition TEXT DEFAULT 'Height');
        CREATE TABLE gpkg_2d_gridded_tile_ancillary (id INTEGER PRIMARY KEY AUTOINCREMENT NOT NULL, tpudt_name TEXT NOT NULL, tpudt_id INTEGER NOT NULL, scale REAL NOT NULL DEFAULT 1.0, offset REAL NOT NULL DEFAULT 0.0, min REAL, max REAL, mean REAL, std_dev REAL, UNIQUE (tpudt_name, tpudt_id));
        CREATE TABLE interpolations (id INTEGER PRIMARY KEY AUTOINCREMENT, zoom_level INTEGER NOT NULL, tile_column INTEGER NOT NULL, tile_row INTEGER NOT NULL, tile_data BLOB NOT NULL, UNIQUE (zoom_level, tile_column, tile_row));").map_err(e)?;
    tx.execute("INSERT OR IGNORE INTO gpkg_spatial_ref_sys VALUES ('WGS 84 3D', 4979, 'EPSG', 4979, ?1, 'WGS 84 geographic 3D')",
        params![CrsSpec::Epsg(4979).wkt().unwrap_or_else(|| "undefined".into())]).map_err(e)?;
    tx.execute("INSERT INTO gpkg_contents (table_name, data_type, identifier, description, min_x, min_y, max_x, max_y, srs_id) VALUES ('interpolations', '2d-gridded-coverage', 'interpolations', ?1, ?2, ?3, ?4, ?5, ?6)",
        params![description, extent[0], extent[1], extent[2], extent[3], srs]).map_err(e)?;
    tx.execute("INSERT INTO gpkg_tile_matrix_set VALUES ('interpolations', ?1, ?2, ?3, ?4, ?5)", params![srs, extent[0], extent[1], extent[2], extent[3]]).map_err(e)?;
    tx.execute("INSERT INTO gpkg_tile_matrix VALUES ('interpolations', 0, ?1, ?2, 256, 256, ?3, ?3)", params![mw as i64, mh as i64, cell]).map_err(e)?;
    tx.execute("INSERT INTO gpkg_2d_gridded_coverage_ancillary (tile_matrix_set_name, datatype, scale, offset, precision, data_null, grid_cell_encoding, uom, field_name, quantity_definition) VALUES ('interpolations', 'float', 1.0, 0.0, 0.001, -9999.0, 'grid-value-is-center', ?1, ?2, ?3)",
        params![value.unit(), value.field(), value.quantity()]).map_err(e)?;
    for (table, column) in [("gpkg_2d_gridded_coverage_ancillary", None), ("gpkg_2d_gridded_tile_ancillary", None), ("interpolations", Some("tile_data"))] {
        tx.execute("INSERT INTO gpkg_extensions VALUES (?1, ?2, 'gpkg_2d_gridded_coverage', 'http://docs.opengeospatial.org/is/17-066r1/17-066r1.html', 'read-write')", params![table, column]).map_err(e)?;
    }
    let mut written = 0;
    // shortcut: nearest-cell sampling oversamples coarse zones; smooth only if mixed resolutions need it.
    for row in 0..mh {
        for col in 0..mw {
            let mut tile = vec![-9999.0f32; 256 * 256];
            let mut filled = false;
            for py in 0..256 {
                let j = row * 256 + py;
                if j >= ny { break; }
                let y = extent[3] - (j as f64 + 0.5) * cell;
                for px in 0..256 {
                    let i = col * 256 + px;
                    if i >= nx { break; }
                    let x = extent[0] + (i as f64 + 0.5) * cell;
                    for (_, g) in &grids {
                        let (gx, gy) = (((x - g.origin[0]) / g.cell_m).floor() as isize, ((y - g.origin[1]) / g.cell_m).floor() as isize);
                        if gx >= 0 && gy >= 0 && (gx as usize) < g.nx && (gy as usize) < g.ny {
                            let sample = value.samples(g)[gy as usize * g.nx + gx as usize];
                            if sample.is_finite() { tile[py * 256 + px] = sample; filled = true; }
                        }
                    }
                }
            }
            if !filled { continue; }
            let mut bytes = std::io::Cursor::new(Vec::new());
            tiff::encoder::TiffEncoder::new(&mut bytes).map_err(|e| e.to_string())?
                .write_image::<tiff::encoder::colortype::Gray32Float>(256, 256, &tile).map_err(|e| e.to_string())?;
            tx.execute("INSERT INTO interpolations (zoom_level, tile_column, tile_row, tile_data) VALUES (0, ?1, ?2, ?3)",
                params![col as i64, row as i64, bytes.into_inner()]).map_err(e)?;
            let id = tx.last_insert_rowid();
            tx.execute("INSERT INTO gpkg_2d_gridded_tile_ancillary (tpudt_name, tpudt_id, scale, offset) VALUES ('interpolations', ?1, 1.0, 0.0)", params![id]).map_err(e)?;
            written += 1;
        }
    }
    Ok(Some(written))
}

#[cfg(test)]
mod raster_tests {
    use super::*;
    use tiff::decoder::{Decoder, DecodingResult};

    fn zone(name: &str, origin: [f64; 2], nx: usize, ny: usize, depth: Vec<f32>) -> Zone {
        let n = nx * ny;
        Zone { id: 1, name: name.into(), type_idx: 0, visible: true, polygon: Vec::new(),
            grid: Some(Grid { origin, cell_m: 1.0, nx, ny, time_ns: vec![5.0; n], depth_m: depth,
                altitude_m: vec![100.0; n], max_radius_m: 2.0, power: 2.0 }), rev: 0 }
    }

    fn path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("gpr_raster_{name}_{}.gpkg", std::process::id()))
    }

    fn opts(value: RasterValue) -> ExportOptions {
        ExportOptions { profiles: false, markers: false, layers: false, trace_points: false, trace_step: 1,
            interpolations: true, raster_value: value }
    }

    fn tile(c: &Connection) -> Vec<f32> {
        let bytes: Vec<u8> = c.query_row("SELECT tile_data FROM interpolations WHERE zoom_level=0 AND tile_column=0 AND tile_row=0", [], |r| r.get(0)).unwrap();
        let mut decoder = Decoder::new(std::io::Cursor::new(bytes)).unwrap();
        assert_eq!(decoder.dimensions().unwrap(), (256, 256));
        match decoder.read_image().unwrap() { DecodingResult::F32(v) => v, _ => panic!("expected Float32 TIFF") }
    }

    #[test]
    fn writes_float_raster_and_ogc_metadata() {
        let path = path("single");
        let zones = [zone("ramp", [0.0, 0.0], 3, 3, vec![1.0, 2.0, 3.0, 4.0, f32::NAN, 6.0, 7.0, 8.0, 9.0])];
        export(&path, &CrsSpec::Epsg(2056), &opts(RasterValue::Depth), &[], &[], &[], &zones).unwrap();
        let c = Connection::open(&path).unwrap();
        let (kind, srs, ext): (String, i32, f64) = c.query_row("SELECT data_type,srs_id,max_x FROM gpkg_contents WHERE table_name='interpolations'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!((kind.as_str(), srs, ext), ("2d-gridded-coverage", 2056, 3.0));
        let matrix: (i64, i64, f64) = c.query_row("SELECT matrix_width,matrix_height,pixel_x_size FROM gpkg_tile_matrix WHERE table_name='interpolations'", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(matrix, (1, 1, 1.0));
        let matrix_set: i64 = c.query_row("SELECT count(*) FROM gpkg_tile_matrix_set WHERE table_name='interpolations'", [], |r| r.get(0)).unwrap();
        assert_eq!(matrix_set, 1);
        let extensions: i64 = c.query_row("SELECT count(*) FROM gpkg_extensions WHERE extension_name='gpkg_2d_gridded_coverage'", [], |r| r.get(0)).unwrap();
        assert_eq!(extensions, 3);
        let ancillary: (String, f64, String) = c.query_row("SELECT datatype,data_null,uom FROM gpkg_2d_gridded_coverage_ancillary", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).unwrap();
        assert_eq!(ancillary, ("float".into(), -9999.0, "m".into()));
        let tile_ancillary: i64 = c.query_row("SELECT count(*) FROM gpkg_2d_gridded_tile_ancillary", [], |r| r.get(0)).unwrap();
        assert_eq!(tile_ancillary, 1);
        let pixels = tile(&c);
        assert_eq!((pixels[0], pixels[1], pixels[256], pixels[257], pixels[512], pixels[3]),
            (7.0, 8.0, 4.0, -9999.0, 1.0, -9999.0));
        let obsolete: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name='interpolation_zones'", [], |r| r.get(0)).unwrap();
        assert_eq!(obsolete, 0);
        let geom: i64 = c.query_row("SELECT count(*) FROM gpkg_geometry_columns WHERE table_name='interpolations'", [], |r| r.get(0)).unwrap();
        assert_eq!(geom, 0);
        drop(c);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn mosaics_zones_without_erasing_valid_overlap() {
        let path = path("mosaic");
        let zones = [zone("west", [0.0, 0.0], 2, 2, vec![1.0, 2.0, 3.0, 4.0]),
            zone("east", [1.0, 0.0], 2, 2, vec![f32::NAN, 6.0, 7.0, 8.0])];
        export(&path, &CrsSpec::Epsg(2056), &opts(RasterValue::Depth), &[], &[], &[], &zones).unwrap();
        let c = Connection::open(&path).unwrap();
        let extent: (f64, f64) = c.query_row("SELECT min_x,max_x FROM gpkg_tile_matrix_set", [], |r| Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(extent, (0.0, 3.0));
        let pixels = tile(&c);
        assert_eq!((pixels[0], pixels[1], pixels[2], pixels[256 + 1], pixels[256 + 2]), (3.0, 7.0, 8.0, 2.0, 6.0));
        drop(c);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn value_choice_and_geographic_skip() {
        let zones = [zone("one", [0.0, 0.0], 1, 1, vec![2.0])];
        for (choice, expected) in [(RasterValue::Time, 5.0), (RasterValue::Altitude, 100.0)] {
            let path = path(choice.field());
            export(&path, &CrsSpec::Epsg(2056), &opts(choice), &[], &[], &[], &zones).unwrap();
            let c = Connection::open(&path).unwrap();
            assert_eq!(tile(&c)[0], expected);
            drop(c);
            std::fs::remove_file(path).unwrap();
        }
        let path = path("geographic");
        let summary = export(&path, &CrsSpec::Epsg(4326), &opts(RasterValue::Depth), &[], &[], &[], &zones).unwrap();
        assert!(summary.contains("Interpolation raster needs a projected CRS"));
        let c = Connection::open(&path).unwrap();
        let count: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name='interpolations'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        drop(c);
        std::fs::remove_file(path).unwrap();

        let path = self::path("feet");
        let feet = CrsSpec::Proj("+proj=utm +zone=32 +datum=WGS84 +units=ft +no_defs".into());
        let summary = export(&path, &feet, &opts(RasterValue::Depth), &[], &[], &[], &zones).unwrap();
        assert!(summary.contains("Interpolation raster needs a projected CRS"));
        let c = Connection::open(&path).unwrap();
        let count: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name='interpolations'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        drop(c);
        std::fs::remove_file(path).unwrap();

        let path = self::path("no_grid");
        export(&path, &CrsSpec::Epsg(2056), &opts(RasterValue::Depth), &[], &[], &[], &[]).unwrap();
        let c = Connection::open(&path).unwrap();
        let count: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE name='interpolations'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0);
        drop(c);
        std::fs::remove_file(path).unwrap();
    }
}
