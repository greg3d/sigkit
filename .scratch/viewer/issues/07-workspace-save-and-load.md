# 07: Workspace save and load

**What to build:** A user saves the current comparison as a `.sigws.json` workspace (open recordings, panels and their channels, orientation, view ranges, mode) and reopens it later, by double-click or drag-and-drop, to find everything as left. The workspace is saved explicitly, unlike header edits which autosave. A workspace that references a missing recording opens with that recording marked missing, not with an error that blocks everything else.

**Blocked by:** 05 (Sidebar, panels, overlay, cursor, and multiple recordings)

**Status:** ready-for-agent

- [ ] Save, quit, reopen the workspace: the same recordings, panels, orientation, ranges and mode are shown
- [ ] Recording paths in the workspace are relative to the workspace file when possible, so a folder can be moved as a whole
- [ ] A missing recording is shown as missing in the tree; the rest of the workspace loads
- [ ] The workspace file does not duplicate anything the recording headers hold
