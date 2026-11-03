# 10: Detect time gaps

**What to build:** A tool that finds the gaps in a timestamped stream instead of relying on the eye. It lists every place where consecutive stamps are more than 1.5 nominal intervals apart, with:
- the gap's start in display time;
- its length in seconds and in missing samples;
- its UTC, when the stream has an anchor.

`sigkit inspect --gaps` prints the list. In the viewer, the list is reachable from the stream and each entry jumps the view to its gap. This tool is the answer to "where is data missing" at any zoom: the plot shows a gap as a break only where the zoom can resolve it (decided on ticket 04).

**Blocked by:** 04 (Timestamped streams and the residual channel), 05 (Sidebar panels, overlay, cursor and multiple recordings)

**Optional:** yes. Nice to have, not on the path of any other ticket.

**Status:** ready-for-agent

- [ ] On a generated recording, the gap list matches the samples the generator dropped: count, positions and lengths
- [ ] A recording on a sample counter (`samples` unit) lists its lost samples exactly
- [ ] `sigkit inspect --gaps` prints the list; without the flag, the output is unchanged
- [ ] Picking a gap in the viewer moves the view to it, animated, at a zoom where the break is visible
