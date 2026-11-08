# 11: Open a workspace by double-click

**What to build:** A user double-clicks a `.sigws.json` workspace in Finder and the viewer opens it, as dropping it on the window does since ticket 07. If the viewer is already running, the workspace opens in that window instead of starting a second copy. This needs:
- a macOS app bundle for `sigkit-viewer` (an `Info.plist` and a build step that produces `sigkit.app`);
- the workspace declared as a document type the app opens;
- the viewer handling the open-file event macOS sends. egui/eframe and winit do not pass it on out of the box: check the winit version in use, otherwise hook the application delegate's `application:openURLs:`.

Optionally, the same mechanism lets a `.sigrec` recording open by double-click. That needs the recording declared as a package type, so Finder shows the directory as one document.

**Decide first:** Launch Services matches a document type on the last filename extension only, so `.sigws.json` reads as `json`. Claiming every `.json` file is wrong. Pick one:
- rename the extension to `.sigws`, and either keep reading `.sigws.json` or drop it (the spec's name changes either way);
- register only a `.sigws` alias and keep `.sigws.json` for drag-and-drop.

**Blocked by:** 07 (Workspace save and load)

**Status:** ready-for-agent, once the extension question is decided

- [ ] A documented command builds `sigkit.app`; it opens and behaves as `cargo run -p sigkit-viewer` does
- [ ] Double-clicking a workspace in Finder opens it in the viewer, as dropping it on the window does
- [ ] With the viewer already running, double-clicking a workspace opens it in that window
- [ ] Other `.json` files are not claimed by the viewer
- [ ] The command line and drag-and-drop still open workspaces and recordings as before
