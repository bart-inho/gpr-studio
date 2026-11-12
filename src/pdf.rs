//! A small PDF writer: vector lines, filled shapes, Helvetica text and RGB images (Flate).
//! The page API uses a top-left origin in points.

use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;

pub struct Pdf {
    objects: Vec<Vec<u8>>, // object bodies, id = index + 1
    pages: Vec<usize>,
    images: Vec<usize>,
}

#[derive(Clone, Copy)]
pub struct ImageId(pub usize);

pub struct Page {
    pub w: f32,
    pub h: f32,
    content: Vec<u8>,
    used_images: Vec<usize>,
}

fn latin1(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for c in s.chars() {
        let b = match c {
            '(' | ')' | '\\' => {
                out.push(b'\\');
                c as u8
            }
            c if (c as u32) < 256 => c as u32 as u8,
            '–' | '—' => b'-',
            '→' => b'>',
            _ => b'?',
        };
        out.push(b);
    }
    out
}

/// Approximate Helvetica text width in points.
pub fn text_width(s: &str, size: f32) -> f32 {
    s.chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | '.' | ',' | ':' | ';' | '\'' | '|' | '!' => 0.25,
            'f' | 't' | 'r' | 'I' | ' ' | '(' | ')' | '-' | '/' => 0.33,
            'm' | 'w' | 'M' | 'W' => 0.83,
            c if c.is_ascii_uppercase() => 0.67,
            c if c.is_ascii_digit() => 0.556,
            _ => 0.52,
        })
        .sum::<f32>()
        * size
}

impl Page {
    pub fn new(w: f32, h: f32) -> Self {
        Page { w, h, content: Vec::new(), used_images: Vec::new() }
    }
    fn put(&mut self, s: &str) {
        self.content.extend_from_slice(s.as_bytes());
    }
    fn y(&self, y: f32) -> f32 {
        self.h - y
    }
    pub fn stroke_color(&mut self, c: [u8; 3]) {
        self.put(&format!("{:.3} {:.3} {:.3} RG\n", c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0));
    }
    pub fn fill_color(&mut self, c: [u8; 3]) {
        self.put(&format!("{:.3} {:.3} {:.3} rg\n", c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0));
    }
    pub fn line_width(&mut self, w: f32) {
        self.put(&format!("{w:.2} w\n"));
    }
    pub fn dash(&mut self, on: f32, off: f32) {
        if on <= 0.0 {
            self.put("[] 0 d\n");
        } else {
            self.put(&format!("[{on:.1} {off:.1}] 0 d\n"));
        }
    }
    pub fn polyline(&mut self, pts: &[[f32; 2]]) {
        if pts.len() < 2 {
            return;
        }
        let mut s = format!("{:.2} {:.2} m\n", pts[0][0], self.y(pts[0][1]));
        for p in &pts[1..] {
            s.push_str(&format!("{:.2} {:.2} l\n", p[0], self.y(p[1])));
        }
        s.push_str("S\n");
        self.put(&s);
    }
    pub fn line(&mut self, a: [f32; 2], b: [f32; 2]) {
        self.polyline(&[a, b]);
    }
    pub fn polygon(&mut self, pts: &[[f32; 2]], fill: bool, stroke: bool) {
        if pts.len() < 3 {
            return;
        }
        let mut s = format!("{:.2} {:.2} m\n", pts[0][0], self.y(pts[0][1]));
        for p in &pts[1..] {
            s.push_str(&format!("{:.2} {:.2} l\n", p[0], self.y(p[1])));
        }
        s.push_str(match (fill, stroke) {
            (true, true) => "b\n",
            (true, false) => "f\n",
            _ => "s\n",
        });
        self.put(&s);
    }
    pub fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, fill: bool, stroke: bool) {
        self.polygon(&[[x, y], [x + w, y], [x + w, y + h], [x, y + h]], fill, stroke);
    }
    pub fn circle(&mut self, cx: f32, cy: f32, r: f32, fill: bool, stroke: bool) {
        let n = 16;
        let pts: Vec<[f32; 2]> = (0..n).map(|i| {
            let a = i as f32 / n as f32 * std::f32::consts::TAU;
            [cx + r * a.cos(), cy + r * a.sin()]
        }).collect();
        self.polygon(&pts, fill, stroke);
    }
    pub fn text(&mut self, x: f32, y: f32, size: f32, s: &str, bold: bool) {
        let font = if bold { "F2" } else { "F1" };
        let mut b = format!("BT /{font} {size:.1} Tf {:.2} {:.2} Td (", x, self.y(y)).into_bytes();
        b.extend(latin1(s));
        b.extend_from_slice(b") Tj ET\n");
        self.content.extend(b);
    }
    pub fn text_right(&mut self, x: f32, y: f32, size: f32, s: &str, bold: bool) {
        let w = text_width(s, size);
        self.text(x - w, y, size, s, bold);
    }
    pub fn text_center(&mut self, x: f32, y: f32, size: f32, s: &str, bold: bool) {
        let w = text_width(s, size);
        self.text(x - w / 2.0, y, size, s, bold);
    }
    pub fn text_rotated(&mut self, x: f32, y: f32, size: f32, s: &str) {
        let mut b = format!("BT /F1 {size:.1} Tf 0 1 -1 0 {:.2} {:.2} Tm (", x, self.y(y)).into_bytes();
        b.extend(latin1(s));
        b.extend_from_slice(b") Tj ET\n");
        self.content.extend(b);
    }
    pub fn save(&mut self) {
        self.put("q\n");
    }
    pub fn restore(&mut self) {
        self.put("Q\n");
    }
    pub fn clip_rect(&mut self, x: f32, y: f32, w: f32, h: f32) {
        self.put(&format!("{:.2} {:.2} {:.2} {:.2} re W n\n", x, self.y(y + h), w, h));
    }
    pub fn image(&mut self, img: ImageId, x: f32, y: f32, w: f32, h: f32) {
        self.used_images.push(img.0);
        self.put(&format!("q {w:.2} 0 0 {h:.2} {:.2} {:.2} cm /Im{} Do Q\n", x, self.y(y + h), img.0));
    }
}

impl Pdf {
    pub fn new() -> Self {
        // Reserve: 1 catalog, 2 pages tree, 3 font, 4 bold font.
        let mut p = Pdf { objects: vec![Vec::new(); 4], pages: Vec::new(), images: Vec::new() };
        p.objects[2] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>".to_vec();
        p.objects[3] = b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica-Bold /Encoding /WinAnsiEncoding >>".to_vec();
        p
    }

    fn add_object(&mut self, body: Vec<u8>) -> usize {
        self.objects.push(body);
        self.objects.len()
    }

    fn stream(dict: &str, data: &[u8]) -> Vec<u8> {
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(data).ok();
        let z = enc.finish().unwrap_or_default();
        let mut b = format!("<< {dict} /Filter /FlateDecode /Length {} >>\nstream\n", z.len()).into_bytes();
        b.extend(z);
        b.extend_from_slice(b"\nendstream");
        b
    }

    /// RGB image, 3 bytes per pixel.
    pub fn add_image(&mut self, w: usize, h: usize, rgb: &[u8]) -> ImageId {
        let body = Self::stream(&format!("/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceRGB /BitsPerComponent 8"), rgb);
        let id = self.add_object(body);
        self.images.push(id);
        ImageId(id)
    }

    pub fn add_page(&mut self, page: Page) {
        let content = Self::stream("", &page.content);
        let cid = self.add_object(content);
        let mut xo = String::new();
        let mut used = page.used_images.clone();
        used.sort();
        used.dedup();
        for i in used {
            xo.push_str(&format!("/Im{i} {i} 0 R "));
        }
        let body = format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Contents {cid} 0 R /Resources << /Font << /F1 3 0 R /F2 4 0 R >> /XObject << {xo}>> >> >>",
            page.w, page.h
        );
        let pid = self.add_object(body.into_bytes());
        self.pages.push(pid);
    }

    pub fn write(mut self, path: &std::path::Path) -> std::io::Result<()> {
        self.objects[0] = b"<< /Type /Catalog /Pages 2 0 R >>".to_vec();
        let kids: Vec<String> = self.pages.iter().map(|p| format!("{p} 0 R")).collect();
        self.objects[1] = format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), self.pages.len()).into_bytes();
        let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
        let mut offsets = Vec::new();
        for (i, o) in self.objects.iter().enumerate() {
            offsets.push(out.len());
            out.extend(format!("{} 0 obj\n", i + 1).into_bytes());
            out.extend_from_slice(o);
            out.extend_from_slice(b"\nendobj\n");
        }
        let xref = out.len();
        out.extend(format!("xref\n0 {}\n0000000000 65535 f \n", self.objects.len() + 1).into_bytes());
        for o in offsets {
            out.extend(format!("{o:010} 00000 n \n").into_bytes());
        }
        out.extend(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n", self.objects.len() + 1).into_bytes());
        std::fs::write(path, out)
    }
}
