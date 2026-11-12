# GPR Studio – mechanics sheet

Sources: the original program `Reflexw.EXE` (strings, form resources and code read with Craft Anything,
evidence ids below) and the REFLEXW manual shipped beside it (`reflexw_manual_a4.pdf`, section 1.11).
Reflexw is stripped Delphi: the processing click handlers only set up dialogs (for example
`FUN_00902e94`, the "KirchhoffMigrationClick" handler at 0x00902e94, sets the parameters
"summation width", "velocity", "start time", "end time"); the numerical formulas below are from the
manual's descriptions of each step. Values marked *estimated* were looked for and not found.

## Import (original list: form `InputFormatBox`, .rsrc 0x4ae0a00, ev_56d799…)
SEGY, SEG2, RADAN (GSSI), PULSEEKKO, MALA RD3/RD6/RD7, IDS, UTSI, SEGY-PRISM … Implemented:

| Format | Layout used |
|---|---|
| GSSI .dzt | header: rh_data@2, nsamp@4, bits@6, sps@10, spm@14, position@22, range@26, nchan@52, epsr@54, antenna name@98; data starts at rh_data·1024 (if <1024); 8-bit −128, 16-bit −32768, 32-bit signed; channels interleaved per scan; first 2 samples zeroed (scan/mark words). dt = range/nsamp, dx = 1/spm. |
| MALA .rd3/.rd7 + .rad | SAMPLES, FREQUENCY (sampling MHz → dt = 1000/f), DISTANCE INTERVAL, ANTENNAS; int16 (rd3) / int32 (rd7) |
| ImpulseRadar .iprb + .iprh | SAMPLES, FREQUENCY, USER DISTANCE INTERVAL, DATA VERSION (16/32 bit), ANTENNA |
| S&S .dt1 + .hd | NUMBER OF PTS/TRC, TOTAL TIME WINDOW, STEP SIZE USED, NOMINAL FREQUENCY, TIMEZERO AT POINT; 128-byte trace header + int16 |
| SEG-Y | 3200+400 header, byte order detected from format code @3225; formats 1 (IBM), 2, 3, 5, 8; dt @3217 read as **picoseconds** by default for GPR (Reflexw option "segy-timeincrement assumed in picosec", ev_013456…); coordinates bytes 73-88 with scalar @71, elevation @41 with scalar @69, units @89 (2 = arc seconds) |
| REFLEXW .??R/.??T | samples u16@0x1a4, traces u32@0x1c4, dx f32@0x1cc, dt f32@0x1d0; trace = 156-byte header + int16 (or float32) samples (measured on DEMODATA files) |
| IDS native .dt | **not readable** (layout unknown) – use the SEG-Y export |

GPS (original list `UpdateTraceHeaders`, .rsrc 0x4ae4300, ev_d2e56b…: RAMAC-GPS, CSV-GPS, IDS-GPS, PulseEkko-GPS, GSSI-GPS, …):
.cor (MALA/ImpulseRadar, trace numbers 1-based, decimal degrees), .dzg ($GSSIS,trace + GGA), S&S .gps ("Trace #n" + GGA),
NMEA log (spread evenly – *estimated* when no trace numbers), CSV/TXT table, SEG-Y header coordinates.
Positions between fixes: linear interpolation by trace number; ends held.
Automatic GPS association (one dialog for all imported profiles): GPS files are searched in a chosen folder, the data folder and
its `GPS` sub-folder, and matched by name in this order: same name (1000) › same name without a channel ending `_0`, `_12`,
`_A1`, `_ch2` (900) › GPS name that starts the data name (600–800) › same text and same last profile number (500) › same
number with a similar prefix (200+). A 4-digit profile number like `_0001` is never treated as a channel ending.
Original coordinate conversions: UTM, UTM-degree, RD (NL), Swiss CH1903 only; here any EPSG code (≈6000 built in) or PROJ string, default **EPSG:2056**.

## Processing (manual §1.11)
- **Move start time / set time zero** (§1.11.3.3): data before time zero removed, start time set to 0. Automatic first break: first sample whose |amplitude| ≥ threshold × max of the mean |trace|; threshold default 20 % (*estimated*).
- **Subtract-mean (dewow)** (§1.11.1.10): running mean over the window subtracted from the centre sample; window ≈ one period (default 1000/f ns).
- **Subtract DC shift** (§1.11.1.11): mean between time 1 and time 2 subtracted from the whole trace.
- **Background removal** (§1.11.8.3): mean trace over the chosen traces (whole profile or running window) subtracted, only inside start–end time.
- **Bandpass frequency** (§1.11.1.3): FFT; 4 corner frequencies; cosine or cosine² tapers between f1–f2 and f3–f4, zero outside.
- **Bandpass Butterworth** (§1.11.1.4): recursive time-domain filter, low cut + high cut, order N; run forward and backward (zero phase – *estimated*, the manual does not say).
- **AGC gain** (§1.11.2.1): gain = mean |a| of whole trace / mean |a| in window, limited by max. gain, times scale.
- **Energy decay** (§1.11.2.2): mean decay curve over all traces, median filtered (11 samples – *estimated*), data divided by it, times scale.
- **Gain function** (§1.11.2.3): g(t) = (1 + a·t)·e^(b·t), a = a′/pulse width (pulse width = 1000/f ns), b = b′·v/8.69 with v = 0.1 m/ns, b′ in dB/m; g = 1 at start time; capped by max. gain.
- **Divergence compensation** (§1.11.2.5): × t (spherical) or × √t (line source).
- **Diffraction stack** (§1.11.9.1): unweighted sum along t = √(t0² + (2x/v)²) over the summation width.
- **Kirchhoff migration** (§1.11.9.2): time derivative first, then weighted sum (obliquity t0/t) along the hyperbola; parameters summation width, velocity, start/end time (FUN_00902e94).
- **FK migration (Stolt)** (§1.11.9.3): 2D FFT, ω = sign(ωτ)·√(ωτ² + (v·kx/2)²), Jacobian |ωτ|/ω, no interpolation in the original – linear here; time window enlarged by the scaling factor (default 2).
- **Time-depth conversion** (§1.11.9.8): constant velocity, z = v·t/2.

## Picking
Point markers, hyperbola fit (v = 2|Δx| / √(t² − t0²), ε = (0.2998/v)²), layer lines with phase following
(snap to |max|, max, min or zero crossing within ± N samples). Depth = v·t/2; elevation = GPS height − depth.
Marker types (name, colour, symbol, point/line) are editable. Originals' pick categories are not documented – the default list is *estimated* for utility surveys.

Controls in the picking modes: right-click a pick (marker, fitted hyperbola, layer line, or the hyperbola / line in progress)
deletes it; Shift + right-drag deletes every pick inside the box. Deleting only ever touches picks of the marker type selected
in the toolbar (other types are kept and the status line says so). No undo.
A layer line ends with a double-click (or Enter); the next click starts a new line. With Snap = None the
phase follow draws straight between clicks (no snapping at all).

## Background work
Processing, file import and opening a project run on a background thread. The status bar shows a progress bar at
once; after 2 s a popup shows the job, the current profile and step, a progress bar, the time left and Cancel.
The bar is shared between steps by rough cost (Kirchhoff/diffraction stack 40, FK 15, filters 2, others 1 – estimated).
Map labels show the profile number only: the last group of digits after removing a channel ending (`_0` HF, `_1` LF).

## SEG-Y export
SEG-Y rev 1 (0x0100 @3501), big endian, format 5 (IEEE float), fixed trace length, sorting 4, metres; EBCDIC text header with
profile, content, sample interval, CRS, header byte map and the processing steps. Sample interval in **picoseconds** at
3217/117 (depth sections: micrometres). Trace header: trace no @1/5/9/21/193, line no @189, X/Y @73/77, 81/85, 181/185 with
scalar -100 (cm) at 71 (geographic CRS: arc seconds, scalar -1000, units 2 @89), elevation @41/45 scalar -100 at 69,
distance along profile @197 with scalar -100 at 201. One file per profile and checked step: data processed up to and including
that step. Verified with segyio and the app's own reader.

## Exports
GeoPackage 1.2 (application_id 0x47504B47, user_version 10200): `profiles` (LineStringZ), `trace_points`, `markers` (PointZ),
`layer_picks` (LineStringZ, elevation = surface − depth) and `layer_pick_vertices`; SRS = project CRS (WKT from the EPSG table).
PDF: A4/A3 landscape; per page radargram section + GPS map with the same section highlighted + tables.
