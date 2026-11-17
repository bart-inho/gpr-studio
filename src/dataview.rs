//! The radargram view: image, axes, zoom/pan and picking.

use crate::app::{App, HyperbolaFit, PickMode, ViewState, YAxis};
use crate::draw::{self, c32, fmt_tick, nice_step};
use crate::model::{LinePick, Marker, MarkerKind, Profile};
use crate::processing::{snap_sample, Snap};
use egui::{Align2, Color32, FontId, Pos2, Rect, Sense, Stroke};

struct Frame {
    plot: Rect,
    reversed: bool,
    tr0: f32,
    tr1: f32,
    s0: f32,
    s1: f32,
}

impl Frame {
    fn x_of_trace(&self, tr: f32) -> f32 {
        let x = (tr + 0.5 - self.tr0) / (self.tr1 - self.tr0);
        self.plot.left() + (if self.reversed { 1.0 - x } else { x }) * self.plot.width()
    }
    fn trace_of_x(&self, x: f32) -> f32 {
        let x = (x - self.plot.left()) / self.plot.width();
        self.tr0 + (if self.reversed { 1.0 - x } else { x }) * (self.tr1 - self.tr0) - 0.5
    }
    fn y_of_sample(&self, s: f32) -> f32 {
        self.plot.top() + (s + 0.5 - self.s0) / (self.s1 - self.s0) * self.plot.height()
    }
    fn sample_of_y(&self, y: f32) -> f32 {
        self.s0 + (y - self.plot.top()) / self.plot.height() * (self.s1 - self.s0) - 0.5
    }
}

#[cfg(test)]
mod orientation_tests {
    use super::*;

    #[test]
    fn reversed_frame_keeps_picks_on_their_traces() {
        let plot = Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(100.0, 100.0));
        let normal = Frame { plot, reversed: false, tr0: 0.0, tr1: 10.0, s0: 0.0, s1: 10.0 };
        let reversed = Frame { reversed: true, ..normal };
        assert!(normal.x_of_trace(2.0) < normal.x_of_trace(7.0));
        assert!(reversed.x_of_trace(2.0) > reversed.x_of_trace(7.0));
        assert!((reversed.trace_of_x(reversed.x_of_trace(2.0)) - 2.0).abs() < 1e-5);
    }
}

#[cfg(test)]
mod undo_tests {
    use super::*;

    fn app() -> App {
        let mut app = App::headless(vec![], None);
        app.add_profile(crate::demo::make().remove(0));
        app
    }

    #[test]
    fn undo_restores_deleted_point_then_undoes_its_placement() {
        let mut app = app();
        app.apply_action(0, Action::Point(5.0, 3.0, 1.0));
        let id = app.markers[0].id;
        app.delete_with_undo(&[id], false);
        assert!(app.markers.is_empty());
        app.undo_pick();
        assert_eq!(app.markers.len(), 1);
        app.undo_pick();
        assert!(app.markers.is_empty());
        assert_eq!(app.next_pick_id, id);
    }

    #[test]
    fn undo_walks_back_through_layer_coordinates() {
        let mut app = app();
        app.auto_track = false;
        app.apply_action(0, Action::LayerPoint(4, 10.0, false));
        app.apply_action(0, Action::LayerPoint(8, 12.0, false));
        app.finish_line();
        assert_eq!(app.lines.len(), 1);
        app.undo_pick();
        assert_eq!(app.open_line.as_ref().unwrap().points.len(), 2);
        app.undo_pick();
        assert_eq!(app.open_line.as_ref().unwrap().points.len(), 1);
        app.undo_pick();
        assert!(app.open_line.is_none());
        assert!(app.lines.is_empty());
    }

    #[test]
    fn undo_saved_hyperbola_restores_fit() {
        let mut app = app();
        app.apply_action(0, Action::Apex(5.0, 10.0));
        app.save_hyperbola();
        assert_eq!(app.markers.len(), 1);
        app.undo_pick();
        assert!(app.markers.is_empty());
        assert!(app.hyperbola.is_some());
        app.undo_pick();
        assert!(app.hyperbola.is_none());
    }

    #[test]
    fn one_undo_restores_box_deleted_picks_and_fit() {
        let mut app = app();
        app.apply_action(0, Action::Point(5.0, 3.0, 1.0));
        let id = app.markers[0].id;
        app.apply_action(0, Action::Apex(6.0, 10.0));
        app.delete_with_undo(&[id], true);
        assert!(app.markers.is_empty() && app.hyperbola.is_none());
        app.undo_pick();
        assert_eq!(app.markers.len(), 1);
        assert!(app.hyperbola.is_some());
    }
}

impl App {
    fn snap_at(&self, p: &Profile, tr: usize, s: f32) -> f32 {
        if tr >= p.proc.ntr {
            return s;
        }
        snap_sample(p.proc.trace(tr), s, self.snap_win, self.snap)
    }

    pub fn dataview_ui(&mut self, ui: &mut egui::Ui) {
        let Some(a) = self.active else {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.3);
                ui.heading("GPR Studio");
                ui.label("Ground-penetrating radar processing, picking, mapping and reporting.");
                ui.add_space(10.0);
                ui.label("File > Open GPR files… reads GSSI .dzt, MALA .rd3/.rd7, ImpulseRadar .iprb, Sensors & Software .dt1, SEG-Y (IDS, Proceq…) and REFLEXW files.");
                ui.label("Or try File > Load demo survey.");
            });
            return;
        };
        if !ui.input(|i| i.pointer.button_down(egui::PointerButton::Primary)) { self.velocity_drag_active = false; }
        let pid = self.profiles[a].id;
        let reversed = self.profiles[a].reverse_display;
        // Topography: per-trace shift (display samples) and the shifted section to draw.
        let migrated = self.profiles[a].proc.depth_datum_m.is_some();
        let topo = if migrated { None } else { self.profiles[a].topo_shifts() };
        let sh: Vec<f32> = topo.as_ref().map(|t| t.1.clone()).unwrap_or_default();
        let zmax = self.profiles[a].proc.depth_datum_m.or_else(|| topo.as_ref().map(|t| t.0));
        let max_shift = sh.iter().fold(0f32, |m, v| m.max(*v)).ceil() as usize;
        if topo.is_some() {
            let p = &self.profiles[a];
            let key = format!("{}-{}-{:.5}-{}", p.id, p.version, p.velocity, p.coords.len());
            if self.topo_cache.as_ref().map(|c| c.0 != key).unwrap_or(true) {
                let s = &p.proc;
                let mut d = crate::model::Section { ns: s.ns + max_shift, ntr: s.ntr, dt: s.dt, t0: s.t0, depth_axis: s.depth_axis, depth_datum_m: s.depth_datum_m, data: vec![f32::NAN; (s.ns + max_shift) * s.ntr] };
                for t in 0..s.ntr {
                    let k = sh[t].round().max(0.0) as usize;
                    let ns2 = d.ns;
                    d.data[t * ns2 + k..t * ns2 + k + s.ns].copy_from_slice(s.trace(t));
                }
                self.topo_cache = Some((key, d));
            }
        }
        let shift = |tr: f32| -> f32 { if sh.is_empty() { 0.0 } else { sh[(tr.round().max(0.0) as usize).min(sh.len() - 1)] } };
        let (ntr, ns) = (self.profiles[a].proc.ntr as f32, (self.profiles[a].proc.ns + if topo.is_some() { max_shift } else { 0 }) as f32);
        let mut view = self.views.get(&pid).cloned().unwrap_or(ViewState { tr0: 0.0, tr1: ntr, s0: 0.0, s1: ns });
        // Topography switched on or off: the section height changed, show it all.
        let topo_now = topo.is_some();
        if self.topo_shown.insert(pid, topo_now).map(|was| was != topo_now).unwrap_or(topo_now) {
            view.s0 = 0.0;
            view.s1 = ns;
        }

        // Header line
        {
            let p = &self.profiles[a];
            ui.horizontal(|ui| {
                ui.strong(&p.name);
                ui.weak(format!(
                    "{} · {} traces · {} samples · dt {:.3} {} · {:.1} m{}",
                    p.format,
                    p.proc.ntr,
                    p.proc.ns,
                    p.proc.dt,
                    if p.proc.depth_axis { "m" } else { "ns" },
                    p.axis_length(),
                    if p.has_coords() { " · GPS" } else { " · no GPS" }
                ));
                let ed = p.steps.iter().filter(|e| e.enabled).count();
                if ed > 0 {
                    ui.weak(format!("· {ed} processing steps"));
                }
            });
        }
        if migrated {
            ui.label("Topographic migration: depth below highest antenna. Picks are stored as acquisition time below the local surface.");
        }

        let avail = ui.available_rect_before_wrap();
        let ascan_w = 110.0;
        let left = 58.0;
        let right = 58.0;
        let top = 38.0;
        let bottom = 36.0;
        let plot = Rect::from_min_max(avail.min + egui::vec2(left, top), avail.max - egui::vec2(right + ascan_w + 8.0, bottom));
        if plot.width() < 50.0 || plot.height() < 50.0 {
            return;
        }
        self.plot_px = [plot.width(), plot.height()];
        if self.fit_one_to_one.remove(&pid) {
            // First display after loading: 1x1 from the start of the profile as displayed (left edge).
            let start = if reversed { ntr } else { 0.0 };
            if let Some(v) = self.one_to_one(a, &ViewState { tr0: start, tr1: start, s0: 0.0, s1: ns }) {
                view = v;
            }
        }
        let resp = ui.interact(plot, ui.id().with("dataview"), Sense::click_and_drag());
        ui.allocate_rect(avail, Sense::hover());

        // ---------------------------------------------------------- interaction: zoom / pan
        let hover_pos = resp.hover_pos();
        let (scroll, zoom, mods) = ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.modifiers));
        if let Some(hp) = hover_pos {
            let fr = Frame { plot, reversed, tr0: view.tr0, tr1: view.tr1, s0: view.s0, s1: view.s1 };
            let ct = fr.trace_of_x(hp.x) + 0.5;
            let cs = fr.sample_of_y(hp.y) + 0.5;
            let mut fx = 1.0f32;
            let mut fy = 1.0f32;
            // Shift + wheel crops the time axis; Ctrl + wheel zooms horizontally.
            // Otherwise the main wheel scrolls vertically and the side wheel horizontally.
            let ctrl = mods.ctrl || mods.command;
            let wheel = scroll.x + scroll.y; // Shift turns the wheel into a horizontal scroll
            if ctrl {
                if zoom != 1.0 {
                    fx /= zoom;
                } else if wheel != 0.0 {
                    fx *= (-wheel * 0.0025).exp();
                }
            } else if mods.shift {
                if wheel != 0.0 {
                    // Crop from time zero: the top stays at 0, only the bottom moves.
                    view.s0 = 0.0;
                    view.s1 = (view.s1 * (-wheel * 0.0025).exp()).clamp(4.0, ns.max(4.0));
                }
            } else {
                let dtr = (if reversed { scroll.x } else { -scroll.x }) / plot.width() * (view.tr1 - view.tr0);
                let ds = -scroll.y / plot.height() * (view.s1 - view.s0);
                view.tr0 += dtr;
                view.tr1 += dtr;
                view.s0 += ds;
                view.s1 += ds;
            }
            if fx != 1.0 {
                view.tr0 = ct - (ct - view.tr0) * fx;
                view.tr1 = ct + (view.tr1 - ct) * fx;
            }
            if fy != 1.0 {
                view.s0 = cs - (cs - view.s0) * fy;
                view.s1 = cs + (view.s1 - cs) * fy;
            }
        }
        let selecting = self.select_rect.is_some() || (mods.shift && resp.drag_started_by(egui::PointerButton::Secondary));
        let pan_drag = (resp.dragged_by(egui::PointerButton::Secondary) && !selecting) || resp.dragged_by(egui::PointerButton::Middle) || (self.mode == PickMode::Navigate && resp.dragged_by(egui::PointerButton::Primary)) || (self.mode == PickMode::Layer && resp.dragged_by(egui::PointerButton::Primary));
        if pan_drag {
            let d = resp.drag_delta();
            let dtr = (if reversed { d.x } else { -d.x }) / plot.width() * (view.tr1 - view.tr0);
            let ds = -d.y / plot.height() * (view.s1 - view.s0);
            view.tr0 += dtr;
            view.tr1 += dtr;
            view.s0 += ds;
            view.s1 += ds;
        }
        if resp.double_clicked() && self.mode == PickMode::Navigate {
            view = ViewState { tr0: 0.0, tr1: ntr, s0: 0.0, s1: ns };
        }
        // Clamp
        // Horizontally the view may be wider than the profile (short profiles get small);
        // the unused part stays grey and the profile starts at the left edge.
        let wtr = (view.tr1 - view.tr0).clamp(4.0, ntr.max(4.0) * 50.0);
        let wtr = if wtr.is_finite() { wtr } else { ntr };
        if wtr > ntr {
            view.tr0 = 0.0;
            view.tr1 = wtr;
        } else {
            let c = ((view.tr0 + view.tr1) / 2.0).clamp(wtr / 2.0 - ntr * 0.02, ntr - wtr / 2.0 + ntr * 0.02);
            view.tr0 = c - wtr / 2.0;
            view.tr1 = c + wtr / 2.0;
        }
        let ws = (view.s1 - view.s0).clamp(4.0, ns.max(4.0));
        let cs = ((view.s0 + view.s1) / 2.0).clamp(ws / 2.0, (ns - ws / 2.0).max(ws / 2.0));
        view.s0 = cs - ws / 2.0;
        view.s1 = cs + ws / 2.0;
        self.views.insert(pid, view.clone());
        // Zoomed out beyond the profile: the plot gets narrower from the right at the same
        // scale (no stretching), so a 2 m profile can be shown 2 m wide.
        let full_plot = plot;
        let (plot, view) = if view.tr1 - view.tr0 > ntr {
            let w = (plot.width() * ntr / (view.tr1 - view.tr0)).max(8.0);
            (Rect::from_min_size(plot.min, egui::vec2(w, plot.height())), ViewState { tr0: 0.0, tr1: ntr, s0: view.s0, s1: view.s1 })
        } else {
            (plot, view)
        };
        let _ = full_plot;
        let fr = Frame { plot, reversed, tr0: view.tr0, tr1: view.tr1, s0: view.s0, s1: view.s1 };

        // ---------------------------------------------------------- image
        let p = &self.profiles[a];
        let clip = *self.clip_cache.entry((pid, p.version)).or_insert_with(|| crate::render::clip_level(&p.proc, 1.0));
        let clip = clip / self.contrast.max(0.01);
        let ppp = ui.ctx().pixels_per_point();
        let (w, h) = (((plot.width() * ppp) as usize).max(1), ((plot.height() * ppp) as usize).max(1));
        let key = format!("{pid}-{}-{:.3}-{:.3}-{:.3}-{:.3}-{w}-{h}-{:?}-{:.4}-{}", p.version, view.tr0, view.tr1, view.s0, view.s1, self.colormap, clip, self.topo_cache.as_ref().filter(|_| topo.is_some()).map(|c| c.0.as_str()).unwrap_or("flat"));
        if key != self.section_key || self.section_tex.is_none() {
            let src_sec = if topo.is_some() { self.topo_cache.as_ref().map(|c| &c.1).unwrap_or(&p.proc) } else { &p.proc };
            let img = crate::render::render(src_sec, view.tr0, view.tr1, view.s0, view.s1, w, h, self.colormap, clip);
            let ci = egui::ColorImage::from_rgba_unmultiplied([w, h], &img);
            match &mut self.section_tex {
                Some(t) => t.set(ci, egui::TextureOptions::NEAREST),
                None => self.section_tex = Some(ui.ctx().load_texture("section", ci, egui::TextureOptions::NEAREST)),
            }
            self.section_key = key;
        }
        let painter = ui.painter_at(avail);
        painter.rect_filled(plot.expand(1.0), 0.0, Color32::from_gray(72));
        if let Some(t) = &self.section_tex {
            let uv = if reversed { Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0)) } else { Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)) };
            painter.image(t.id(), plot, uv, Color32::WHITE);
        }
        let p = &self.profiles[a];
        let plot_painter = ui.painter_at(plot);


        // ---------------------------------------------------------- axes
        let axis_col = ui.visuals().strong_text_color();
        let font = FontId::proportional(11.0);
        let depth_sec = p.proc.depth_axis;
        let y_top = p.proc.y_of(view.s0 - 0.5);
        let y_bot = p.proc.y_of(view.s1 - 0.5);
        let primary_depth = depth_sec || self.y_axis == YAxis::Depth;
        // Values of the y axis as functions of sample index
        let to_time = |y: f32| if depth_sec { p.time_of_depth(y) } else { y };
        let to_depth = |y: f32| if depth_sec { y } else { p.depth_of_time(y) };
        let y_px_of_val = |val: f32, is_depth: bool| -> f32 {
            let yv = if depth_sec {
                if is_depth { val } else { p.depth_of_time(val) }
            } else if is_depth {
                p.time_of_depth(val)
            } else {
                val
            };
            fr.y_of_sample(p.proc.sample_of(yv))
        };
        let draw_y_axis = |is_depth: bool, at_left: bool| {
            let (va, vb) = if is_depth { (to_depth(y_top), to_depth(y_bot)) } else { (to_time(y_top), to_time(y_bot)) };
            let step = nice_step((vb - va).abs() as f64, (plot.height() / 45.0) as f64);
            let mut v = (va as f64 / step).ceil() * step;
            let x = if at_left { plot.left() } else { plot.right() };
            while v <= vb as f64 + 1e-9 {
                let y = y_px_of_val(v as f32, is_depth);
                if y >= plot.top() - 0.5 && y <= plot.bottom() + 0.5 {
                    let dx = if at_left { -5.0 } else { 5.0 };
                    painter.line_segment([Pos2::new(x, y), Pos2::new(x + dx, y)], Stroke::new(1.0_f32, axis_col));
                    painter.text(Pos2::new(x + dx * 1.4, y), if at_left { Align2::RIGHT_CENTER } else { Align2::LEFT_CENTER }, fmt_tick(v, step), font.clone(), axis_col);
                }
                v += step;
            }
            let label = if migrated { "depth below datum [m]".to_string() } else if is_depth { format!("depth [m] (v={:.3})", p.velocity) } else if zmax.is_some() { "time [ns] below the highest point".to_string() } else { "time [ns]".to_string() };
            let lx = if at_left { avail.left() + 2.0 } else { plot.right() + right - 12.0 };
            let galley = painter.layout_no_wrap(label, font.clone(), axis_col);
            let pos = Pos2::new(lx, plot.center().y + galley.size().x / 2.0);
            painter.add(egui::epaint::TextShape::new(pos, galley, axis_col).with_angle(-std::f32::consts::FRAC_PI_2));
        };
        draw_y_axis(primary_depth, true);
        if let Some(zm) = zmax {
            // Elevation axis: surface at the highest point minus the depth below it.
            let (da, db) = (to_depth(y_top) as f64, to_depth(y_bot) as f64);
            let (ea, eb) = (zm - db, zm - da);
            let step = nice_step(eb - ea, (plot.height() / 45.0) as f64);
            let mut e = (ea / step).ceil() * step;
            while e <= eb + 1e-9 {
                let y = y_px_of_val((zm - e) as f32, true);
                if y >= plot.top() - 0.5 && y <= plot.bottom() + 0.5 {
                    painter.line_segment([Pos2::new(plot.right(), y), Pos2::new(plot.right() + 5.0, y)], Stroke::new(1.0_f32, axis_col));
                    painter.text(Pos2::new(plot.right() + 7.0, y), Align2::LEFT_CENTER, fmt_tick(e, step), font.clone(), axis_col);
                }
                e += step;
            }
            let galley = painter.layout_no_wrap(if migrated { "elevation [m]".to_string() } else { format!("elevation [m] (v={:.3})", p.velocity) }, font.clone(), axis_col);
            let pos = Pos2::new(plot.right() + right - 12.0, plot.center().y + galley.size().x / 2.0);
            painter.add(egui::epaint::TextShape::new(pos, galley, axis_col).with_angle(-std::f32::consts::FRAC_PI_2));
        } else {
            draw_y_axis(!primary_depth, false);
        }
        // Distance axis (bottom)
        let d0 = if reversed { p.axis_length() - p.axis_dist_at(view.tr1.min(ntr - 1.0)) } else { p.axis_dist_at(view.tr0.max(0.0)) };
        let d1 = if reversed { p.axis_length() - p.axis_dist_at(view.tr0.max(0.0)) } else { p.axis_dist_at(view.tr1.min(ntr - 1.0)) };
        let step = nice_step(d1 - d0, (plot.width() / 80.0) as f64);
        let mut d = (d0 / step).ceil() * step;
        while d <= d1 + 1e-9 {
            let tr = p.axis_trace_at_dist(if reversed { p.axis_length() - d } else { d });
            let x = fr.x_of_trace(tr);
            painter.line_segment([Pos2::new(x, plot.bottom()), Pos2::new(x, plot.bottom() + 5.0)], Stroke::new(1.0_f32, axis_col));
            painter.text(Pos2::new(x, plot.bottom() + 7.0), Align2::CENTER_TOP, fmt_tick(d, step), font.clone(), axis_col);
            d += step;
        }
        painter.text(Pos2::new(plot.center().x, plot.bottom() + 22.0), Align2::CENTER_TOP, if migrated { "horizontal distance [m] (GPS)" } else if p.use_gps_distance && p.has_coords() { "distance [m] (GPS)" } else { "distance [m]" }, font.clone(), axis_col);
        // Distance in metres on top as well (trace numbers are in the status bar).
        let mut d = (d0 / step).ceil() * step;
        while d <= d1 + 1e-9 {
            let x = fr.x_of_trace(p.axis_trace_at_dist(if reversed { p.axis_length() - d } else { d }));
            if x >= plot.left() - 0.5 && x <= plot.right() + 0.5 {
                painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.top() - 4.0)], Stroke::new(1.0_f32, axis_col));
                painter.text(Pos2::new(x, plot.top() - 5.0), Align2::CENTER_BOTTOM, fmt_tick(d, step), font.clone(), axis_col);
            }
            d += step;
        }
        let src = if migrated || p.use_gps_distance && p.has_coords() { "from GPS".to_string() } else { format!("trace spacing {} m", format!("{:.6}", p.dx).trim_end_matches('0').trim_end_matches('.')) };
        painter.text(Pos2::new(plot.left(), avail.top() + 2.0), Align2::LEFT_TOP, format!("distance [m] · {src}"), font.clone(), ui.visuals().weak_text_color());

        // ---------------------------------------------------------- picks overlay
        let marker_pos = |m: &Marker| -> Pos2 {
            let y = p.y_of_pick_time(m.trace, m.time_ns);
            Pos2::new(fr.x_of_trace(m.trace), fr.y_of_sample(p.proc.sample_of(y) + shift(m.trace)))
        };
        let line_pt = |tr: f32, t: f32| -> Pos2 {
            let y = p.y_of_pick_time(tr, t);
            Pos2::new(fr.x_of_trace(tr), fr.y_of_sample(p.proc.sample_of(y) + shift(tr)))
        };
        let draw_line = |l: &LinePick, open: bool, sel: bool| {
            let Some(t) = self.marker_types.get(l.type_idx) else { return };
            let pts: Vec<Pos2> = l.points.iter().map(|(tr, tt)| line_pt(*tr, *tt)).collect();
            let w = if sel { 3.5 } else { 2.2_f32 };
            if sel {
                plot_painter.add(egui::Shape::line(pts.clone(), Stroke::new(w + 2.0, Color32::YELLOW)));
            }
            plot_painter.add(egui::Shape::line(pts.clone(), Stroke::new(w, c32(t.color))));
            if open || sel {
                for q in pts.iter().step_by(((pts.len() / 60).max(1)) as usize) {
                    plot_painter.circle_filled(*q, 2.5, Color32::WHITE);
                }
            }
            if let Some(f) = pts.first() {
                plot_painter.text(*f + egui::vec2(4.0, -4.0), Align2::LEFT_BOTTOM, format!("#{} {}", l.id, t.name), FontId::proportional(10.5), c32(t.color));
            }
        };
        for l in self.lines.iter().filter(|l| l.profile_id == pid) {
            draw_line(l, false, self.selected_pick == Some(l.id));
        }
        if let Some(l) = &self.open_line {
            if l.profile_id == pid {
                draw_line(l, true, false);
            }
        }
        for m in self.markers.iter().filter(|m| m.profile_id == pid) {
            let Some(t) = self.marker_types.get(m.type_idx) else { continue };
            let pos = marker_pos(m);
            if !plot.expand(10.0).contains(pos) {
                continue;
            }
            draw::symbol(&plot_painter, pos, 6.0, t.symbol, c32(t.color), self.selected_pick == Some(m.id));
            plot_painter.text(pos + egui::vec2(9.0, -8.0), Align2::LEFT_BOTTOM, format!("#{}", m.id), FontId::proportional(10.5), Color32::from_rgb(255, 255, 160));
            // Velocity hyperbola of a fitted marker
            if let Some(v) = m.velocity {
                if !depth_sec {
                    let mut pts = Vec::new();
                    let d0 = p.dist_at(m.trace);
                    let xr = plot.width() as usize / 3;
                    for k in 0..=xr {
                        let tr = view.tr0 + (view.tr1 - view.tr0) * k as f32 / xr as f32;
                        let dx = (p.dist_at(tr) - d0) as f32;
                        let tt = (m.time_ns * m.time_ns + (2.0 * dx / v).powi(2)).sqrt();
                        if tt - m.time_ns < 25.0 * p.proc.dt.max(0.05) * 10.0 {
                            pts.push(line_pt(tr, tt));
                        } else if !pts.is_empty() {
                            plot_painter.add(egui::Shape::dashed_line(&pts, Stroke::new(1.0_f32, c32(t.color)), 4.0, 3.0));
                            pts.clear();
                        }
                    }
                    if pts.len() > 1 {
                        plot_painter.add(egui::Shape::dashed_line(&pts, Stroke::new(1.0_f32, c32(t.color)), 4.0, 3.0));
                    }
                }
            }
        }
        // Active hyperbola fit
        if let Some(hf) = self.hyperbola.as_ref().filter(|_| !migrated) {
            let mut pts = Vec::new();
            let d0 = p.dist_at(hf.trace);
            let n = plot.width() as usize / 2;
            for k in 0..=n {
                let tr = view.tr0 + (view.tr1 - view.tr0) * k as f32 / n as f32;
                let dx = (p.dist_at(tr) - d0) as f32;
                let tt = (hf.time_ns * hf.time_ns + (2.0 * dx / hf.velocity).powi(2)).sqrt();
                pts.push(line_pt(tr, tt));
            }
            plot_painter.add(egui::Shape::line(pts.clone(), Stroke::new(3.0_f32, Color32::BLACK)));
            plot_painter.add(egui::Shape::line(pts, Stroke::new(1.8_f32, Color32::from_rgb(0, 255, 120))));
            let ap = line_pt(hf.trace, hf.time_ns);
            plot_painter.circle_filled(ap, 4.0, Color32::from_rgb(0, 255, 120));
            plot_painter.text(ap + egui::vec2(8.0, 6.0), Align2::LEFT_TOP, format!("v = {:.4} m/ns  (drag a flank, Enter saves)", hf.velocity), FontId::proportional(12.0), Color32::from_rgb(0, 255, 120));
        }
        // Visible trace from the map
        if let Some((mpid, mtr)) = self.map.hover_trace {
            if mpid == pid {
                let x = fr.x_of_trace(mtr);
                plot_painter.line_segment([Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())], Stroke::new(1.5_f32, Color32::from_rgb(255, 60, 60)));
            }
        }

        // ---------------------------------------------------------- hover + picking
        self.hover = None;
        let mut action: Option<Action> = None;
        if let Some(hp) = hover_pos.filter(|h| plot.contains(*h)) {
            let tr = fr.trace_of_x(hp.x).clamp(0.0, ntr - 1.0);
            // Sample of the processed data under the pointer (the topography shift removed).
            let s = (fr.sample_of_y(hp.y) - shift(tr)).clamp(0.0, (p.proc.ns as f32 - 1.0).max(0.0));
            self.hover = Some((tr, p.proc.y_of(s)));
            plot_painter.line_segment([Pos2::new(hp.x, plot.top()), Pos2::new(hp.x, plot.bottom())], Stroke::new(0.6_f32, Color32::from_rgba_unmultiplied(255, 255, 0, 120)));
            plot_painter.line_segment([Pos2::new(plot.left(), hp.y), Pos2::new(plot.right(), hp.y)], Stroke::new(0.6_f32, Color32::from_rgba_unmultiplied(255, 255, 0, 120)));
            let tri = tr.round() as usize;
            let cur_type = self.cur_type;
            let cur_name = self.marker_types.get(cur_type).map(|t| t.name.clone()).unwrap_or_default();
            let other_type_msg = format!("That pick is of another type – only \"{cur_name}\" picks can be deleted now. Choose its type in the toolbar to delete it.");
            // Snap preview
            if matches!(self.mode, PickMode::Point | PickMode::Hyperbola | PickMode::Layer) && self.snap != Snap::None {
                let ss = self.snap_at(p, tri, s);
                plot_painter.circle_stroke(Pos2::new(fr.x_of_trace(tri as f32), fr.y_of_sample(ss + shift(tri as f32))), 4.0, Stroke::new(1.5_f32, Color32::from_rgb(0, 255, 255)));
            }
            // `only`: restrict to one marker type (deleting never touches other types).
            let nearest_pick = |only: Option<usize>| -> Option<u32> {
                let ok = |t: usize| only.map(|o| o == t).unwrap_or(true);
                let mut best: Option<(f32, u32)> = None;
                for m in self.markers.iter().filter(|m| m.profile_id == pid && ok(m.type_idx)) {
                    let d = marker_pos(m).distance(hp);
                    if d < 10.0 && best.map(|b| d < b.0).unwrap_or(true) {
                        best = Some((d, m.id));
                    }
                }
                // A fitted marker's velocity hyperbola counts as the marker.
                if !depth_sec {
                    let trh = fr.trace_of_x(hp.x);
                    for m in self.markers.iter().filter(|m| m.profile_id == pid && ok(m.type_idx)) {
                        let Some(v) = m.velocity else { continue };
                        let dx = (p.dist_at(trh) - p.dist_at(m.trace)) as f32;
                        let tt = (m.time_ns * m.time_ns + (2.0 * dx / v).powi(2)).sqrt();
                        if tt - m.time_ns > 25.0 * p.proc.dt.max(0.05) * 10.0 {
                            continue;
                        }
                        let d = (line_pt(trh, tt).y - hp.y).abs() + 1.0;
                        if d < 7.0 && best.map(|b| d < b.0).unwrap_or(true) {
                            best = Some((d, m.id));
                        }
                    }
                }
                for l in self.lines.iter().filter(|l| l.profile_id == pid && ok(l.type_idx)) {
                    for (a2, b2) in l.points.iter().zip(l.points.iter().skip(1)) {
                        let pa = line_pt(a2.0, a2.1);
                        let pb = line_pt(b2.0, b2.1);
                        let d = seg_dist(hp, pa, pb);
                        if d < 7.0 && best.map(|b| d < b.0).unwrap_or(true) {
                            best = Some((d, l.id));
                        }
                    }
                }
                best.map(|b| b.1)
            };
            if resp.clicked() {
                match self.mode {
                    PickMode::Navigate => action = Some(Action::Select(nearest_pick(None))),
                    PickMode::Delete => {
                        if let Some(id) = nearest_pick(Some(cur_type)) {
                            action = Some(Action::Delete(id));
                        } else if nearest_pick(None).is_some() {
                            action = Some(Action::Status(other_type_msg.clone()));
                        }
                    }
                    PickMode::Point => {
                        let ss = self.snap_at(p, tri, s);
                        let y = p.proc.y_of(ss);
                        let t = p.pick_time_of_y(tri as f32, y);
                        action = Some(Action::Point(tri as f32, t, p.proc.value(tri, ss.round() as usize)));
                    }
                    PickMode::Hyperbola => {
                        if depth_sec {
                            action = Some(Action::Status("Fit hyperbolas on a time section (before time-depth conversion).".into()));
                        } else {
                            let ss = self.snap_at(p, tri, s);
                            action = Some(Action::Apex(tri as f32, p.proc.y_of(ss).max(0.01)));
                        }
                    }
                    PickMode::Layer => {
                        let ss = self.snap_at(p, tri, s);
                        action = Some(Action::LayerPoint(tri, ss, resp.double_clicked()));
                    }
                }
            }
            // Right-click deletes the pick under the cursor (in the picking modes).
            if resp.secondary_clicked() && !mods.shift && matches!(self.mode, PickMode::Point | PickMode::Hyperbola | PickMode::Layer | PickMode::Delete) {
                let near_fit = self.hyperbola.as_ref().map(|hf| {
                    let trh = fr.trace_of_x(hp.x);
                    let dx = (p.dist_at(trh) - p.dist_at(hf.trace)) as f32;
                    let tt = (hf.time_ns * hf.time_ns + (2.0 * dx / hf.velocity).powi(2)).sqrt();
                    (line_pt(trh, tt).y - hp.y).abs() < 8.0 || line_pt(hf.trace, hf.time_ns).distance(hp) < 10.0
                }).unwrap_or(false);
                let near_open = self.open_line.as_ref().filter(|l| l.profile_id == pid).map(|l| {
                    l.points.iter().zip(l.points.iter().skip(1)).any(|(a2, b2)| seg_dist(hp, line_pt(a2.0, a2.1), line_pt(b2.0, b2.1)) < 7.0)
                        || l.points.iter().any(|q| line_pt(q.0, q.1).distance(hp) < 8.0)
                }).unwrap_or(false);
                if near_fit {
                    action = Some(Action::CancelFit);
                } else if near_open {
                    action = Some(Action::CancelLine);
                } else if let Some(id) = nearest_pick(Some(cur_type)) {
                    action = Some(Action::Delete(id));
                } else if nearest_pick(None).is_some() {
                    action = Some(Action::Status(other_type_msg.clone()));
                } else {
                    action = Some(Action::Status("Right-click on a pick to delete it (Shift + right-drag deletes all picks in a box).".into()));
                }
            }
            if self.mode == PickMode::Hyperbola && resp.dragged_by(egui::PointerButton::Primary) {
                if let Some(hf) = &self.hyperbola {
                    let t = p.proc.y_of(s);
                    let dx = (p.dist_at(tr) - p.dist_at(hf.trace)).abs() as f32;
                    if t > hf.time_ns + p.proc.dt && dx > 0.0 {
                        let v = 2.0 * dx / (t * t - hf.time_ns * hf.time_ns).sqrt();
                        action = Some(Action::Velocity(v.clamp(0.01, 0.3)));
                    }
                }
            }
        }
        // Shift + right-drag: delete every pick inside the box.
        if mods.shift && resp.drag_started_by(egui::PointerButton::Secondary) {
            if let Some(pp) = resp.interact_pointer_pos() {
                self.select_rect = Some((pp, pp));
            }
        }
        if let Some((a0, _)) = self.select_rect {
            if let Some(pp) = ui.input(|i| i.pointer.latest_pos()) {
                self.select_rect = Some((a0, pp));
            }
            let (a0, b0) = self.select_rect.unwrap();
            let r = Rect::from_two_pos(a0, b0);
            plot_painter.rect_filled(r, 0.0, Color32::from_rgba_unmultiplied(255, 60, 60, 40));
            plot_painter.rect_stroke(r, 0.0, Stroke::new(1.5_f32, Color32::from_rgb(255, 60, 60)), egui::StrokeKind::Middle);
            let released = ui.input(|i| i.pointer.button_released(egui::PointerButton::Secondary)) || !ui.input(|i| i.pointer.button_down(egui::PointerButton::Secondary));
            if released {
                // Only picks of the selected type are deleted.
                let ct = self.cur_type;
                let in_box_m = |m: &&Marker| m.profile_id == pid && r.contains(marker_pos(m));
                let in_box_l = |l: &&LinePick| l.profile_id == pid && l.points.iter().any(|q| r.contains(line_pt(q.0, q.1)));
                let mut ids: Vec<u32> = self.markers.iter().filter(|m| in_box_m(m) && m.type_idx == ct).map(|m| m.id).collect();
                ids.extend(self.lines.iter().filter(|l| in_box_l(l) && l.type_idx == ct).map(|l| l.id));
                let others = self.markers.iter().filter(|m| in_box_m(m) && m.type_idx != ct).count() + self.lines.iter().filter(|l| in_box_l(l) && l.type_idx != ct).count();
                let tname = self.marker_types.get(ct).map(|t| t.name.clone()).unwrap_or_default();
                let clear_fit = self.hyperbola.as_ref().map(|hf| r.contains(line_pt(hf.trace, hf.time_ns))).unwrap_or(false);
                self.select_rect = None;
                action = Some(if ids.is_empty() {
                    if clear_fit { Action::CancelFit } else { Action::Status(if others > 0 { format!("No \"{tname}\" picks inside the box ({others} of other types kept).") } else { "No picks inside the box.".into() }) }
                } else {
                    if others > 0 {
                        self.status = format!("{others} pick(s) of other types kept.");
                    }
                    Action::DeleteMany(ids, clear_fit)
                });
            }
        }
        if self.hyperbola.is_some() && self.mode == PickMode::Hyperbola && hover_pos.is_some() && mods.alt && scroll.y != 0.0 {
            self.record_pick_undo();
            if let Some(hf) = &mut self.hyperbola {
                hf.velocity = (hf.velocity * (scroll.y * 0.001).exp()).clamp(0.01, 0.3);
            }
        }
        let (enter, backspace, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Backspace), i.key_pressed(egui::Key::Escape)));
        if enter {
            if self.hyperbola.is_some() {
                self.save_hyperbola();
            } else if self.open_line.is_some() {
                self.finish_line();
            }
        }
        if esc {
            if self.hyperbola.is_some() || self.open_line.is_some() { self.record_pick_undo(); }
            self.hyperbola = None;
            self.open_line = None;
        }
        if backspace && self.open_line.is_some() && !ui.ctx().wants_keyboard_input() {
            if self.open_line.as_ref().is_some_and(|l| !l.points.is_empty()) { self.record_pick_undo(); }
            if let Some(l) = &mut self.open_line {
                l.points.pop();
            }
        }
        if let Some(act) = action {
            self.apply_action(a, act);
        }

        // ---------------------------------------------------------- A-scan
        let p = &self.profiles[a];
        let arect = Rect::from_min_max(Pos2::new(avail.right() - ascan_w, plot.top()), Pos2::new(avail.right(), plot.bottom()));
        let ap = ui.painter_at(arect.expand(2.0));
        ap.rect_filled(arect, 0.0, Color32::from_gray(248));
        ap.rect_stroke(arect, 0.0, Stroke::new(1.0_f32, Color32::from_gray(160)), egui::StrokeKind::Inside);
        let tr_show = self.hover.map(|h| h.0.round() as usize).unwrap_or(((view.tr0 + view.tr1) / 2.0) as usize).min(p.proc.ntr.saturating_sub(1));
        ap.line_segment([Pos2::new(arect.center().x, arect.top()), Pos2::new(arect.center().x, arect.bottom())], Stroke::new(0.5_f32, Color32::GRAY));
        if p.proc.ntr > 0 {
            let col = p.proc.trace(tr_show);
            let sa = view.s0.max(0.0) as usize;
            let sb = (view.s1.ceil() as usize).min(p.proc.ns);
            let off = shift(tr_show as f32);
            let sa = (view.s0 - off).max(0.0) as usize;
            let sb = ((view.s1 - off).ceil().max(0.0) as usize).min(p.proc.ns);
            let pts: Vec<Pos2> = (sa..sb).map(|s| Pos2::new(arect.center().x + (col[s] / (clip * 1.5)).clamp(-1.0, 1.0) * ascan_w * 0.48, fr.y_of_sample(s as f32 + off))).collect();
            ap.add(egui::Shape::line(pts, Stroke::new(1.0_f32, Color32::BLACK)));
        }
        ap.text(arect.left_top() + egui::vec2(3.0, 2.0), Align2::LEFT_TOP, format!("trace {}", tr_show + 1), FontId::proportional(10.5), Color32::DARK_GRAY);
    }

    pub fn delete_with_undo(&mut self, ids: &[u32], clear_fit: bool) {
        let n = self.markers.iter().filter(|m| ids.contains(&m.id)).count() + self.lines.iter().filter(|l| ids.contains(&l.id)).count();
        if n == 0 && !clear_fit {
            return;
        }
        self.record_pick_undo();
        self.markers.retain(|m| !ids.contains(&m.id));
        self.lines.retain(|l| !ids.contains(&l.id));
        if clear_fit { self.hyperbola = None; }
        if self.selected_pick.map(|s| ids.contains(&s)).unwrap_or(false) {
            self.selected_pick = None;
        }
        let kept = if self.status.contains("kept") { format!(" {}", self.status) } else { String::new() };
        self.status = if n == 1 { format!("Pick #{} deleted.{kept}", ids[0]) } else { format!("{n} picks deleted.{kept}") };
    }

    fn apply_action(&mut self, a: usize, act: Action) {
        let pid = self.profiles[a].id;
        match act {
            Action::Select(id) => self.selected_pick = id,
            Action::Delete(id) => {
                self.status.clear();
                self.delete_with_undo(&[id], false);
            }
            Action::DeleteMany(ids, clear_fit) => {
                self.delete_with_undo(&ids, clear_fit);
            }
            Action::CancelFit => {
                self.record_pick_undo();
                self.hyperbola = None;
                self.status = "Hyperbola removed.".into();
            }
            Action::CancelLine => {
                self.record_pick_undo();
                self.open_line = None;
                self.status = "Layer line in progress removed.".into();
            }
            Action::Point(tr, t, amp) => {
                self.record_pick_undo();
                let ti = if self.marker_types.get(self.cur_type).map(|t| t.kind == MarkerKind::Point).unwrap_or(false) { self.cur_type } else { 0 };
                self.markers.push(Marker { id: self.next_pick_id, profile_id: pid, type_idx: ti, trace: tr, time_ns: t, amplitude: amp, velocity: None, comment: String::new() });
                self.selected_pick = Some(self.next_pick_id);
                let p = &self.profiles[a];
                self.status = format!("Marker #{} at {:.2} m, {:.2} ns, depth {:.2} m.", self.next_pick_id, p.dist_at(tr), t, p.depth_of_time(t));
                self.next_pick_id += 1;
            }
            Action::Apex(tr, t) => {
                self.record_pick_undo();
                let v = self.hyperbola.as_ref().map(|h| h.velocity).unwrap_or(self.profiles[a].velocity);
                self.hyperbola = Some(HyperbolaFit { trace: tr, time_ns: t, velocity: v });
            }
            Action::Velocity(v) => {
                if !self.velocity_drag_active {
                    self.record_pick_undo();
                    self.velocity_drag_active = true;
                }
                if let Some(h) = &mut self.hyperbola {
                    h.velocity = v;
                }
            }
            Action::LayerPoint(tr, s, finish) => {
                if !finish && self.open_line.as_ref().filter(|l| l.profile_id == pid)
                    .and_then(|l| l.points.last()).is_some_and(|q| q.0.round() as usize == tr) { return; }
                self.record_pick_undo();
                let p = &self.profiles[a];
                let to_t = |tr: usize, s: f32| p.pick_time_of_y(tr as f32, p.proc.y_of(s));
                let ti = if self.marker_types.get(self.cur_type).map(|t| t.kind == MarkerKind::Line).unwrap_or(false) {
                    self.cur_type
                } else {
                    self.marker_types.iter().position(|t| t.kind == MarkerKind::Line).unwrap_or(0)
                };
                let mut line = self.open_line.take().filter(|l| l.profile_id == pid).unwrap_or_else(|| {
                    let id = self.next_pick_id;
                    LinePick { id, profile_id: pid, type_idx: ti, points: Vec::new(), comment: String::new() }
                });
                if line.points.is_empty() {
                    self.next_pick_id += 1;
                }
                let mut new_pts = Vec::new();
                let same_trace = line.points.last().map(|q| q.0.round() as usize == tr).unwrap_or(false);
                if same_trace {
                    // Second click of a double-click: nothing new to add.
                } else if let (true, Some(&(ptr, pt))) = (self.auto_track, line.points.last()) {
                    // Follow the phase from the last point to the new one, trace by trace.
                    let ptr_i = ptr.round() as isize;
                    let tr_i = tr as isize;
                    let ps = p.proc.sample_of(p.y_of_pick_time(ptr, pt));
                    let n = (tr_i - ptr_i).abs();
                    let dir = (tr_i - ptr_i).signum();
                    let mut cur = ps;
                    for k in 1..=n {
                        let t_i = (ptr_i + k * dir) as usize;
                        let target = ps + (s - ps) * k as f32 / n as f32;
                        // With snapping off the line runs straight between the clicks.
                        cur = if self.snap == Snap::None {
                            target
                        } else {
                            let guess = cur + (target - cur) * 0.5;
                            snap_sample(p.proc.trace(t_i), guess, (self.snap_win / 2).max(2), self.snap)
                        };
                        if k == n {
                            cur = s;
                        }
                        new_pts.push((t_i as f32, to_t(t_i, cur)));
                    }
                } else {
                    new_pts.push((tr as f32, to_t(tr, s)));
                }
                line.points.extend(new_pts);
                self.open_line = Some(line);
                if finish {
                    self.finish_line_without_undo();
                }
            }
            Action::FinishLine => self.finish_line(),
            Action::Status(s) => self.status = s,
        }
    }
}

enum Action {
    Select(Option<u32>),
    Delete(u32),
    DeleteMany(Vec<u32>, bool),
    CancelFit,
    CancelLine,
    Point(f32, f32, f32),
    Apex(f32, f32),
    Velocity(f32),
    LayerPoint(usize, f32, bool),
    FinishLine,
    Status(String),
}

fn seg_dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let l2 = ab.length_sq();
    if l2 < 1e-6 {
        return p.distance(a);
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}
