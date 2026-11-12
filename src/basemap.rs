//! GeoTIFF basemaps of any size: a downsampled overview is read chunk by chunk, and a
//! full-resolution detail of the visible window is read on demand when zoomed in.

use crate::crs::CrsSpec;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use tiff::decoder::{Decoder, DecodingResult, Limits};
use tiff::tags::Tag;
use tiff::ColorType;

pub const OVERVIEW_MAX: usize = 4096;
pub const DETAIL_MAX: usize = 2048;

#[derive(Clone)]
pub struct Raster {
    pub w: usize,
    pub h: usize,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct GeoInfo {
    pub width: u32,
    pub height: u32,
    /// X = g0 + g1*col + g2*row ; Y = g3 + g4*col + g5*row (pixel corners)
    pub geo: [f64; 6],
    pub crs: Option<CrsSpec>,
    pub nodata: Option<f64>,
    pub georef_source: String,
}

impl GeoInfo {
    pub fn pixel_to_world(&self, col: f64, row: f64) -> [f64; 2] {
        let g = &self.geo;
        [g[0] + g[1] * col + g[2] * row, g[3] + g[4] * col + g[5] * row]
    }
    pub fn world_to_pixel(&self, x: f64, y: f64) -> [f64; 2] {
        let g = &self.geo;
        let det = g[1] * g[5] - g[2] * g[4];
        let dx = x - g[0];
        let dy = y - g[3];
        [(dx * g[5] - dy * g[2]) / det, (dy * g[1] - dx * g[4]) / det]
    }
}

/// How sample values become colours (shared by overview and detail).
#[derive(Clone, Debug)]
pub struct Style {
    pub bands: usize,
    pub kind: BandKind,
    pub lo: f64,
    pub hi: f64,
    pub palette: Option<Vec<[u8; 3]>>,
    pub nodata: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BandKind {
    Gray,
    GrayAlpha,
    Rgb,
    Rgba,
    Palette,
}

pub struct Loaded {
    pub info: GeoInfo,
    pub overview: Raster,
    pub ov_factor: f64,
    pub style: Style,
}

fn open(path: &Path) -> Result<Decoder<BufReader<File>>, String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Decoder::new(BufReader::new(f)).map(|d| d.with_limits(Limits::unlimited())).map_err(|e| format!("not a readable TIFF: {e}"))
}

fn tag_f64s(d: &mut Decoder<BufReader<File>>, t: u16) -> Option<Vec<f64>> {
    d.get_tag_f64_vec(Tag::from_u16_exhaustive(t)).ok()
}

pub fn read_info(path: &Path) -> Result<GeoInfo, String> {
    let mut d = open(path)?;
    let (w, h) = d.dimensions().map_err(|e| e.to_string())?;
    let scale = tag_f64s(&mut d, 33550);
    let tie = tag_f64s(&mut d, 33922);
    let trans = tag_f64s(&mut d, 34264);
    let keys = d.get_tag_u16_vec(Tag::from_u16_exhaustive(34735)).ok();
    let nodata = d.get_tag_ascii_string(Tag::from_u16_exhaustive(42113)).ok().and_then(|s| s.trim().trim_end_matches('\0').parse::<f64>().ok());
    let mut crs = None;
    let mut point = false;
    if let Some(k) = &keys {
        let n = k.get(3).copied().unwrap_or(0) as usize;
        for i in 0..n {
            let b = 4 + i * 4;
            if b + 3 >= k.len() {
                break;
            }
            let (id, loc, val) = (k[b], k[b + 1], k[b + 3]);
            if loc != 0 {
                continue;
            }
            match id {
                3072 | 2048 if val != 32767 && val != 0 => {
                    if crs_definitions::from_code(val).is_some() && (crs.is_none() || id == 3072) {
                        crs = Some(CrsSpec::Epsg(val));
                    }
                }
                1025 => point = val == 2,
                _ => {}
            }
        }
    }
    let half = if point { 0.5 } else { 0.0 };
    let (geo, src) = if let Some(m) = trans.filter(|m| m.len() >= 8) {
        ([m[3] - (m[0] + m[1]) * half, m[0], m[1], m[7] - (m[4] + m[5]) * half, m[4], m[5]], "GeoTIFF transformation".to_string())
    } else if let (Some(s), Some(t)) = (scale.filter(|s| s.len() >= 2), tie.filter(|t| t.len() >= 6)) {
        let (i, j, x, y) = (t[0], t[1], t[3], t[4]);
        let (sx, sy) = (s[0], s[1]);
        ([x - (i + half) * sx, sx, 0.0, y + (j + half) * sy, 0.0, -sy], "GeoTIFF tie point".to_string())
    } else if let Some(wf) = world_file(path) {
        wf
    } else {
        return Err("the TIFF has no georeference (no GeoTIFF tags and no .tfw world file)".into());
    };
    Ok(GeoInfo { width: w, height: h, geo, crs, nodata, georef_source: src })
}

fn world_file(path: &Path) -> Option<([f64; 6], String)> {
    for e in ["tfw", "TFW", "tifw", "TIFW", "wld", "WLD"] {
        let p = path.with_extension(e);
        if let Ok(t) = std::fs::read_to_string(&p) {
            let v: Vec<f64> = t.split_whitespace().filter_map(|s| s.parse().ok()).collect();
            if v.len() >= 6 {
                let (a, d, b, e2, c, f) = (v[0], v[1], v[2], v[3], v[4], v[5]);
                return Some(([c - 0.5 * a - 0.5 * b, a, b, f - 0.5 * d - 0.5 * e2, d, e2], "world file".into()));
            }
        }
    }
    None
}

fn to_f32(r: DecodingResult) -> Vec<f32> {
    match r {
        DecodingResult::U8(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::U64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::F32(v) => v,
        DecodingResult::F64(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I8(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I16(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I32(v) => v.into_iter().map(|x| x as f32).collect(),
        DecodingResult::I64(v) => v.into_iter().map(|x| x as f32).collect(),
    }
}

struct Layout {
    w: usize,
    h: usize,
    cw: usize,
    ch: usize,
    across: usize,
    tiled: bool,
    count: usize,
}

fn layout(d: &mut Decoder<BufReader<File>>) -> Result<Layout, String> {
    let (w, h) = d.dimensions().map_err(|e| e.to_string())?;
    let (cw, ch) = d.chunk_dimensions();
    let tiled = matches!(d.get_chunk_type(), tiff::decoder::ChunkType::Tile);
    let count = if tiled { d.tile_count().map_err(|e| e.to_string())? } else { d.strip_count().map_err(|e| e.to_string())? } as usize;
    let (w, h, cw, ch) = (w as usize, h as usize, cw.max(1) as usize, ch.max(1) as usize);
    let across = if tiled { w.div_ceil(cw) } else { 1 };
    Ok(Layout { w, h, cw, ch, across, tiled, count })
}

impl Layout {
    fn origin(&self, i: usize) -> (usize, usize) {
        if self.tiled {
            ((i % self.across) * self.cw, (i / self.across) * self.ch)
        } else {
            (0, i * self.ch)
        }
    }
    /// Chunks that intersect rows [r0, r1) and columns [c0, c1).
    fn chunks_for(&self, c0: usize, c1: usize, r0: usize, r1: usize) -> Vec<usize> {
        let mut v = Vec::new();
        let ry0 = r0 / self.ch;
        let ry1 = (r1.saturating_sub(1)) / self.ch;
        if self.tiled {
            let cx0 = c0 / self.cw;
            let cx1 = (c1.saturating_sub(1)) / self.cw;
            for ry in ry0..=ry1 {
                for cx in cx0..=cx1.min(self.across - 1) {
                    let i = ry * self.across + cx;
                    if i < self.count {
                        v.push(i);
                    }
                }
            }
        } else {
            for ry in ry0..=ry1 {
                if ry < self.count {
                    v.push(ry);
                }
            }
        }
        v
    }
}

fn band_kind(ct: ColorType) -> (BandKind, usize) {
    match ct {
        ColorType::Gray(_) => (BandKind::Gray, 1),
        ColorType::GrayA(_) => (BandKind::GrayAlpha, 2),
        ColorType::RGB(_) | ColorType::YCbCr(_) => (BandKind::Rgb, 3),
        ColorType::RGBA(_) | ColorType::CMYK(_) => (BandKind::Rgba, 4),
        ColorType::Palette(_) => (BandKind::Palette, 1),
        _ => (BandKind::Gray, 1),
    }
}

fn bit_depth(ct: ColorType) -> u8 {
    match ct {
        ColorType::Gray(b) | ColorType::GrayA(b) | ColorType::RGB(b) | ColorType::RGBA(b) | ColorType::Palette(b) | ColorType::CMYK(b) | ColorType::YCbCr(b) => b,
        _ => 8,
    }
}

/// Sample a pixel window [c0,c1) x [r0,r1) into an ow x oh grid of raw band values.
fn sample_window(path: &Path, c0: f64, c1: f64, r0: f64, r1: f64, ow: usize, oh: usize, progress: Option<&dyn Fn(f32)>) -> Result<(Vec<f32>, usize, ColorType, Option<Vec<[u8; 3]>>), String> {
    let mut d = open(path)?;
    let ct = d.colortype().map_err(|e| e.to_string())?;
    let palette = if matches!(ct, ColorType::Palette(_)) {
        d.get_tag_u16_vec(Tag::ColorMap).ok().map(|m| {
            let n = m.len() / 3;
            (0..n).map(|i| [(m[i] >> 8) as u8, (m[n + i] >> 8) as u8, (m[2 * n + i] >> 8) as u8]).collect()
        })
    } else {
        None
    };
    let lay = layout(&mut d)?;
    let (_, spp_guess) = band_kind(ct);
    let cols: Vec<usize> = (0..ow).map(|x| ((c0 + (x as f64 + 0.5) * (c1 - c0) / ow as f64) as usize).min(lay.w - 1)).collect();
    let rows: Vec<usize> = (0..oh).map(|y| ((r0 + (y as f64 + 0.5) * (r1 - r0) / oh as f64) as usize).min(lay.h - 1)).collect();
    let mut spp = spp_guess;
    let mut out = vec![f32::NAN; ow * oh * 4];
    let (cmin, cmax) = (*cols.first().unwrap_or(&0), *cols.last().unwrap_or(&0) + 1);
    let (rmin, rmax) = (*rows.first().unwrap_or(&0), *rows.last().unwrap_or(&0) + 1);
    let chunks = lay.chunks_for(cmin, cmax, rmin, rmax);
    let total = chunks.len().max(1);
    for (ci, idx) in chunks.into_iter().enumerate() {
        if let Some(p) = progress {
            p(ci as f32 / total as f32);
        }
        let (ox, oy) = lay.origin(idx);
        let (dw, dh) = d.chunk_data_dimensions(idx as u32);
        let (dw, dh) = (dw as usize, dh as usize);
        // Skip chunks with no sampled row/column.
        let ys: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| **r >= oy && **r < oy + dh).map(|(i, _)| i).collect();
        if ys.is_empty() {
            continue;
        }
        let xs: Vec<usize> = cols.iter().enumerate().filter(|(_, c)| **c >= ox && **c < ox + dw).map(|(i, _)| i).collect();
        if xs.is_empty() {
            continue;
        }
        let buf = match d.read_chunk(idx as u32) {
            Ok(b) => to_f32(b),
            Err(_) => continue,
        };
        if dh == 0 {
            continue;
        }
        // Row stride from the buffer (tiles may be padded to full tile width).
        let per_row = buf.len() / dh.max(1);
        let stride_px = if per_row >= lay.cw * spp && lay.tiled { lay.cw } else { dw };
        if spp_guess == 1 && per_row >= dw * 3 && per_row % dw == 0 && !lay.tiled {
            spp = per_row / dw;
        }
        for &yi in &ys {
            let r = rows[yi] - oy;
            for &xi in &xs {
                let c = cols[xi] - ox;
                let base = (r * stride_px + c) * spp;
                if base + spp > buf.len() {
                    continue;
                }
                let o = (yi * ow + xi) * 4;
                for b in 0..spp.min(4) {
                    out[o + b] = buf[base + b];
                }
            }
        }
    }
    Ok((out, spp, ct, palette))
}

fn make_style(vals: &[f32], spp: usize, ct: ColorType, palette: Option<Vec<[u8; 3]>>, nodata: Option<f64>) -> Style {
    let (mut kind, _) = band_kind(ct);
    if kind == BandKind::Gray && spp >= 3 {
        kind = if spp >= 4 { BandKind::Rgba } else { BandKind::Rgb };
    }
    let bands = match kind {
        BandKind::Gray | BandKind::Palette => 1,
        BandKind::GrayAlpha => 1,
        _ => 3,
    };
    let depth = bit_depth(ct);
    let (lo, hi) = if depth == 8 || kind == BandKind::Palette {
        (0.0, 255.0)
    } else {
        // 2-98 % stretch over the colour bands.
        let mut v: Vec<f32> = vals.chunks(4).flat_map(|p| p[..bands].to_vec()).filter(|x| x.is_finite() && nodata.map(|n| (*x as f64 - n).abs() > 1e-9).unwrap_or(true)).collect();
        if v.is_empty() {
            (0.0, 1.0)
        } else {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let lo = v[(v.len() as f32 * 0.02) as usize] as f64;
            let hi = v[((v.len() as f32 * 0.98) as usize).min(v.len() - 1)] as f64;
            (lo, if hi > lo { hi } else { lo + 1.0 })
        }
    };
    Style { bands, kind, lo, hi, palette, nodata }
}

fn colorize(vals: &[f32], w: usize, h: usize, st: &Style) -> Raster {
    let mut rgba = vec![0u8; w * h * 4];
    let sc = |v: f32| -> u8 { (((v as f64 - st.lo) / (st.hi - st.lo)) * 255.0).clamp(0.0, 255.0) as u8 };
    let alpha_scale = |v: f32| -> u8 { if st.hi > 255.0 { (v / 257.0).clamp(0.0, 255.0) as u8 } else { v.clamp(0.0, 255.0) as u8 } };
    for i in 0..w * h {
        let p = &vals[i * 4..i * 4 + 4];
        let o = &mut rgba[i * 4..i * 4 + 4];
        if !p[0].is_finite() || st.nodata.map(|n| (p[0] as f64 - n).abs() < 1e-9).unwrap_or(false) {
            continue; // transparent
        }
        match st.kind {
            BandKind::Gray => {
                let g = sc(p[0]);
                o.copy_from_slice(&[g, g, g, 255]);
            }
            BandKind::GrayAlpha => {
                let g = sc(p[0]);
                o.copy_from_slice(&[g, g, g, alpha_scale(p[1])]);
            }
            BandKind::Palette => {
                let c = st.palette.as_ref().and_then(|pl| pl.get(p[0] as usize)).copied().unwrap_or([0, 0, 0]);
                o.copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
            BandKind::Rgb => o.copy_from_slice(&[sc(p[0]), sc(p[1]), sc(p[2]), 255]),
            BandKind::Rgba => {
                let a = if p[3].is_finite() { alpha_scale(p[3]) } else { 255 };
                o.copy_from_slice(&[sc(p[0]), sc(p[1]), sc(p[2]), a]);
            }
        }
    }
    Raster { w, h, rgba }
}

/// Read georeference and an overview no larger than OVERVIEW_MAX pixels on a side.
pub fn load(path: &Path, progress: &dyn Fn(f32)) -> Result<Loaded, String> {
    let info = read_info(path)?;
    let (w, h) = (info.width as f64, info.height as f64);
    let factor = (w.max(h) / OVERVIEW_MAX as f64).max(1.0);
    let ow = ((w / factor).round() as usize).max(1);
    let oh = ((h / factor).round() as usize).max(1);
    let (vals, spp, ct, pal) = sample_window(path, 0.0, w, 0.0, h, ow, oh, Some(progress))?;
    let style = make_style(&vals, spp, ct, pal, info.nodata);
    let overview = colorize(&vals, ow, oh, &style);
    Ok(Loaded { info, overview, ov_factor: factor, style })
}

/// Full-resolution read of a pixel window, at most DETAIL_MAX pixels on a side.
pub fn read_detail(path: &Path, style: &Style, c0: f64, c1: f64, r0: f64, r1: f64, want_w: usize, want_h: usize) -> Result<Raster, String> {
    let ow = want_w.clamp(1, DETAIL_MAX).min(((c1 - c0).ceil() as usize).max(1));
    let oh = want_h.clamp(1, DETAIL_MAX).min(((r1 - r0).ceil() as usize).max(1));
    let (vals, _, _, _) = sample_window(path, c0, c1, r0, r1, ow, oh, None)?;
    Ok(colorize(&vals, ow, oh, style))
}

pub struct Basemap {
    pub name: String,
    pub path: PathBuf,
    pub info: GeoInfo,
    pub crs: CrsSpec,
    pub overview: Raster,
    pub ov_factor: f64,
    pub style: Style,
    pub visible: bool,
    pub opacity: f32,
    pub tex: Option<egui::TextureHandle>,
    /// Overview mesh vertices in project coordinates (cached per project CRS).
    pub mesh_cache: Option<(CrsSpec, Vec<[f64; 2]>)>,
    pub detail: Option<Detail>,
    pub detail_pending: Option<([i64; 4], std::sync::mpsc::Receiver<Result<(Raster, [f64; 4]), String>>)>,
}

pub struct Detail {
    /// Pixel window [c0, c1, r0, r1] in full-resolution pixels.
    pub window: [f64; 4],
    pub key: [i64; 4],
    pub raster: Raster,
    pub tex: Option<egui::TextureHandle>,
    pub mesh: Option<(CrsSpec, Vec<[f64; 2]>)>,
}

pub const GRID: usize = 16;

/// Vertex positions (project coordinates) of a GRID x GRID mesh over a pixel window.
pub fn mesh_vertices(info: &GeoInfo, src: &CrsSpec, dst: &CrsSpec, win: [f64; 4]) -> Vec<[f64; 2]> {
    let mut pts = Vec::with_capacity((GRID + 1) * (GRID + 1));
    for j in 0..=GRID {
        for i in 0..=GRID {
            let col = win[0] + (win[1] - win[0]) * i as f64 / GRID as f64;
            let row = win[2] + (win[3] - win[2]) * j as f64 / GRID as f64;
            let w = info.pixel_to_world(col, row);
            pts.push([w[0], w[1], 0.0]);
        }
    }
    let _ = crate::crs::transform_spec(src, dst, &mut pts);
    pts.into_iter().map(|p| [p[0], p[1]]).collect()
}
