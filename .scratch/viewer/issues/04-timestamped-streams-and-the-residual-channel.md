# 04: Timestamped streams and the residual channel

**What to build:** A user opens a timestamped recording and sees the truth of its clock. Gaps longer than the base bin break the line at every zoom level; raw samples for the viewport are found by binary search over the monotonic time column. Each timestamped stream exposes a virtual residual channel `t_i − (i / f + t0)` against the nominal rate by default, with a toggle to subtract the fitted rate; the fitted rate is shown as a number and as ppm from nominal. Display time applies the stream's `k` and `shift` from the header. Clock domains are converted on read to seconds from the recording's zero; the cursor shows UTC when the stream has an anchor.

**Blocked by:** 02 (Viewer opens a recording and draws one channel)

**Status:** ready-for-agent

- [ ] On a generated recording with 50 ppm drift the residual against nominal is a straight slope; with the fit subtracted it is flat noise of the injected jitter amplitude
- [ ] Each dropped sample injected by the generator shows as a step of `1/f` in the residual and as a break in the signal line at every zoom
- [ ] The ppm readout matches the generator's drift within the fit's uncertainty
- [ ] A recording in `android.elapsedRealtimeNanos` and one in `ios.systemUptime` both display in seconds from their own zero
- [ ] A residual and rate-fit test recovers injected drift and drops from a generated time column
- [ ] Changing `k` or `shift` in the header moves the displayed stream accordingly
