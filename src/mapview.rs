//! The GPS map: basemaps, profile tracks, the visible section and markers.

use crate::app::App;
use crate::basemap::{self, Detail, GRID};
use crate::draw::{self, c32, nice_step};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};

pub const SECTION_COLOR: Color32 = Color32::from_rgb(255, 140, 0);
pub const ACTIVE_COLOR: Color32 = Color32::from_rgb(25, 85, 220);

impl App {
    pub fn is_geographic(&self) -> bool {
        self.project_crs.proj4().map(|s| s.contains("longlat") || s.contains("latlong")).unwrap_or(false)
    }

    fn fit_map(&mut self, rect: Rect) {
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in &self.profiles {
            for c in &p.coords {
                if c[0].is_finite() && c[1].is_finite() {
                    x0 = x0.min(c[0]);
                    x1 = x1.max(c[0]);
                    y0 = y0.min(c[1]);
                    y1 = y1.max(c[1]);
                }
            }
        }
        if x0 > x1 {
            for b in self.basemaps.iter_mut() {
                let pts = b.mesh_cache.as_ref().filter(|(c, _)| *c == self.project_crs).map(|m| m.1.clone()).unwrap_or_else(|| basemap::mesh_vertices(&b.info, &b.crs, &self.project_crs, [0.0, b.info.width as f64, 0.0, b.info.height as f64]));
                for q in pts {
                    if q[0].is_finite() {
                        x0 = x0.min(q[0]);
                        x1 = x1.max(q[0]);
                        y0 = y0.min(q[1]);
                        y1 = y1.max(q[1]);
                    }
                }
            }
        }
        if x0 > x1 {
            return;
        }
        let w = (x1 - x0).max(1e-6);
        let h = (y1 - y0).max(1e-6);
        self.map.center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
        self.map.scale = (w / rect.width() as f64).max(h / rect.height() as f64) * 1.15;
        if self.map.scale <= 0.0 || !self.map.scale.is_finite() {
            self.map.scale = 1.0;
        }
        self.map.fitted = true;
    }

    pub fn map_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("GPS tracks");
            if ui.small_button("Fit").clicked() {
                self.map.fitted = false;
            }
            if ui.small_button("+ GeoTIFF").on_hover_text("Add a GeoTIFF basemap (any size)").clicked() {
                if let Some(fs) = rfd::FileDialog::new().add_filter("GeoTIFF", &["tif", "tiff", "TIF", "TIFF"]).pick_files() {
                    for f in fs {
                        self.add_basemap(f);
                    }
                }
            }
            if ui.small_button(self.project_crs.short()).on_hover_text(format!("{}\nClick to change the output coordinate system", self.project_crs.label())).clicked() {
                self.show_crs = true;
            }
            let label = if self.map_detached { "Dock" } else { "Own window" };
            if ui.small_button(label).clicked() {
                self.map_detached = !self.map_detached;
            }
        });
        if !self.basemaps.is_empty() {
            egui::CollapsingHeader::new(format!("Basemaps ({})", self.basemaps.len())).default_open(false).show(ui, |ui| {
                let mut remove = None;
                let crs_list: Vec<String> = self.basemaps.iter().map(|b| b.crs.short()).collect();
                for (i, b) in self.basemaps.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut b.visible, "");
                        ui.label(&b.name).on_hover_text(format!("{} x {} px\n{}\n{}", b.info.width, b.info.height, b.info.georef_source, crs_list[i]));
                        ui.add(egui::Slider::new(&mut b.opacity, 0.0..=1.0).show_value(false));
                        let mut t = match &b.crs {
                            crate::crs::CrsSpec::Epsg(c) => c.to_string(),
                            crate::crs::CrsSpec::Proj(s) => s.clone(),
                        };
                        if ui.add(egui::TextEdit::singleline(&mut t).desired_width(46.0)).on_hover_text("CRS of this GeoTIFF (EPSG)").lost_focus() {
                            if let Some(c) = crate::crs::CrsSpec::parse(&t) {
                                if c != b.crs {
                                    b.crs = c;
                                    b.mesh_cache = None;
                                    b.detail = None;
                                }
                            }
                        }
                        if ui.small_button("✖").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.basemaps.remove(i);
                }
            });
        }
        let avail = ui.available_rect_before_wrap();
        let rect = avail;
        let resp = ui.allocate_rect(rect, Sense::click_and_drag());
        if !self.map.fitted {
            self.fit_map(rect);
        }
        // Interaction
        if resp.dragged_by(egui::PointerButton::Primary) || resp.dragged_by(egui::PointerButton::Secondary) {
            let d = resp.drag_delta();
            self.map.center[0] -= d.x as f64 * self.map.scale;
            self.map.center[1] += d.y as f64 * self.map.scale;
        }
        if let Some(hp) = resp.hover_pos() {
            let (scroll, zoom) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let mut f = (-scroll as f64 * 0.0025).exp();
            if zoom != 1.0 {
                f /= zoom as f64;
            }
            if f != 1.0 {
                let wx = self.map.center[0] + (hp.x - rect.center().x) as f64 * self.map.scale;
                let wy = self.map.center[1] - (hp.y - rect.center().y) as f64 * self.map.scale;
                self.map.scale *= f;
                self.map.center[0] = wx - (hp.x - rect.center().x) as f64 * self.map.scale;
                self.map.center[1] = wy + (hp.y - rect.center().y) as f64 * self.map.scale;
            }
        }
        if resp.double_clicked() {
            self.map.fitted = false;
        }
        let (cx, cy, sc) = (self.map.center[0], self.map.center[1], self.map.scale);
        let to_s = |x: f64, y: f64| Pos2::new(rect.center().x + ((x - cx) / sc) as f32, rect.center().y - ((y - cy) / sc) as f32);
        let to_w = |p: Pos2| [cx + (p.x - rect.center().x) as f64 * sc, cy - (p.y - rect.center().y) as f64 * sc];

        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, Color32::from_rgb(236, 238, 232));

        // ---------------------------------------------------------- basemaps
        let project = self.project_crs.clone();
        let ctx = ui.ctx().clone();
        for b in self.basemaps.iter_mut() {
            if !b.visible {
                continue;
            }
            if b.tex.is_none() {
                let ci = egui::ColorImage::from_rgba_unmultiplied([b.overview.w, b.overview.h], &b.overview.rgba);
                b.tex = Some(ctx.load_texture(format!("bm-{}", b.name), ci, egui::TextureOptions::LINEAR));
            }
            if b.mesh_cache.as_ref().map(|m| m.0 != project).unwrap_or(true) {
                let v = basemap::mesh_vertices(&b.info, &b.crs, &project, [0.0, b.info.width as f64, 0.0, b.info.height as f64]);
                b.mesh_cache = Some((project.clone(), v));
            }
            let alpha = (b.opacity * 255.0) as u8;
            if let (Some(t), Some((_, verts))) = (&b.tex, &b.mesh_cache) {
                draw_mesh(&painter, t.id(), verts, &to_s, alpha);
            }
            // Detail when zoomed in beyond the overview resolution.
            let corners = [rect.left_top(), rect.right_top(), rect.left_bottom(), rect.right_bottom(), rect.center()];
            let mut pts: Vec<[f64; 3]> = corners.iter().map(|c| {
                let w = to_w(*c);
                [w[0], w[1], 0.0]
            }).collect();
            let _ = crate::crs::transform_spec(&project, &b.crs, &mut pts);
            let px: Vec<[f64; 2]> = pts.iter().filter(|p| p[0].is_finite()).map(|p| b.info.world_to_pixel(p[0], p[1])).collect();
            if px.len() >= 4 {
                let c0 = px.iter().map(|p| p[0]).fold(f64::MAX, f64::min).max(0.0);
                let c1 = px.iter().map(|p| p[0]).fold(f64::MIN, f64::max).min(b.info.width as f64);
                let r0 = px.iter().map(|p| p[1]).fold(f64::MAX, f64::min).max(0.0);
                let r1 = px.iter().map(|p| p[1]).fold(f64::MIN, f64::max).min(b.info.height as f64);
                let full_per_screen = (c1 - c0) / rect.width() as f64;
                if c1 > c0 && r1 > r0 && full_per_screen < b.ov_factor * 0.7 {
                    let q = ((c1 - c0).max(r1 - r0) / 4.0).max(1.0);
                    let key = [(c0 / q).floor() as i64, (r0 / q).floor() as i64, (q.log2() * 2.0).round() as i64, 0];
                    let have = b.detail.as_ref().map(|d| d.key == key).unwrap_or(false);
                    let pending = b.detail_pending.as_ref().map(|p| p.0 == key).unwrap_or(false);
                    if !have && !pending {
                        // Read a window a bit larger than the view.
                        let mx = (c1 - c0) * 0.25;
                        let my = (r1 - r0) * 0.25;
                        let win = [(c0 - mx).max(0.0), (c1 + mx).min(b.info.width as f64), (r0 - my).max(0.0), (r1 + my).min(b.info.height as f64)];
                        let ppp = ctx.pixels_per_point();
                        let ww = (rect.width() * ppp * 1.5) as usize;
                        let hh = ((win[3] - win[2]) / (win[1] - win[0]) * ww as f64) as usize;
                        let (tx, rx) = std::sync::mpsc::channel();
                        let path = b.path.clone();
                        let style = b.style.clone();
                        let c2 = ctx.clone();
                        std::thread::spawn(move || {
                            let r = basemap::read_detail(&path, &style, win[0], win[1], win[2], win[3], ww, hh.max(1)).map(|r| (r, win));
                            let _ = tx.send(r);
                            c2.request_repaint();
                        });
                        b.detail_pending = Some((key, rx));
                    }
                }
            }
            if let Some((key, rx)) = &b.detail_pending {
                if let Ok(r) = rx.try_recv() {
                    let key = *key;
                    b.detail_pending = None;
                    if let Ok((raster, win)) = r {
                        let ci = egui::ColorImage::from_rgba_unmultiplied([raster.w, raster.h], &raster.rgba);
                        let tex = ctx.load_texture(format!("bmd-{}", b.name), ci, egui::TextureOptions::LINEAR);
                        b.detail = Some(Detail { window: win, key, raster, tex: Some(tex), mesh: None });
                    }
                }
            }
            if let Some(d) = &mut b.detail {
                if d.mesh.as_ref().map(|m| m.0 != project).unwrap_or(true) {
                    d.mesh = Some((project.clone(), basemap::mesh_vertices(&b.info, &b.crs, &project, d.window)));
                }
                // Only draw the detail while it is finer than the overview on screen.
                let full_per_screen = (d.window[1] - d.window[0]) / (d.raster.w as f64).max(1.0);
                let _ = full_per_screen;
                if let (Some(t), Some((_, verts))) = (&d.tex, &d.mesh) {
                    let a = to_s(verts[0][0], verts[0][1]);
                    let z = to_s(verts[verts.len() - 1][0], verts[verts.len() - 1][1]);
                    let span = (z - a).length();
                    if span > rect.width() * 0.5 && span < rect.width() * 8.0 {
                        draw_mesh(&painter, t.id(), verts, &to_s, alpha);
                    }
                }
            }
        }

        // ---------------------------------------------------------- tracks
        let active_id = self.active_profile().map(|p| p.id);
        let mut hover_best: Option<(f32, u32, f32)> = None;
        let hp = resp.hover_pos();
        let mut drawn_labels: Vec<(String, Pos2)> = Vec::new();
        for p in &self.profiles {
            if p.coords.len() < 2 {
                continue;
            }
            let n = p.coords.len();
            let step = (n / 3000).max(1);
            let pts: Vec<Pos2> = p.coords.iter().step_by(step).chain(std::iter::once(&p.coords[n - 1])).map(|c| to_s(c[0], c[1])).collect();
            let is_active = Some(p.id) == active_id;
            if is_active {
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(4.5, Color32::WHITE)));
                // Visible section of the data view
                if let Some(v) = self.views.get(&p.id) {
                    let a = v.tr0.max(0.0) as usize;
                    let b = (v.tr1.ceil() as usize).min(n);
                    if b > a + 1 {
                        let st = ((b - a) / 2000).max(1);
                        let sp: Vec<Pos2> = p.coords[a..b].iter().step_by(st).chain(std::iter::once(&p.coords[b - 1])).map(|c| to_s(c[0], c[1])).collect();
                        painter.add(egui::Shape::line(sp.clone(), Stroke::new(9.0, SECTION_COLOR)));
                        for e in [sp[0], *sp.last().unwrap()] {
                            painter.circle_filled(e, 5.0, SECTION_COLOR);
                            painter.circle_stroke(e, 5.0, Stroke::new(1.0, Color32::BLACK));
                        }
                    }
                }
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(2.5, ACTIVE_COLOR)));
            } else {
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.5, Color32::WHITE)));
                painter.add(egui::Shape::line(pts.clone(), Stroke::new(1.8, Color32::from_rgb(70, 70, 70))));
            }
            // Start marker, direction and label
            let s = pts[0];
            painter.circle_filled(s, 4.0, if is_active { ACTIVE_COLOR } else { Color32::from_gray(70) });
            if pts.len() > 2 {
                let e = pts[pts.len() - 1];
                let pre = pts[pts.len() - 2];
                let dir = (e - pre).normalized();
                let nrm = egui::vec2(-dir.y, dir.x);
                let tri = vec![e + dir * 7.0, e - dir * 3.0 + nrm * 5.0, e - dir * 3.0 - nrm * 5.0];
                painter.add(egui::Shape::convex_polygon(tri, if is_active { ACTIVE_COLOR } else { Color32::from_gray(70) }, Stroke::NONE));
            }
            // Only the profile number; channels of one profile (same number, same place) get one label.
            let short = crate::model::short_label(&p.name);
            if !drawn_labels.iter().any(|(t, q): &(String, Pos2)| *t == short && q.distance(s) < 25.0) {
                let galley = painter.layout_no_wrap(short.clone(), FontId::proportional(11.0), if is_active { ACTIVE_COLOR } else { Color32::BLACK });
                let lr = Rect::from_min_size(s + egui::vec2(6.0, -14.0), galley.size()).expand(2.0);
                painter.rect_filled(lr, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 210));
                painter.galley(lr.min + egui::vec2(2.0, 2.0), galley, Color32::BLACK);
                drawn_labels.push((short, s));
            }
            // Hover: nearest trace
            if let Some(h) = hp {
                for (i, c) in p.coords.iter().enumerate().step_by(((n / 4000).max(1)) as usize) {
                    let d = to_s(c[0], c[1]).distance(h);
                    if d < 12.0 && hover_best.map(|b| d < b.0).unwrap_or(true) {
                        hover_best = Some((d, p.id, i as f32));
                    }
                }
            }
        }
        // Layer picks on the map
        for l in &self.lines {
            let Some(p) = self.profiles.iter().find(|p| p.id == l.profile_id) else { continue };
            let Some(t) = self.marker_types.get(l.type_idx) else { continue };
            if !p.has_coords() {
                continue;
            }
            let pts: Vec<Pos2> = l.points.iter().filter_map(|(tr, _)| p.coord_at(*tr)).map(|c| to_s(c[0], c[1])).collect();
            painter.add(egui::Shape::dashed_line(&pts, Stroke::new(3.0, c32(t.color)), 6.0, 3.0));
        }
        // Markers
        for m in &self.markers {
            let Some(p) = self.profiles.iter().find(|p| p.id == m.profile_id) else { continue };
            let Some(t) = self.marker_types.get(m.type_idx) else { continue };
            if let Some(c) = p.coord_at(m.trace) {
                let at = to_s(c[0], c[1]);
                if rect.expand(10.0).contains(at) {
                    draw::symbol(&painter, at, 5.0, t.symbol, c32(t.color), self.selected_pick == Some(m.id));
                    painter.text(at + egui::vec2(7.0, 4.0), Align2::LEFT_TOP, format!("#{}", m.id), FontId::proportional(10.0), Color32::BLACK);
                }
            }
        }
        // Cursor from the data view
        if let (Some(p), Some((tr, _))) = (self.active_profile(), self.hover) {
            if let Some(c) = p.coord_at(tr) {
                let at = to_s(c[0], c[1]);
                painter.circle_filled(at, 5.5, Color32::from_rgb(230, 30, 30));
                painter.circle_stroke(at, 5.5, Stroke::new(1.5, Color32::WHITE));
            }
        }
        // Hover on the map
        self.map.hover_trace = None;
        if let Some((_, pid, tr)) = hover_best {
            self.map.hover_trace = Some((pid, tr));
            if let Some(p) = self.profiles.iter().find(|p| p.id == pid) {
                if let Some(c) = p.coord_at(tr) {
                    let at = to_s(c[0], c[1]);
                    painter.circle_stroke(at, 7.0, Stroke::new(2.0, Color32::from_rgb(230, 30, 30)));
                    painter.text(at + egui::vec2(10.0, 8.0), Align2::LEFT_TOP, format!("{}  trace {:.0}  {:.1} m", p.name, tr + 1.0, p.dist_at(tr)), FontId::proportional(11.0), Color32::BLACK);
                }
            }
            if resp.clicked() {
                self.focus_trace(pid, tr);
            }
        }
        self.map.hover_world = hp.map(to_w);

        // ---------------------------------------------------------- decorations
        if self.profiles.iter().all(|p| !p.has_coords()) && self.basemaps.is_empty() {
            painter.text(rect.center(), Align2::CENTER_CENTER, "No GPS tracks yet.\nImport profiles with GPS, or add a GeoTIFF basemap.", FontId::proportional(13.0), Color32::DARK_GRAY);
        }
        // Scale bar
        let geo = self.is_geographic();
        let m_per_unit = if geo { 111_320.0 * (cy.to_radians().cos()).max(0.01) } else { 1.0 };
        let target_m = sc * m_per_unit * rect.width() as f64 / 5.0;
        let bar_m = nice_step(target_m, 1.0);
        let bar_px = (bar_m / m_per_unit / sc) as f32;
        let b0 = Pos2::new(rect.left() + 14.0, rect.bottom() - 16.0);
        painter.rect_filled(Rect::from_min_max(b0 + egui::vec2(-6.0, -20.0), b0 + egui::vec2(bar_px + 40.0, 8.0)), 3.0, Color32::from_rgba_unmultiplied(255, 255, 255, 200));
        painter.rect_filled(Rect::from_min_max(b0 + egui::vec2(0.0, -4.0), b0 + egui::vec2(bar_px / 2.0, 0.0)), 0.0, Color32::BLACK);
        painter.rect_stroke(Rect::from_min_max(b0 + egui::vec2(0.0, -4.0), b0 + egui::vec2(bar_px, 0.0)), 0.0, Stroke::new(1.0, Color32::BLACK), egui::StrokeKind::Middle);
        let lbl = if bar_m >= 1000.0 { format!("{:.0} km", bar_m / 1000.0) } else if bar_m >= 1.0 { format!("{bar_m:.0} m") } else { format!("{bar_m:.2} m") };
        painter.text(b0 + egui::vec2(bar_px + 4.0, -2.0), Align2::LEFT_CENTER, lbl, FontId::proportional(11.0), Color32::BLACK);
        // North arrow
        let na = Pos2::new(rect.right() - 22.0, rect.top() + 30.0);
        painter.add(egui::Shape::convex_polygon(vec![na + egui::vec2(0.0, -16.0), na + egui::vec2(7.0, 6.0), na, na + egui::vec2(-7.0, 6.0)], Color32::BLACK, Stroke::new(1.0, Color32::BLACK)));
        painter.text(na + egui::vec2(0.0, 8.0), Align2::CENTER_TOP, "N", FontId::proportional(12.0), Color32::BLACK);
        // CRS and coordinates
        painter.text(rect.left_top() + egui::vec2(6.0, 4.0), Align2::LEFT_TOP, self.project_crs.label(), FontId::proportional(11.0), Color32::from_gray(60));
        if let Some(w) = self.map.hover_world {
            let s = if geo { format!("{:.6}°, {:.6}°", w[1], w[0]) } else { format!("E {:.2}  N {:.2}", w[0], w[1]) };
            painter.text(rect.right_bottom() - egui::vec2(6.0, 4.0), Align2::RIGHT_BOTTOM, s, FontId::monospace(11.0), Color32::BLACK);
        }
        // Legend for the highlight
        if active_id.is_some() {
            let lp = rect.left_top() + egui::vec2(6.0, 20.0);
            painter.line_segment([lp + egui::vec2(0.0, 6.0), lp + egui::vec2(22.0, 6.0)], Stroke::new(7.0, SECTION_COLOR));
            painter.text(lp + egui::vec2(28.0, 6.0), Align2::LEFT_CENTER, "section shown in the data view", FontId::proportional(10.5), Color32::from_gray(50));
        }
    }
}

fn draw_mesh(painter: &egui::Painter, tex: egui::TextureId, verts: &[[f64; 2]], to_s: &dyn Fn(f64, f64) -> Pos2, alpha: u8) {
    if verts.iter().any(|v| !v[0].is_finite() || !v[1].is_finite()) {
        return;
    }
    let mut mesh = egui::Mesh::with_texture(tex);
    let col = Color32::from_white_alpha(alpha);
    for j in 0..=GRID {
        for i in 0..=GRID {
            let v = verts[j * (GRID + 1) + i];
            mesh.vertices.push(egui::epaint::Vertex { pos: to_s(v[0], v[1]), uv: Pos2::new(i as f32 / GRID as f32, j as f32 / GRID as f32), color: col });
        }
    }
    let w = (GRID + 1) as u32;
    for j in 0..GRID as u32 {
        for i in 0..GRID as u32 {
            let a = j * w + i;
            mesh.add_triangle(a, a + 1, a + w);
            mesh.add_triangle(a + 1, a + w + 1, a + w);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
}
