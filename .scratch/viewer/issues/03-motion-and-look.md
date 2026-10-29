# 03: Motion and look

**What to build:** The viewer feels like an iPhone: panning has inertia, zoom and autoscale animate over about 150 ms with easing, double-click on the plot autoscales to the visible window, double-click on a legend autoscales to the whole signal. `Shift+pinch` zooms across value around the cursor. A segment control at the bottom shows the modes Pan, Align, Box; keys 1, 2, 3 switch them (Align is a placeholder until ticket 06). Box mode draws a rectangle and autoscales to it on release. The app follows the system light/dark theme, uses Inter, thin antialiased lines, a muted grid and a fixed channel palette. No modal dialogs anywhere.

**Blocked by:** 02 (Viewer opens a recording and draws one channel)

**Status:** ready-for-agent

- [ ] A flick pans with inertia and comes to rest without jitter; a tap stops it
- [ ] Zoom and every autoscale animate; the animation is interruptible by the next gesture
- [ ] Double-click on the plot and on the legend autoscale as specified; Box fits the drawn rectangle on release
- [ ] `Shift+pinch` zooms value about the cursor
- [ ] The segment control reflects the current mode and keys 1, 2, 3 change it
- [ ] Switching the macOS appearance switches the app theme without restart
- [ ] No gesture exists without a visible control or key hint for the same action
