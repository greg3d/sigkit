# sigkit

A toolkit for looking at raw sensor signals honestly: a recording format, a desktop viewer, later a logger per phone and a GPS/INS filter.

Scientific inside, iPhone outside.

## Layout

- `GLOSSARY.md` — the vocabulary every doc and ticket uses.
- `docs/specs/` — design specs (HTML).
- `docs/agents/` — conventions agents follow in this repo (issue tracker).
- `.scratch/<feature>/issues/` — tickets, one file each, blockers first.
- `sigkit-format/`, `sigkit-viewer/`, later `sigkit-ekf/` — cargo workspace crates (not created yet).
- `loggers/android`, `loggers/ios` — phone loggers (not created yet).

## Toolchain

Rust stable via rustup. Not yet installed on the dev Mac at the time of writing.
