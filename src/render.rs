//! Turning a section into pixels: colour scales and resampling.

use crate::model::Section;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ColorMap {
    Gray,
    GrayInverted,
    RedWhiteBlue,
    Seismic,
    Rainbow,
}

impl ColorMap {
    pub const ALL: [ColorMap; 5] = [ColorMap::Gray, ColorMap::GrayInverted, ColorMap::RedWhiteBlue, ColorMap::Seismic, ColorMap::Rainbow];
    pub fn name(&self) -> &'static str {
        match self {
            ColorMap::Gray => "Grey",
            ColorMap::GrayInverted => "Grey inverted",
            ColorMap::RedWhiteBlue => "Red-white-blue",
            ColorMap::Seismic => "Seismic",
            ColorMap::Rainbow => "Rainbow",
        }
    }
    /// v in [-1, 1]
    pub fn color(&self, v: f32) -> [u8; 3] {
        let v = v.clamp(-1.0, 1.0);
        let u = (v + 1.0) * 0.5; // 0..1
        match self {
            ColorMap::Gray => {
                let g = (u * 255.0) as u8;
                [g, g, g]
            }
            ColorMap::GrayInverted => {
                let g = 255 - (u * 255.0) as u8;
                [g, g, g]
            }
            ColorMap::RedWhiteBlue => {
                if v >= 0.0 {
                    let k = 255 - (v * 255.0) as u8;
                    [255, k, k]
                } else {
                    let k = 255 - (-v * 255.0) as u8;
                    [k, k, 255]
                }
            }
            ColorMap::Seismic => {
                let stops = [(0.0, [0, 0, 90]), (0.25, [0, 70, 255]), (0.5, [250, 250, 250]), (0.75, [255, 60, 0]), (1.0, [110, 0, 0])];
                grad(&stops, u)
            }
            ColorMap::Rainbow => {
                let stops = [(0.0, [0, 0, 140]), (0.2, [0, 80, 255]), (0.4, [0, 230, 230]), (0.6, [120, 255, 60]), (0.8, [255, 200, 0]), (1.0, [200, 0, 0])];
                grad(&stops, u)
            }
        }
    }
}

fn grad(stops: &[(f32, [u8; 3])], u: f32) -> [u8; 3] {
    for w in stops.windows(2) {
        let (a, ca) = w[0];
        let (b, cb) = w[1];
        if u <= b {
            let f = ((u - a) / (b - a)).clamp(0.0, 1.0);
            return [
                (ca[0] as f32 + (cb[0] as f32 - ca[0] as f32) * f) as u8,
                (ca[1] as f32 + (cb[1] as f32 - ca[1] as f32) * f) as u8,
                (ca[2] as f32 + (cb[2] as f32 - ca[2] as f32) * f) as u8,
            ];
        }
    }
    stops.last().unwrap().1
}

/// Amplitude that maps to full colour: the 98th percentile of |amplitude| divided by contrast.
pub fn clip_level(s: &Section, contrast: f32) -> f32 {
    let n = s.data.len();
    if n == 0 {
        return 1.0;
    }
    let step = (n / 200_000).max(1);
    let mut v: Vec<f32> = s.data.iter().step_by(step).map(|x| x.abs()).filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 1.0;
    }
    let k = ((v.len() as f32) * 0.98) as usize;
    let k = k.min(v.len() - 1);
    v.select_nth_unstable_by(k, |a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p = v[k];
    let p = if p > 0.0 { p } else { 1.0 };
    p / contrast.max(0.01)
}

/// Render traces [tr0, tr1) and samples [s0, s1) into a w x h RGBA image.
pub fn render(s: &Section, tr0: f32, tr1: f32, s0: f32, s1: f32, w: usize, h: usize, cmap: ColorMap, clip: f32) -> Vec<u8> {
    let mut img = vec![0u8; w * h * 4];
    if s.ntr == 0 || s.ns == 0 || w == 0 || h == 0 {
        return img;
    }
    let lut: Vec<[u8; 3]> = (0..512).map(|i| cmap.color(i as f32 / 255.5 - 1.0)).collect();
    let bg = cmap.color(0.0);
    let inv = 1.0 / clip.max(1e-30);
    let col_tr: Vec<Option<usize>> = (0..w)
        .map(|x| {
            let t = tr0 + (x as f32 + 0.5) / w as f32 * (tr1 - tr0);
            if t < 0.0 || t >= s.ntr as f32 {
                None
            } else {
                Some(t as usize)
            }
        })
        .collect();
    let row_s: Vec<Option<(usize, f32)>> = (0..h)
        .map(|y| {
            let v = s0 + (y as f32 + 0.5) / h as f32 * (s1 - s0) - 0.5;
            if v < -0.5 || v > s.ns as f32 - 0.5 {
                None
            } else {
                let v = v.clamp(0.0, (s.ns - 1) as f32);
                let i = v.floor() as usize;
                Some((i, v - i as f32))
            }
        })
        .collect();
    for (x, ct) in col_tr.iter().enumerate() {
        let Some(t) = ct else {
            for y in 0..h {
                let o = (y * w + x) * 4;
                img[o..o + 4].copy_from_slice(&[bg[0], bg[1], bg[2], 255]);
            }
            continue;
        };
        let tr = s.trace(*t);
        for (y, rs) in row_s.iter().enumerate() {
            let o = (y * w + x) * 4;
            let c = match rs {
                None => bg,
                Some((i, f)) => {
                    let a = tr[*i];
                    let b = if i + 1 < tr.len() { tr[i + 1] } else { a };
                    let v = (a + (b - a) * f) * inv;
                    let k = ((v.clamp(-1.0, 1.0) + 1.0) * 255.5) as usize;
                    lut[k.min(511)]
                }
            };
            img[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    img
}
