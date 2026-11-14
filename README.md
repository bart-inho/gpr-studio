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
- **Processing** – set time zero, dewow, DC shift, background removal, bandpass (FFT and Butterworth),
  AGC, energy decay, gain function, divergence compensation, trace normalisation, running average,
  diffraction stack, Kirchhoff and FK (Stolt) migration, time-depth conversion. Runs in the background
  with a progress bar; sequences can be applied to all profiles.
- **Picking** – typed point markers, hyperbola velocity fitting, layer lines with phase following and snapping.
- **Export** – GeoPackage (profiles, markers, layer picks, trace points), SEG-Y of chosen processing steps,
  PDF report (radargram + map with the same section highlighted), CSV of picks.
- **Projects** – `.gprproj` files keep the file list, processing, GPS, picks, CRS and basemaps.

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
