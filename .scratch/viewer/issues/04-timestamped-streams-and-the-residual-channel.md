# 04: Timestamped streams and the residual channel

**What to build:** A user opens a timestamped recording and sees the truth of its clock. Gaps longer than the base bin break the line at every zoom level; raw samples for the viewport are found by binary search over the monotonic time column. Each timestamped stream exposes a virtual residual channel `t_i − (i / f + t0)` against the nominal rate by default, with a toggle to subtract the fitted rate; the fitted rate is shown as a number and as ppm from nominal. Display time applies the stream's `k` and `shift` from the header. Clock domains are converted on read to seconds from the recording's zero; the cursor shows UTC when the stream has an anchor.

**Blocked by:** 02 (Viewer opens a recording and draws one channel)

**Status:** in-progress

- [x] On a generated recording with 50 ppm drift the residual against nominal is a straight slope; with the fit subtracted it is flat noise of the injected jitter amplitude
- [ ] Each dropped sample injected by the generator shows as a step of `1/f` in the residual and as a break in the signal line at every zoom
- [x] The ppm readout matches the generator's drift within the fit's uncertainty
- [x] A recording in `android.elapsedRealtimeNanos` and one in `ios.systemUptime` both display in seconds from their own zero
- [x] A residual and rate-fit test recovers injected drift and drops from a generated time column
- [x] Changing `k` or `shift` in the header moves the displayed stream accordingly

## Comments

2026-11-03: implemented across `sigkit-format` (`residual.rs` new; `ratefit.rs`, `recording.rs`, `header.rs`, `synth.rs`, `wallclock.rs`), `sigkit-cli` and `sigkit-viewer` (`trace.rs`, `app.rs`).

Tests:
- Format seams, against the generator's truth (`tests/residual.rs`, `tests/timestamped.rs`):
  - with 50 ppm drift, the residual against nominal is a straight line of slope `1/1.00005 − 1` to within 2 ns;
  - with the fit subtracted, it is flat noise whose standard deviation is within 5 % of the 50 µs of jitter injected;
  - each dropped sample is a step of exactly `1/f`, and the steps add up to the fit's `missing`;
  - the fitted ppm is within 4σ of the injected drift, with and without drops (4σ so the test is not flaky; the uncertainty itself is under 1 ppm);
  - the recording's zero holds across ns and µs units in one domain;
  - a time column that runs backwards is rejected;
  - the residual pyramid is binned by time and has the stream's holes as gaps;
  - the fit reads the mapped time column;
  - `k` must be positive and `shift` finite.
- Viewer `trace.rs`:
  - raw candles and the exact `A` range on a jittered time column match brute force;
  - a 5 s hole is a gap at every zoom from raw to level 1;
  - the raw-sample line breaks at a missing sample and nowhere else;
  - `k` and `shift` move candles, ranges and samples exactly;
  - UTC is counted from the anchor;
  - a residual trace draws its time column's residual.
- CLI: `--clock android|ios` writes those domains, and `inspect` prints `± ppm`.
- Checked by hand through `--screenshot` renders of generated recordings:
  - a 60 s, 50 ppm recording without drops: the residual against nominal is a −3 ms straight slope; with the fit subtracted it is flat ±0.15 ms noise; the readout shows `+50.00 ± 0.02 ppm`;
  - with 0.5 % drops, the nominal residual is a staircase of 2.4 ms steps, and zoomed to 50 ms the signal line breaks at a dropped sample;
  - an iOS recording in µs starts at 0 s;
  - editing the header to `k = 2`, `shift = 5` moved the 60 s stream to 5–125 s.

Decisions:
- The residual uses the raw sample index (the glossary's `t_i − (i/f + t0)`). Dropped samples therefore stay steps of `1/f` after the fit is subtracted, and "flat noise" holds between the steps. The fit's `t0` comes from its gap-aware index.
- The recording's zero is per clock domain: the earliest first stamp among the recording's streams in that domain, compared in nanoseconds across units. Streams on unrelated clocks (say `gps.utc` and `android.elapsedRealtimeNanos`) cannot share a zero, so each sits at 0 until it is shifted. Fixed-rate streams start at the zero.
- Gaps break the line wherever the zoom can show them:
  - Above the base level, a pixel whose bins are all empty is a gap. That holds at every level for a hole at least about two pixels wide. A hole narrower than a pixel cannot be drawn as a gap, because its pixel also holds data.
  - Below the base level, raw candles leave a pixel with no sample empty. A timestamped stream switches to a line through the samples at 2 samples per pixel rather than 1, so jitter never empties a pixel.
  - That line breaks between two stamps more than 1.5 nominal intervals apart, so each dropped sample shows as a break once the zoom resolves it.
  - The ticket's "each dropped sample … as a break in the signal line at every zoom" cannot hold at full zoom-out, where one pixel covers thousands of samples.
- Raw samples of a timestamped stream are found by binary search over the mapped time column, in float seconds within 1e-6 of an interval. The exact range for `A` reads one extra sample on each side of a bin edge, because integer binning and float search may disagree on a stamp sitting exactly on the edge.
- Monotonicity is checked while the time-binned pyramid is built (a backwards stamp is an error naming the sample). A cached pyramid was built from a column that passed the check, and its fingerprint guards against changes.
- The fit now streams over the mapped time column in three passes with no allocation, and reports the standard error of the rate (`rate_sd_hz`), shown as `± ppm`.
- Residuals are drawn in ms. Two residual pyramids are built per timestamped stream at open (against nominal and against the fit) and are not cached yet: on top of the channel pyramids, that is two passes over the time column, plus three for the fit, on every open. This is milliseconds for an hour at 500 Hz. For recordings like ticket 02's 72 GiB one it would undo the instant second open, so it is the follow-up to do next.
- `k ≤ 0` or a non-finite `shift` is rejected when the header is validated.
- Display time is `k · (start_s + t) + shift_s`, applied the same way to fixed-rate and timestamped streams.
- UTC appears in a cursor readout in the bottom bar (`t … s · UTC …Z`, to the millisecond) when the stream has an anchor. The cursor line through the panels is ticket 05's.
- Until ticket 05's sidebar, a channel picker in the bottom bar reaches every channel and each stream's residual, with the fit readout and a "subtract fitted rate" toggle; `--channel stream/channel` picks the first one. The viewer now loads every channel of a recording, not just the first fixed-rate one. A stream that cannot be read is reported in the status line and skipped.
- `sigkit gen … --clock android|ios` writes `android.elapsedRealtimeNanos` in ns or `ios.systemUptime` in µs, for checking clock domains.

Open: "a break in the signal line at every zoom" is met only where the zoom can resolve a gap (see above). Confirm that reading or reword the criterion. Caching the residual pyramids (and the fit) is the follow-up.

2026-11-03: added the clock unit `samples`, for a hardware sample counter. Real loggers either leave a lost sample out or write a zero in its place, and some carry a hardware counter that is exact. With unit `samples`, ticks per second are the stream's nominal rate:
- the residual shows each lost sample as an exact step of `1/f`, with no jitter;
- the fit is the nominal rate, and `missing` counts the lost samples exactly;
- binning and the recording's zero work as for any other clock.

`sigkit gen … --clock counter` writes such a recording (`sigkit.counter`, counting from 1 000 000).

Zeros written in place of lost samples are left alone for now: without stamps they cannot be told from data, and no heuristic is wanted yet.
