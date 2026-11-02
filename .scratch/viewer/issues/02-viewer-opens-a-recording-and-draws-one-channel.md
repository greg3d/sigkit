# 02: Viewer opens a recording and draws one channel

**What to build:** A user opens a recording (command-line argument or drag-and-drop) and sees one fixed-rate channel drawn in a custom plot widget with min/max candles at any zoom. The widget lives in along-time / across-value coordinates; orientation is a parameter, vertical by default, toggled with one key. The pyramid is built on first open, binned by time with a base bin of 64 nominal intervals, cached inside the recording, and loaded instantly on second open. Below the base level the widget reads raw samples for the viewport from the memory-mapped column. Grid lines fall only on 1-2-5 multiples on both axes and the time axis switches its unit between ms, s and min. Pinch zooms time around the cursor, two-finger scroll pans (no inertia yet), `A` autoscales to the visible window, `Shift+A` to the whole signal. egui on wgpu.

**Blocked by:** 01 (Format crate, CLI, and synthetic recordings)

**Status:** done

- [x] A one-hour 500 Hz generated recording pans and zooms at 60 fps on the dev Mac with no stutter when crossing the base level into raw samples
- [x] Memory after open is the pyramid plus touched pages, not the whole file: opening a recording several times the size of RAM works
- [x] Second open of the same recording shows the plot immediately; the cache is invalidated when the column changes
- [x] At full zoom-out a single injected spike is still visible as a candle
- [x] Every grid label on both axes is a 1, 2 or 5 times a power of ten; the time axis shows ms, s or min as appropriate
- [x] Toggling orientation preserves the visible time and value ranges; every gesture does the same thing in both orientations
- [x] Pinch zooms about the cursor position, not the centre; `A` and `Shift+A` fit as specified
- [x] A pyramid test proves each bin's min/max equals the brute-force min/max of the samples it covers

## Comments

2026-10-29 — Implemented in `sigkit-format` (pyramid, mapped column, cache) and the new `sigkit-viewer` crate (egui 0.37 on wgpu). Seams tested, per the spec's Testing Decisions plus this ticket's criteria: pyramid min/max against brute force at every level, empty bins as gaps, a spike surviving to the top (`sigkit-format/tests/pyramid.rs`); mapped column against the whole column, cache hit on second open, rebuild on a changed column, time binning of a timestamped stream with a hole (same file); 1-2-5 ticks and the ms/s/min switch (`sigkit-viewer/src/ticks.rs`); pan, zoom about the cursor and the orientation mapping (`view.rs`); level choice, per-pixel candles above and below the base, neighbour samples, calibration (`trace.rs`). The widget itself was checked by hand through `--screenshot` renders of the hour-long generator recording at four zooms, in both orientations, and of a copy with one injected spike.

Decisions:
- Bins hold `f32` min and max. A bin covering no sample (or only `NaN`) is empty and stays a gap until merged with a filled sibling. Above the base, each bin is drawn in the one pixel holding its centre, so every sample is drawn exactly once and a spike never smears; the level drawn is the coarsest whose bin fits in one physical pixel. Below the base, raw samples go to the pixel holding their instant; below one sample per pixel they are drawn as a line through their exact instants, with a dot per sample from 6 px spacing on.
- Candles are one axis-aligned mesh, each candle extended to the previous one's nearest edge so the trace is continuous. A single zig-zag path produced egui's sharp-join artifacts, so it was replaced.
- Cache file `.cache/<stream>.<channel>.pyr`: magic and version, a fingerprint (length and mtime of the value column and, for timestamped streams, the time column; dtype; base bin seconds), then every level. A fingerprint mismatch rebuilds. Writing the cache is best effort, so a read-only recording still opens.
- Time bins are `floor(Δticks · rate / ticks_per_second) / 64`, in integers when the rate is a whole number of hertz (exact however long the recording), in `f64` otherwise (exact while `Δticks · rate` stays below 2^53). A stamp on a bin boundary lands in the bin that starts there. A last stamp that would need over 1024 times more base bins than samples is reported as a corrupt time column, and a sample placed beyond the base is dropped, so one bad stamp cannot exhaust memory. Monotonicity is ticket 04's.
- Time axis unit follows the step: steps up to 500 ms label in ms, 1 s to 20 s in s, from 1 min up in minutes as 1-2-5 of minutes (so a 50 s step never appears). A millisecond step 10 s or more from the zero labels in seconds with millisecond decimals (`1800.005 s`, not `1800005 ms`). Labels carry only the decimals the step needs.
- Timestamped streams get a pyramid too (binned by time, gaps marked), but the viewer draws the first fixed-rate channel only; a recording without one reports that ticket 04 draws timestamped streams.
- The pyramid is built on a background thread with a progress readout in the status bar. A 72 GiB sparse fixed-rate recording (19.3e9 samples, three times the dev Mac's 24 GiB of RAM) opened in 103 s the first time, peak RSS 7.2 GB: the 4.8 GB pyramid plus touched pages. The second open took 2.8 s at 4.3 GB, which is reading the 4.8 GB cache. The file is sparse, so this proves the mapping and paging path, not disk throughput. The 1 h 500 Hz recording: 450 KB cache (a quarter byte per sample), pyramid from cache in 1 ms, plot CPU 0.0 to 0.1 ms per frame in release (shown in the status bar).
- Command line: `--horizontal`, `--time from:to`, and a hidden `--screenshot <png>` that saves the window once drawn and quits. Workspace `rust-version` moved to 1.95 for eframe 0.37.

- `A` fits exactly the samples with instants in the window: partial base bins at the edges are read raw, whole bins between come from the pyramid, coarsest first. Fitting from the drawn candles would let bins straddling the edges pull in samples up to half a bin outside.
- A column whose mtime cannot be read is never served from the cache. A corrupt cache claiming absurd sizes is rejected before allocating.

Review (standards and spec sub-agents), settled without change:
- "No allocation in the frame loop" and "two vertices per pixel": the candle and sample buffers are reused frame to frame, but egui shapes own their vertex buffers and are rebuilt each frame, and each candle is a quad (4 vertices). The spec mandates egui; plot CPU is 0.1 ms per frame, so this was left as is.
- macOS momentum scrolling reaches `smooth_scroll_delta`, so the OS already coasts a flick. egui 0.37 does not expose the momentum phase in its events; ticket 03 owns inertia and "a tap stops it" and will need to tell the two apart.
- Light and dark plot colours exist only so the plot is legible in either system theme; the theme work proper (Inter, palette, switching without restart) stays with ticket 03. The `--time`, `--screenshot`, progress and frame-time readouts are dev aids.
- The viewer's pure modules (`ticks`, `view`, `trace`) carry unit tests despite "the viewer UI is not unit-tested": they check this ticket's own criteria (1-2-5 labels, zoom about the cursor, orientation, exact fit), and no widget or egui code is tested.
- The timestamped pyramid's zero is the stream's first stamp; ticket 04 decides the recording's zero.

Not checked by the agent: 60 fps and the feel of pinch and two-finger scroll on the trackpad, which need the dev Mac at the keyboard, so that criterion stays open and the ticket stays in-progress until someone checks it. The gesture math has unit tests and the frame CPU time sits in the status bar for that check.

2026-11-02: hand check on the dev Mac with a generated one-hour, 500 Hz recording. Pan and zoom feel smooth, including the crossing into raw samples. First open built the pyramid in 16 ms, the second loaded it from the cache, and the plot took 0.1 ms of CPU per frame. The "inertia" and smooth motion felt during the check come from macOS: trackpad momentum scrolling and continuous pinch events. The app itself has no inertia or animation yet. Ticket 03 should keep that feel and add a tap to stop.
