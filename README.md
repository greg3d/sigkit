# sigkit

A toolkit for looking at raw sensor signals honestly: a recording format, a desktop viewer, later a logger per phone and a GPS/INS filter.

Scientific inside, iPhone outside.

## Layout

- `GLOSSARY.md` — the vocabulary every doc and ticket uses.
- `docs/specs/` — design specs (HTML).
- `docs/agents/` — conventions agents follow in this repo (issue tracker).
- `.scratch/<feature>/issues/` — tickets, one file each, blockers first.
- `sigkit-format/` — the recording format crate: header, planar columns, memory-mapped columns, the pyramid and its cache, rate fit, synthetic generator.
- `sigkit-cli/` — the `sigkit` binary: `sigkit gen` writes synthetic recordings, `sigkit inspect` prints one.
- `sigkit-viewer/` — the desktop viewer (egui on wgpu): opens a recording, draws a channel as min/max candles at every zoom.
- later `sigkit-ekf/` — cargo workspace crate (not created yet).
- `loggers/android`, `loggers/ios` — phone loggers (not created yet).

## Toolchain

Rust stable via rustup (`~/.cargo/bin` on the dev Mac).

```bash
cargo test --workspace
cargo run -p sigkit-cli -- gen pair out/pair --shift 0.25 --ppm-b 150
cargo run -p sigkit-cli -- inspect out/pair-b.sigrec
cargo run --release -p sigkit-viewer -- out/hour.sigrec            # or drop a recording on the window
cargo run --release -p sigkit-viewer -- out/hour.sigrec --horizontal --time 10:13
cargo run -p sigkit-cli -- gen timestamped out/drift --ppm 50 --drop 0.005 --clock ios
cargo run --release -p sigkit-viewer -- out/drift.sigrec --channel signal/residual
cargo run --release -p sigkit-viewer -- out/pair.sigws.json                        # a saved workspace
mkdir -p out/legacy && cargo run -p sigkit-cli -- gen raw out/legacy/bench.dat --dtype i16 --channels 2 --byte-order big
cargo run --release -p sigkit-viewer -- out/legacy/bench.dat                       # a raw file: the import sheet
```

In the viewer: pinch (or `+`/`−`) zooms time about the cursor, `Shift` with either zooms value; two-finger scroll, dragging in Pan mode and the arrow keys pan, and a flick coasts until a tap stops it. `A` or double-click autoscales to the visible window, `Shift+A` or double-click on the legend to the whole signal; in Box mode a drawn rectangle becomes the view. `1`, `2`, `3` switch Pan, Align, Box (also the segment control at the bottom); `O` toggles orientation. In Align, click a legend to pick a stream, then drag it along time to shift it (`Shift`: stretch `k` about the point pressed, the stream's far end following the pointer) or across value to offset its channel (`Shift`: scale `gain`); the arrows nudge by a pixel, `Shift` by ten, and the sidebar on the right takes typed shift, `k`, ppm, corrected rate, gain and offset. Every change is written to the recording's `header.json` at once; `Cmd+Z` undoes, step by step. `Cmd+S` saves the workspace (recordings, panels, orientation, time and value spans, mode) as a `.sigws.json`, asking where the first time in a sheet at the bottom (`Cmd+Shift+S` asks again); drop that file on the window, or pass it on the command line, to reopen everything as left. Recordings inside the workspace's folder are saved relative to it, so the folder can move as a whole; a recording that is gone is marked missing in the tree and the rest opens. The channel picker at the bottom reaches every channel and each timestamped stream's residual, with the fitted rate in ppm and a toggle that subtracts it. Dropping a raw file with no header (or passing it on the command line) slides up the import sheet: dtype, byte order, header offset, interleaved channel count, rate, gain and offset, starting from a guess, with a preview that redraws on every change; Import (or Enter) writes a `header.json` beside the file that reads it in place as a fixed-rate stream, without copying it, and opens it; dropping it again opens that recording. A swipe down or Escape puts a sheet away. The hidden `--screenshot <png>` flag saves the window once drawn and quits, for checking renders by hand.
