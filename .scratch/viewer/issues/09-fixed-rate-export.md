# 09: Fixed-rate export

**What to build:** A user exports a timestamped stream to a new fixed-rate recording through a sheet that proposes the fitted rate and lets them change it. Samples are linearly interpolated onto the uniform grid; a gap longer than two nominal intervals is written as `NaN`, not interpolated across. The residual is exported beside the data as a channel. The output recording is written next to the source and opens in the viewer like any other.

**Blocked by:** 04 (Timestamped streams and the residual channel), 08 (Raw file import sheet with live preview)

**Status:** ready-for-agent

- [ ] Exporting a generated timestamped sine yields a fixed-rate recording whose samples match the analytic sine within interpolation error
- [ ] Each injected gap longer than two nominal intervals is `NaN` in the output and shows as a break in the viewer
- [ ] The export sheet defaults to the fitted rate and accepts another rate
- [ ] The source recording is unchanged; the output is a separate `.sigrec` beside it
- [ ] A resampling test covers exact interpolation on a linear signal and `NaN` across gaps
