# 07: Workspace save and load

**What to build:** A user saves the current comparison as a `.sigws.json` workspace (open recordings, panels and their channels, orientation, view ranges, mode) and reopens it later, by double-click (split off to ticket 11) or drag-and-drop, to find everything as left. The workspace is saved explicitly, unlike header edits which autosave. A workspace that references a missing recording opens with that recording marked missing, not with an error that blocks everything else.

**Blocked by:** 05 (Sidebar, panels, overlay, cursor, and multiple recordings)

**Status:** done

- [x] Save, quit, reopen the workspace: the same recordings, panels, orientation, ranges and mode are shown
- [x] Recording paths in the workspace are relative to the workspace file when possible, so a folder can be moved as a whole
- [x] A missing recording is shown as missing in the tree; the rest of the workspace loads
- [x] The workspace file does not duplicate anything the recording headers hold

## Comments

- 2026-11-10: implemented in f73aa13; double-click split off to ticket 11. Closed at the owner's request before a hand check of saving through the UI (`Cmd+S` and the save sheet); that is checked later with the rest, on raw signals.
