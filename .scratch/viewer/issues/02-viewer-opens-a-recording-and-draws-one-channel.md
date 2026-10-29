# 02: Viewer opens a recording and draws one channel

**What to build:** A user opens a recording (command-line argument or drag-and-drop) and sees one fixed-rate channel drawn in a custom plot widget with min/max candles at any zoom. The widget lives in along-time / across-value coordinates; orientation is a parameter, vertical by default, toggled with one key. The pyramid is built on first open, binned by time with a base bin of 64 nominal intervals, cached inside the recording, and loaded instantly on second open. Below the base level the widget reads raw samples for the viewport from the memory-mapped column. Grid lines fall only on 1-2-5 multiples on both axes and the time axis switches its unit between ms, s and min. Pinch zooms time around the cursor, two-finger scroll pans (no inertia yet), `A` autoscales to the visible window, `Shift+A` to the whole signal. egui on wgpu.

**Blocked by:** 01 (Format crate, CLI, and synthetic recordings)

**Status:** ready-for-agent

- [ ] A one-hour 500 Hz generated recording pans and zooms at 60 fps on the dev Mac with no stutter when crossing the base level into raw samples
- [ ] Memory after open is the pyramid plus touched pages, not the whole file: opening a recording several times the size of RAM works
- [ ] Second open of the same recording shows the plot immediately; the cache is invalidated when the column changes
- [ ] At full zoom-out a single injected spike is still visible as a candle
- [ ] Every grid label on both axes is a 1, 2 or 5 times a power of ten; the time axis shows ms, s or min as appropriate
- [ ] Toggling orientation preserves the visible time and value ranges; every gesture does the same thing in both orientations
- [ ] Pinch zooms about the cursor position, not the centre; `A` and `Shift+A` fit as specified
- [ ] A pyramid test proves each bin's min/max equals the brute-force min/max of the samples it covers
