//! The plot widget: one trace drawn on the egui painter as min/max candles,
//! raw samples below the base bin, a 1-2-5 grid on both axes. It lives in
//! along-time / across-value coordinates; [`Mapping`] does the orientation.

use crate::ticks;
use crate::trace::Trace;
use crate::view::{Mapping, Orientation, Span, View};
use egui::{
    Color32, FontId, Key, Mesh, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2, pos2,
};
use sigkit_format::Bin;

/// Least pixels between two grid lines along time.
const MIN_PX_PER_TIME_TICK: f32 = 90.0;
/// Least pixels between two grid lines across value.
const MIN_PX_PER_VALUE_TICK: f32 = 60.0;
/// From this many pixels between raw samples on, each sample also gets a dot.
const DOT_SPACING_PX: f64 = 6.0;

/// What persists between frames: the view, the orientation, and the candle
/// and sample buffers, reused frame to frame. The shapes handed to egui are
/// owned by egui and rebuilt each frame.
pub struct PlotState {
    pub view: View,
    pub orientation: Orientation,
    candles: Vec<Bin>,
    samples: Vec<(f64, f64)>,
    points: Vec<Pos2>,
}

impl PlotState {
    pub fn new(view: View, orientation: Orientation) -> PlotState {
        PlotState {
            view,
            orientation,
            candles: Vec::new(),
            samples: Vec::new(),
            points: Vec::new(),
        }
    }
}

/// Draw `trace` into the available space, handling its gestures: pinch zooms
/// time about the cursor, two-finger scroll pans, `A` autoscales the value
/// axis to the visible window, `Shift+A` to the whole signal.
pub fn show(ui: &mut Ui, trace: &Trace, state: &mut PlotState, unit: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return response;
    }
    let mut mapping = Mapping::new(rect, state.orientation, state.view);

    if response.hovered() {
        let (zoom, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta));
        if let Some(cursor) = response.hover_pos()
            && zoom != 1.0
        {
            state.view = mapping.zoomed_time(zoom as f64, cursor);
            mapping = Mapping::new(rect, state.orientation, state.view);
        }
        if scroll != Vec2::ZERO {
            state.view = mapping.panned(scroll);
            mapping = Mapping::new(rect, state.orientation, state.view);
        }
    }

    // Candles are computed per physical pixel, not per point, so a Retina
    // screen gets one candle per device pixel.
    let pixels_per_point = ui.ctx().pixels_per_point();
    let px = (mapping.along_px() * pixels_per_point).round().max(1.0) as usize;
    let time_per_px = state.view.time.len() / px as f64;
    let sparse = trace.samples_per_px(time_per_px) < 1.0;
    if sparse {
        trace.samples(state.view.time, &mut state.samples);
    } else {
        trace.candles(state.view.time, px, &mut state.candles);
    }

    let (autoscale, shift) = ui.input(|i| (i.key_pressed(Key::A), i.modifiers.shift));
    if autoscale {
        let range = if shift {
            trace.whole_signal()
        } else {
            trace.range(state.view.time)
        };
        if let Some((lo, hi)) = range {
            state.view.value = Span::fitted(lo, hi);
            mapping = Mapping::new(rect, state.orientation, state.view);
        }
    }

    let colors = Colors::of(ui.visuals());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, colors.background);
    let grid = Grid::new(&mapping, unit);
    grid.draw_lines(&painter, &colors);
    let stroke = Stroke::new(1.0, colors.line);
    if sparse {
        let dots =
            (1.0 / trace.rate_hz()) / (time_per_px * pixels_per_point as f64) >= DOT_SPACING_PX;
        draw_samples(
            &painter,
            &mapping,
            &state.samples,
            &mut state.points,
            stroke,
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
    response
}

struct Colors {
    background: Color32,
    grid: Color32,
    text: Color32,
    /// Behind grid labels, so they read over a dense trace.
    pill: Color32,
    line: Color32,
}

impl Colors {
    fn of(visuals: &egui::Visuals) -> Colors {
        if visuals.dark_mode {
            Colors {
                background: Color32::from_gray(22),
                grid: Color32::from_gray(46),
                text: Color32::from_gray(150),
                pill: Color32::from_rgba_unmultiplied(22, 22, 22, 190),
                line: Color32::from_rgb(0x6c, 0xb4, 0xf5),
            }
        } else {
            Colors {
                background: Color32::WHITE,
                grid: Color32::from_gray(228),
                text: Color32::from_gray(110),
                pill: Color32::from_rgba_unmultiplied(255, 255, 255, 190),
                line: Color32::from_rgb(0x1f, 0x6f, 0xeb),
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
