# GPR Studio

Desktop software for ground-penetrating radar (GPR) data: import, GPS association, processing,
picking, mapping and reporting. Written in Rust with egui; runs on Windows, macOS and Linux.

## Features

- **Import** – GSSI `.dzt`, MALA `.rd3`/`.rd7` (+ `.rad`), ImpulseRadar `.iprb` (+ `.iprh`),
  Sensors & Software `.dt1` (+ `.hd`) and SEG-Y (IDS, Proceq and others).
- **GPS** – one dialog for all imported profiles; GPS files (`.cor`, `.dzg`, `.gps`, NMEA, CSV/TXT,
  SEG-Y trace headers) are matched automatically by file name, including channel endings such as `_0`/`_1`.
- **Map** – GPS tracks of every profile, the part shown in the data view highlighted, markers and layer picks,
  GeoTIFF basemaps of any size, any output coordinate system (EPSG code or PROJ string; default EPSG:2056).
  The map sits on the right, at the bottom of the main window (full width for long profiles) or in its own
  window (buttons in the map header, or the Map menu); the choice is kept for the next start.
  The map header can switch to **Manual projection** for a local Proceq area-scan grid. Import a tab-separated
  `.csv`/`.txt` file (UTF-8 or BOM-marked UTF-16, as in Proceq exports) with `Mode`,
  `Grid Size - X/Y [cm]`, `Grid Spacing - X/Y [cm]`, and a
  `SCAN DISTANCE [m]` section of `Line N` plus scanned length. Lines run along X; their Y positions follow
  file order, while Proceq line numbers match profile names or can be assigned in the map panel. Zero-length
  lines have no profile. The project saves the grid values, source file name, assignments and map mode, but no
  `USER INFO` or CSV path. GPS coordinates and the project CRS remain separate. Manual coordinates are not
  exported to GeoPackage; that export continues to use GPS coordinates.
  **Interpolation zones** (left panel, under Picks): *New zone* draws a polygon on the map (left click adds a
  vertex, a drag still pans, right click or Enter closes, Esc cancels); *Interpolate* fills it with an
  inverse-distance-weighted surface of one layer-pick type, shown as a Jet-coloured depth raster with a legend.
  Cells farther than *Max radius* from every pick stay empty, so only the space between picks is filled.
- **Processing** – set time zero, dewow, DC shift, background removal, bandpass (FFT and Butterworth),
  AGC (optionally with the gain smoothed over neighbouring traces), energy decay, gain function,
  divergence compensation, trace normalisation (90th percentile of absolute amplitude per trace), running (moving) average over traces, average xy-filter,
  diffraction stack, flat and topographic Kirchhoff migration, FK (Stolt) migration, time-depth conversion. Runs in the background
  with a progress bar; sequences can be applied to all profiles.
- **Data view** – distance in metres from the file's trace spacing, GPS or manual grid. Editing `dx` applies
  that nominal spacing to every loaded profile. Editing toolbar `v` likewise sets the velocity for every
  loaded profile; **Use v for profile** after a hyperbola fit calibrates just that profile.
  "Show topography" lets time
  zero follow the GPS elevation (velocity from the toolbar) with an elevation axis. The main wheel scrolls
  vertically; the side wheel slides along the profile. Shift + wheel crops the time axis from 0,
  Ctrl + wheel zooms horizontally (short profiles are drawn narrower at the same scale, never stretched).
  Up and Down select the previous and next profile. **Fit** shows the full section on every loaded profile.
  **Orient profiles**, next to Fit, uses GPS to display each profile west-to-east or north-to-south
  along its dominant direction. Source traces and pick indices remain in acquisition order; the
  display direction is saved in the project. **1x1 ratio** applies to every loaded profile, shows the whole height of each section and
  only changes the horizontal span (around the current centre) so that one metre is as long horizontally
  as vertically; time sections use the toolbar velocity. A profile too short to fill the width is drawn narrower.
- **Picking** – typed point markers, hyperbola velocity fitting, layer lines with phase following and snapping; Ctrl+Z undoes picking steps and deletions.
  **Show crossing picks** (on by default): where another profile crosses this one, its layer picks appear as a dot
  labelled `p<profile number>` at the same depth (hover for the profile, pick and depth). Interpolated zones are
  drawn on every profile they cover as a pink dashed line. Both are read-only.
- **Export** – GeoPackage (GPS profiles, markers, layer picks, optional trace points and layer vertices,
  plus one Float32 interpolations raster), SEG-Y of chosen processing steps,
  PDF report (the top three quarters for the radargram, with optional exact 1×1 metre scale and a minimum section width; the bottom quarter split equally between the whole-survey map, profile details, and processing), CSV of picks.
  The GeoPackage **Interpolations raster** stores Depth (m), Time (ns), or Altitude (m), chosen in
  the export dialog. Export again to a different file for another quantity; the others remain in the
  project. Empty cells and tile padding use NoData `-9999` and appear transparent. A projected CRS
  in metres is required; vector export still succeeds when the raster is skipped for a geographic CRS.
- **Projects** – `.gprproj` files keep the file list, processing, GPS, picks, CRS and basemaps.
  Ctrl+S saves to the project's file, File › Open last project reopens the previous one, and closing only
  asks to save when something changed. Files inside the project's folder are stored with relative paths;
  a missing file is also looked for by name in the project's `data/` folder.
  **File › Collect and save project…** copies the project and every file it uses (data files with their
  headers such as `.rad`/`.iprh`/`.hd`/REFLEXW `.??R`, original GPS files, GeoTIFFs with their world files)
  into one folder (`<name>/<name>.gprproj`, `<name>/data/`, `data/gps/`, `data/basemaps/`) that can be zipped
  and opened on another computer. Files with the same name get a `_2` ending; nothing is overwritten or
  deleted. Demo profiles have no file and are skipped.
- **Settings** – File › Settings…: dark mode, and a default folder where the export dialogs (GeoPackage,
  PDF, picks CSV, SEG-Y) start. Saved in `%APPDATA%\GPRStudio\settings.json` (Windows) or
  `~/.config/GPRStudio/settings.json`.

## Topographic Kirchhoff migration

Associate GPS with measured elevations, choose a projected CRS in metres, then add **Topo Kirchhoff** to the processing sequence. Set the constant velocity, summation width, maximum measured GPS gap, and the vertical offset from antenna to GPS receiver (positive when the GPS is above the antenna). A zero offset means the GPS height is already the antenna height. This operation uses the unshifted time data at that point in the sequence. Keep it as the last step; further time-based processing is not applicable to its depth output. The ordinary Kirchhoff and FK steps retain their flat-surface assumptions.

The output vertical axis is **depth below the highest antenna elevation**; the right axis shows elevation. The horizontal axis uses horizontal GPS distance even if the ordinary profile display uses a nominal trace spacing. Blank space above the local surface is stored as no data. **Show topography** is display-only and is ignored for this migrated result. Existing picks remain in acquisition time. The PDF projects them onto the migrated depth grid; disable the migration step to view or edit them in the app. Pick CSV/GeoPackage exports retain the original time and local-depth meaning. SEG-Y includes the datum in its text header, corrected antenna elevations in trace headers, and IEEE NaN above the local surface.

Full first-to-last trace coverage and finite, increasing positions are required. GPS gaps larger than the stated limit cause an error. Older project GPS entries without verified height provenance need to be re-associated with the original GPS file. See [MECHANICS.md](MECHANICS.md) for the equation, assumptions, and references.

## Build

Requires a recent stable Rust toolchain (https://rustup.rs). On Windows the MSVC build tools are needed
(SQLite for the GeoPackage export is compiled from source).

```
cargo build --locked --release
```

The program is `target/release/gpr_studio` (`gpr_studio.exe` on Windows).

### Linux

Both **Wayland and X11** are enabled in the standard build. The application uses OpenGL;
run it from a graphical desktop session with a working graphics driver (Mesa is supported).

Install the native build and runtime dependencies first. For Debian/Ubuntu:

```sh
sudo apt-get update
sudo apt-get install build-essential pkg-config libwayland-dev libxkbcommon-dev \
  libxkbcommon-x11-0 libgl1-mesa-dev libegl1-mesa-dev
cargo build --locked --release
./target/release/gpr_studio
```

`libxkbcommon-x11-0` is required at runtime by the X11 backend, including under Xvfb.

File and folder dialogs use **XDG Desktop Portal**. Make sure `xdg-desktop-portal` and
the backend for your desktop are installed and running in your user session, for example
`xdg-desktop-portal-gtk` on a GTK desktop or `xdg-desktop-portal-kde` on KDE Plasma.

To check processing and exports without a graphical session:

```sh
./target/release/gpr_studio --selftest selftest_out
```

The Linux CI also checks graphical startup and screenshot capture under Xvfb with Mesa
software rendering. To run that check locally on Debian/Ubuntu:

```sh
sudo apt-get install xvfb xauth libgl1-mesa-dri
env -u WAYLAND_DISPLAY LIBGL_ALWAYS_SOFTWARE=1 timeout 90s \
  xvfb-run -a -s "-screen 0 1600x1000x24" \
  ./target/release/gpr_studio --screenshot shots/linux-x11.png
test -s shots/linux-x11.png
```

## Command line

```
gpr_studio [files...]                  # open GPR files (.tif/.tiff files are added as basemaps)
gpr_studio --screenshot out.png        # open the demo survey, save a picture of the window, exit
gpr_studio --selftest out_dir [files]  # headless test: imports, processing, exports
```

## Documentation

`MECHANICS.md` lists the file layouts, formulas, defaults and export header layouts used.
