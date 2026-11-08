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
| **Clock domain** | Which clock a time column counts in (`android.elapsedRealtimeNanos`, `ios.systemUptime`, `gps.utc`, `sigkit.synthetic` for generated recordings, custom) and its unit (`ns`, `us`, `ms`, `s`, or `samples` for a hardware sample counter that ticks once per nominal interval, so its seconds are counts divided by the nominal rate). Conversion happens on read; stamps stay raw on disk. |
| **UTC anchor** | A pair (monotonic stamp, UTC) recorded by the logger so a recording can be placed in wall-clock time. Used only in the cursor readout in v1. |
| **Nominal rate** | The rate the logger requested or the sensor reported, written to the header. Required for every timestamped stream. The reference the residual is computed against by default, and the yardstick the fitted rate uses to tell a dropped sample from jitter. |
| **Fitted rate** | The rate estimated by least squares over a stream's stamps, against a gap-aware index: consecutive stamps about two nominal intervals apart count as two steps, so dropped samples do not bend the fit. Shown as a number and as ppm from nominal ± one standard error (the fit's uncertainty, from the scatter of the stamps about the line), with the count of missing samples. |
| **Residual** | The virtual channel `t_i − (i / f + t0)` of a timestamped stream. Jitter shows as noise, dropped samples as steps of `1/f`, clock drift as a slope (against nominal) that vanishes when the fit is subtracted. |
| **Recording's zero** | Display time 0 of a recording. Each clock domain in a recording has its own zero: the earliest first stamp among the recording's streams in that domain, whatever their units. A fixed-rate stream starts at the zero. In code, a stream's offset from it is `start_s`. |
| **Display time** | Where a sample is drawn: `k · (start_s + t) + shift`, with `t` seconds from the stream's first sample (its *local* time). In code, `Placement` holds `start_s`, `k` and `shift`. |
| **Time correction** | Per-stream `k` and `shift`: display time is `k · t + shift`. The tool that stretches and moves a signal against another. Persisted in the header. In code, `Correction`. |
| **Calibration** | Per-channel `gain` and `offset`: display value is `gain · raw + offset`. Persisted in the header. In code, `Calibration`. |
| **Pyramid** | The min/max decimation levels built per channel at open time and cached inside the recording. Binned by time; base bin is 64 nominal intervals; each level above merges two bins. Bins hold `f32` min and max; a bin covering no sample is empty and stays a gap at every level. |
| **Base bin** | The finest pyramid level. Below it the widget reads raw samples for the viewport straight from the memory-mapped column. |
| **Cache** | `.cache/<stream>.<channel>.pyr` inside a recording: a pyramid plus the fingerprint of the columns it was built from (length and mtime of each, dtype, base bin). A fingerprint mismatch means rebuild. Regenerable, never committed. |
| **Trace** | In code, one channel ready to draw: its values (a mapped column, or a stream's residual), its clock (a rate or a mapped time column), its pyramid, its calibration and its placement in display time. It answers, per frame, the min and max in each pixel along time. |
| **View** | What the widget shows: a time span along and a value span across, in data units. Orientation does not change a view; the mapping onto the screen does. |
| **Bin** | One cell of a pyramid level: the `f32` min and max of the samples it covers, or empty (a gap). The same type holds one pixel's candle. |
| **Mapped column** | A column read through a read-only memory map, so the OS pages it in as the viewport touches it. Never written; a column truncated while mapped kills the process. |
| **Mapping** | In code, a view laid onto a screen rectangle in one orientation: the one place that turns (time, value) into screen points and back. |
| **Candle** | How a pixel column (or row) is drawn when it covers many samples: the min and the max of everything packed into it. Never an average. |
| **Panel** | One plotting area with one value axis. Channels laid on the same panel are overlaid in one coordinate system. |
| **Overlay** | Several channels on one panel, sharing its value axis. Level mismatches are fixed with gain, not with separate axes. |
| **Along-time / across-value** | The widget's two axes. The widget never thinks in x and y; orientation maps these to the screen. |
| **Orientation** | A workspace setting: vertical (time runs top to bottom, panels side by side; the default) or horizontal. |
| **Mode** | The mouse tool: Pan (default), Align, Box. A segment control at the bottom; keys 1/2/3 are accelerators. Arrow keys pan in Pan and Box; Align keeps them for nudging. |
| **Flick** | A pan released while still moving. The view coasts with speed decaying as `e^(−t/0.5 s)` (in code, `Inertia`) until it comes to rest; a tap stops it. The OS's own momentum events are ignored. |
| **Animation** | A jump of the view (zoom key, wheel notch, autoscale, box) eases out over 150 ms (in code, `Tween`). The next direct gesture interrupts it where it is. |
| **Legend** | The chips in a panel's free corner, one per channel, with its colour and name and, under the cursor, its nearest sample's value and stamp. Double-click fits that channel's whole signal; dragging a chip moves its channel to another panel, or back to the tree to remove it. |
| **Channel palette** | The fixed colours channels are drawn in, by their place in the tree across every open recording (so the same channel of two recordings differs on an overlay), each with a light-theme and a dark-theme shade. |
| **Align** | The mode where the active stream is dragged directly on the plot: along time = shift, Shift+along = k anchored under the cursor, across = offset, Shift+across = gain anchored under the cursor. A drag commits to along or across after its first few pixels. |
| **Active stream** | The stream Align moves, picked by clicking one of its channels' legends; that channel is the one whose calibration Align changes. In code, `PlotState::active`, a channel. |
| **Drag anchor** | The point under the cursor that stays fixed while stretching in Align: where the drag was pressed. |
| **Far end** | What a stretch pulls along with the pointer: the end of the stream's display span farther from the drag anchor (for `k`), or the farther extreme of the channel's whole signal (for `gain`). Zoomed out it is a rubber band; zoomed in, a fine adjustment. |
| **Nudge** | An arrow key in Align: the active stream moves one pixel the way the arrow points (ten with Shift), along time as shift, across value as offset. |
| **Undo** | `Cmd+Z` takes back the session's header edits one change at a time, on screen and on disk. A change is a drag, a nudge or a typed field. In code, an `Edit` in the `History`. |
| **Box** | The mode that draws a rectangle and autoscales to it on release. |
| **Autoscale** | Fit the value axis to: the visible window, the whole signal, or a box. |
| **Workspace** | A `.sigws.json` file: which recordings are open together, panels, orientation, view ranges, mode. Saved explicitly (`Cmd+S`). Not part of any recording, and holds nothing a header holds. A recording inside the workspace's folder is listed relative to it, one elsewhere by its absolute path. Opening one replaces what is open. |
| **Missing recording** | A recording a workspace lists that could not be opened. Shown as missing in the tree; its channels are left off the panels; it stays listed when the workspace is saved again. |
| **Sheet** | The bottom panel that slides up for anything needing input (import, export). Always with a live preview; never a modal dialog. |
| **Import** | Opening a legacy raw file with no header: the sheet guesses dtype, the user confirms, and a `header.json` is written beside the file. Data is not copied. |
| **Export** | The explicit command that writes a new fixed-rate recording from a timestamped one: fitted rate, linear interpolation, `NaN` across gaps longer than two nominal intervals. |
| **Expression channel** | A per-element formula stored in the header and evaluated on load. Schema slot reserved; feature is v2. |
| **Preset** | A recipe of `sigkit gen`: `fixed-rate` (a sine plus noise), `timestamped` (a sine with jitter, dropped samples and ppm drift) or `pair` (two recordings of one signal on two drifting clocks, the second shifted). The header records the preset under `source`. |
| **Truth** | What a preset injected (ppm, jitter, drop fraction, shift, seed), written to `source.truth` in the header so a feature can be checked against what was put in. |
