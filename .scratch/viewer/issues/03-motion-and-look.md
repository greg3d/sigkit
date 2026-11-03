# 03: Motion and look

**What to build:** The viewer feels like an iPhone: panning has inertia, zoom and autoscale animate over about 150 ms with easing, double-click on the plot autoscales to the visible window, double-click on a legend autoscales to the whole signal. `Shift+pinch` zooms across value around the cursor. A segment control at the bottom shows the modes Pan, Align, Box; keys 1, 2, 3 switch them (Align is a placeholder until ticket 06). Box mode draws a rectangle and autoscales to it on release. The app follows the system light/dark theme, uses Inter, thin antialiased lines, a muted grid and a fixed channel palette. No modal dialogs anywhere.

**Blocked by:** 02 (Viewer opens a recording and draws one channel)

**Status:** in-progress

- [ ] A flick pans with inertia and comes to rest without jitter; a tap stops it
- [ ] Zoom and every autoscale animate; the animation is interruptible by the next gesture
- [ ] Double-click on the plot and on the legend autoscale as specified; Box fits the drawn rectangle on release
- [ ] `Shift+pinch` zooms value about the cursor
- [ ] The segment control reflects the current mode and keys 1, 2, 3 change it
- [ ] Switching the macOS appearance switches the app theme without restart
- [ ] No gesture exists without a visible control or key hint for the same action

## Comments

2026-11-03: implemented in `sigkit-viewer` (`motion.rs` new, `plot.rs`, `app.rs`, `view.rs`). The unit tests cover the motion maths, which is pure: velocity from recent movement and none after a resting finger, a flick that slows every frame and comes to rest within 2 pt of `v·τ`, a coast that depends only on the clock however unevenly frames arrive, a 150 ms ease-out tween that never overshoots, value zoom about the cursor, and a box in either drag direction and orientation becoming the view. The widget is not unit-tested; it was checked through `--screenshot` renders (segment control, legend, Inter, palette, bottom bar).

Decisions:
- Inertia is the app's own. egui-winit 0.37 marks macOS momentum events as `MouseWheelSource::Momentum`; those are ignored, and when the fingers lift (`TouchPhase::End` from the trackpad) the plot coasts at the measured velocity, decaying as `e^(−t/0.5 s)` (close to iOS's 0.998 per ms). Any press on the plot, or a new scroll, stops it. The coast is a function of the clock, not integrated frame by frame, so it lands smoothly and does not jitter at rest.
- In Pan mode a mouse drag pans and flicks too, for anyone without a trackpad.
- Direct gestures (pinch, two-finger scroll, drag) move the view at once and interrupt any animation where it is. Jumps ease out over 150 ms: `+`/`−` zoom by two (`Shift`: value), wheel notches, arrow keys (a quarter view, not in Align, which keeps them for ticket 06's nudges), `A`/`Shift+A`, double-click on the plot and on the legend, and a released box. A second press during an animation steps on from its target. Cmd-wheel zoom is smoothed by egui itself.
- Every gesture has a key or a control, listed in the hint row for the current mode: pinch → `+`/`−`, scroll and drag → arrows, double-clicks → `A`/`Shift+A`, Box → the segment control and `3`.
- Box sets both time and value to the drawn rectangle ("fits the drawn rectangle"), rather than fitting the value axis to the data under it.
- The legend sits in the corner no axis labels use: bottom right when vertical, top right when horizontal.
- Theme: egui's default `ThemePreference::System` follows macOS appearance changes live; plot colours come from `visuals.dark_mode` every frame. Font: Inter 3.019 Regular (OFL, licence in `sigkit-viewer/assets/`) is the first proportional font, and egui's own fonts are the fallback for glyphs such as ⇧. Palette: eight fixed hues, each with a light and a dark shade, assigned by a channel's place in the recording. There are no modal dialogs; the channel picker is a popup list.
- Align (2) is selectable but drag does nothing yet, and its hint says so.

Not checked by the agent: anything that needs hands on the trackpad. Flick feel and coming to rest, tap-to-stop, interrupting an animation, double-clicks, Box, `Shift+pinch`, keys 1/2/3 and a live switch of macOS appearance all need a hand check on the dev Mac. The ticket stays in progress until that's done.
