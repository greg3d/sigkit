# sigkit

A toolkit for looking at raw sensor signals honestly: a recording format, a desktop viewer, later a logger per phone and a GPS/INS filter.

Scientific inside, iPhone outside.

## Layout

- `GLOSSARY.md` — the vocabulary every doc and ticket uses.
- `docs/specs/` — design specs (HTML).
- `docs/agents/` — conventions agents follow in this repo (issue tracker).
- `.scratch/<feature>/issues/` — tickets, one file each, blockers first.
- `sigkit-format/` — the recording format crate: header, planar columns, rate fit, synthetic generator.
- `sigkit-cli/` — the `sigkit` binary: `sigkit gen` writes synthetic recordings, `sigkit inspect` prints one.
- `sigkit-viewer/`, later `sigkit-ekf/` — cargo workspace crates (not created yet).
- `loggers/android`, `loggers/ios` — phone loggers (not created yet).

## Toolchain

Rust stable via rustup (`~/.cargo/bin` on the dev Mac).

```bash
cargo test --workspace
cargo run -p sigkit-cli -- gen pair out/pair --shift 0.25 --ppm-b 150
cargo run -p sigkit-cli -- inspect out/pair-b.sigrec
```
