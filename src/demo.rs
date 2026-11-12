//! Synthetic demo survey: three 400 MHz profiles with pipes, a layer and GPS tracks (LV95).

use crate::crs::CrsSpec;
use crate::model::{GpsFix, GpsTrack, Profile, Section};
use std::path::PathBuf;

fn ricker(t: f32, f_ghz: f32) -> f32 {
    let a = (std::f32::consts::PI * f_ghz * t).powi(2);
    (1.0 - 2.0 * a) * (-a).exp()
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        ((self.0 >> 11) as f32 / (1u64 << 53) as f32) * 2.0 - 1.0
    }
}

pub fn make() -> Vec<Profile> {
    let mut out = Vec::new();
    let v = 0.1f32; // m/ns
    let f = 0.4f32; // GHz
    let t_zero = 6.0f32; // ns of air before the direct wave
    let lines: [(f64, f64, f64, &[(f32, f32)]); 3] = [
        // start E, start N, heading (deg from east), pipes (x m, depth m)
        (2_600_120.0, 1_199_860.0, 20.0, &[(6.0, 0.6), (14.5, 1.1), (22.0, 0.45)]),
        (2_600_118.0, 1_199_866.0, 20.0, &[(6.4, 0.62), (15.0, 1.12), (21.0, 0.8)]),
        (2_600_128.0, 1_199_848.0, 110.0, &[(5.0, 0.9), (12.0, 0.5)]),
    ];
    for (k, (e0, n0, head, pipes)) in lines.iter().enumerate() {
        let ns = 512;
        let dt = 0.1f32;
        let dx = 0.05f32;
        let ntr = if k == 2 { 400 } else { 600 };
        let mut sec = Section::new(ns, ntr, dt);
        let mut rng = Rng(0x9e3779b97f4a7c15 ^ (k as u64 + 1) * 7919);
        for tr in 0..ntr {
            let x = tr as f32 * dx;
            // layer depth (m)
            let layer = 1.45 + 0.18 * (x * 0.35 + k as f32).sin() + 0.04 * (x * 1.7).cos();
            let col = sec.trace_mut(tr);
            for s in 0..ns {
                let t = s as f32 * dt;
                let tt = t - t_zero;
                let att = (-(tt.max(0.0)) * 0.07).exp();
                let mut a = 9000.0 * ricker(tt, f) + 2500.0 * ricker(tt - 2.6, f); // direct wave + ground ringing
                a += 1300.0 * ricker(tt - 2.0 * layer / v, f) * att;
                for (px, pz) in pipes.iter() {
                    let t0 = 2.0 * pz / v;
                    let th = (t0 * t0 + (2.0 * (x - px) / v).powi(2)).sqrt();
                    let w = t0 / th.max(0.1);
                    a += 3200.0 * ricker(tt - th, f) * att * w;
                }
                // system ringing (horizontal bands), low-frequency wow and noise
                a += 400.0 * (t * 2.0).sin() * (t > t_zero) as i32 as f32;
                a += 1500.0 * (-t / 30.0).exp();
                a += 35.0 * rng.next();
                col[s] = a;
            }
        }
        let name = format!("DEMO_{:03}", k + 1);
        let mut p = Profile::new(name.clone(), PathBuf::from(format!("{name}.demo")), "Demo (synthetic)", sec, dx);
        p.freq_mhz = 400.0;
        p.velocity = v;
        p.header = vec![("source".into(), "synthetic demo data".into()), ("antenna".into(), "400 MHz".into())];
        // GPS fix every 10 traces with a little wobble.
        let h = head.to_radians();
        let mut fixes = Vec::new();
        for tr in (0..ntr).step_by(10).chain(std::iter::once(ntr - 1)) {
            let d = tr as f64 * dx as f64;
            let wob = 0.15 * (d * 0.4).sin();
            fixes.push(GpsFix { trace: tr as f64, x: e0 + d * h.cos() - wob * h.sin(), y: n0 + d * h.sin() + wob * h.cos(), z: 540.0 + 0.05 * d });
        }
        p.gps = Some(GpsTrack { source: "demo GPS (LV95)".into(), crs: CrsSpec::Epsg(2056), fixes });
        out.push(p);
    }
    out
}
