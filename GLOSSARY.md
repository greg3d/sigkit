# Glossary

| Term | Meaning |
|---|---|
| **Recording** | A `.sigrec` bundle: a directory with `header.json` and one raw column file per channel (plus one time column per timestamped stream). Raw column bytes are never rewritten. |
| **Header** | `header.json` inside a recording. Holds facts about the recording: streams, channels, calibration, corrections, clock domains. The only file the viewer edits. |
| **Column** | One planar little-endian array on disk, one file, one dtype. In code, `Column` is the same array read into memory, typed as on disk. |
| **Stream** | A set of channels sharing one time column (or one rate). An accelerometer is a stream of three channels. A single channel is a stream of one. Stream and channel ids become file names, so they are non-empty and contain no `.`, `/`, `\\` or control characters; the channel id `t` is reserved for the time column. |
| **Channel** | One scalar series inside a stream, with its own dtype, unit, gain and offset. |
| **Timestamped stream** | A stream with a per-sample time column (`i64` in its clock domain). The per-sample stamp is the truth; any rate is derived from it. |
| **Fixed-rate stream** | A stream with no time column; sample time is index divided by the rate in the header. Produced by the fixed-rate export or by importing legacy raw files. |
| **Clock domain** | Which clock a time column counts in (`android.elapsedRealtimeNanos`, `ios.systemUptime`, `gps.utc`, `sigkit.synthetic` for generated recordings, custom) and its unit (`ns`, `us`, `ms`, `s`). Conversion happens on read; stamps stay raw on disk. |
| **UTC anchor** | A pair (monotonic stamp, UTC) recorded by the logger so a recording can be placed in wall-clock time. Used only in the cursor readout in v1. |
| **Nominal rate** | The rate the logger requested or the sensor reported, written to the header. Required for every timestamped stream. The reference the residual is computed against by default, and the yardstick the fitted rate uses to tell a dropped sample from jitter. |
| **Fitted rate** | The rate estimated by least squares over a stream's stamps, against a gap-aware index: consecutive stamps about two nominal intervals apart count as two steps, so dropped samples do not bend the fit. Shown as a number and as ppm from nominal, with the count of missing samples. |
| **Residual** | The virtual channel `t_i − (i / f + t0)` of a timestamped stream. Jitter shows as noise, dropped samples as steps of `1/f`, clock drift as a slope (against nominal) that vanishes when the fit is subtracted. |
| **Time correction** | Per-stream `k` and `shift`: display time is `k · t + shift`. The tool that stretches and moves a signal against another. Persisted in the header. |
| **Calibration** | Per-channel `gain` and `offset`: display value is `gain · raw + offset`. Persisted in the header. |
| **Pyramid** | The min/max decimation levels built per channel at open time and cached inside the recording. Binned by time; base bin is 64 nominal intervals; each level above merges two bins. |
| **Base bin** | The finest pyramid level. Below it the widget reads raw samples for the viewport straight from the memory-mapped column. |
| **Candle** | How a pixel column (or row) is drawn when it covers many samples: the min and the max of everything packed into it. Never an average. |
| **Panel** | One plotting area with one value axis. Channels laid on the same panel are overlaid in one coordinate system. |
| **Overlay** | Several channels on one panel, sharing its value axis. Level mismatches are fixed with gain, not with separate axes. |
| **Along-time / across-value** | The widget's two axes. The widget never thinks in x and y; orientation maps these to the screen. |
| **Orientation** | A workspace setting: vertical (time runs top to bottom, panels side by side; the default) or horizontal. |
| **Mode** | The mouse tool: Pan (default), Align, Box. A segment control at the bottom; keys 1/2/3 are accelerators. |
| **Align** | The mode where the active stream is dragged directly on the plot: along time = shift, Shift+along = k anchored under the cursor, across = offset, Shift+across = gain anchored under the cursor. |
| **Drag anchor** | The point under the cursor that stays fixed while stretching in Align. |
| **Box** | The mode that draws a rectangle and autoscales to it on release. |
| **Autoscale** | Fit the value axis to: the visible window, the whole signal, or a box. |
| **Workspace** | A `.sigws.json` file: which recordings are open together, panels, orientation, view ranges, mode. Saved explicitly. Not part of any recording. |
| **Sheet** | The bottom panel that slides up for anything needing input (import, export). Always with a live preview; never a modal dialog. |
| **Import** | Opening a legacy raw file with no header: the sheet guesses dtype, the user confirms, and a `header.json` is written beside the file. Data is not copied. |
| **Export** | The explicit command that writes a new fixed-rate recording from a timestamped one: fitted rate, linear interpolation, `NaN` across gaps longer than two nominal intervals. |
| **Expression channel** | A per-element formula stored in the header and evaluated on load. Schema slot reserved; feature is v2. |
| **Preset** | A recipe of `sigkit gen`: `fixed-rate` (a sine plus noise), `timestamped` (a sine with jitter, dropped samples and ppm drift) or `pair` (two recordings of one signal on two drifting clocks, the second shifted). The header records the preset under `source`. |
| **Truth** | What a preset injected (ppm, jitter, drop fraction, shift, seed), written to `source.truth` in the header so a feature can be checked against what was put in. |
