# GPR Studio glossary

- **Project**: A `.gprproj` file holding the survey's profile paths, processing sequences, GPS, picks, coordinate system, basemaps, and import settings. Radar data remains in its source files.
- **Collected project**: A project saved with copies of its source data, GPS files, and basemaps in one portable folder. The open session points to those copies only after the project file is saved successfully.
- **Trace normalisation**: A processing step that divides each trace by its 90th percentile absolute amplitude after replacing non-finite samples with zero. A trace whose percentile is zero is left unscaled.
