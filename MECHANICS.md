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
- **Trace normalisation** (after dewow in the demo sequence): replace non-finite samples with zero, then divide each trace by the 1-based `round(0.9 × samples)`-th sorted absolute amplitude. A zero scale leaves the trace unchanged. This follows the legacy MATLAB `tracenormalize` function and is less sensitive to isolated peaks than maximum-amplitude scaling.
- **Subtract DC shift** (§1.11.1.11): mean between time 1 and time 2 subtracted from the whole trace.
- **Background removal** (§1.11.8.3): mean trace over the chosen traces (whole profile or running window) subtracted, only inside start–end time.
- **Bandpass frequency** (§1.11.1.3): FFT; 4 corner frequencies; cosine or cosine² tapers between f1–f2 and f3–f4, zero outside.
- **Bandpass Butterworth** (§1.11.1.4): recursive time-domain filter, low cut + high cut, order N; run forward and backward (zero phase – *estimated*, the manual does not say).
- **AGC gain** (§1.11.2.1): gain = mean |a| of whole trace / mean |a| in window, limited by max. gain, times scale.
- **Energy decay** (§1.11.2.2): mean decay curve over all traces, median filtered (11 samples – *estimated*), data divided by it, times scale.
- **Gain function** (§1.11.2.3): g(t) = (1 + a·t)·e^(b·t), a = a′/pulse width (pulse width = 1000/f ns), b = b′·v/8.69 with v = 0.1 m/ns, b′ in dB/m; g = 1 at start time; capped by max. gain.
- **Running average / moving average** (§1.11.8.1): every sample = mean over the current trace and traces/2 traces on each side (4 → 5 traces), window shortened at the profile ends, max. 1024 traces, only between start and end time. Smooths trace-to-trace level (gain) differences.
- **Average xy-filter** (§1.11.8.10): mean over traces/2 traces and samples/2 samples on each side of every point (max. 256 traces), start/end time.
- **AGC option "smooth gain over N traces"** (added, not in REFLEXW): the AGC gain curves are averaged over N neighbouring traces before they are applied.
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
in the toolbar (other types are kept and the status line says so). Ctrl+Z undoes the last 100 picking actions, including point and layer placement, hyperbola fitting, and deletion; repeated undo walks back through layer coordinates.
A layer line ends with a double-click (or Enter); the next click starts a new line. With Snap = None the
phase follow draws straight between clicks (no snapping at all).

## Crossing picks and interpolation zones
Crossings: every intersection of two GPS tracks (x/y segments in the project CRS; track and 32-segment bounding boxes
first). Profiles with the same map number (channels `_0`/`_1` of one line) are not crossings. The fractional trace
on each profile is linear along the crossing segment; the list is recomputed only when coordinates change.
A layer pick of P1 is shown on P2 when it covers the crossing trace (linear between its two neighbouring vertices,
no extrapolation): depth d = v₁·t₁/2, then t₂ = 2d/v₂, drawn like a P2 pick (depth axis, topography, reversed
display). Depth-matched, not elevation-matched: at one point both profiles share the surface.

Interpolation (IDW): input points are the vertices of the zone's pick type inside the polygon (even–odd rule),
densified along the profile every ≈ cell size, each with depth d = v·t/2 and altitude z = GPS height − d.
Cells are the cell-size grid over the polygon's bounding box, kept inside the polygon. For a cell, with the
points i within the **max radius** R (distance rᵢ), wᵢ = 1/rᵢᵖ (p = power, default 2):
depth = Σwᵢdᵢ/Σwᵢ and altitude = Σwᵢzᵢ/Σwᵢ (separately), v_mean = Σwᵢvᵢ/Σwᵢ, time = 2·depth/v_mean.
A point at the cell centre gives its own value; no point within R leaves the cell empty (only gaps between
picks are filled). Defaults: R = 5 m, cell 0.25 m, power 2. On a profile the surface is sampled at each trace
position (bilinear between the 4 neighbouring cells, nearest cell when one is empty) and drawn at t = 2·depth/v
of that profile. Map raster: Jet from the zone's minimum to maximum depth, 70 % opacity, empty cells transparent.

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

## Data view controls and topography
The main wheel scrolls vertically; the side wheel slides along the profile. Shift + wheel crops the time/depth axis from 0 down (top fixed at 0); Ctrl + wheel (or Ctrl + Shift) zooms horizontally. Zoomed out past the profile, the plot narrows from the right at the same scale.
Orient profiles (beside Fit) uses each GPS track's dominant direction to show west-to-east or north-to-south from left to right. This display direction is saved per profile; source traces, GPS positions, and picks keep their acquisition indices. Profiles without a clear GPS direction are skipped.
Horizontal zoom-out goes up to 50 × the profile length (independent of the window width); a view wider than the profile starts
at its first trace and the unused part is drawn in neutral dark grey (72, 72, 72).
"Show topography" (one setting for all profiles, needs GPS elevations that vary): display only. Each trace is shifted down by
2·(z_max − z)/v (time sections; v = the profile velocity in the toolbar, default 0.100 m/ns) or z_max − z (depth sections);
the left axis is time below the highest point, the right axis elevation = z_max − v·t/2. Picks are stored unshifted.

## Exports
GeoPackage 1.2 (application_id 0x47504B47, user_version 10200): `profiles` (LineStringZ), `trace_points`, `markers` (PointZ),
`layer_picks` (LineStringZ, elevation = surface − depth) and `layer_pick_vertices`; SRS = project CRS (WKT from the EPSG table).
`interpolations` (PointZ, one row per non-empty cell, z = altitude: zone, pick_type, time_ns, depth_m, altitude_m,
max_radius_m, cell_size_m) and `interpolation_zones` (Polygon, 2D). Points rather than a raster tile table: QGIS styles them with any ramp.
PDF: A4/A3 landscape; per page radargram section + GPS map framed to the whole survey, with the current profile and section highlighted + tables. The optional 1×1 metre scale is on by default: it keeps the chosen metres per page, uses equal horizontal and vertical metre scales when the depth fits, and compresses the vertical scale when needed to show the full depth. Whole-profile pages use the full vertical range; visible-section pages keep the current vertical view.

## Topographic Kirchhoff migration

The implemented weighting uses obliquity only; it omits the geometrical-spreading factor of a full amplitude-preserving Kirchhoff integral. This keeps the flat-surface limit consistent with GPR Studio's existing operator. Relative amplitudes are imaging values rather than calibrated reflectivity.

Let antenna `i` have horizontal chainage `x_i` (metres) and elevation `h_i` (metres, positive upward). GPS coordinates are transformed to the project's projected CRS; chainage sums horizontal XY distances, not terrain-following 3D distances and not the optional nominal trace spacing. The user supplies `H`, GPS receiver height above antenna, so `h_i = GPS_z_i - H`. No antenna height correction existed previously. If the GPS records antenna height, set `H=0`. The model assumes the vertical CRS and GPS height units are metres and consistent across the line; it does not convert ellipsoidal height to orthometric height.

Output trace `j` is at `x_j`, and its vertical coordinate is depth `d` below datum `D=max(h_i)`, so target elevation is `z=D-d`. For constant velocity `v` in metres/ns and zero source-receiver offset, the predicted two-way arrival at input trace `i` is `T_ij(d)=2 sqrt((x_i-x_j)^2+(h_i-z)^2)/v`. The migrated value is the average over the selected trace aperture of `cos(theta_i) * (A_i[k+1]-A_i[k-1])/2`, with `cos(theta_i)=(h_i-z)/sqrt((x_i-x_j)^2+(h_i-z)^2)`, at the linearly interpolated input sample `k=(T_ij(d)-t_first)/dt`. Endpoint differences use one-sided neighbours. Samples outside the input time window and rays starting below the candidate point contribute nothing. Output spacing is `v*dt/2`, first output depth is `v*t_first/2`, and sufficient rows are added for the surveyed relief. The start/end parameters select local two-way time below each output trace's surface. Above-ground cells are NaN. This is the 2D zero-offset diffraction summation and obliquity factor described by [Dujardin and Bano (2013)](https://doi.org/10.1016/j.crte.2013.01.003); the derivative/obliquity Kirchhoff form follows [Smitha et al. (2016)](https://doi.org/10.1186/s40703-016-0019-6). The discrete normalization by valid trace count matches the existing flat Kirchhoff operator; absolute amplitudes are not calibrated reflectivity.

This approximation is best for near-zero antenna separation, a locally straight profile, moderate slopes, and reflectors near the 2D profile plane. Finite antenna separation changes travel time, sharply curved tracks violate the 2D chainage geometry, and off-plane reflectors or steep terrain can misfocus. Velocity must be representative and constant. The measured GPS positions must cover every trace by interpolation between fixes no farther apart than the chosen horizontal gap limit. Missing, implicit, invalid, duplicated, or unsupported height/position gaps reject the topographic step. The operation never uses the image created by Show topography, and that display shift is not applied to migrated output.

Old picks are `(trace, acquisition time)` and remain so. They are hidden and cannot be edited on the migrated depth grid in the app; PDF radargrams project them onto that grid. Pick CSV and GeoPackage exports retain acquisition time and depth below local GPS surface, based on the profile's pick velocity. The migrated SEG-Y writes datum depth, GPS horizontal chainage, antenna elevations corrected by `H`, and NaN above ground. Reimporting that SEG-Y with the current reader replaces nonfinite samples with zero; reopen the project to preserve the no-data mask.
