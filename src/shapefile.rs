//! Shapefile geometry for map overlays. Attributes are not needed for drawing.

use crate::crs::CrsSpec;
use std::path::{Path, PathBuf};

pub struct Overlay {
    pub path: PathBuf,
    pub name: String,
    pub crs: CrsSpec,
    pub visible: bool,
    pub parts: Vec<Vec<[f64; 2]>>,
}

fn le_i32(b: &[u8], at: usize) -> Result<i32, String> {
    Ok(i32::from_le_bytes(b.get(at..at + 4).ok_or("truncated shapefile")?.try_into().unwrap()))
}
fn le_f64(b: &[u8], at: usize) -> Result<f64, String> {
    Ok(f64::from_le_bytes(b.get(at..at + 8).ok_or("truncated shapefile")?.try_into().unwrap()))
}

pub fn load(path: PathBuf, fallback_crs: &CrsSpec) -> Result<Overlay, String> {
    let b = std::fs::read(&path).map_err(|e| e.to_string())?;
    if b.len() < 100 || b.get(0..4) != Some(&[0, 0, 0x27, 0x0a]) || le_i32(&b, 28)? != 1000 {
        return Err("invalid shapefile header".into());
    }
    let mut parts = Vec::new();
    let mut at = 100;
    while at < b.len() {
        let header = b.get(at..at + 8).ok_or("truncated shapefile record header")?;
        let bytes = usize::try_from(i32::from_be_bytes(header[4..8].try_into().unwrap()))
            .map_err(|_| "invalid shapefile record length")?.checked_mul(2).ok_or("invalid shapefile record length")?;
        at += 8;
        let rec = b.get(at..at.checked_add(bytes).ok_or("invalid shapefile record length")?).ok_or("truncated shapefile record")?;
        at += bytes;
        let kind = le_i32(rec, 0)?;
        match kind {
            0 => {},
            1 | 11 | 21 => parts.push(vec![[le_f64(rec, 4)?, le_f64(rec, 12)?]]),
            3 | 5 | 13 | 15 | 23 | 25 | 8 | 18 | 28 => {
                let (starts, n, points_at) = if matches!(kind, 8 | 18 | 28) {
                    (vec![0], usize::try_from(le_i32(rec, 36)?).map_err(|_| "invalid point count")?, 40)
                } else {
                    let count = usize::try_from(le_i32(rec, 36)?).map_err(|_| "invalid part count")?;
                    let n = usize::try_from(le_i32(rec, 40)?).map_err(|_| "invalid point count")?;
                    let end = 44usize.checked_add(count.checked_mul(4).ok_or("invalid part count")?).ok_or("invalid part count")?;
                    if end > rec.len() { return Err("truncated shapefile parts".into()); }
                    let mut starts = Vec::with_capacity(count);
                    for i in 0..count { starts.push(usize::try_from(le_i32(rec, 44 + i * 4)?).map_err(|_| "invalid part index")?); }
                    (starts, n, end)
                };
                let end = points_at.checked_add(n.checked_mul(16).ok_or("invalid point count")?).ok_or("invalid point count")?;
                if end > rec.len() || (n > 0 && starts.first() != Some(&0)) || starts.windows(2).any(|w| w[0] >= w[1]) || starts.last().is_some_and(|&s| s >= n) {
                    return Err("invalid shapefile points or parts".into());
                }
                for (i, &start) in starts.iter().enumerate() {
                    let stop = starts.get(i + 1).copied().unwrap_or(n);
                    let mut part = Vec::with_capacity(stop - start);
                    for j in start..stop { part.push([le_f64(rec, points_at + j * 16)?, le_f64(rec, points_at + j * 16 + 8)?]); }
                    if matches!(kind, 8 | 18 | 28) { parts.extend(part.into_iter().map(|p| vec![p])); }
                    else { parts.push(part); }
                }
            }
            _ => return Err(format!("unsupported shapefile shape type {kind}")),
        }
    }
    let crs = read_crs(&path).unwrap_or_else(|| fallback_crs.clone());
    Ok(Overlay { name: path.file_name().unwrap_or_default().to_string_lossy().into_owned(), path, crs, visible: true, parts })
}

fn read_crs(path: &Path) -> Option<CrsSpec> {
    let prj = std::fs::read_to_string(path.with_extension("prj")).or_else(|_| std::fs::read_to_string(path.with_extension("PRJ"))).ok()?;
    // The final EPSG authority in WKT names the complete projected CRS.
    let upper = prj.to_ascii_uppercase();
    let pos = upper.rfind("EPSG")?;
    let code: String = upper[pos + 4..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    CrsSpec::parse(&code)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn loads_polyline_and_prj_crs() {
        let dir = std::env::temp_dir().join(format!("gpr-shp-line-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("line.shp");
        let mut b = vec![0; 100];
        b[2..4].copy_from_slice(&[0x27, 0x0a]);
        b[28..32].copy_from_slice(&1000i32.to_le_bytes());
        let mut rec = vec![0; 80];
        rec[0..4].copy_from_slice(&3i32.to_le_bytes());
        rec[36..40].copy_from_slice(&1i32.to_le_bytes());
        rec[40..44].copy_from_slice(&2i32.to_le_bytes());
        rec[48..56].copy_from_slice(&1.0f64.to_le_bytes());
        rec[56..64].copy_from_slice(&2.0f64.to_le_bytes());
        rec[64..72].copy_from_slice(&3.0f64.to_le_bytes());
        rec[72..80].copy_from_slice(&4.0f64.to_le_bytes());
        b.extend_from_slice(&1i32.to_be_bytes());
        b.extend_from_slice(&40i32.to_be_bytes());
        b.extend_from_slice(&rec);
        std::fs::write(&path, b).unwrap();
        std::fs::write(path.with_extension("prj"), "PROJCS[\"test\",AUTHORITY[\"EPSG\",\"2056\"]]").unwrap();
        let layer = load(path.clone(), &CrsSpec::Epsg(4326)).unwrap();
        assert_eq!(layer.crs, CrsSpec::Epsg(2056));
        assert_eq!(layer.parts, vec![vec![[1.0, 2.0], [3.0, 4.0]]]);
        std::fs::remove_file(path.with_extension("prj")).unwrap();
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn rejects_truncated_record() {
        let dir = std::env::temp_dir().join(format!("gpr-shp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.shp");
        let mut b = vec![0; 108];
        b[2..4].copy_from_slice(&[0x27, 0x0a]);
        b[28..32].copy_from_slice(&1000i32.to_le_bytes());
        b[104..108].copy_from_slice(&10i32.to_be_bytes());
        std::fs::write(&path, b).unwrap();
        assert!(load(path.clone(), &CrsSpec::default()).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
}
