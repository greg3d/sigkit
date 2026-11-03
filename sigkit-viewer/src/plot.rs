//! The plot widget: panels side by side along the stacking axis, sharing one
//! time span, each with its own value axis. Every channel is drawn on the
//! egui painter as min/max candles, raw samples below the base bin, over a
//! 1-2-5 grid, with a legend chip per channel. A cursor line runs through
//! every panel and reads each channel's nearest sample. The widget lives in
//! along-time / across-value coordinates; [`Mapping`] does the orientation.
//! Gestures move the view directly; jumps (keys, autoscale, box, wheel
//! steps) ease to their target and a flick coasts, see [`crate::motion`].

use crate::layout::{ChannelRef, Dragged, Drop, Layout, PanelId, Views};
use crate::motion::{self, Inertia, Tween, VelocityTracker};
use crate::ticks;
use crate::trace::{Reading, Trace};
use crate::view::{Mapping, Orientation, Span, View};
use egui::{
    Color32, Event, FontId, Galley, Key, Mesh, MouseWheelSource, MouseWheelUnit, Pos2, Rect, Sense,
    Shape, Stroke, StrokeKind, TouchPhase, Ui, Vec2, pos2, vec2,
};
use sigkit_format::Bin;
use std::sync::Arc;

/// Least pixels between two grid lines along time.
const MIN_PX_PER_TIME_TICK: f32 = 90.0;
/// Least pixels between two grid lines across value.
const MIN_PX_PER_VALUE_TICK: f32 = 60.0;
/// From this many pixels between raw samples on, each sample also gets a dot.
const DOT_SPACING_PX: f64 = 6.0;
/// A box smaller than this on either side is a slip, not a box.
const MIN_BOX_PX: f32 = 4.0;
/// What a zoom key multiplies the visible span by.
const ZOOM_STEP: f64 = 2.0;
/// What an arrow key pans by, as a fraction of the visible span.
const ARROW_PAN: f32 = 0.25;
/// How far an Align drag goes, in pixels, before it commits to moving along
/// time or across value.
const AXIS_LOCK_PX: f64 = 4.0;
/// Pixels an arrow key nudges the active stream by in Align, and with Shift.
const NUDGE_PX: f32 = 1.0;
const SHIFT_NUDGE_PX: f32 = 10.0;

/// The mouse tool. Keys 1, 2, 3 are its accelerators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Drag pans, with inertia (the default).
    Pan,
    /// Drag the active stream to align it; the arrows nudge it.
    Align,
    /// Drag a rectangle; the view fits it on release.
    Box,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Pan, Mode::Align, Mode::Box];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Pan => "Pan",
            Mode::Align => "Align",
            Mode::Box => "Box",
        }
    }

    pub fn key(self) -> Key {
        match self {
            Mode::Pan => Key::Num1,
            Mode::Align => Key::Num2,
            Mode::Box => Key::Num3,
        }
    }
}

/// The fixed channel palette, as (light theme, dark theme) pairs: the same
/// hue in both, lifted for a dark background.
const PALETTE: [(Color32, Color32); 8] = [
    (
        Color32::from_rgb(0x1f, 0x6f, 0xeb),
        Color32::from_rgb(0x6c, 0xb4, 0xf5),
    ),
    (
        Color32::from_rgb(0xd1, 0x49, 0x5b),
        Color32::from_rgb(0xf0, 0x7a, 0x8a),
    ),
    (
        Color32::from_rgb(0x1f, 0x8a, 0x7e),
        Color32::from_rgb(0x5c, 0xc8, 0xb8),
    ),
    (
        Color32::from_rgb(0xc9, 0x7c, 0x10),
        Color32::from_rgb(0xf2, 0xbf, 0x63),
    ),
    (
        Color32::from_rgb(0x7a, 0x55, 0xd6),
        Color32::from_rgb(0xb3, 0x9c, 0xf0),
    ),
    (
        Color32::from_rgb(0x34, 0x8a, 0x47),
        Color32::from_rgb(0x6c, 0xc4, 0x7e),
    ),
    (
        Color32::from_rgb(0xb8, 0x4a, 0x78),
        Color32::from_rgb(0xe8, 0x89, 0xad),
    ),
    (
        Color32::from_rgb(0x52, 0x60, 0x6e),
        Color32::from_rgb(0x9a, 0xa8, 0xb6),
    ),
];

/// The colour of the `index`th channel in the tree, in this theme.
pub fn channel_color(index: usize, dark_mode: bool) -> Color32 {
    let (light, dark) = PALETTE[index % PALETTE.len()];
    if dark_mode { dark } else { light }
}

/// What persists between frames: the shared time span, the orientation, the
/// mode, any motion in progress, and the buffers reused frame to frame. Each
/// panel's value span lives in its [`crate::layout::Panel`]. The shapes
/// handed to egui are owned by egui and rebuilt each frame.
pub struct PlotState {
    /// What every panel shows along time.
    pub time: Span,
    /// From the earliest start to the latest end of every channel on a panel:
    /// the time span never leaves it.
    bounds: Option<Span>,
    /// When the view last ran into the start and the end of `bounds`: the
    /// edge flashes from then.
    bumped: (Option<f64>, Option<f64>),
    /// This frame's time, for stamping a bump.
    clock: f64,
    pub orientation: Orientation,
    pub mode: Mode,
    tween: Option<Tween<Views>>,
    /// A flick coasting, in the panel it was flicked in.
    inertia: Option<(PanelId, Inertia)>,
    velocity: VelocityTracker,
    /// Where the box being drawn started, and in which panel.
    box_from: Option<(PanelId, Pos2)>,
    /// The panel the keys act on: the one last under the pointer.
    focus: Option<PanelId>,
    /// The channel Align moves, picked by clicking its legend.
    pub active: Option<ChannelRef>,
    /// An Align drag in progress.
    grab: Option<Grab>,
    /// A cursor at this display time while the pointer is off the plot, for
    /// scripted screenshots.
    pub pinned_cursor: Option<f64>,
    wheel: Vec<Wheel>,
    candles: Vec<Bin>,
    samples: Vec<(f64, f64)>,
    points: Vec<Pos2>,
}

/// One scroll event the plot acts on.
struct Wheel {
    /// Already a pan: along time, or with Shift across value.
    delta: Vec2,
    unit: MouseWheelUnit,
    phase: TouchPhase,
    source: MouseWheelSource,
}

impl PlotState {
    pub fn new(time: Span, orientation: Orientation) -> PlotState {
        PlotState {
            time,
            bounds: None,
            bumped: (None, None),
            clock: 0.0,
            orientation,
            mode: Mode::Pan,
            tween: None,
            inertia: None,
            velocity: VelocityTracker::default(),
            box_from: None,
            focus: None,
            active: None,
            grab: None,
            pinned_cursor: None,
            wheel: Vec::new(),
            candles: Vec::new(),
            samples: Vec::new(),
            points: Vec::new(),
        }
    }

    /// What every panel shows now.
    pub fn views(&self, layout: &Layout) -> Views {
        Views::of(self.time, layout)
    }

    /// Ease from what the panels show to `target`.
    pub fn animate_to(&mut self, layout: &Layout, mut target: Views, now: f64) {
        target.time = self.bounded(target.time);
        self.inertia = None;
        self.tween = Some(Tween::new(self.views(layout), target, now));
    }

    /// `time` kept within the channels' time. An end it went past flashes.
    fn bounded(&mut self, time: Span) -> Span {
        let Some(bounds) = self.bounds else {
            return time;
        };
        let past = time.pushed_past(bounds);
        if past.start {
            self.bumped.0 = Some(self.clock);
        }
        if past.end {
            self.bumped.1 = Some(self.clock);
        }
        time.within(bounds)
    }

    /// True when the view ran into an end this frame.
    fn bumped_now(&self) -> bool {
        self.bumped.0 == Some(self.clock) || self.bumped.1 == Some(self.clock)
    }

    /// Where the views are heading: the end of a running animation, or the
    /// views themselves. Steps (keys, wheel notches) add up from here, so a
    /// second press during the animation goes twice as far.
    pub fn target(&self, layout: &Layout) -> Views {
        match &self.tween {
            Some(tween) => tween.target().clone(),
            None => self.views(layout),
        }
    }

    /// True while the views animate or coast.
    pub fn is_moving(&self) -> bool {
        self.tween.is_some() || self.inertia.is_some()
    }

    /// An Align drag in progress is over: the rest of it moves nothing.
    pub fn let_go(&mut self) {
        self.grab = None;
    }

    /// Whatever moved stops where it is: a direct gesture takes over, or the
    /// panels changed under the motion.
    pub fn interrupt(&mut self) {
        self.tween = None;
        self.inertia = None;
    }

    fn set(&mut self, layout: &mut Layout, views: &Views) {
        self.time = self.bounded(views.time);
        views.apply_to(layout);
    }

    /// Advance any animation and coasting to `now`.
    fn advance(&mut self, layout: &mut Layout, panels: &Panels, now: f64) {
        if let Some(tween) = &self.tween {
            let views = tween.at(now);
            let done = tween.is_done(now);
            let applied = views.apply_to(layout);
            if applied {
                self.time = views.time;
            }
            if done || !applied {
                self.tween = None;
            }
        }
        if let Some((id, mut inertia)) = self.inertia {
            let delta = inertia.step(now);
            let views = self.views(layout);
            match panels.mapping(&views, id) {
                Some(mapping) => self.set(layout, &views.with_panel(id, mapping.panned(delta))),
                None => self.inertia = None,
            }
            // A coast that runs into an end stops there.
            if self.bumped_now() {
                self.inertia = None;
            }
            if self.inertia.is_some() {
                self.inertia = (!inertia.is_done(now)).then_some((id, inertia));
            }
        }
    }

    /// Fingers down: whatever moved stops, and a new velocity is measured.
    fn begin_pan(&mut self) {
        self.interrupt();
        self.velocity.clear();
    }

    /// Move panel `id`'s content by `delta` points as a pan gesture reports it.
    fn pan(&mut self, layout: &mut Layout, panels: &Panels, id: PanelId, delta: Vec2, now: f64) {
        let views = self.views(layout);
        if let Some(mapping) = panels.mapping(&views, id) {
            self.set(layout, &views.with_panel(id, mapping.panned(delta)));
            self.velocity.add(now, delta);
        }
    }

    /// Fingers lifted: coast if they were still moving.
    fn release_pan(&mut self, id: PanelId, now: f64) {
        self.inertia = Inertia::flick(self.velocity.velocity(now), now).map(|i| (id, i));
        self.velocity.clear();
    }

    /// Ease panel `id` of the target views to `view`.
    fn animate_panel(&mut self, layout: &Layout, id: PanelId, view: View, now: f64) {
        let target = self.target(layout).with_panel(id, view);
        self.animate_to(layout, target, now);
    }
}

/// An Align drag in progress: where it was pressed, the panel's mapping
/// then, and once it has gone far enough, which axis it moves along.
struct Grab {
    panel: PanelId,
    press: Pos2,
    mapping: Mapping,
    axis: Option<Axis>,
    stretch: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Time,
    Value,
}

impl Grab {
    /// The drag with the pointer at `pointer`, measured in the mapping it
    /// started in, so the anchor stays where it was pressed. The first
    /// [`AXIS_LOCK_PX`] decide the axis, and whether `shift` makes it a
    /// stretch, for the rest of the drag.
    fn follow(&mut self, pointer: Pos2, shift: bool, started: bool, released: bool) -> AlignDrag {
        let (t0, v0) = self.mapping.data_at(self.press);
        let (t1, v1) = self.mapping.data_at(pointer);
        let (dt, dv) = (t1 - t0, v1 - v0);
        if self.axis.is_none() {
            let along = dt.abs() / self.mapping.time_per_px();
            let across = dv.abs() / self.mapping.value_per_px();
            if along.max(across) >= AXIS_LOCK_PX {
                self.axis = Some(if along >= across {
                    Axis::Time
                } else {
                    Axis::Value
                });
                self.stretch = shift;
            }
        }
        let moved = match self.axis {
            Some(Axis::Time) => (dt, 0.0),
            Some(Axis::Value) => (0.0, dv),
            None => (0.0, 0.0),
        };
        AlignDrag {
            anchor: (t0, v0),
            moved,
            stretch: self.stretch,
            started,
            released,
        }
    }
}

/// An Align drag this frame, in display units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AlignDrag {
    /// Display time and value under the press: the drag anchor.
    pub anchor: (f64, f64),
    /// How far the pointer has moved since the press, along time or across
    /// value; the other is zero.
    pub moved: (f64, f64),
    /// Stretch about the anchor instead of moving.
    pub stretch: bool,
    /// The press was this frame: whatever a drag before it left is done.
    pub started: bool,
    /// The pointer let go: the drag is one change.
    pub released: bool,
}

/// One channel a panel draws and how it is labelled.
pub struct Series<'a> {
    pub trace: &'a Trace,
    pub label: &'a str,
    pub unit: &'a str,
    /// The unit of a time column's raw stamps, for the cursor readout.
    pub stamp_unit: &'a str,
    pub color: Color32,
    pub channel: ChannelRef,
}

/// What the plot reports back to the app this frame.
pub struct Shown {
    /// The display time under the pointer.
    pub hover_time: Option<f64>,
    /// The whole plot area.
    pub area: Rect,
    /// Each panel's rectangle, in layout order.
    pub panels: Vec<Rect>,
    /// A legend clicked: Align moves its channel from now on.
    pub clicked: Option<ChannelRef>,
    /// The active channel dragged in Align.
    pub align: Option<AlignDrag>,
    /// The active channel nudged by the arrows in Align: display time and
    /// value to add.
    pub nudge: Option<(f64, f64)>,
}

impl Shown {
    /// Where a channel dragged to `pos` would land; `None` off the plot.
    pub fn drop_target(&self, orientation: Orientation, pos: Pos2) -> Option<Drop> {
        if !self.area.contains(pos) {
            return None;
        }
        let along = |r: Rect| match orientation {
            Orientation::Vertical => (r.min.x, r.max.x),
            Orientation::Horizontal => (r.min.y, r.max.y),
        };
        let spans: Vec<(f32, f32)> = self.panels.iter().map(|&r| along(r)).collect();
        let at = match orientation {
            Orientation::Vertical => pos.x,
            Orientation::Horizontal => pos.y,
        };
        crate::layout::drop_target(&spans, at)
    }
}

/// The panels' rectangles this frame, and their ids, in layout order.
struct Panels {
    ids: Vec<PanelId>,
    rects: Vec<Rect>,
    orientation: Orientation,
}

impl Panels {
    /// The area split evenly along the stacking axis: left to right when
    /// time runs vertically, top to bottom when it runs horizontally.
    fn new(area: Rect, layout: &Layout, orientation: Orientation) -> Panels {
        let ids: Vec<PanelId> = layout.panels().iter().map(|p| p.id).collect();
        let n = ids.len().max(1) as f32;
        let rects = (0..ids.len())
            .map(|i| {
                let (a, b) = (i as f32 / n, (i + 1) as f32 / n);
                match orientation {
                    Orientation::Vertical => Rect::from_x_y_ranges(
                        area.min.x + a * area.width()..=area.min.x + b * area.width(),
                        area.y_range(),
                    ),
                    Orientation::Horizontal => Rect::from_x_y_ranges(
                        area.x_range(),
                        area.min.y + a * area.height()..=area.min.y + b * area.height(),
                    ),
                }
            })
            .collect();
        Panels {
            ids,
            rects,
            orientation,
        }
    }

    /// Panel `id`'s part of `views`, laid onto its rectangle.
    fn mapping(&self, views: &Views, id: PanelId) -> Option<Mapping> {
        let i = self.ids.iter().position(|&p| p == id)?;
        Some(Mapping::new(
            self.rects[i],
            self.orientation,
            views.panel(id)?,
        ))
    }
}

/// From the earliest first sample to the latest last one of every channel
/// on the panels, in display time.
fn time_bounds(series: &[Vec<Series>]) -> Option<Span> {
    series
        .iter()
        .flatten()
        .filter_map(|s| s.trace.display_span())
        .reduce(|a, b| Span::new(a.from.min(b.from), a.to.max(b.to)))
}

/// The value span that fits every one of `traces` over `time`, or over its
/// whole signal when `time` is `None`; `None` when there is nothing to fit.
pub fn fitted_value<'a>(
    traces: impl IntoIterator<Item = &'a Trace>,
    time: Option<Span>,
) -> Option<Span> {
    let mut all = Bin::EMPTY;
    for trace in traces {
        let range = match time {
            Some(time) => trace.range(time),
            None => trace.whole_signal(),
        };
        if let Some((lo, hi)) = range {
            all.include(lo);
            all.include(hi);
        }
    }
    (!all.is_empty()).then(|| Span::fitted(all.min as f64, all.max as f64))
}

/// Draw the panels of `layout`, panel `i` holding `series[i]`, into the
/// available space and handle their gestures and keys:
///
/// - pinch zooms time about the cursor, `Shift+pinch` the value of the panel
///   under it; `+`/`-` zoom time by two about the cursor (with `Shift`,
///   value), animated;
/// - two-finger scroll and the wheel pan along time, with `Shift` across
///   value, and released moving coast; dragging in Pan mode pans both; the arrow keys pan by a quarter view (not in
///   Align); a tap stops a coast;
/// - double-click fits a panel's value axis to the visible window, `A` the
///   panel under the pointer (every panel when it is over none), `Shift+A`
///   the same to the whole signal; double-click on a legend fits its
///   channel's whole signal; a box drawn in Box mode becomes the view. All
///   animate.
///
/// Every panel shows the same time, never past the first or last sample of
/// the channels on them; a gesture in one moves time in all of
/// them and value only in itself. A legend chip dragged away carries its
/// channel as a [`Dragged`] payload.
pub fn show(
    ui: &mut Ui,
    layout: &mut Layout,
    series: &[Vec<Series>],
    state: &mut PlotState,
) -> Shown {
    let (area, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let orientation = state.orientation;
    let panels = Panels::new(area, layout, orientation);
    let colors = Colors::of(ui.visuals());
    let mut shown = Shown {
        hover_time: None,
        area,
        panels: panels.rects.clone(),
        clicked: None,
        align: None,
        nudge: None,
    };
    if area.width() < 2.0 || area.height() < 2.0 {
        return shown;
    }
    if panels.ids.is_empty() {
        let painter = ui.painter_at(area);
        painter.rect_filled(area, 0.0, colors.background);
        painter.text(
            area.center(),
            egui::Align2::CENTER_CENTER,
            "drag a channel here from the tree",
            FontId::proportional(13.0),
            colors.text,
        );
        return shown;
    }
    let now = ui.input(|i| i.time);
    state.clock = now;
    state.bounds = time_bounds(series);
    // Quietly: nobody pushed, the time or the channels just changed. Not
    // under an Align drag, which measures in the view it started in.
    if state.tween.is_none()
        && state.grab.is_none()
        && let Some(bounds) = state.bounds
    {
        state.time = state.time.within(bounds);
    }
    state.advance(layout, &panels, now);

    let pointer = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|&p| ui.rect_contains_pointer(area) && area.contains(p));
    let hovered = pointer.and_then(|p| panels.rects.iter().position(|r| r.contains(p)));
    if let Some(i) = hovered {
        state.focus = Some(panels.ids[i]);
    }
    let responses: Vec<egui::Response> = panels
        .ids
        .iter()
        .zip(&panels.rects)
        .map(|(id, rect)| {
            ui.interact(
                *rect,
                ui.scope_id().with(("panel", *id)),
                Sense::click_and_drag(),
            )
        })
        .collect();

    // While a channel is dragged the plot holds still under the drop target.
    let dragging = egui::DragAndDrop::has_any_payload(ui.ctx());
    if let Some(i) = hovered.filter(|_| !dragging) {
        // A tap stops a coast.
        if ui.input(|i| i.pointer.any_pressed()) {
            state.inertia = None;
        }
        scroll_and_pinch(ui, state, layout, &panels, i, pointer, now);
    }

    // Dragging: pan with inertia, or draw a box.
    for (i, response) in responses.iter().enumerate() {
        let id = panels.ids[i];
        match state.mode {
            Mode::Pan => {
                if response.drag_started() {
                    state.begin_pan();
                }
                if response.dragged() {
                    state.pan(layout, &panels, id, response.drag_delta(), now);
                }
                if response.drag_stopped() {
                    state.release_pan(id, now);
                }
            }
            Mode::Box => {
                if response.drag_started()
                    && let Some(from) = ui.input(|i| i.pointer.press_origin())
                {
                    state.box_from = Some((id, from));
                }
                if response.drag_stopped()
                    && let (Some((_, from)), Some(to)) =
                        (state.box_from.take(), response.interact_pointer_pos())
                {
                    let drawn = Rect::from_two_pos(from, to).intersect(panels.rects[i]);
                    let views = state.views(layout);
                    if drawn.width() >= MIN_BOX_PX
                        && drawn.height() >= MIN_BOX_PX
                        && let Some(mapping) = panels.mapping(&views, id)
                    {
                        state.animate_panel(layout, id, mapping.view_of(drawn), now);
                    }
                }
            }
            Mode::Align => {
                let holds_active = holds(&series[i], state.active);
                if holds_active && (response.hovered() || response.dragged()) {
                    ui.ctx().set_cursor_icon(if response.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Grab
                    });
                }
                let mut started = false;
                if response.drag_started()
                    && holds_active
                    && let Some(press) = ui.input(|i| i.pointer.press_origin())
                    && let Some(mapping) = panels.mapping(&state.views(layout), id)
                {
                    state.interrupt();
                    state.grab = Some(Grab {
                        panel: id,
                        press,
                        mapping,
                        axis: None,
                        stretch: false,
                    });
                    started = true;
                }
                let released = response.drag_stopped();
                if (response.dragged() || released)
                    && let Some(grab) = state.grab.as_mut().filter(|g| g.panel == id)
                    && let Some(pointer) = response.interact_pointer_pos()
                {
                    let shift = ui.input(|i| i.modifiers.shift);
                    shown.align = Some(grab.follow(pointer, shift, started, released));
                }
                if released {
                    state.grab = None;
                }
            }
        }
        // Double-click fits this panel to the visible window.
        if response.double_clicked() {
            autoscale(state, layout, series, &[i], false, now);
        }
    }

    if state.mode != Mode::Align {
        state.grab = None;
    }

    // Keys typed into a field are not the plot's.
    let typing = ui.ctx().egui_wants_keyboard_input();
    // A or Shift+A: the panel under the pointer, or every panel.
    let (key_a, shift) = ui.input(|i| (i.key_pressed(Key::A), i.modifiers.shift));
    if key_a && !typing {
        let which: Vec<usize> = match hovered {
            Some(i) => vec![i],
            None => (0..panels.ids.len()).collect(),
        };
        autoscale(state, layout, series, &which, shift, now);
    }

    let focus = state
        .focus
        .and_then(|id| panels.ids.iter().position(|&p| p == id))
        .unwrap_or(0);
    // Align nudges in the focused panel when it shows the active channel,
    // else in the first one that does.
    let nudged = std::iter::once(focus)
        .chain(0..panels.ids.len())
        .find(|&i| holds(&series[i], state.active));
    if !typing {
        shown.nudge = keys(ui, state, layout, &panels, focus, nudged, pointer, now);
    }

    if state.is_moving() {
        ui.ctx().request_repaint();
    }

    let views = state.views(layout);
    let pixels_per_point = ui.ctx().pixels_per_point();
    shown.hover_time = hovered
        .zip(pointer)
        .and_then(|(i, p)| Some(panels.mapping(&views, panels.ids[i])?.data_at(p).0))
        .or(state.pinned_cursor);
    // Time labels where time starts on the stacking axis: the first panel
    // when vertical, the last when horizontal.
    let time_panel = match orientation {
        Orientation::Vertical => 0,
        Orientation::Horizontal => panels.ids.len() - 1,
    };
    for (i, &id) in panels.ids.iter().enumerate() {
        let Some(mapping) = panels.mapping(&views, id) else {
            continue;
        };
        let rect = panels.rects[i];
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, colors.background);
        let grid = Grid::new(&mapping, common_unit(&series[i]), i == time_panel);
        grid.draw_lines(&painter, &colors);
        for s in &series[i] {
            draw_trace(&painter, &mapping, s, state, pixels_per_point);
        }
        grid.draw_labels(&painter, &colors);
        if draw_edges(&painter, &mapping, state, &colors) {
            ui.ctx().request_repaint();
        }

        let readings: Vec<Option<Reading>> = series[i]
            .iter()
            .map(|s| shown.hover_time.and_then(|t| s.trace.nearest(t)))
            .collect();
        if let Some(t) = shown.hover_time {
            draw_cursor(
                &painter,
                &mapping,
                t,
                &series[i],
                &readings,
                i == time_panel,
                &colors,
            );
        }

        let legend = Legend::new(ui, &mapping, &series[i], &readings, &colors);
        for (k, chip) in legend.chips.iter().enumerate() {
            let response = ui.interact(
                chip.rect,
                ui.scope_id().with(("legend", id, k)),
                Sense::click_and_drag(),
            );
            response.dnd_set_drag_payload(Dragged {
                channel: series[i][k].channel,
                from: Some(id),
            });
            if response.clicked() {
                shown.clicked = Some(series[i][k].channel);
            }
            if response.double_clicked()
                && let Some(value) = fitted_value([series[i][k].trace], None)
            {
                let time = state.target(layout).time;
                state.animate_panel(layout, id, View { time, value }, now);
            }
            let active = state.mode == Mode::Align && state.active == Some(series[i][k].channel);
            chip.draw(
                &painter,
                &colors,
                series[i][k].color,
                response.hovered(),
                active,
            );
        }

        // A stretch's anchor: the point that stays put.
        if let Some(grab) = &state.grab
            && grab.panel == id
            && grab.stretch
            && grab.axis.is_some()
        {
            let (t, v) = grab.mapping.data_at(grab.press);
            let at = mapping.screen_pos(t, v);
            let line = series[i]
                .iter()
                .find(|s| Some(s.channel) == state.active)
                .map_or(colors.text, |s| s.color);
            painter.circle(at, 5.0, colors.background, Stroke::new(1.5, line));
            painter.circle_filled(at, 1.5, line);
        }

        if state.mode == Mode::Box
            && let Some((box_id, from)) = state.box_from
            && box_id == id
            && let Some(to) = responses[i].interact_pointer_pos()
        {
            let drawn = Rect::from_two_pos(from, to).intersect(rect);
            let line = series[i].first().map_or(colors.text, |s| s.color);
            painter.rect(
                drawn,
                2.0,
                line.gamma_multiply(0.12),
                Stroke::new(1.0, line),
                StrokeKind::Inside,
            );
        }
    }
    // A hairline between neighbouring panels.
    let painter = ui.painter_at(area);
    for rect in &panels.rects[1..] {
        match orientation {
            Orientation::Vertical => painter.vline(
                painter.round_to_pixel_center(rect.min.x),
                area.y_range(),
                Stroke::new(1.0, colors.separator),
            ),
            Orientation::Horizontal => painter.hline(
                area.x_range(),
                painter.round_to_pixel_center(rect.min.y),
                Stroke::new(1.0, colors.separator),
            ),
        };
    }
    shown
}

/// Two-finger scroll, wheels and pinch over panel `i`. A scroll pans along
/// time, with Shift across value. The OS's own momentum is ignored: the plot
/// coasts by itself, so a tap can stop it.
fn scroll_and_pinch(
    ui: &Ui,
    state: &mut PlotState,
    layout: &mut Layout,
    panels: &Panels,
    i: usize,
    cursor: Option<Pos2>,
    now: f64,
) {
    let id = panels.ids[i];
    let orientation = state.orientation;
    state.wheel.clear();
    ui.input(|input| {
        state
            .wheel
            .extend(input.events.iter().filter_map(|e| match e {
                Event::MouseWheel {
                    unit,
                    delta,
                    phase,
                    source,
                    modifiers,
                } if !modifiers.command && !modifiers.ctrl => Some(Wheel {
                    delta: orientation.scroll(*delta, modifiers.shift),
                    unit: *unit,
                    phase: *phase,
                    source: *source,
                }),
                _ => None,
            }))
    });
    let line_px = ui.ctx().options(|o| o.input_options.line_scroll_speed);
    for k in 0..state.wheel.len() {
        let wheel = &state.wheel[k];
        let (delta, unit) = (wheel.delta, wheel.unit);
        match (wheel.source, unit, wheel.phase) {
            (MouseWheelSource::Momentum, ..) => {}
            (_, _, TouchPhase::Start) => state.begin_pan(),
            (MouseWheelSource::Trackpad, _, TouchPhase::End | TouchPhase::Cancel) => {
                state.release_pan(id, now)
            }
            (_, MouseWheelUnit::Point, _) => {
                state.interrupt();
                state.pan(layout, panels, id, delta, now);
            }
            (..) => {
                // A wheel notch is a step: ease to it.
                let step = match unit {
                    MouseWheelUnit::Page => delta * panels.rects[i].size(),
                    _ => delta * line_px,
                };
                let target = state.target(layout);
                if let Some(mapping) = panels.mapping(&target, id) {
                    state.animate_to(layout, target.with_panel(id, mapping.panned(step)), now);
                }
            }
        }
    }

    let (zoom, shift) = ui.input(|i| (i.zoom_delta(), i.modifiers.shift));
    if let Some(cursor) = cursor
        && zoom != 1.0
    {
        state.interrupt();
        let views = state.views(layout);
        if let Some(mapping) = panels.mapping(&views, id) {
            let view = if shift {
                mapping.zoomed_value(zoom as f64, cursor)
            } else {
                mapping.zoomed_time(zoom as f64, cursor)
            };
            state.set(layout, &views.with_panel(id, view));
        }
    }
}

/// Fit the value axis of each panel in `which` to its channels over the
/// visible window, or over their whole signals, in one animation.
pub fn autoscale(
    state: &mut PlotState,
    layout: &Layout,
    series: &[Vec<Series>],
    which: &[usize],
    whole: bool,
    now: f64,
) {
    let mut target = state.target(layout);
    for &i in which {
        let id = layout.panels()[i].id;
        let time = target.time;
        if let Some(value) =
            fitted_value(series[i].iter().map(|s| s.trace), (!whole).then_some(time))
        {
            target = target.with_panel(id, View { time, value });
        }
    }
    state.animate_to(layout, target, now);
}

/// Whether `series` draws `channel`.
fn holds(series: &[Series], channel: Option<ChannelRef>) -> bool {
    channel.is_some_and(|c| series.iter().any(|s| s.channel == c))
}

/// The plot's keys, on the focused panel: `+`/`-` zoom time by two about the
/// cursor (or the centre), with `Shift` value; the arrows pan by a quarter
/// view, each a step, so it eases. In Align the arrows nudge the active
/// channel instead, shown in panel `nudged`, by a pixel (`Shift`: ten) the
/// way they point: the display time and value to add are returned.
#[allow(clippy::too_many_arguments)]
fn keys(
    ui: &Ui,
    state: &mut PlotState,
    layout: &Layout,
    panels: &Panels,
    focus: usize,
    nudged: Option<usize>,
    cursor: Option<Pos2>,
    now: f64,
) -> Option<(f64, f64)> {
    let (zoom, value, arrow) = ui.input(|i| {
        let mut zoom = 1.0;
        let mut arrow = Vec2::ZERO;
        for event in &i.events {
            let Event::Key {
                key,
                physical_key,
                pressed: true,
                ..
            } = event
            else {
                continue;
            };
            match physical_key.unwrap_or(*key) {
                Key::Equals | Key::Plus => zoom *= ZOOM_STEP,
                Key::Minus => zoom /= ZOOM_STEP,
                Key::ArrowUp => arrow.y -= 1.0,
                Key::ArrowDown => arrow.y += 1.0,
                Key::ArrowLeft => arrow.x -= 1.0,
                Key::ArrowRight => arrow.x += 1.0,
                _ => {}
            }
        }
        (zoom, i.modifiers.shift, arrow)
    });
    if state.mode == Mode::Align && arrow != Vec2::ZERO {
        let i = nudged?;
        let mapping = panels.mapping(&state.views(layout), panels.ids[i])?;
        let step = if value { SHIFT_NUDGE_PX } else { NUDGE_PX };
        let from = panels.rects[i].center();
        let (t0, v0) = mapping.data_at(from);
        let (t1, v1) = mapping.data_at(from + arrow * step);
        return Some((t1 - t0, v1 - v0));
    }
    let id = panels.ids[focus];
    let rect = panels.rects[focus];
    let target = state.target(layout);
    let mapping = panels.mapping(&target, id)?;
    if zoom != 1.0 {
        let about = cursor
            .filter(|p| rect.contains(*p))
            .unwrap_or(rect.center());
        let view = if value {
            mapping.zoomed_value(zoom, about)
        } else {
            mapping.zoomed_time(zoom, about)
        };
        state.animate_to(layout, target.with_panel(id, view), now);
    } else if arrow != Vec2::ZERO {
        // The window moves the way the arrow points; the content the other.
        let view = mapping.panned(-arrow * rect.size() * ARROW_PAN);
        state.animate_to(layout, target.with_panel(id, view), now);
    }
    None
}

/// The unit every channel on a panel shares, or none when they differ.
fn common_unit<'a>(series: &[Series<'a>]) -> &'a str {
    match series.split_first() {
        Some((first, rest)) if rest.iter().all(|s| s.unit == first.unit) => first.unit,
        _ => "",
    }
}

/// One channel as candles or, zoomed in far enough, as its raw samples.
/// Candles are computed per physical pixel, not per point, so a Retina screen
/// gets one candle per device pixel.
fn draw_trace(
    painter: &egui::Painter,
    mapping: &Mapping,
    series: &Series,
    state: &mut PlotState,
    pixels_per_point: f32,
) {
    let trace = series.trace;
    let time = mapping.view.time;
    let px = (mapping.along_px() * pixels_per_point).round().max(1.0) as usize;
    let time_per_px = time.len() / px as f64;
    if trace.draws_samples(time_per_px) {
        trace.samples(time, &mut state.samples);
        let dots =
            trace.samples_per_px(time_per_px * pixels_per_point as f64) <= 1.0 / DOT_SPACING_PX;
        draw_samples(
            painter,
            mapping,
            &state.samples,
            &mut state.points,
            Stroke::new(1.0, series.color),
            dots,
        );
    } else {
        trace.candles(time, px, &mut state.candles);
        draw_candles(
            painter,
            mapping,
            &state.candles,
            pixels_per_point,
            series.color,
        );
    }
}

/// A thick bar on each end of the panel's time where the view rests
/// against the start or end of the channels' time, flashing red when the
/// view ran into it and fading back to grey. True while a flash fades.
fn draw_edges(
    painter: &egui::Painter,
    mapping: &Mapping,
    state: &PlotState,
    colors: &Colors,
) -> bool {
    const THICKNESS: f32 = 4.0;
    let Some(bounds) = state.bounds else {
        return false;
    };
    let against = mapping.view.time.touches(bounds);
    let flash = |at: Option<f64>| at.map_or(0.0, |at| motion::flash(state.clock - at));
    let strength = (flash(state.bumped.0), flash(state.bumped.1));
    let rect = mapping.rect;
    let bar = |start: bool| match (mapping.orientation, start) {
        (Orientation::Vertical, true) => {
            Rect::from_x_y_ranges(rect.x_range(), rect.min.y..=rect.min.y + THICKNESS)
        }
        (Orientation::Vertical, false) => {
            Rect::from_x_y_ranges(rect.x_range(), rect.max.y - THICKNESS..=rect.max.y)
        }
        (Orientation::Horizontal, true) => {
            Rect::from_x_y_ranges(rect.min.x..=rect.min.x + THICKNESS, rect.y_range())
        }
        (Orientation::Horizontal, false) => {
            Rect::from_x_y_ranges(rect.max.x - THICKNESS..=rect.max.x, rect.y_range())
        }
    };
    let ends = [
        (true, against.start, strength.0),
        (false, against.end, strength.1),
    ];
    for (start, rests, flash) in ends {
        if rests || flash > 0.0 {
            painter.rect_filled(
                bar(start),
                0.0,
                colors.edge.lerp_to_gamma(colors.bump, flash),
            );
        }
    }
    strength.0 > 0.0 || strength.1 > 0.0
}

/// The cursor across the panel at display time `t`, a dot on each channel's
/// nearest sample, and on the time panel the instant itself.
fn draw_cursor(
    painter: &egui::Painter,
    mapping: &Mapping,
    t: f64,
    series: &[Series],
    readings: &[Option<Reading>],
    time_label: bool,
    colors: &Colors,
) {
    let rect = mapping.rect;
    let along = painter.round_to_pixel_center(mapping.time_to_along(t));
    let (a, b) = match mapping.orientation {
        Orientation::Vertical => (pos2(rect.min.x, along), pos2(rect.max.x, along)),
        Orientation::Horizontal => (pos2(along, rect.min.y), pos2(along, rect.max.y)),
    };
    painter.extend(Shape::dashed_line(
        &[a, b],
        Stroke::new(1.0, colors.cursor),
        3.0,
        3.0,
    ));
    for (s, reading) in series.iter().zip(readings) {
        if let Some(r) = reading {
            let p = mapping.screen_pos(r.time, r.value);
            if rect.contains(p) {
                painter.circle(p, 3.5, colors.background, Stroke::new(1.5, s.color));
            }
        }
    }
    if time_label {
        // As many decimals as a pixel along time resolves.
        let decimals = (-mapping.time_per_px().log10()).ceil().clamp(0.0, 9.0) as usize;
        let text = format!("{t:.decimals$} s");
        let (pos, anchor) = match mapping.orientation {
            Orientation::Vertical => (pos2(rect.min.x + 4.0, along), egui::Align2::LEFT_CENTER),
            Orientation::Horizontal => {
                (pos2(along, rect.max.y - 18.0), egui::Align2::CENTER_BOTTOM)
            }
        };
        label_with_pill(
            painter,
            pos,
            anchor,
            &text,
            &FontId::proportional(11.0),
            colors,
            colors.background,
        );
    }
}

/// The legend: a chip per channel, stacked in the corner no axis labels use
/// (bottom right when vertical, top right when horizontal), with the
/// channel's colour and name and, under the cursor, its nearest sample's
/// value and stamp. Double-click fits the channel's whole signal; dragging a
/// chip moves its channel.
struct Legend {
    chips: Vec<Chip>,
}

struct Chip {
    rect: Rect,
    name: Arc<Galley>,
    /// The reading's value and unit, then its stamp, when the cursor is on.
    lines: Vec<Arc<Galley>>,
}

impl Legend {
    const SWATCH: f32 = 14.0;
    const PAD: Vec2 = vec2(8.0, 4.0);
    const GAP: f32 = 4.0;

    fn new(
        ui: &Ui,
        mapping: &Mapping,
        series: &[Series],
        readings: &[Option<Reading>],
        colors: &Colors,
    ) -> Legend {
        let painter = ui.painter();
        let mono = FontId::monospace(11.0);
        let mut chips: Vec<Chip> = series
            .iter()
            .zip(readings)
            .map(|(s, reading)| {
                let name = painter.layout_no_wrap(
                    s.label.to_string(),
                    FontId::proportional(12.0),
                    colors.text,
                );
                let mut lines = Vec::new();
                if let Some(r) = reading {
                    let value = format!("{} {}", r.value_text(), s.unit);
                    lines.push(painter.layout_no_wrap(
                        value.trim_end().to_string(),
                        mono.clone(),
                        colors.strong,
                    ));
                    if let Some(stamp) = r.stamp {
                        let stamp = format!("{stamp} {}", s.stamp_unit);
                        lines.push(painter.layout_no_wrap(stamp, mono.clone(), colors.text));
                    }
                }
                let indent = Self::SWATCH + 6.0;
                let width = lines
                    .iter()
                    .map(|g| g.size().x)
                    .fold(name.size().x, f32::max);
                let height = name.size().y + lines.iter().map(|g| g.size().y).sum::<f32>();
                Chip {
                    rect: Rect::from_min_size(
                        Pos2::ZERO,
                        vec2(indent + width, height) + 2.0 * Self::PAD,
                    ),
                    name,
                    lines,
                }
            })
            .collect();
        // Stack away from the corner, first channel nearest the time axis's
        // start.
        let plot = mapping.rect.shrink(8.0);
        match mapping.orientation {
            Orientation::Vertical => {
                let mut bottom = plot.max.y;
                for chip in chips.iter_mut().rev() {
                    chip.rect = egui::Align2::RIGHT_BOTTOM
                        .anchor_size(pos2(plot.max.x, bottom), chip.rect.size());
                    bottom = chip.rect.min.y - Self::GAP;
                }
            }
            Orientation::Horizontal => {
                let mut top = plot.min.y;
                for chip in chips.iter_mut() {
                    chip.rect = egui::Align2::RIGHT_TOP
                        .anchor_size(pos2(plot.max.x, top), chip.rect.size());
                    top = chip.rect.max.y + Self::GAP;
                }
            }
        }
        Legend { chips }
    }
}

impl Chip {
    /// The chip, outlined in its channel's colour when Align moves it.
    fn draw(
        &self,
        painter: &egui::Painter,
        colors: &Colors,
        line: Color32,
        hovered: bool,
        active: bool,
    ) {
        let fill = if hovered {
            colors.pill_hover
        } else {
            colors.pill
        };
        painter.rect_filled(self.rect, 6.0, fill);
        if active {
            painter.rect_stroke(self.rect, 6.0, Stroke::new(1.5, line), StrokeKind::Inside);
        }
        let x = self.rect.min.x + Legend::PAD.x;
        let mut y = self.rect.min.y + Legend::PAD.y;
        let name_mid = y + self.name.size().y / 2.0;
        painter.hline(x..=x + Legend::SWATCH, name_mid, Stroke::new(2.0, line));
        let text_x = x + Legend::SWATCH + 6.0;
        painter.galley(pos2(text_x, y), self.name.clone(), colors.text);
        y += self.name.size().y;
        for galley in &self.lines {
            painter.galley(pos2(text_x, y), galley.clone(), colors.text);
            y += galley.size().y;
        }
    }
}

/// Where a channel dragged over the plot would land: a dashed outline over
/// the panel it would overlay, or a bar on the edge where a new panel would
/// open. `text` says which.
pub fn draw_drop(
    painter: &egui::Painter,
    shown: &Shown,
    orientation: Orientation,
    drop: Drop,
    color: Color32,
    text: &str,
) {
    let target = match drop {
        Drop::Overlay(i) => shown.panels[i].shrink(4.0),
        Drop::NewPanel(_) if shown.panels.is_empty() => shown.area.shrink(4.0),
        Drop::NewPanel(i) => {
            let edge = match (orientation, shown.panels.get(i)) {
                (Orientation::Vertical, Some(r)) => r.min.x,
                (Orientation::Vertical, None) => shown.area.max.x,
                (Orientation::Horizontal, Some(r)) => r.min.y,
                (Orientation::Horizontal, None) => shown.area.max.y,
            };
            let area = shown.area.shrink(2.0);
            match orientation {
                Orientation::Vertical => {
                    let x = edge.clamp(area.min.x + 2.0, area.max.x - 2.0);
                    Rect::from_x_y_ranges(x - 2.0..=x + 2.0, area.y_range())
                }
                Orientation::Horizontal => {
                    let y = edge.clamp(area.min.y + 2.0, area.max.y - 2.0);
                    Rect::from_x_y_ranges(area.x_range(), y - 2.0..=y + 2.0)
                }
            }
        }
    };
    let bar = target.width() <= 4.0 || target.height() <= 4.0;
    if bar {
        painter.rect_filled(target, 2.0, color);
    } else {
        draw_outline(painter, target, color);
    }
    let galley =
        painter.layout_no_wrap(text.to_string(), FontId::proportional(12.0), Color32::WHITE);
    let tag = Rect::from_center_size(target.center(), galley.size() + vec2(16.0, 6.0));
    painter.rect_filled(tag, 9.0, color);
    painter.galley(tag.center() - galley.size() / 2.0, galley, Color32::WHITE);
}

/// A drop target's area: tinted in `color` and outlined with dashes.
pub fn draw_outline(painter: &egui::Painter, rect: Rect, color: Color32) {
    painter.rect_filled(rect, 6.0, color.gamma_multiply(0.10));
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    painter.extend(Shape::dashed_line(
        &corners,
        Stroke::new(2.0, color),
        6.0,
        4.0,
    ));
}

struct Colors {
    background: Color32,
    grid: Color32,
    text: Color32,
    /// The cursor readout's values.
    strong: Color32,
    /// Behind grid labels and the legend, so they read over a dense trace.
    pill: Color32,
    pill_hover: Color32,
    cursor: Color32,
    separator: Color32,
    /// An end of the channels' time the view rests against.
    edge: Color32,
    /// The flash of an end the view ran into.
    bump: Color32,
}

impl Colors {
    fn of(visuals: &egui::Visuals) -> Colors {
        if visuals.dark_mode {
            Colors {
                background: Color32::from_gray(22),
                grid: Color32::from_gray(44),
                text: Color32::from_gray(150),
                strong: Color32::from_gray(225),
                pill: Color32::from_rgba_unmultiplied(22, 22, 22, 190),
                pill_hover: Color32::from_rgba_unmultiplied(44, 44, 44, 230),
                cursor: Color32::from_gray(120),
                separator: Color32::from_gray(60),
                edge: Color32::from_gray(95),
                bump: Color32::from_rgb(0xf0, 0x5a, 0x6a),
            }
        } else {
            Colors {
                background: Color32::WHITE,
                grid: Color32::from_gray(230),
                text: Color32::from_gray(110),
                strong: Color32::from_gray(25),
                pill: Color32::from_rgba_unmultiplied(255, 255, 255, 190),
                pill_hover: Color32::from_rgba_unmultiplied(236, 236, 236, 235),
                cursor: Color32::from_gray(140),
                separator: Color32::from_gray(215),
                edge: Color32::from_gray(175),
                bump: Color32::from_rgb(0xe0, 0x2f, 0x44),
            }
        }
    }
}

/// The 1-2-5 grid of one frame: lines go under the trace, labels on top of
/// it (with a backing pill so they stay legible over a dense signal). Time
/// labels sit where time starts, value labels where value starts.
struct Grid {
    mapping: Mapping,
    time_lines: Vec<(f32, String)>,
    value_lines: Vec<(f32, String)>,
    /// Draw the time labels: only one panel does.
    time_labels: bool,
}

impl Grid {
    fn new(mapping: &Mapping, unit: &str, time_labels: bool) -> Grid {
        let view = mapping.view;
        let axis = ticks::time_axis(view.time, mapping.along_px(), MIN_PX_PER_TIME_TICK);
        let time_lines = ticks::ticks(view.time.from, view.time.to, axis.step_s)
            .into_iter()
            .map(|t| (mapping.time_to_along(t), axis.label(t)))
            .collect();
        let step = ticks::value_step(view.value.len(), mapping.across_px(), MIN_PX_PER_VALUE_TICK);
        let suffix = if unit.is_empty() {
            String::new()
        } else {
            format!(" {unit}")
        };
        let value_lines = ticks::ticks(view.value.from, view.value.to, step)
            .into_iter()
            .map(|v| {
                (
                    mapping.value_to_across(v),
                    format!("{}{suffix}", ticks::format_value(v, step)),
                )
            })
            .collect();
        Grid {
            mapping: *mapping,
            time_lines,
            value_lines,
            time_labels,
        }
    }

    fn draw_lines(&self, painter: &egui::Painter, colors: &Colors) {
        let rect = self.mapping.rect;
        let stroke = Stroke::new(1.0, colors.grid);
        for (along, _) in &self.time_lines {
            let along = painter.round_to_pixel_center(*along);
            match self.mapping.orientation {
                Orientation::Vertical => painter.hline(rect.x_range(), along, stroke),
                Orientation::Horizontal => painter.vline(along, rect.y_range(), stroke),
            };
        }
        for (across, _) in &self.value_lines {
            let across = painter.round_to_pixel_center(*across);
            match self.mapping.orientation {
                Orientation::Vertical => painter.vline(across, rect.y_range(), stroke),
                Orientation::Horizontal => painter.hline(rect.x_range(), across, stroke),
            };
        }
    }

    fn draw_labels(&self, painter: &egui::Painter, colors: &Colors) {
        let rect = self.mapping.rect;
        let font = FontId::proportional(11.0);
        // Time labels: left edge when vertical, bottom edge when horizontal.
        // Value labels: top edge when vertical, left edge when horizontal.
        if self.time_labels {
            for (along, label) in &self.time_lines {
                let (pos, anchor) = match self.mapping.orientation {
                    Orientation::Vertical => {
                        (pos2(rect.min.x + 4.0, along + 2.0), egui::Align2::LEFT_TOP)
                    }
                    Orientation::Horizontal => (
                        pos2(along + 3.0, rect.max.y - 2.0),
                        egui::Align2::LEFT_BOTTOM,
                    ),
                };
                label_with_pill(painter, pos, anchor, label, &font, colors, colors.pill);
            }
        }
        for (across, label) in &self.value_lines {
            let (pos, anchor) = match self.mapping.orientation {
                Orientation::Vertical => {
                    (pos2(across + 3.0, rect.min.y + 2.0), egui::Align2::LEFT_TOP)
                }
                Orientation::Horizontal => (
                    pos2(rect.min.x + 4.0, across - 2.0),
                    egui::Align2::LEFT_BOTTOM,
                ),
            };
            label_with_pill(painter, pos, anchor, label, &font, colors, colors.pill);
        }
    }
}

fn label_with_pill(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: &str,
    font: &FontId,
    colors: &Colors,
    fill: Color32,
) {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), colors.text);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(rect.expand2(Vec2::new(3.0, 1.0)), 3.0, fill);
    painter.galley(rect.min, galley, colors.text);
}

/// One candle per physical pixel along time: an axis-aligned bar from the
/// min to the max, extended to touch the previous candle so the trace is
/// continuous, as one mesh (no path joins, so no join artifacts). An empty
/// bin draws nothing: a gap.
fn draw_candles(
    painter: &egui::Painter,
    mapping: &Mapping,
    candles: &[Bin],
    pixels_per_point: f32,
    color: Color32,
) {
    let along_start = match mapping.orientation {
        Orientation::Vertical => mapping.rect.min.y,
        Orientation::Horizontal => mapping.rect.min.x,
    };
    let width = 1.0 / pixels_per_point;
    let mut mesh = Mesh::default();
    let mut previous: Option<(f32, f32)> = None;
    for (i, bin) in candles.iter().enumerate() {
        if bin.is_empty() {
            previous = None;
            continue;
        }
        let a = mapping.value_to_across(bin.min as f64);
        let b = mapping.value_to_across(bin.max as f64);
        let (mut lo, mut hi) = (a.min(b), a.max(b));
        if let Some((prev_lo, prev_hi)) = previous {
            // Reach the previous candle's nearest edge so the line has no breaks.
            if prev_hi < lo {
                lo = prev_hi;
            } else if prev_lo > hi {
                hi = prev_lo;
            }
        }
        previous = Some((a.min(b), a.max(b)));
        // At least one device pixel thick, so a flat signal is still a line.
        if hi - lo < width {
            let mid = 0.5 * (lo + hi);
            lo = mid - 0.5 * width;
            hi = mid + 0.5 * width;
        }
        let along = along_start + i as f32 * width;
        let rect = match mapping.orientation {
            Orientation::Vertical => Rect::from_min_max(pos2(lo, along), pos2(hi, along + width)),
            Orientation::Horizontal => Rect::from_min_max(pos2(along, lo), pos2(along + width, hi)),
        };
        mesh.add_colored_rect(rect, color);
    }
    if !mesh.is_empty() {
        painter.add(Shape::mesh(mesh));
    }
}

/// Raw samples as a line through their exact instants and, when they are far
/// enough apart, a dot on each. A `NaN` sample breaks the line.
fn draw_samples(
    painter: &egui::Painter,
    mapping: &Mapping,
    samples: &[(f64, f64)],
    points: &mut Vec<Pos2>,
    stroke: Stroke,
    dots: bool,
) {
    points.clear();
    for &(t, v) in samples {
        if v.is_nan() {
            flush(painter, points, stroke);
            continue;
        }
        let p = mapping.screen_pos(t, v);
        if dots {
            painter.circle_filled(p, 2.0, stroke.color);
        }
        points.push(p);
    }
    flush(painter, points, stroke);
}

/// Draw the path collected so far and start a new one. A lone point is a dot,
/// so a single visible sample never disappears.
fn flush(painter: &egui::Painter, points: &mut Vec<Pos2>, stroke: Stroke) {
    match points.len() {
        0 => {}
        1 => {
            painter.circle_filled(points[0], 1.0, stroke.color);
            points.clear();
        }
        _ => {
            painter.add(Shape::line(std::mem::take(points), stroke));
        }
    }
}
