//! Coordinate reference systems: EPSG lookup and point transformation (pure Rust).

use proj4rs::proj::Proj;
use proj4rs::transform::{Transform, TransformClosure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum CrsSpec {
    Epsg(u16),
    Proj(String),
}

impl Default for CrsSpec {
    fn default() -> Self {
        CrsSpec::Epsg(2056)
    }
}

impl CrsSpec {
    pub fn label(&self) -> String {
        match self {
            CrsSpec::Epsg(c) => format!("EPSG:{c}{}", common_name(*c).map(|n| format!(" ({n})")).unwrap_or_default()),
            CrsSpec::Proj(s) => format!("custom: {s}"),
        }
    }
    pub fn short(&self) -> String {
        match self {
            CrsSpec::Epsg(c) => format!("EPSG:{c}"),
            CrsSpec::Proj(_) => "custom".into(),
        }
    }
    pub fn proj4(&self) -> Option<String> {
        match self {
            CrsSpec::Epsg(c) => crs_definitions::from_code(*c).map(|d| d.proj4.to_string()),
            CrsSpec::Proj(s) => Some(s.clone()),
        }
    }
    pub fn wkt(&self) -> Option<String> {
        match self {
            CrsSpec::Epsg(c) => crs_definitions::from_code(*c).map(|d| d.wkt.to_string()),
            CrsSpec::Proj(_) => None,
        }
    }
    pub fn epsg(&self) -> Option<u16> {
        match self {
            CrsSpec::Epsg(c) => Some(*c),
            _ => None,
        }
    }
    pub fn build(&self) -> Result<Crs, String> {
        let p4 = self.proj4().ok_or_else(|| format!("{} is not in the built-in EPSG list", self.short()))?;
        let proj = Proj::from_proj_string(&p4).map_err(|e| format!("cannot use {}: {e:?}", self.short()))?;
        Ok(Crs { spec: self.clone(), geographic: proj.is_latlong(), proj })
    }
    /// Parse "2056", "EPSG:2056" or a "+proj=..." string.
    pub fn parse(text: &str) -> Option<CrsSpec> {
        let t = text.trim();
        if t.starts_with('+') {
            return Some(CrsSpec::Proj(t.to_string()));
        }
        let digits = t.trim_start_matches("EPSG:").trim_start_matches("epsg:").trim();
        digits.parse::<u16>().ok().filter(|c| crs_definitions::from_code(*c).is_some()).map(CrsSpec::Epsg)
    }
}

pub fn common_name(code: u16) -> Option<&'static str> {
    COMMON.iter().find(|(c, _)| *c == code).map(|(_, n)| *n)
}

pub const COMMON: &[(u16, &str)] = &[
    (2056, "CH1903+ / LV95"),
    (21781, "CH1903 / LV03"),
    (4326, "WGS 84 geographic"),
    (3857, "WGS 84 / Pseudo-Mercator"),
    (25832, "ETRS89 / UTM 32N"),
    (25833, "ETRS89 / UTM 33N"),
    (32631, "WGS 84 / UTM 31N"),
    (32632, "WGS 84 / UTM 32N"),
    (32633, "WGS 84 / UTM 33N"),
    (31467, "DHDN / Gauss-Kruger 3"),
    (2154, "RGF93 / Lambert-93"),
    (27700, "OSGB 1936 / British National Grid"),
    (28992, "Amersfoort / RD New"),
    (3035, "ETRS89 / LAEA Europe"),
    (31287, "MGI / Austria Lambert"),
];

pub struct Crs {
    pub spec: CrsSpec,
    pub geographic: bool,
    proj: Proj,
}

struct Pts<'a>(&'a mut [[f64; 3]]);

impl Transform for Pts<'_> {
    fn transform_coordinates<F: TransformClosure>(&mut self, f: &mut F) -> proj4rs::errors::Result<()> {
        for p in self.0.iter_mut() {
            let (x, y, z) = f(p[0], p[1], p[2])?;
            *p = [x, y, z];
        }
        Ok(())
    }
}

/// Transform points in place from `src` to `dst`. Geographic coordinates are degrees (x = lon, y = lat).
pub fn transform(src: &Crs, dst: &Crs, pts: &mut [[f64; 3]]) -> Result<(), String> {
    if src.spec == dst.spec {
        return Ok(());
    }
    if src.geographic {
        for p in pts.iter_mut() {
            p[0] = p[0].to_radians();
            p[1] = p[1].to_radians();
        }
    }
    // Transform point by point so one bad point does not lose the rest.
    for p in pts.iter_mut() {
        let mut one = [*p];
        let mut w = Pts(&mut one);
        match proj4rs::transform::transform(&src.proj, &dst.proj, &mut w) {
            Ok(()) => *p = one[0],
            Err(_) => *p = [f64::NAN, f64::NAN, p[2]],
        }
    }
    if dst.geographic {
        for p in pts.iter_mut() {
            p[0] = p[0].to_degrees();
            p[1] = p[1].to_degrees();
        }
    }
    Ok(())
}

pub fn transform_spec(src: &CrsSpec, dst: &CrsSpec, pts: &mut [[f64; 3]]) -> Result<(), String> {
    if src == dst {
        return Ok(());
    }
    let s = src.build()?;
    let d = dst.build()?;
    transform(&s, &d, pts)
}
