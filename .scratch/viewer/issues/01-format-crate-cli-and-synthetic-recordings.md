# 01: Format crate, CLI, and synthetic recordings

**What to build:** A developer can generate a `.sigrec` recording with known truth from the command line, inspect it, and round-trip it through the format crate. `sigkit gen` writes synthetic recordings: a fixed-rate sine plus noise; a timestamped stream with configurable jitter, dropped samples and ppm clock drift, with a UTC anchor; and a `pair` preset that writes two recordings of the same physical signal on two drifting clocks with a time shift between them. `sigkit inspect <rec>` prints streams, channels, dtypes, sample counts, duration, nominal rate and fitted rate. The crate reads and writes headers and planar columns for both stream kinds, validates column lengths, preserves unknown header fields, and rewrites the header atomically. The cargo workspace is created in this ticket.

**Blocked by:** None (can start immediately)

**Status:** ready-for-agent

- [ ] Cargo workspace with `sigkit-format` and a `sigkit` CLI builds on stable Rust
- [ ] `sigkit gen` writes the three presets (fixed-rate, timestamped, pair) with the jitter, drop and ppm parameters it was given
- [ ] `sigkit inspect` on a generated recording prints every stream and channel with counts, duration, nominal and fitted rate, and the fitted rate recovers the injected ppm drift within the noise the generator adds
- [ ] A read → write → read round-trip test reproduces the header exactly, including a field the crate does not know
- [ ] A recording whose columns disagree in length fails to open with an error naming the stream
- [ ] Header rewrite goes through a temp file and rename, so an interrupted write leaves the old header intact
- [ ] Clock domains, nominal rate, time correction, calibration and UTC anchor survive the round-trip
