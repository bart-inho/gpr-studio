//! PDF report: per page the radargram section, the GPS map with the same section highlighted,
//! scale bar, north arrow, profile information, processing sequence and picks.

use crate::app::{App, PdfRange};
use crate::draw::{fmt_tick, nice_step};
use crate::model::{Profile, Symbol};
use crate::pdf::{Page, Pdf};

const ORANGE: [u8; 3] = [255, 140, 0];
const BLUE: [u8; 3] = [25, 85, 220];

fn today() -> String {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs / 86400;
    // Civil date from days since 1970-01-01.
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

fn pdf_symbol(pg: &mut Page, x: f32, y: f32, r: f32, sym: Symbol, col: [u8; 3]) {
    pg.fill_color(col);
    pg.stroke_color([0, 0, 0]);
    pg.line_width(0.6);
    match sym {
        Symbol::Circle => pg.circle(x, y, r, true, true),
        Symbol::Square => pg.rect(x - r, y - r, 2.0 * r, 2.0 * r, true, true),
        Symbol::Triangle => pg.polygon(&[[x, y - r * 1.15], [x + r, y + r * 0.75], [x - r, y + r * 0.75]], true, true),
        Symbol::Diamond => pg.polygon(&[[x, y - r * 1.2], [x + r, y], [x, y + r * 1.2], [x - r, y]], true, true),
        Symbol::Cross => {
            pg.stroke_color(col);
            pg.line_width(1.6);
            pg.line([x - r, y - r], [x + r, y + r]);
            pg.line([x - r, y + r], [x + r, y - r]);
        }
        Symbol::Star => {
            let pts: Vec<[f32; 2]> = (0..10)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
                    let rr = if k % 2 == 0 { r * 1.3 } else { r * 0.55 };
                    [x + a.cos() * rr, y + a.sin() * rr]
                })
                .collect();
            pg.polygon(&pts, true, true);
        }
    }
}

/// Text with a white backing so it stays readable over a basemap.
fn label(pg: &mut Page, x: f32, y: f32, size: f32, text: &str, bold: bool, col: [u8; 3]) {
    let w = crate::pdf::text_width(text, size);
    pg.fill_color([255, 255, 255]);
    pg.rect(x - 1.0, y - size * 0.85, w + 2.0, size * 1.1, true, false);
    pg.fill_color(col);
    pg.text(x, y, size, text, bold);
}

/// Trace ranges (one per page) for a profile.
fn ranges(app: &App, p: &Profile) -> Vec<(f32, f32)> {
    let n = p.proc.ntr as f32;
    match app.pdf_opts.range {
        PdfRange::Visible => {
            let v = app.views.get(&p.id);
            match (v, app.active_profile().map(|a| a.id == p.id).unwrap_or(false)) {
                (Some(v), true) => vec![(v.tr0.max(0.0), v.tr1.min(n))],
                _ => vec![(0.0, n)],
            }
        }
        PdfRange::Whole => vec![(0.0, n)],
        PdfRange::Chunks => {
            let len = p.length().max(1e-6);
            let step = app.pdf_opts.chunk_m.max(0.5) as f64;
            let mut out = Vec::new();
            let mut d = 0.0;
            while d < len - 1e-6 {
                let a = p.trace_at_dist(d);
                let b = p.trace_at_dist((d + step).min(len)) + 1.0;
                out.push((a, b.min(n)));
                d += step;
            }
            if out.is_empty() {
                out.push((0.0, n));
            }
            out
        }
    }
}

struct MapFrame {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    cx: f64,
    cy: f64,
    scale: f64, // world units per point
}

impl MapFrame {
    fn to_page(&self, wx: f64, wy: f64) -> [f32; 2] {
        [self.x + self.w / 2.0 + ((wx - self.cx) / self.scale) as f32, self.y + self.h / 2.0 - ((wy - self.cy) / self.scale) as f32]
    }
    fn to_world(&self, px: f32, py: f32) -> [f64; 2] {
        [self.cx + (px - self.x - self.w / 2.0) as f64 * self.scale, self.cy - (py - self.y - self.h / 2.0) as f64 * self.scale]
    }
}

/// Render the visible basemaps into an RGB image covering the map frame.
fn basemap_image(app: &App, mf: &MapFrame, wpx: usize, hpx: usize) -> Option<Vec<u8>> {
    let maps: Vec<_> = app.basemaps.iter().filter(|b| b.visible).collect();
    if maps.is_empty() {
        return None;
    }
    let mut img = vec![236u8; wpx * hpx * 3];
    const G: usize = 32;
    for b in maps {
        // Grid of basemap pixel coordinates for the frame.
        let mut pts = Vec::with_capacity((G + 1) * (G + 1));
        for j in 0..=G {
            for i in 0..=G {
                let w = mf.to_world(mf.x + mf.w * i as f32 / G as f32, mf.y + mf.h * j as f32 / G as f32);
                pts.push([w[0], w[1], 0.0]);
            }
        }
        if crate::crs::transform_spec(&app.project_crs, &b.crs, &mut pts).is_err() {
            continue;
        }
        let grid: Vec<[f64; 2]> = pts.iter().map(|p| b.info.world_to_pixel(p[0], p[1])).collect();
        let ov = &b.overview;
        let a = b.opacity;
        for y in 0..hpx {
            let gy = y as f64 / hpx as f64 * G as f64;
            let j = (gy.floor() as usize).min(G - 1);
            let fy = gy - j as f64;
            for x in 0..wpx {
                let gx = x as f64 / wpx as f64 * G as f64;
                let i = (gx.floor() as usize).min(G - 1);
                let fx = gx - i as f64;
                let g = |ii: usize, jj: usize| grid[jj * (G + 1) + ii];
                let (p00, p10, p01, p11) = (g(i, j), g(i + 1, j), g(i, j + 1), g(i + 1, j + 1));
                if !p00[0].is_finite() || !p11[0].is_finite() {
                    continue;
                }
                let col = p00[0] * (1.0 - fx) * (1.0 - fy) + p10[0] * fx * (1.0 - fy) + p01[0] * (1.0 - fx) * fy + p11[0] * fx * fy;
                let row = p00[1] * (1.0 - fx) * (1.0 - fy) + p10[1] * fx * (1.0 - fy) + p01[1] * (1.0 - fx) * fy + p11[1] * fx * fy;
                let ox = col / b.ov_factor;
                let oy = row / b.ov_factor;
                if ox < 0.0 || oy < 0.0 || ox >= ov.w as f64 || oy >= ov.h as f64 {
                    continue;
                }
                let o = (oy as usize * ov.w + ox as usize) * 4;
                let al = ov.rgba[o + 3] as f32 / 255.0 * a;
                let d = (y * wpx + x) * 3;
                for k in 0..3 {
                    img[d + k] = (img[d + k] as f32 * (1.0 - al) + ov.rgba[o + k] as f32 * al) as u8;
                }
            }
        }
    }
    Some(img)
}

pub fn export(app: &App, path: &std::path::Path) -> Result<usize, String> {
    let profiles: Vec<&Profile> = if app.pdf_opts.all_profiles { app.profiles.iter().collect() } else { app.active_profile().into_iter().collect() };
    if profiles.is_empty() {
        return Err("no profile to export".into());
    }
    let (pw, ph) = if app.pdf_opts.a3 { (1190.55, 841.89) } else { (841.89, 595.28) };
    let mut pdf = Pdf::new();
    let mut jobs = Vec::new();
    for p in &profiles {
        for r in ranges(app, p) {
            jobs.push((*p, r));
        }
    }
    let total = jobs.len();
    let date = today();
    for (pi, (p, (tr0, tr1))) in jobs.into_iter().enumerate() {
        let mut pg = Page::new(pw, ph);
        let m = 26.0f32;
        let k = pw / 841.89; // scale for A3
        // ---------------------------------------------------------- title bar
        pg.fill_color([30, 50, 80]);
        pg.rect(0.0, 0.0, pw, 30.0 * k, true, false);
        pg.fill_color([255, 255, 255]);
        let title = if app.pdf_opts.title.trim().is_empty() { "GPR profile report".to_string() } else { app.pdf_opts.title.clone() };
        pg.text(m, 20.0 * k, 13.0 * k, &title, true);
        pg.text(m + crate::pdf::text_width(&title, 13.0 * k) + 14.0, 20.0 * k, 11.0 * k, &format!("Profile {}  ·  {:.1} - {:.1} m", p.name, p.dist_at(tr0), p.dist_at((tr1 - 1.0).max(tr0))), false);
        pg.text_right(pw - m, 20.0 * k, 9.0 * k, &format!("{}  ·  {}  ·  page {}/{}", date, app.project_crs.short(), pi + 1, total), false);

        // ---------------------------------------------------------- radargram
        let gx = m + 34.0 * k;
        let gy = 30.0 * k + 26.0 * k;
        let gw = pw - gx - m - 40.0 * k;
        let gh = ph * 0.42;
        let res = 2.4f32;
        let (iw, ih) = (((gw * res) as usize).clamp(200, 4000), ((gh * res) as usize).clamp(100, 2400));
        let clip = crate::render::clip_level(&p.proc, 1.0) / app.contrast.max(0.01);
        let view = app.views.get(&p.id);
        let (s0, s1) = view.map(|v| (v.s0, v.s1)).unwrap_or((0.0, p.proc.ns as f32));
        let rgba = crate::render::render(&p.proc, tr0, tr1, s0, s1, iw, ih, app.colormap, clip);
        let rgb: Vec<u8> = rgba.chunks(4).flat_map(|c| [c[0], c[1], c[2]]).collect();
        let img = pdf.add_image(iw, ih, &rgb);
        pg.image(img, gx, gy, gw, gh);
        pg.stroke_color([0, 0, 0]);
        pg.line_width(0.8);
        pg.rect(gx, gy, gw, gh, false, true);
        let x_of = |tr: f32| gx + (tr + 0.5 - tr0) / (tr1 - tr0) * gw;
        let y_of = |s: f32| gy + (s + 0.5 - s0) / (s1 - s0) * gh;
        let depth_sec = p.proc.depth_axis;
        // Axes: left time (or depth), right depth, bottom distance.
        let fs = 7.5 * k;
        pg.fill_color([0, 0, 0]);
        let yv_top = p.proc.y_of(s0 - 0.5);
        let yv_bot = p.proc.y_of(s1 - 0.5);
        let (la, lb) = if depth_sec { (yv_top, yv_bot) } else { (yv_top, yv_bot) };
        let step = nice_step((lb - la) as f64, 6.0);
        let mut v = (la as f64 / step).ceil() * step;
        while v <= lb as f64 + 1e-9 {
            let y = y_of(p.proc.sample_of(v as f32));
            pg.line([gx - 4.0, y], [gx, y]);
            pg.text_right(gx - 6.0, y + 2.5, fs, &fmt_tick(v, step), false);
            v += step;
        }
        pg.text_rotated(m + 6.0 * k, gy + gh / 2.0 + 25.0, fs, if depth_sec { "depth [m]" } else { "time [ns]" });
        if !depth_sec {
            let (da, db) = (p.depth_of_time(la), p.depth_of_time(lb));
            let step = nice_step((db - da) as f64, 6.0);
            let mut v = (da as f64 / step).ceil() * step;
            while v <= db as f64 + 1e-9 {
                let y = y_of(p.proc.sample_of(p.time_of_depth(v as f32)));
                pg.line([gx + gw, y], [gx + gw + 4.0, y]);
                pg.text(gx + gw + 6.0, y + 2.5, fs, &fmt_tick(v, step), false);
                v += step;
            }
            pg.text_rotated(pw - m - 2.0, gy + gh / 2.0 + 40.0, fs, &format!("depth [m] (v={:.3} m/ns)", p.velocity));
        }
        let d0 = p.dist_at(tr0);
        let d1 = p.dist_at((tr1 - 1.0).max(tr0));
        let step = nice_step(d1 - d0, 10.0);
        let mut d = (d0 / step).ceil() * step;
        while d <= d1 + 1e-9 {
            let x = x_of(p.trace_at_dist(d));
            pg.line([x, gy + gh], [x, gy + gh + 4.0]);
            pg.text_center(x, gy + gh + 12.0, fs, &fmt_tick(d, step), false);
            d += step;
        }
        pg.text_center(gx + gw / 2.0, gy + gh + 22.0 * k, fs, "distance [m]", false);
        // Picks on the radargram
        if app.pdf_opts.markers {
            pg.save();
            pg.clip_rect(gx, gy, gw, gh);
            let sample_of_t = |t: f32| p.proc.sample_of(if depth_sec { p.depth_of_time(t) } else { t });
            for l in app.lines.iter().filter(|l| l.profile_id == p.id) {
                let Some(t) = app.marker_types.get(l.type_idx) else { continue };
                let pts: Vec<[f32; 2]> = l.points.iter().map(|(tr, tt)| [x_of(*tr), y_of(sample_of_t(*tt))]).collect();
                pg.stroke_color([255, 255, 255]);
                pg.line_width(2.6);
                pg.polyline(&pts);
                pg.stroke_color(t.color);
                pg.line_width(1.4);
                pg.polyline(&pts);
            }
            for mk in app.markers.iter().filter(|mk| mk.profile_id == p.id && mk.trace >= tr0 && mk.trace <= tr1) {
                let Some(t) = app.marker_types.get(mk.type_idx) else { continue };
                let (x, y) = (x_of(mk.trace), y_of(sample_of_t(mk.time_ns)));
                pdf_symbol(&mut pg, x, y, 3.6 * k, t.symbol, t.color);
                pg.fill_color([255, 255, 0]);
                pg.text(x + 5.0, y - 4.0, 7.0 * k, &format!("#{}", mk.id), true);
            }
            pg.restore();
        }

        // ---------------------------------------------------------- map
        let my = gy + gh + 32.0 * k;
        let mh = ph - my - m - 4.0;
        let mw = (pw - 2.0 * m) * 0.54;
        let mx = m;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for c in &p.coords {
            x0 = x0.min(c[0]);
            x1 = x1.max(c[0]);
            y0 = y0.min(c[1]);
            y1 = y1.max(c[1]);
        }
        pg.stroke_color([0, 0, 0]);
        pg.line_width(0.8);
        if x0 <= x1 {
            let span = (x1 - x0).max(y1 - y0).max(5.0);
            let cx = (x0 + x1) / 2.0;
            let cy = (y0 + y1) / 2.0;
            let scale = ((x1 - x0).max(span * 0.3) / mw as f64).max((y1 - y0).max(span * 0.3) / mh as f64) * 1.25;
            let mf = MapFrame { x: mx, y: my, w: mw, h: mh, cx, cy, scale };
            if app.pdf_opts.basemap {
                let (bw, bh) = ((mw * 2.4) as usize, (mh * 2.4) as usize);
                if let Some(bimg) = basemap_image(app, &mf, bw, bh) {
                    let id = pdf.add_image(bw, bh, &bimg);
                    pg.image(id, mx, my, mw, mh);
                } else {
                    pg.fill_color([240, 241, 236]);
                    pg.rect(mx, my, mw, mh, true, false);
                }
            } else {
                pg.fill_color([240, 241, 236]);
                pg.rect(mx, my, mw, mh, true, false);
            }
            pg.save();
            pg.clip_rect(mx, my, mw, mh);
            // Coordinate grid
            let gstep = nice_step(mw as f64 * scale, 4.0);
            pg.stroke_color([150, 150, 150]);
            pg.line_width(0.3);
            pg.dash(2.0, 2.0);
            let wl = mf.to_world(mx, my + mh);
            let wr = mf.to_world(mx + mw, my);
            let mut gxv = (wl[0] / gstep).ceil() * gstep;
            let mut glabels = Vec::new();
            while gxv <= wr[0] {
                let a = mf.to_page(gxv, wl[1]);
                pg.line([a[0], my], [a[0], my + mh]);
                glabels.push((a[0], true, gxv));
                gxv += gstep;
            }
            let mut gyv = (wl[1] / gstep).ceil() * gstep;
            while gyv <= wr[1] {
                let a = mf.to_page(wl[0], gyv);
                pg.line([mx, a[1]], [mx + mw, a[1]]);
                glabels.push((a[1], false, gyv));
                gyv += gstep;
            }
            pg.dash(0.0, 0.0);
            // Other profiles
            for q in app.profiles.iter().filter(|q| q.id != p.id && q.coords.len() > 1) {
                let pts: Vec<[f32; 2]> = q.coords.iter().step_by((q.coords.len() / 1500).max(1)).map(|c| mf.to_page(c[0], c[1])).collect();
                pg.stroke_color([90, 90, 90]);
                pg.line_width(0.9);
                pg.polyline(&pts);
                if let Some(s) = pts.first() {
                    label(&mut pg, s[0] + 3.0, s[1] - 3.0, 6.5 * k, &crate::model::short_label(&q.name), false, [60, 60, 60]);
                }
            }
            // This profile + highlighted section
            let pts: Vec<[f32; 2]> = p.coords.iter().step_by((p.coords.len() / 2000).max(1)).map(|c| mf.to_page(c[0], c[1])).collect();
            pg.stroke_color([255, 255, 255]);
            pg.line_width(3.0);
            pg.polyline(&pts);
            let a = (tr0.max(0.0) as usize).min(p.coords.len() - 1);
            let b = (tr1.ceil() as usize).min(p.coords.len());
            if b > a + 1 {
                let sp: Vec<[f32; 2]> = p.coords[a..b].iter().step_by(((b - a) / 1500).max(1)).chain(std::iter::once(&p.coords[b - 1])).map(|c| mf.to_page(c[0], c[1])).collect();
                pg.stroke_color(ORANGE);
                pg.line_width(6.0);
                pg.polyline(&sp);
            }
            pg.stroke_color(BLUE);
            pg.line_width(1.6);
            pg.polyline(&pts);
            if let Some(s) = pts.first() {
                pg.fill_color(BLUE);
                pg.circle(s[0], s[1], 2.6, true, false);
                label(&mut pg, s[0] + 4.0, s[1] - 4.0, 7.5 * k, &format!("{} (start)", crate::model::short_label(&p.name)), true, BLUE);
            }
            // Distance ticks along the track
            let tstep = nice_step(p.length(), 6.0);
            let mut dd = tstep;
            pg.fill_color([0, 0, 0]);
            while dd < p.length() {
                if let Some(c) = p.coord_at(p.trace_at_dist(dd)) {
                    let q = mf.to_page(c[0], c[1]);
                    pg.circle(q[0], q[1], 1.3, true, false);
                    label(&mut pg, q[0] + 3.0, q[1] + 8.0, 6.0 * k, &format!("{dd:.0} m"), false, [0, 0, 0]);
                }
                dd += tstep;
            }
            if app.pdf_opts.markers {
                for l in app.lines.iter().filter(|l| l.profile_id == p.id) {
                    let Some(t) = app.marker_types.get(l.type_idx) else { continue };
                    let lp: Vec<[f32; 2]> = l.points.iter().filter_map(|(tr, _)| p.coord_at(*tr)).map(|c| mf.to_page(c[0], c[1])).collect();
                    pg.stroke_color(t.color);
                    pg.line_width(1.4);
                    pg.dash(3.0, 2.0);
                    pg.polyline(&lp);
                    pg.dash(0.0, 0.0);
                }
                for mk in &app.markers {
                    let Some(q) = app.profiles.iter().find(|q| q.id == mk.profile_id) else { continue };
                    let Some(t) = app.marker_types.get(mk.type_idx) else { continue };
                    if let Some(c) = q.coord_at(mk.trace) {
                        let s = mf.to_page(c[0], c[1]);
                        pdf_symbol(&mut pg, s[0], s[1], 3.0 * k, t.symbol, t.color);
                        label(&mut pg, s[0] + 4.0, s[1] + 7.0, 6.5 * k, &format!("#{}", mk.id), false, [0, 0, 0]);
                    }
                }
            }
            pg.restore();
            pg.stroke_color([0, 0, 0]);
            pg.line_width(0.8);
            pg.rect(mx, my, mw, mh, false, true);
            // Grid labels outside the frame
            pg.fill_color([60, 60, 60]);
            for (pos, is_x, val) in glabels {
                if is_x {
                    pg.text_center(pos, my - 2.0, 6.0 * k, &format!("{val:.0}"), false);
                } else {
                    pg.text_rotated(mx - 2.0, pos + 12.0, 6.0 * k, &format!("{val:.0}"));
                }
            }
            // Scale bar
            let geo = app.is_geographic();
            let m_per_unit = if geo { 111_320.0 * cy.to_radians().cos().max(0.01) } else { 1.0 };
            let bar_m = nice_step(mw as f64 * 0.25 * scale * m_per_unit, 1.0);
            let bar_pt = (bar_m / m_per_unit / scale) as f32;
            let bx = mx + 8.0;
            let by = my + mh - 10.0;
            pg.fill_color([255, 255, 255]);
            pg.rect(bx - 4.0, by - 12.0, bar_pt + 50.0, 18.0, true, false);
            pg.fill_color([0, 0, 0]);
            pg.rect(bx, by - 3.0, bar_pt / 2.0, 3.0, true, false);
            pg.rect(bx, by - 3.0, bar_pt, 3.0, false, true);
            pg.text(bx + bar_pt + 4.0, by, 7.0 * k, &if bar_m >= 1.0 { format!("{bar_m:.0} m") } else { format!("{bar_m:.2} m") }, false);
            // North arrow
            let (nx, ny) = (mx + mw - 16.0, my + 14.0);
            pg.fill_color([0, 0, 0]);
            pg.polygon(&[[nx, ny - 8.0], [nx + 5.0, ny + 7.0], [nx, ny + 3.0], [nx - 5.0, ny + 7.0]], true, false);
            pg.text_center(nx, ny + 16.0, 7.0 * k, "N", true);
            label(&mut pg, mx + 4.0, my + 9.0, 6.5 * k, &app.project_crs.label(), false, [40, 40, 40]);
        } else {
            pg.fill_color([245, 245, 245]);
            pg.rect(mx, my, mw, mh, true, true);
            pg.fill_color([90, 90, 90]);
            pg.text_center(mx + mw / 2.0, my + mh / 2.0, 10.0 * k, "No GPS track for this profile", false);
        }

        // ---------------------------------------------------------- info block
        let ix = mx + mw + 16.0 * k;
        let iw = pw - m - ix;
        let mut iy = my + 6.0;
        let lh = 9.6 * k;
        let ifs = 7.4 * k;
        pg.fill_color([30, 50, 80]);
        pg.text(ix, iy, 9.0 * k, "Profile", true);
        iy += lh * 1.2;
        let sec_a = p.coord_at(tr0);
        let sec_b = p.coord_at((tr1 - 1.0).max(tr0));
        let rows: Vec<(&str, String)> = vec![
            ("File", p.path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string()),
            ("Format", p.format.clone()),
            ("Traces / samples", format!("{} / {}", p.raw.ntr, p.raw.ns)),
            ("Sample interval", format!("{:.4} ns  (window {:.1} ns)", p.raw.dt, p.raw.dt * p.raw.ns as f32)),
            ("Antenna", if p.freq_mhz > 0.0 { format!("{:.0} MHz", p.freq_mhz) } else { "-".into() }),
            ("Velocity", format!("{:.4} m/ns  (eps {:.1})", p.velocity, (0.2998 / p.velocity).powi(2))),
            ("Profile length", format!("{:.2} m", p.length())),
            ("This section", format!("{:.2} - {:.2} m (traces {:.0}-{:.0})", p.dist_at(tr0), p.dist_at((tr1 - 1.0).max(tr0)), tr0 + 1.0, tr1)),
            ("Section start", sec_a.map(|c| format!("E {:.2}  N {:.2}", c[0], c[1])).unwrap_or("-".into())),
            ("Section end", sec_b.map(|c| format!("E {:.2}  N {:.2}", c[0], c[1])).unwrap_or("-".into())),
            ("GPS", p.gps.as_ref().map(|g| g.source.clone()).unwrap_or("none".into())),
        ];
        for (k2, v2) in rows {
            pg.fill_color([90, 90, 90]);
            pg.text(ix, iy, ifs, k2, false);
            pg.fill_color([0, 0, 0]);
            pg.text(ix + 78.0 * k, iy, ifs, &v2, false);
            iy += lh;
        }
        if app.pdf_opts.processing {
            iy += lh * 0.4;
            pg.fill_color([30, 50, 80]);
            pg.text(ix, iy, 9.0 * k, "Processing", true);
            iy += lh * 1.1;
            let steps: Vec<_> = p.steps.iter().filter(|e| e.enabled).collect();
            if steps.is_empty() {
                pg.fill_color([0, 0, 0]);
                pg.text(ix, iy, ifs, "raw data", false);
                iy += lh;
            }
            for (n, e) in steps.iter().enumerate() {
                if iy > ph - m - lh * 6.0 {
                    break;
                }
                pg.fill_color([0, 0, 0]);
                pg.text(ix, iy, ifs, &format!("{}. {} - {}", n + 1, e.step.name(), e.step.summary()), false);
                iy += lh;
            }
            // Picks in this section
            let picks: Vec<_> = app.markers.iter().filter(|mk| mk.profile_id == p.id && mk.trace >= tr0 && mk.trace <= tr1).collect();
            if !picks.is_empty() && iy < ph - m - lh * 3.0 {
                iy += lh * 0.4;
                pg.fill_color([30, 50, 80]);
                pg.text(ix, iy, 9.0 * k, "Markers in this section", true);
                iy += lh * 1.1;
                let cols = [0.0, 18.0, 92.0, 128.0, 160.0, 210.0];
                pg.fill_color([90, 90, 90]);
                for (c, h) in cols.iter().zip(["#", "type", "dist m", "depth m", "E", "N"]) {
                    pg.text(ix + c * k, iy, ifs, h, true);
                }
                iy += lh;
                for mk in picks {
                    if iy > ph - m - 2.0 {
                        break;
                    }
                    let t = app.marker_types.get(mk.type_idx);
                    let depth = mk.velocity.unwrap_or(p.velocity) * mk.time_ns / 2.0;
                    let c = p.coord_at(mk.trace);
                    if let Some(t) = t {
                        pdf_symbol(&mut pg, ix + 92.0 * k - 6.0, iy - 2.5, 2.3, t.symbol, t.color);
                    }
                    pg.fill_color([0, 0, 0]);
                    let vals = [
                        mk.id.to_string(),
                        t.map(|t| t.name.chars().take(16).collect::<String>()).unwrap_or_default(),
                        format!("{:.2}", p.dist_at(mk.trace)),
                        format!("{depth:.2}"),
                        c.map(|c| format!("{:.2}", c[0])).unwrap_or("-".into()),
                        c.map(|c| format!("{:.2}", c[1])).unwrap_or("-".into()),
                    ];
                    for (cx2, v2) in cols.iter().zip(vals.iter()) {
                        pg.text(ix + cx2 * k, iy, ifs, v2, false);
                    }
                    iy += lh;
                }
            }
        }
        let _ = iw;
        // Legend under the map title line
        pg.stroke_color(ORANGE);
        pg.line_width(5.0);
        pg.line([mx, my - 14.0 * k], [mx + 18.0, my - 14.0 * k]);
        pg.fill_color([0, 0, 0]);
        pg.text(mx + 22.0, my - 11.5 * k, 7.0 * k, "section shown above", false);
        pg.stroke_color(BLUE);
        pg.line_width(1.6);
        pg.line([mx + 110.0 * k, my - 14.0 * k], [mx + 128.0 * k, my - 14.0 * k]);
        pg.text(mx + 132.0 * k, my - 11.5 * k, 7.0 * k, "profile track", false);
        pdf.add_page(pg);
    }
    pdf.write(path).map_err(|e| e.to_string())?;
    Ok(total)
}
