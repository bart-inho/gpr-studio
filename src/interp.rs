//! Profile crossings (layer picks seen on the crossing profile) and interpolation zones:
//! inverse-distance-weighted surfaces of layer picks inside a polygon drawn on the map.

use crate::model::{short_label, LinePick, Profile};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------- geometry

/// Intersection of segments p1-p2 and q1-q2 (x/y): the fractions along each, half-open [0, 1)
/// so a crossing exactly on a shared vertex is counted once.
pub fn seg_intersect(p1: [f64; 2], p2: [f64; 2], q1: [f64; 2], q2: [f64; 2]) -> Option<(f64, f64)> {
    let r = [p2[0] - p1[0], p2[1] - p1[1]];
    let s = [q2[0] - q1[0], q2[1] - q1[1]];
    let den = r[0] * s[1] - r[1] * s[0];
    if den == 0.0 {
        return None; // parallel or degenerate (no threshold: geographic CRS segments are tiny)
    }
    let qp = [q1[0] - p1[0], q1[1] - p1[1]];
    let t = (qp[0] * s[1] - qp[1] * s[0]) / den;
    let u = (qp[0] * r[1] - qp[1] * r[0]) / den;
    ((0.0..1.0).contains(&t) && (0.0..1.0).contains(&u)).then_some((t, u))
}

/// Point in polygon, even-odd rule (the polygon is implicitly closed).
pub fn inside(poly: &[[f64; 2]], x: f64, y: f64) -> bool {
    let mut c = false;
    let mut j = poly.len().wrapping_sub(1);
    for i in 0..poly.len() {
        let (a, b) = (poly[i], poly[j]);
        if (a[1] > y) != (b[1] > y) && x < (b[0] - a[0]) * (y - a[1]) / (b[1] - a[1]) + a[0] {
            c = !c;
        }
        j = i;
    }
    c
}

fn overlap(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

// ---------------------------------------------------------------- crossings

/// Where two profiles cross: the fractional trace on each.
#[derive(Clone, Copy, Debug)]
pub struct Crossing {
    pub a: u32,
    pub tr_a: f32,
    pub b: u32,
    pub tr_b: f32,
}

/// Cache key: crossings only change with the coordinates.
pub fn crossings_key(profiles: &[Profile]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in profiles {
        (p.id, p.version, p.coords.len(), short_label(&p.name)).hash(&mut h);
        for c in p.coords.first().into_iter().chain(p.coords.last()) {
            (c[0].to_bits(), c[1].to_bits()).hash(&mut h);
        }
    }
    h.finish()
}

/// Every intersection of the GPS tracks of two profiles (channels of one profile, same `short_label`, skipped).
/// Bounding boxes of the whole track, then of chunks of 32 segments, keep this fast for 100+ profiles.
pub fn crossings(profiles: &[Profile]) -> Vec<Crossing> {
    const CH: usize = 32;
    struct Prep<'a> {
        id: u32,
        label: String,
        c: &'a [[f64; 3]],
        bbox: [f64; 4],
        chunks: Vec<(usize, usize, [f64; 4])>,
    }
    let bbox_of = |pts: &[[f64; 3]]| pts.iter().fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, p| [b[0].min(p[0]), b[1].min(p[1]), b[2].max(p[0]), b[3].max(p[1])]);
    let prep: Vec<Prep> = profiles
        .iter()
        .filter(|p| p.coords.len() >= 2)
        .map(|p| {
            let n = p.coords.len() - 1; // segments
            let chunks = (0..n).step_by(CH).map(|s| (s, (s + CH).min(n), bbox_of(&p.coords[s..=(s + CH).min(n)]))).collect();
            Prep { id: p.id, label: short_label(&p.name), c: &p.coords, bbox: bbox_of(&p.coords), chunks }
        })
        .collect();
    let xy = |c: [f64; 3]| [c[0], c[1]];
    let mut out = Vec::new();
    for i in 0..prep.len() {
        for j in i + 1..prep.len() {
            let (a, b) = (&prep[i], &prep[j]);
            if a.label == b.label || !overlap(&a.bbox, &b.bbox) {
                continue;
            }
            for ca in a.chunks.iter().filter(|c| overlap(&c.2, &b.bbox)) {
                for cb in b.chunks.iter().filter(|c| overlap(&c.2, &ca.2)) {
                    for sa in ca.0..ca.1 {
                        for sb in cb.0..cb.1 {
                            if let Some((t, u)) = seg_intersect(xy(a.c[sa]), xy(a.c[sa + 1]), xy(b.c[sb]), xy(b.c[sb + 1])) {
                                out.push(Crossing { a: a.id, tr_a: (sa as f64 + t) as f32, b: b.id, tr_b: (sb as f64 + u) as f32 });
                            }
                        }
                    }
                }
            }
        }
    }
    out
}

/// Pick time at a trace, linear between the two neighbouring vertices; None outside the pick (no extrapolation).
pub fn pick_time_at(l: &LinePick, tr: f32) -> Option<f32> {
    l.points.windows(2).find_map(|w| {
        let ((a, ta), (b, tb)) = (w[0], w[1]);
        (tr >= a.min(b) && tr <= a.max(b)).then(|| if b != a { ta + (tb - ta) * (tr - a) / (b - a) } else { ta })
    })
}

/// A layer pick of another profile, at the place where it crosses profile `pid`.
pub struct CrossPick {
    pub trace: f32,
    /// Two-way time on `pid` (ns) for the same depth.
    pub time_ns: f32,
    pub depth_m: f32,
    pub label: String,
    pub from_name: String,
    pub line_id: u32,
    pub type_idx: usize,
}

/// Depth-matched: d = v1·t1/2 on the picked profile, t2 = 2d/v2 here.
/// shortcut: depth-matched, not elevation-matched; with topography and different antenna heights this assumes the same surface at the crossing point.
pub fn crossing_picks(profiles: &[Profile], lines: &[LinePick], crossings: &[Crossing], pid: u32) -> Vec<CrossPick> {
    let Some(me) = profiles.iter().find(|p| p.id == pid) else { return vec![] };
    let mut out = Vec::new();
    for c in crossings {
        let (my_tr, other, other_tr) = if c.a == pid { (c.tr_a, c.b, c.tr_b) } else if c.b == pid { (c.tr_b, c.a, c.tr_a) } else { continue };
        let Some(op) = profiles.iter().find(|p| p.id == other) else { continue };
        for l in lines.iter().filter(|l| l.profile_id == other) {
            let Some(t1) = pick_time_at(l, other_tr) else { continue };
            let d = op.depth_of_time(t1);
            out.push(CrossPick { trace: my_tr, time_ns: me.time_of_depth(d), depth_m: d, label: format!("p{}", short_label(&op.name)), from_name: op.name.clone(), line_id: l.id, type_idx: l.type_idx });
        }
    }
    out
}

// ---------------------------------------------------------------- interpolation zones

pub fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InterpParams {
    pub max_radius_m: f64,
    pub cell_m: f64,
    pub power: f64,
}

impl Default for InterpParams {
    fn default() -> Self {
        InterpParams { max_radius_m: 5.0, cell_m: 0.25, power: 2.0 }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Zone {
    pub id: u32,
    pub name: String,
    pub type_idx: usize,
    #[serde(default = "yes")]
    pub visible: bool,
    /// Polygon vertices in the project CRS (closed implicitly).
    pub polygon: Vec<[f64; 2]>,
    #[serde(default)]
    pub grid: Option<Grid>,
    /// Bumped when the grid changes (rebuilds the map texture).
    #[serde(skip)]
    pub rev: u64,
}

/// Interpolated cells, row-major from the south-west corner: value[j * nx + i] at
/// (origin + (i + 0.5)·cell, origin + (j + 0.5)·cell). NaN = empty.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Grid {
    pub origin: [f64; 2],
    pub cell_m: f64,
    pub nx: usize,
    pub ny: usize,
    #[serde(with = "nan_vec")]
    pub time_ns: Vec<f32>,
    #[serde(with = "nan_vec")]
    pub depth_m: Vec<f32>,
    #[serde(with = "nan_vec")]
    pub altitude_m: Vec<f32>,
    pub max_radius_m: f64,
    pub power: f64,
}

/// JSON has no NaN: empty cells are written as null.
mod nan_vec {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};
    pub fn serialize<S: Serializer>(v: &[f32], s: S) -> Result<S::Ok, S::Error> {
        v.iter().map(|x| x.is_finite().then_some(*x)).collect::<Vec<_>>().serialize(s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<f32>, D::Error> {
        Ok(Vec::<Option<f32>>::deserialize(d)?.into_iter().map(|x| x.unwrap_or(f32::NAN)).collect())
    }
}

impl Grid {
    pub fn filled(&self) -> usize {
        self.depth_m.iter().filter(|v| v.is_finite()).count()
    }
    pub fn depth_range(&self) -> Option<(f32, f32)> {
        let (a, b) = self.depth_m.iter().filter(|v| v.is_finite()).fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(*v), b.max(*v)));
        (a <= b).then_some((a, b))
    }
    /// Non-empty cells: (x, y, time ns, depth m, altitude m).
    pub fn cells(&self) -> Vec<[f64; 5]> {
        let mut out = Vec::new();
        for j in 0..self.ny {
            for i in 0..self.nx {
                let k = j * self.nx + i;
                if self.depth_m[k].is_finite() {
                    let (x, y) = (self.origin[0] + (i as f64 + 0.5) * self.cell_m, self.origin[1] + (j as f64 + 0.5) * self.cell_m);
                    out.push([x, y, self.time_ns[k] as f64, self.depth_m[k] as f64, self.altitude_m[k] as f64]);
                }
            }
        }
        out
    }
    /// Depth at a map position: bilinear between the 4 neighbouring cells, the nearest cell when one is empty.
    pub fn depth_at(&self, x: f64, y: f64) -> Option<f32> {
        let fx = (x - self.origin[0]) / self.cell_m - 0.5;
        let fy = (y - self.origin[1]) / self.cell_m - 0.5;
        let get = |i: i64, j: i64| (i >= 0 && j >= 0 && (i as usize) < self.nx && (j as usize) < self.ny).then(|| self.depth_m[j as usize * self.nx + i as usize]).filter(|v| v.is_finite());
        let (i0, j0) = (fx.floor() as i64, fy.floor() as i64);
        let (u, v) = ((fx - i0 as f64) as f32, (fy - j0 as f64) as f32);
        match (get(i0, j0), get(i0 + 1, j0), get(i0, j0 + 1), get(i0 + 1, j0 + 1)) {
            (Some(a), Some(b), Some(c), Some(d)) => Some((a * (1.0 - u) + b * u) * (1.0 - v) + (c * (1.0 - u) + d * u) * v),
            _ => get(fx.round() as i64, fy.round() as i64),
        }
    }
    /// RGBA image (north up) in the Jet scale from the min to the max depth; empty cells transparent.
    pub fn rgba(&self) -> Vec<u8> {
        let (lo, hi) = self.depth_range().unwrap_or((0.0, 1.0));
        let mut img = vec![0u8; self.nx * self.ny * 4];
        for j in 0..self.ny {
            for i in 0..self.nx {
                let d = self.depth_m[j * self.nx + i];
                if d.is_finite() {
                    let u = if hi > lo { (d - lo) / (hi - lo) } else { 0.5 };
                    let c = crate::render::ColorMap::Jet.color(u * 2.0 - 1.0);
                    let o = ((self.ny - 1 - j) * self.nx + i) * 4;
                    img[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                }
            }
        }
        img
    }
}

/// One input point of an interpolation.
#[derive(Clone, Copy, Debug)]
pub struct InPt {
    pub x: f64,
    pub y: f64,
    pub depth: f64,
    pub alt: f64,
    /// Velocity of the profile it was picked on (m/ns).
    pub v: f64,
}

/// Every vertex of the zone's pick type inside the polygon, densified along the profile at about `step` metres.
pub fn zone_points(profiles: &[Profile], lines: &[LinePick], zone: &Zone, step: f64) -> Vec<InPt> {
    let mut out = Vec::new();
    for l in lines.iter().filter(|l| l.type_idx == zone.type_idx) {
        let Some(p) = profiles.iter().find(|p| p.id == l.profile_id && p.has_coords()) else { continue };
        let mut push = |tr: f32, t: f32| {
            if let Some(c) = p.coord_at(tr) {
                if inside(&zone.polygon, c[0], c[1]) {
                    let d = p.depth_of_time(t) as f64;
                    out.push(InPt { x: c[0], y: c[1], depth: d, alt: c[2] - d, v: p.velocity as f64 });
                }
            }
        };
        for w in l.points.windows(2) {
            let ((ta, ya), (tb, yb)) = (w[0], w[1]);
            let (ca, cb) = (p.coord_at(ta), p.coord_at(tb));
            let len = ca.zip(cb).map(|(a, b)| (b[0] - a[0]).hypot(b[1] - a[1])).unwrap_or(0.0);
            let n = ((len / step.max(1e-6)).ceil() as usize).clamp(1, 100_000);
            for k in 0..n {
                let f = k as f32 / n as f32;
                push(ta + (tb - ta) * f, ya + (yb - ya) * f);
            }
        }
        if let Some(&(tr, t)) = l.points.last() {
            push(tr, t);
        }
    }
    out
}

/// IDW of the points within the max radius on the cells of the polygon. A cell with no point within
/// the radius stays empty (NaN): only places between picks get a value.
/// Depth and altitude are interpolated separately; time = 2·depth / v_mean (v_mean = IDW mean velocity).
pub fn interpolate(pts: &[InPt], poly: &[[f64; 2]], p: &InterpParams) -> Result<Grid, String> {
    if poly.len() < 3 {
        return Err("the zone needs a polygon of at least 3 vertices".into());
    }
    if !(p.cell_m > 0.0 && p.max_radius_m > 0.0 && p.power > 0.0) {
        return Err("cell size, max radius and power must be positive".into());
    }
    let (x0, y0, x1, y1) = poly.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, q| (b.0.min(q[0]), b.1.min(q[1]), b.2.max(q[0]), b.3.max(q[1])));
    let nx = ((x1 - x0) / p.cell_m).ceil().max(1.0);
    let ny = ((y1 - y0) / p.cell_m).ceil().max(1.0);
    if !(nx * ny <= 20e6) {
        return Err(format!("{nx:.0} x {ny:.0} cells is too many; use a larger cell size"));
    }
    let (nx, ny) = (nx as usize, ny as usize);
    // Points bucketed in cells of the max radius: neighbours are in the 3 x 3 buckets around.
    let r = p.max_radius_m;
    let key = |x: f64, y: f64| (((x - x0) / r).floor() as i64, ((y - y0) / r).floor() as i64);
    let mut buckets: std::collections::HashMap<(i64, i64), Vec<usize>> = Default::default();
    for (k, q) in pts.iter().enumerate() {
        buckets.entry(key(q.x, q.y)).or_default().push(k);
    }
    let mut g = Grid { origin: [x0, y0], cell_m: p.cell_m, nx, ny, time_ns: vec![f32::NAN; nx * ny], depth_m: vec![f32::NAN; nx * ny], altitude_m: vec![f32::NAN; nx * ny], max_radius_m: r, power: p.power };
    for j in 0..ny {
        for i in 0..nx {
            let (x, y) = (x0 + (i as f64 + 0.5) * p.cell_m, y0 + (j as f64 + 0.5) * p.cell_m);
            if !inside(poly, x, y) {
                continue;
            }
            let (bx, by) = key(x, y);
            let (mut sw, mut sd, mut sa, mut sv) = (0.0, 0.0, 0.0, 0.0);
            let mut exact = None;
            for q in (-1..=1).flat_map(|dx| (-1..=1).map(move |dy| (bx + dx, by + dy))).filter_map(|b| buckets.get(&b)).flatten().map(|&k| &pts[k]) {
                let d = (q.x - x).hypot(q.y - y);
                if d > r {
                    continue;
                }
                if d < 1e-9 {
                    exact = Some(q);
                    break;
                }
                let w = d.powf(-p.power);
                sw += w;
                sd += w * q.depth;
                sa += w * q.alt;
                sv += w * q.v;
            }
            let (d, a, v) = match exact {
                Some(q) => (q.depth, q.alt, q.v),
                None if sw > 0.0 => (sd / sw, sa / sw, sv / sw),
                None => continue,
            };
            let k = j * nx + i;
            g.depth_m[k] = d as f32;
            g.altitude_m[k] = a as f32;
            g.time_ns[k] = if v > 0.0 { (2.0 * d / v) as f32 } else { f32::NAN };
        }
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Section;
    use std::path::PathBuf;

    fn straight(name: &str, id: u32, from: [f64; 2], to: [f64; 2], ntr: usize, v: f32) -> Profile {
        let mut p = Profile::new(name.into(), PathBuf::new(), "test", Section::new(10, ntr, 0.1), 0.1);
        p.id = id;
        p.velocity = v;
        p.coords = (0..ntr).map(|i| {
            let f = i as f64 / (ntr - 1) as f64;
            [from[0] + (to[0] - from[0]) * f, from[1] + (to[1] - from[1]) * f, 500.0]
        }).collect();
        p
    }

    #[test]
    fn segment_intersection_and_point_in_polygon() {
        let (t, u) = seg_intersect([0.0, 0.0], [2.0, 0.0], [1.0, -1.0], [1.0, 3.0]).unwrap();
        assert!((t - 0.5).abs() < 1e-12 && (u - 0.25).abs() < 1e-12);
        assert!(seg_intersect([0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]).is_none()); // parallel
        assert!(seg_intersect([0.0, 0.0], [1.0, 0.0], [2.0, -1.0], [2.0, 1.0]).is_none()); // beyond the end
        let sq = [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]];
        assert!(inside(&sq, 2.0, 2.0));
        assert!(!inside(&sq, 5.0, 2.0));
        // Concave "U": the notch is outside.
        let u_shape = [[0.0, 0.0], [3.0, 0.0], [3.0, 3.0], [2.0, 3.0], [2.0, 1.0], [1.0, 1.0], [1.0, 3.0], [0.0, 3.0]];
        assert!(!inside(&u_shape, 1.5, 2.0));
        assert!(inside(&u_shape, 0.5, 2.0));
    }

    #[test]
    fn layer_pick_appears_depth_matched_on_the_crossing_profile() {
        // P1 west-east, P2 south-north, crossing at the middle of both (trace 50 of 101).
        let p1 = straight("LINE_001", 1, [0.0, 0.0], [10.0, 0.0], 101, 0.1);
        let p2 = straight("LINE_002", 2, [5.0, -5.0], [5.0, 5.0], 101, 0.08);
        let mut ch = straight("LINE_001_1", 3, [0.0, 0.1], [10.0, -0.1], 101, 0.1); // channel of P1: not a crossing
        ch.coords.reverse();
        let profiles = vec![p1, p2, ch];
        let cr = crossings(&profiles);
        assert_eq!(cr.len(), 2, "{cr:?}"); // P1 x P2 and the channel x P2, never P1 x its channel
        let lines = vec![LinePick { id: 7, profile_id: 1, type_idx: 6, points: vec![(0.0, 20.0), (100.0, 20.0)], comment: String::new() }];
        let picks = crossing_picks(&profiles, &lines, &cr, 2);
        assert_eq!(picks.len(), 1);
        assert!((picks[0].trace - 50.0).abs() < 1e-3, "{}", picks[0].trace);
        assert!((picks[0].time_ns - 25.0).abs() < 1e-3, "{}", picks[0].time_ns);
        assert_eq!(picks[0].label, "p001");
        // A pick that does not reach the crossing shows nothing.
        let short = vec![LinePick { points: vec![(0.0, 20.0), (40.0, 20.0)], ..lines[0].clone() }];
        assert!(crossing_picks(&profiles, &short, &cr, 2).is_empty());
    }

    #[test]
    fn idw_fills_between_picks_and_leaves_far_cells_empty() {
        // Two parallel picks 2 m apart at 1.0 m and 2.0 m depth (v = 0.1 m/ns).
        let a = straight("A_1", 1, [0.0, 0.0], [10.0, 0.0], 101, 0.1);
        let b = straight("B_2", 2, [0.0, 2.0], [10.0, 2.0], 101, 0.1);
        let lines = vec![
            LinePick { id: 1, profile_id: 1, type_idx: 6, points: vec![(0.0, 20.0), (100.0, 20.0)], comment: String::new() },
            LinePick { id: 2, profile_id: 2, type_idx: 6, points: vec![(0.0, 40.0), (100.0, 40.0)], comment: String::new() },
        ];
        let poly = vec![[-1.0, -1.0], [11.0, -1.0], [11.0, 13.0], [-1.0, 13.0]];
        let zone = Zone { id: 1, name: "Interpolation 1".into(), type_idx: 6, visible: true, polygon: poly.clone(), grid: None, rev: 0 };
        let params = InterpParams::default();
        let pts = zone_points(&[a, b], &lines, &zone, params.cell_m);
        assert!(pts.len() > 60, "densified: {}", pts.len());
        let g = interpolate(&pts, &poly, &params).unwrap();
        let mid = g.depth_at(5.0, 1.0).unwrap();
        assert!((mid - 1.5).abs() < 0.02, "{mid}");
        assert!(g.depth_at(5.0, 12.0).is_none()); // 10 m from the nearest pick
        let k = ((1.0 + 1.0) / 0.25) as usize * g.nx + ((5.0 + 1.0) / 0.25) as usize;
        assert!((g.time_ns[k] - 2.0 * g.depth_m[k] / 0.1).abs() < 1e-3);
        // Project file round trip keeps empty cells empty.
        let json = serde_json::to_string(&g).unwrap();
        let back: Grid = serde_json::from_str(&json).unwrap();
        assert_eq!(back.filled(), g.filled());
        assert!(back.depth_at(5.0, 12.0).is_none());
    }
}
