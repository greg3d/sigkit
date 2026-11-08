# 06: Align mode with autosave and undo

**What to build:** A user aligns one recording to another by dragging it. Clicking a legend makes its stream active; in Align mode, dragging along time shifts it, `Shift+drag` along time stretches it about the point under the cursor (changing `k`), dragging across value offsets the channel, `Shift+drag` across scales it about the cursor (changing `gain`). Numeric fields next to the plot show shift, `k` (also as corrected rate and ppm), gain and offset and accept typed values; arrow keys nudge by one pixel, `Shift+arrow` by ten. There is no snapping. Every change writes `header.json` immediately; `Cmd+Z` undoes within the session. Raw columns are never touched.

**Blocked by:** 03 (Motion and look), 05 (Sidebar, panels, overlay, cursor, and multiple recordings)

**Status:** done

- [x] The `pair` preset's two recordings can be brought into visible phase by drag alone, and the resulting `k` and `shift` match the generator's injected drift and shift within a sample
- [x] The anchor point under the cursor stays fixed on screen during a stretch
- [x] Typing a value into a field moves the stream exactly; arrows nudge as specified
- [x] After every change `header.json` on disk holds the new values; the column files' bytes and mtimes are unchanged
- [x] `Cmd+Z` restores the previous values on screen and on disk, step by step
- [x] Align works in both orientations without re-mapping any gesture

## Comments

- 2026-11-08: implemented in 750a6de; closed at the owner's request.
