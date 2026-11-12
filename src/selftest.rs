//! `--selftest <out_dir> [files...]`: import files, run every processing step on the demo,
//! write GeoPackage, PDF and CSV exports, and print a summary. No window is opened.

use crate::app::{App, PdfRange};
use crate::crs::CrsSpec;
use crate::processing::{Ctx, Step};
use std::path::PathBuf;

pub fn run(out: PathBuf, files: Vec<PathBuf>) -> i32 {
    let _ = std::fs::create_dir_all(&out);
    let mut fails = 0;
    // CRS round trip
    let mut pts = vec![[2_600_000.0, 1_200_000.0, 500.0]];
    let r1 = crate::crs::transform_spec(&CrsSpec::Epsg(2056), &CrsSpec::Epsg(4326), &mut pts);
    println!("CRS 2056 -> 4326: {:?} {:?}", r1, pts[0]);
    let r2 = crate::crs::transform_spec(&CrsSpec::Epsg(4326), &CrsSpec::Epsg(2056), &mut pts);
    println!("CRS 4326 -> 2056: {:?} {:?}", r2, pts[0]);
    if (pts[0][0] - 2_600_000.0).abs() > 0.01 || (pts[0][1] - 1_200_000.0).abs() > 0.01 {
        println!("FAIL: CRS round trip");
        fails += 1;
    }
    let mut utm = vec![[7.4386, 46.9511, 0.0]];
    let _ = crate::crs::transform_spec(&CrsSpec::Epsg(4326), &CrsSpec::Epsg(32632), &mut utm);
    println!("Bern WGS84 -> UTM32N: {:.2} {:.2}", utm[0][0], utm[0][1]);

    // GPS file name matching
    let base = "20181202 la repetta_2026-10-14_";
    let cases: Vec<(String, Vec<String>, Option<String>)> = vec![
        (format!("{base}0002_0.iprb"), vec![format!("{base}0001.cor"), format!("{base}0002.cor"), format!("{base}0010.cor")], Some(format!("{base}0002.cor"))),
        (format!("{base}0002_1.iprb"), vec![format!("{base}0001.cor"), format!("{base}0002.cor")], Some(format!("{base}0002.cor"))),
        (format!("{base}0001_0.iprb"), vec![format!("{base}0001_0.cor"), format!("{base}0001.cor")], Some(format!("{base}0001_0.cor"))),
        ("Proj_0012_A01.iprb".into(), vec!["Proj_0011.cor".into(), "Proj_0012.cor".into()], Some("Proj_0012.cor".into())),
        ("FILE____005.DZT".into(), vec!["FILE____004.DZG".into(), "FILE____005.DZG".into()], Some("FILE____005.DZG".into())),
        ("LINE_0003.iprb".into(), vec!["LINE_0003.cor".into(), "LINE_0030.cor".into()], Some("LINE_0003.cor".into())),
        ("X_0002_0.iprb".into(), vec!["X_0020.cor".into()], None),
    ];
    for (data, files, want) in cases {
        let files: Vec<PathBuf> = files.iter().map(PathBuf::from).collect();
        let got = crate::gps::best_match(std::path::Path::new(&data), &files).map(|b| b.0.to_string_lossy().to_string());
        let ok = got == want;
        println!("MATCH {} {data} -> {:?}", if ok { "ok  " } else { "FAIL" }, got);
        if !ok {
            fails += 1;
        }
    }

    // Imports
    let opts = crate::formats::ImportOptions::default();
    let is_tif = |f: &PathBuf| f.extension().map(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff")).unwrap_or(false);
    for f in files.iter().filter(|f| !is_tif(f)) {
        match crate::formats::load(f, &opts) {
            Ok(list) => {
                for p in list {
                    let maxabs = p.raw.data.iter().fold(0f32, |a, v| a.max(v.abs()));
                    println!("IMPORT OK {} [{}]: {} traces x {} samples, dt {:.4} ns, dx {:.4} m, freq {} MHz, max|a| {:.0}, embedded GPS {}", p.name, p.format, p.raw.ntr, p.raw.ns, p.raw.dt, p.dx, p.freq_mhz, maxabs, p.embedded_gps.as_ref().map(|g| g.fixes.len()).unwrap_or(0));
                    for (k, v) in &p.header {
                        println!("    {k}: {v}");
                    }
                    if let Some(c) = crate::gps::find_candidate(&p.path) {
                        match crate::gps::read(&c, crate::gps::GpsFormat::Auto, p.raw.ntr, &CrsSpec::Epsg(2056)) {
                            Ok(t) => println!("    GPS {}: {} fixes", t.source, t.fixes.len()),
                            Err(e) => println!("    GPS candidate {} unreadable: {e}", c.display()),
                        }
                    }
                }
            }
            Err(e) => {
                println!("IMPORT FAIL {}: {e}", f.display());
                fails += 1;
            }
        }
    }

    // Batch GPS association for the imported files
    {
        let data: Vec<PathBuf> = files.iter().filter(|f| !is_tif(f)).cloned().collect();
        if !data.is_empty() {
            let mut a2 = App::headless(data, None);
            let mut b = crate::app::GpsBatch { asked: true, rows: Vec::new(), folder: String::new(), format: crate::gps::GpsFormat::Auto, table_crs: "2056".into(), use_embedded: true };
            while let Some(id) = a2.gps_queue.pop_front() {
                if let Some(p) = a2.profiles.iter().find(|p| p.id == id) {
                    b.rows.push(crate::app::GpsRow { profile_id: id, name: p.name.clone(), path: String::new(), score: 0, include: false, has_embedded: p.embedded_gps.is_some(), status: None });
                }
            }
            a2.rematch_gps(&mut b);
            let matched = b.rows.iter().filter(|r| !r.path.is_empty()).count();
            let (ok, bad) = a2.load_gps_batch(&mut b);
            println!("GPS BATCH: {} profiles, {matched} matched, {ok} loaded, {bad} failed", b.rows.len());
            for r in &b.rows {
                println!("    {} -> {} {:?}", r.name, std::path::Path::new(&r.path).file_name().and_then(|s| s.to_str()).unwrap_or("-"), r.status);
            }
        }
    }

    // Processing on the demo
    let mut app = App::headless(files.iter().filter(|f| is_tif(f)).cloned().collect(), None);
    app.load_demo(true);
    app.wait_jobs();
    println!("basemaps in app: {}", app.basemaps.len());
    let p = &app.profiles[0];
    let ctx = Ctx { dx: p.dx, freq_mhz: p.freq_mhz, ..Default::default() };
    for s in Step::defaults() {
        let t = std::time::Instant::now();
        let input = if matches!(s, Step::TimeZero { .. }) { &p.raw } else { &p.proc };
        match s.apply(input, &ctx) {
            Ok(o) => {
                let bad = o.data.iter().any(|v| !v.is_finite());
                let m = o.data.iter().fold(0f32, |a, v| a.max(v.abs()));
                println!("STEP {:<28} ok  {:>4} x {:<4} max|a| {:>10.2}  {:>6} ms{}", s.name(), o.ntr, o.ns, m, t.elapsed().as_millis(), if bad { "  NON-FINITE!" } else { "" });
                if bad {
                    fails += 1;
                }
            }
            Err(e) => {
                println!("STEP {} FAILED: {e}", s.name());
                fails += 1;
            }
        }
    }

    // Background processing and project opening with progress
    {
        let ctx = egui::Context::default();
        let mut a3 = App::headless(vec![], None);
        a3.load_demo(true);
        let ids: Vec<u32> = a3.profiles.iter().map(|p| p.id).collect();
        for p in a3.profiles.iter_mut() {
            p.steps.push(crate::processing::StepEntry { enabled: true, step: Step::KirchhoffMigration { velocity: 0.1, width: 60, t_start: 0.0, t_end: 1e9 } });
        }
        a3.request_process(ids);
        let t0 = std::time::Instant::now();
        let mut samples = Vec::new();
        while a3.busy() {
            a3.pump_tasks(&ctx);
            if let Some(t) = &a3.task {
                samples.push(format!("{:.0}% {}", t.progress.fraction() * 100.0, t.progress.text()));
            }
            std::thread::sleep(std::time::Duration::from_millis(150));
        }
        samples.dedup();
        println!("BG PROCESS done in {:.1} s; {}", t0.elapsed().as_secs_f32(), a3.status);
        for s in samples.iter().step_by((samples.len() / 6).max(1)) {
            println!("    progress: {s}");
        }
        let proj = out.join("selftest.gprproj");
        crate::project::save(&a3, &proj).ok();
        let mut a4 = App::headless(vec![], None);
        a4.open_project(proj);
        let t0 = std::time::Instant::now();
        let mut last = String::new();
        while a4.busy() {
            a4.pump_tasks(&ctx);
            if let Some(t) = &a4.task {
                last = format!("{} · {:.0}% {}", t.title, t.progress.fraction() * 100.0, t.progress.text());
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        println!("PROJECT reopen in {:.1} s (last progress: {last}); {}", t0.elapsed().as_secs_f32(), a4.status);
        if a4.profiles.len() != 3 || a4.markers.len() != 2 {
            println!("FAIL: project reopen");
            fails += 1;
        }
        for p in &a4.profiles {
            println!("    {} -> label '{}', {} steps", p.name, crate::model::short_label(&p.name), p.steps.len());
        }
        for n in ["20181202 la repetta_2026-10-14_0002_1", "Proj_0012_A01", "FILE____005", "Line A"] {
            println!("    label '{}' -> '{}'", n, crate::model::short_label(n));
        }
    }

    // SEG-Y export of chosen stages, read back with the importer
    {
        let ctx = egui::Context::default();
        let mut a5 = App::headless(vec![], None);
        a5.load_demo(true);
        let p = &a5.profiles[0];
        let job = crate::segy_export::SegyJob {
            meta: crate::segy_export::SegyMeta {
                name: p.name.clone(),
                source: "demo".into(),
                line_no: 1,
                coords: p.coords.clone(),
                dist: p.dist.clone(),
                crs_label: a5.project_crs.label(),
                geographic: false,
                freq_mhz: p.freq_mhz,
                velocity: p.velocity,
                dx: p.dx,
            },
            reload: None,
            raw: Some(p.raw.clone()),
            steps: p.steps.clone(),
            stages: vec![0, 2, 4],
            out_dir: out.clone(),
        };
        let (proc_ns, proc_c0, proc_max) = (p.proc.ns, p.coords[0], p.proc.data.iter().fold(0f32, |m, v| m.max(v.abs())));
        a5.task_queue.push_back(crate::tasks::Pending::Segy(vec![job]));
        while a5.busy() {
            a5.pump_tasks(&ctx);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        println!("SEGY EXPORT: {}", a5.status);
        let opts = crate::formats::ImportOptions::default();
        for tag in ["00_raw", "02_Subtract-mean_dewow", "04_Gain_function"] {
            let f = out.join(format!("DEMO_001_{tag}.sgy"));
            match crate::formats::load(&f, &opts) {
                Ok(l) => {
                    let q = &l[0];
                    let g = q.embedded_gps.as_ref();
                    let c0 = g.and_then(|g| g.fixes.first()).map(|f| (f.x, f.y, f.z));
                    let m = q.raw.data.iter().fold(0f32, |a, v| a.max(v.abs()));
                    println!("    {tag}: {} traces x {} samples, dt {:.4} ns, first coord {:?}, max|a| {:.1}", q.raw.ntr, q.raw.ns, q.raw.dt, c0, m);
                    if tag.starts_with("04") && (q.raw.ns != proc_ns || (m - proc_max).abs() > proc_max * 1e-4) {
                        println!("FAIL: stage 4 differs from the processed profile");
                        fails += 1;
                    }
                    if let Some((x, y, _)) = c0 {
                        if (x - proc_c0[0]).abs() > 0.011 || (y - proc_c0[1]).abs() > 0.011 {
                            println!("FAIL: coordinates differ");
                            fails += 1;
                        }
                    }
                }
                Err(e) => {
                    println!("FAIL: {tag} unreadable: {e}");
                    fails += 1;
                }
            }
        }
    }

    // Exports
    let g = out.join("selftest.gpkg");
    app.gpkg_opts.trace_points = true;
    match app.export_gpkg(&g) {
        Ok(s) => println!("GPKG OK {} ({s})", g.display()),
        Err(e) => {
            println!("GPKG FAIL {e}");
            fails += 1;
        }
    }
    app.pdf_opts.all_profiles = true;
    app.pdf_opts.range = PdfRange::Chunks;
    app.pdf_opts.chunk_m = 15.0;
    app.pdf_opts.title = "Self test".into();
    let pdfp = out.join("selftest.pdf");
    match crate::report::export(&app, &pdfp) {
        Ok(n) => println!("PDF OK {} ({n} pages)", pdfp.display()),
        Err(e) => {
            println!("PDF FAIL {e}");
            fails += 1;
        }
    }
    let csv = out.join("selftest.csv");
    match app.export_csv(&csv) {
        Ok(n) => println!("CSV OK {} ({n} picks)", csv.display()),
        Err(e) => {
            println!("CSV FAIL {e}");
            fails += 1;
        }
    }
    // Basemaps given as files
    for f in files.iter().filter(|f| f.extension().map(|e| e.eq_ignore_ascii_case("tif") || e.eq_ignore_ascii_case("tiff")).unwrap_or(false)) {
        match crate::basemap::load(f, &|_| {}) {
            Ok(l) => println!("BASEMAP OK {}: {}x{} overview {}x{} crs {:?} geo {:?}", f.display(), l.info.width, l.info.height, l.overview.w, l.overview.h, l.info.crs, l.info.geo),
            Err(e) => {
                println!("BASEMAP FAIL {}: {e}", f.display());
                fails += 1;
            }
        }
    }
    println!("SELFTEST {}", if fails == 0 { "PASSED".to_string() } else { format!("{fails} FAILURES") });
    fails
}
