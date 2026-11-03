//! The plot widget: one trace drawn on the egui painter as min/max candles,
//! raw samples below the base bin, a 1-2-5 grid on both axes, a legend. It
//! lives in along-time / across-value coordinates; [`Mapping`] does the
//! orientation. Gestures move the view directly; jumps (keys, autoscale,
//! box, wheel steps) ease to their target and a flick coasts, see
//! [`crate::motion`].

use crate::motion::{Inertia, Tween, VelocityTracker};
use crate::ticks;
use crate::trace::Trace;
use crate::view::{Mapping, Orientation, Span, View};
use egui::{
    Color32, Event, FontId, Key, Mesh, MouseWheelSource, MouseWheelUnit, Pos2, Rect, Sense, Shape,
    Stroke, StrokeKind, TouchPhase, Ui, Vec2, pos2, vec2,
};
use sigkit_format::Bin;

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

/// The mouse tool. Keys 1, 2, 3 are its accelerators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Drag pans, with inertia (the default).
    Pan,
    /// Drag the active stream to align it. Arrives with ticket 06.
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

/// The colour of the `index`th channel of a recording, in this theme.
pub fn channel_color(index: usize, dark_mode: bool) -> Color32 {
    let (light, dark) = PALETTE[index % PALETTE.len()];
    if dark_mode { dark } else { light }
}

/// What persists between frames: the view, the orientation, the mode, any
/// motion in progress, and the buffers reused frame to frame. The shapes
/// handed to egui are owned by egui and rebuilt each frame.
pub struct PlotState {
    pub view: View,
    pub orientation: Orientation,
    pub mode: Mode,
    tween: Option<Tween>,
    inertia: Option<Inertia>,
    velocity: VelocityTracker,
    /// Where the box being drawn started, on screen.
    box_from: Option<Pos2>,
    wheel: Vec<Wheel>,
    candles: Vec<Bin>,
    samples: Vec<(f64, f64)>,
    points: Vec<Pos2>,
}

/// One scroll event the plot acts on.
struct Wheel {
    delta: Vec2,
    unit: MouseWheelUnit,
    phase: TouchPhase,
    source: MouseWheelSource,
}

impl PlotState {
    pub fn new(view: View, orientation: Orientation) -> PlotState {
        PlotState {
            view,
            orientation,
            mode: Mode::Pan,
            tween: None,
            inertia: None,
            velocity: VelocityTracker::default(),
            box_from: None,
            wheel: Vec::new(),
            candles: Vec::new(),
            samples: Vec::new(),
            points: Vec::new(),
        }
    }

    /// Ease from the current view to `target`.
    pub fn animate_to(&mut self, target: View, now: f64) {
        self.inertia = None;
        self.tween = Some(Tween::new(self.view, target, now));
    }

    /// Where the view is heading: the end of a running animation, or the
    /// view itself. Steps (keys, wheel notches) add up from here, so a
    /// second press during the animation goes twice as far.
    pub fn target(&self) -> View {
        self.tween.map(|t| t.target()).unwrap_or(self.view)
    }

    /// True while the view animates or coasts.
    pub fn is_moving(&self) -> bool {
        self.tween.is_some() || self.inertia.is_some()
    }

    /// Advance any animation and coasting to `now`.
    fn advance(&mut self, mapping: impl Fn(View) -> Mapping, now: f64) {
        if let Some(tween) = self.tween {
            self.view = tween.at(now);
            if tween.is_done(now) {
                self.tween = None;
            }
        }
        if let Some(inertia) = &mut self.inertia {
            let delta = inertia.step(now);
            self.view = mapping(self.view).panned(delta);
            if inertia.is_done(now) {
                self.inertia = None;
            }
        }
    }

    /// A direct gesture takes over: the view stays where the animation had
    /// got to, and nothing coasts.
    fn interrupt(&mut self) {
        self.tween = None;
        self.inertia = None;
    }

    /// Fingers down: whatever moved stops, and a new velocity is measured.
    fn begin_pan(&mut self) {
        self.interrupt();
        self.velocity.clear();
    }

    /// Move the content by `delta` points as a pan gesture reports it.
    fn pan(&mut self, mapping: &Mapping, delta: Vec2, now: f64) {
        self.view = mapping.panned(delta);
        self.velocity.add(now, delta);
    }

    /// Fingers lifted: coast if they were still moving.
    fn release_pan(&mut self, now: f64) {
        self.inertia = Inertia::flick(self.velocity.velocity(now), now);
        self.velocity.clear();
    }

    /// The box being drawn, from where the drag started to `to`, inside `rect`.
    fn drawn_box(&self, to: Option<Pos2>, rect: Rect) -> Option<Rect> {
        Some(Rect::from_two_pos(self.box_from?, to?).intersect(rect))
    }
}

/// The channel the plot draws and how it is labelled.
pub struct Series<'a> {
    pub trace: &'a Trace,
    pub label: &'a str,
    pub unit: &'a str,
    pub color: Color32,
}

/// What the plot reports back to the app this frame.
pub struct Shown {
    /// The display time under the pointer.
    pub hover_time: Option<f64>,
}

/// Draw `series` into the available space and handle its gestures and keys:
///
/// - pinch zooms time about the cursor, `Shift+pinch` value; `+`/`-` zoom
///   time by two about the cursor (with `Shift`, value), animated;
/// - two-finger scroll pans and, released moving, coasts; dragging in Pan
///   mode does the same; the arrow keys pan by a quarter view (not in
///   Align); a tap stops a coast;
/// - double-click and `A` fit the value axis to the visible window,
///   double-click on the legend and `Shift+A` to the whole signal, a box
///   drawn in Box mode becomes the view. All animate.
pub fn show(ui: &mut Ui, series: &Series, state: &mut PlotState) -> Shown {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return Shown { hover_time: None };
    }
    let trace = series.trace;
    let orientation = state.orientation;
    let mapping_of = |view: View| Mapping::new(rect, orientation, view);
    let now = ui.input(|i| i.time);
    state.advance(mapping_of, now);

    let colors = Colors::of(ui.visuals(), series.color);
    let legend = Legend::new(ui, &mapping_of(state.view), series, &colors);
    let legend_response = ui.interact(legend.rect, response.id.with("legend"), Sense::click());

    let hovered = response.hovered() || legend_response.hovered();
    let cursor = response.hover_pos();

    // A tap stops a coast.
    if hovered && ui.input(|i| i.pointer.any_pressed()) {
        state.inertia = None;
    }

    // Two-finger scroll and wheels. The OS's own momentum is ignored: the
    // plot coasts by itself, so a tap can stop it.
    if hovered {
        state.wheel.clear();
        ui.input(|i| {
            state.wheel.extend(i.events.iter().filter_map(|e| match e {
                Event::MouseWheel {
                    unit,
                    delta,
                    phase,
                    source,
                    modifiers,
                } if !modifiers.command && !modifiers.ctrl => Some(Wheel {
                    delta: *delta,
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
                    state.release_pan(now)
                }
                (_, MouseWheelUnit::Point, _) => {
                    state.interrupt();
                    state.pan(&mapping_of(state.view), delta, now);
                }
                (..) => {
                    // A wheel notch is a step: ease to it.
                    let step = match unit {
                        MouseWheelUnit::Page => delta * rect.size(),
                        _ => delta * line_px,
                    };
                    let target = mapping_of(state.target()).panned(step);
                    state.animate_to(target, now);
                }
            }
        }

        let (zoom, shift) = ui.input(|i| (i.zoom_delta(), i.modifiers.shift));
        if let Some(cursor) = cursor
            && zoom != 1.0
        {
            state.interrupt();
            let mapping = mapping_of(state.view);
            state.view = if shift {
                mapping.zoomed_value(zoom as f64, cursor)
            } else {
                mapping.zoomed_time(zoom as f64, cursor)
            };
        }
    }

    // Dragging: pan with inertia, or draw a box.
    match state.mode {
        Mode::Pan => {
            if response.drag_started() {
                state.begin_pan();
            }
            if response.dragged() {
                state.pan(&mapping_of(state.view), response.drag_delta(), now);
            }
            if response.drag_stopped() {
                state.release_pan(now);
            }
        }
        Mode::Box => {
            if response.drag_started() {
                state.box_from = ui.input(|i| i.pointer.press_origin());
            }
            if response.drag_stopped()
                && let (Some(from), Some(to)) =
                    (state.box_from.take(), response.interact_pointer_pos())
            {
                let drawn = Rect::from_two_pos(from, to).intersect(rect);
                if drawn.width() >= MIN_BOX_PX && drawn.height() >= MIN_BOX_PX {
                    let target = mapping_of(state.view).view_of(drawn);
                    state.animate_to(target, now);
                }
            }
        }
        Mode::Align => {}
    }

    // Autoscale: double-click or A to the window, legend or Shift+A to all.
    let (key_a, shift) = ui.input(|i| (i.key_pressed(Key::A), i.modifiers.shift));
    let whole = legend_response.double_clicked() || (key_a && shift);
    let visible = response.double_clicked() || (key_a && !shift);
    if whole || visible {
        let target = state.target();
        let range = if whole {
            trace.whole_signal()
        } else {
            trace.range(target.time)
        };
        if let Some((lo, hi)) = range {
            let value = Span::fitted(lo, hi);
            state.animate_to(View { value, ..target }, now);
        }
    }

    keys(ui, state, rect, cursor, now);

    if state.is_moving() {
        ui.ctx().request_repaint();
    }
    let mapping = mapping_of(state.view);

    // Candles are computed per physical pixel, not per point, so a Retina
    // screen gets one candle per device pixel.
    let pixels_per_point = ui.ctx().pixels_per_point();
    let px = (mapping.along_px() * pixels_per_point).round().max(1.0) as usize;
    let time_per_px = state.view.time.len() / px as f64;
    let sparse = trace.draws_samples(time_per_px);
    if sparse {
        trace.samples(state.view.time, &mut state.samples);
    } else {
        trace.candles(state.view.time, px, &mut state.candles);
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors.background);
    let grid = Grid::new(&mapping, series.unit);
    grid.draw_lines(&painter, &colors);
    if sparse {
        let dots =
            trace.samples_per_px(time_per_px * pixels_per_point as f64) <= 1.0 / DOT_SPACING_PX;
        draw_samples(
            &painter,
            &mapping,
            &state.samples,
            &mut state.points,
            Stroke::new(1.0, colors.line),
            dots,
        );
    } else {
        draw_candles(
            &painter,
            &mapping,
            &state.candles,
            pixels_per_point,
            colors.line,
        );
    }
    grid.draw_labels(&painter, &colors);
    legend.draw(&painter, &colors, legend_response.hovered());
    if state.mode == Mode::Box
        && let Some(drawn) = state.drawn_box(response.interact_pointer_pos(), rect)
    {
        painter.rect(
            drawn,
            2.0,
            colors.box_fill,
            Stroke::new(1.0, colors.line),
            StrokeKind::Inside,
        );
    }
    Shown {
        hover_time: cursor.map(|p| mapping.data_at(p).0),
    }
}

/// The plot's keys: `+`/`-` zoom time by two about the cursor (or the
/// centre), with `Shift` value; the arrows pan by a quarter view, except in
/// Align, which keeps them for nudging. Each is a step, so it eases.
fn keys(ui: &Ui, state: &mut PlotState, rect: Rect, cursor: Option<Pos2>, now: f64) {
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
    if zoom != 1.0 {
        let mapping = Mapping::new(rect, state.orientation, state.target());
        let about = cursor.unwrap_or(rect.center());
        let target = if value {
            mapping.zoomed_value(zoom, about)
        } else {
            mapping.zoomed_time(zoom, about)
        };
        state.animate_to(target, now);
    }
    if arrow != Vec2::ZERO && state.mode != Mode::Align {
        // The window moves the way the arrow points; the content the other.
        let mapping = Mapping::new(rect, state.orientation, state.target());
        let target = mapping.panned(-arrow * rect.size() * ARROW_PAN);
        state.animate_to(target, now);
    }
}

/// The legend: a chip in the corner no axis labels use (bottom right when
/// vertical, top right when horizontal) with the channel's colour and name.
/// Double-click fits the whole signal.
struct Legend {
    rect: Rect,
    galley: std::sync::Arc<egui::Galley>,
}

impl Legend {
    const SWATCH: f32 = 14.0;
    const PAD: Vec2 = vec2(8.0, 4.0);

    fn new(ui: &Ui, mapping: &Mapping, series: &Series, colors: &Colors) -> Legend {
        let galley = ui.painter().layout_no_wrap(
            series.label.to_string(),
            FontId::proportional(12.0),
            colors.text,
        );
        let size = vec2(Self::SWATCH + 6.0, 0.0) + galley.size() + 2.0 * Self::PAD;
        let plot = mapping.rect.shrink(8.0);
        let corner = match mapping.orientation {
            Orientation::Vertical => egui::Align2::RIGHT_BOTTOM,
            Orientation::Horizontal => egui::Align2::RIGHT_TOP,
        };
        let pos = match mapping.orientation {
            Orientation::Vertical => plot.right_bottom(),
            Orientation::Horizontal => plot.right_top(),
        };
        Legend {
            rect: corner.anchor_size(pos, size),
            galley,
        }
    }

    fn draw(&self, painter: &egui::Painter, colors: &Colors, hovered: bool) {
        let fill = if hovered {
            colors.pill_hover
        } else {
            colors.pill
        };
        painter.rect_filled(self.rect, 6.0, fill);
        let y = self.rect.center().y;
        let x = self.rect.min.x + Self::PAD.x;
        painter.hline(x..=x + Self::SWATCH, y, Stroke::new(2.0, colors.line));
        let text_pos = pos2(x + Self::SWATCH + 6.0, y - self.galley.size().y / 2.0);
        painter.galley(text_pos, self.galley.clone(), colors.text);
    }
}

struct Colors {
    background: Color32,
    grid: Color32,
    text: Color32,
    /// Behind grid labels and the legend, so they read over a dense trace.
    pill: Color32,
    pill_hover: Color32,
    line: Color32,
    box_fill: Color32,
}

impl Colors {
    fn of(visuals: &egui::Visuals, line: Color32) -> Colors {
        let box_fill = line.gamma_multiply(0.12);
        if visuals.dark_mode {
            Colors {
                background: Color32::from_gray(22),
                grid: Color32::from_gray(44),
                text: Color32::from_gray(150),
                pill: Color32::from_rgba_unmultiplied(22, 22, 22, 190),
                pill_hover: Color32::from_rgba_unmultiplied(44, 44, 44, 230),
                line,
                box_fill,
            }
        } else {
            Colors {
                background: Color32::WHITE,
                grid: Color32::from_gray(230),
                text: Color32::from_gray(110),
                pill: Color32::from_rgba_unmultiplied(255, 255, 255, 190),
                pill_hover: Color32::from_rgba_unmultiplied(236, 236, 236, 235),
                line,
                box_fill,
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
}

impl Grid {
    fn new(mapping: &Mapping, unit: &str) -> Grid {
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
            label_with_pill(painter, pos, anchor, label, &font, colors);
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
            label_with_pill(painter, pos, anchor, label, &font, colors);
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
) {
    let galley = painter.layout_no_wrap(text.to_string(), font.clone(), colors.text);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.rect_filled(rect.expand2(Vec2::new(3.0, 1.0)), 3.0, colors.pill);
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
