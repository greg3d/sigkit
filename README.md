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
```

In the viewer: pinch zooms time about the cursor, two-finger scroll pans, `A` autoscales to the visible window, `Shift+A` to the whole signal, `O` toggles orientation. The hidden `--screenshot <png>` flag saves the window once drawn and quits, for checking renders by hand.
