# 01: Format crate, CLI, and synthetic recordings

**What to build:** A developer can generate a `.sigrec` recording with known truth from the command line, inspect it, and round-trip it through the format crate. `sigkit gen` writes synthetic recordings: a fixed-rate sine plus noise; a timestamped stream with configurable jitter, dropped samples and ppm clock drift, with a UTC anchor; and a `pair` preset that writes two recordings of the same physical signal on two drifting clocks with a time shift between them. `sigkit inspect <rec>` prints streams, channels, dtypes, sample counts, duration, nominal rate and fitted rate. The crate reads and writes headers and planar columns for both stream kinds, validates column lengths, preserves unknown header fields, and rewrites the header atomically. The cargo workspace is created in this ticket.

**Blocked by:** None (can start immediately)

**Status:** done

- [x] Cargo workspace with `sigkit-format` and a `sigkit` CLI builds on stable Rust
- [x] `sigkit gen` writes the three presets (fixed-rate, timestamped, pair) with the jitter, drop and ppm parameters it was given
- [x] `sigkit inspect` on a generated recording prints every stream and channel with counts, duration, nominal and fitted rate, and the fitted rate recovers the injected ppm drift within the noise the generator adds
- [x] A read → write → read round-trip test reproduces the header exactly, including a field the crate does not know
- [x] A recording whose columns disagree in length fails to open with an error naming the stream
- [x] Header rewrite goes through a temp file and rename, so an interrupted write leaves the old header intact
- [x] Clock domains, nominal rate, time correction, calibration and UTC anchor survive the round-trip

## Comments

2026-10-29 — Implemented in `sigkit-format` and `sigkit-cli`. Seams tested (per the spec's Testing Decisions): read → write → read round-trip with unknown fields at every level (`tests/roundtrip.rs`), column length validation and id rules (`tests/validation.rs`), atomic header rewrite (`tests/atomic_header.rs`), rate fit against generator truth with and without dropped samples (`tests/ratefit.rs`), and the `sigkit` binary end to end (`sigkit-cli/tests/cli.rs`).

Decision taken during review: the spec's Time section describes the fitted rate as "the least-squares slope of stamp against index". Taken literally, every dropped sample shifts all later stamps by one interval, so one drop in a thousand already reads as a thousand ppm and the acceptance criterion ("recovers the injected ppm drift") could only hold with `--drop 0`. The fit therefore runs against a gap-aware index (consecutive stamps about two nominal intervals apart count as two steps). Drift and jitter are far below an interval, so the rounding is unambiguous; the fit also yields the number of missing samples, which `inspect` prints. The residual channel (ticket 04) keeps the glossary's definition `t_i − (i / f + t0)` over the raw index, so drops still show as steps there. GLOSSARY updated (Fitted rate, Nominal rate, Preset, Truth, id rules).

Known-field normalisation: a rewrite always writes `time_correction`, `notes`, `expressions` and a channel's `unit`/`gain`/`offset`, filling defaults for ones the header left out. Unknown fields pass through untouched at every level.
