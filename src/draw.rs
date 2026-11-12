//! Shared egui drawing helpers.

use crate::model::Symbol;
use egui::{Color32, Painter, Pos2, Shape, Stroke};

pub fn c32(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn symbol(p: &Painter, at: Pos2, r: f32, sym: Symbol, color: Color32, selected: bool) {
    let outline = Stroke::new(1.2, Color32::BLACK);
    match sym {
        Symbol::Circle => {
            p.circle_filled(at, r, color);
            p.circle_stroke(at, r, outline);
        }
        Symbol::Square => {
            let rect = egui::Rect::from_center_size(at, egui::vec2(r * 1.8, r * 1.8));
            p.rect_filled(rect, 0.0, color);
            p.rect_stroke(rect, 0.0, outline, egui::StrokeKind::Middle);
        }
        Symbol::Triangle => {
            let pts = vec![at + egui::vec2(0.0, -r * 1.15), at + egui::vec2(r, r * 0.75), at + egui::vec2(-r, r * 0.75)];
            p.add(Shape::convex_polygon(pts, color, outline));
        }
        Symbol::Diamond => {
            let pts = vec![at + egui::vec2(0.0, -r * 1.2), at + egui::vec2(r, 0.0), at + egui::vec2(0.0, r * 1.2), at + egui::vec2(-r, 0.0)];
            p.add(Shape::convex_polygon(pts, color, outline));
        }
        Symbol::Cross => {
            let s = Stroke::new(3.5, Color32::BLACK);
            p.line_segment([at + egui::vec2(-r, -r), at + egui::vec2(r, r)], s);
            p.line_segment([at + egui::vec2(-r, r), at + egui::vec2(r, -r)], s);
            let s = Stroke::new(2.0, color);
            p.line_segment([at + egui::vec2(-r, -r), at + egui::vec2(r, r)], s);
            p.line_segment([at + egui::vec2(-r, r), at + egui::vec2(r, -r)], s);
        }
        Symbol::Star => {
            let mut pts = Vec::new();
            for k in 0..10 {
                let a = -std::f32::consts::FRAC_PI_2 + k as f32 * std::f32::consts::PI / 5.0;
                let rr = if k % 2 == 0 { r * 1.3 } else { r * 0.55 };
                pts.push(at + egui::vec2(a.cos() * rr, a.sin() * rr));
            }
            p.add(Shape::closed_line(pts.clone(), outline));
            // Fill as triangles from the centre.
            let mut mesh = egui::Mesh::default();
            let c = mesh.vertices.len() as u32;
            mesh.colored_vertex(at, color);
            for q in &pts {
                mesh.colored_vertex(*q, color);
            }
            for k in 0..10u32 {
                mesh.add_triangle(c, c + 1 + k, c + 1 + (k + 1) % 10);
            }
            p.add(Shape::mesh(mesh));
            p.add(Shape::closed_line(pts, outline));
        }
    }
    if selected {
        p.circle_stroke(at, r + 4.0, Stroke::new(2.0, Color32::from_rgb(255, 255, 0)));
        p.circle_stroke(at, r + 6.0, Stroke::new(1.0, Color32::BLACK));
    }
}

/// A "nice" tick step for a range spanning `span` with about `n` ticks.
pub fn nice_step(span: f64, n: f64) -> f64 {
    if span <= 0.0 || !span.is_finite() {
        return 1.0;
    }
    let raw = span / n.max(1.0);
    let mag = 10f64.powf(raw.log10().floor());
    let r = raw / mag;
    let m = if r < 1.5 {
        1.0
    } else if r < 3.5 {
        2.0
    } else if r < 7.5 {
        5.0
    } else {
        10.0
    };
    m * mag
}

pub fn fmt_tick(v: f64, step: f64) -> String {
    if step >= 1.0 {
        format!("{v:.0}")
    } else if step >= 0.1 {
        format!("{v:.1}")
    } else if step >= 0.01 {
        format!("{v:.2}")
    } else {
        format!("{v:.3}")
    }
}
