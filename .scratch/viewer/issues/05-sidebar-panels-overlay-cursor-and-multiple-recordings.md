# 05: Sidebar, panels, overlay, cursor, and multiple recordings

**What to build:** A user sees a sidebar tree of recordings → streams → channels, with the residual as a virtual channel under each timestamped stream, and lays out the view by dragging: a channel onto a panel overlays it in the panel's single coordinate system, between panels creates a new panel, back to the tree removes it. Dropping a second recording on the window adds it to the tree. A cursor line runs through all panels and shows each channel's nearest-sample value and, for timestamped streams, the sample's stamp. Each panel has one value axis; a channel alone in a panel keeps its own view scale.

**Blocked by:** 02 (Viewer opens a recording and draws one channel), 04 (Timestamped streams and the residual channel)

**Status:** ready-for-agent

- [ ] The tree lists every stream and channel of every open recording, plus the residual under timestamped streams
- [ ] Drag onto a panel overlays, drag between panels creates, drag to the tree removes, with a visible drop target in each case
- [ ] Two channels overlaid on one panel share its value axis; the same channel alone in its own panel autoscales independently
- [ ] The `pair` preset's two recordings open side by side and the cursor shows both values at the same instant
- [ ] Cursor values are exact samples; no readout differs from the stored sample after gain and offset
- [ ] The tree, drag and cursor work identically in both orientations
