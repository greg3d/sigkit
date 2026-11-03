//! The widget's coordinates: along time and across value. Orientation maps
//! them to the screen; every gesture is defined here, in the widget's own
//! terms, so switching orientation reassigns nothing.

use egui::{Pos2, Rect, Vec2, pos2};

/// Shortest time span the view can zoom into, in seconds.
const MIN_TIME_SPAN_S: f64 = 1e-6;
/// Longest time span the view can zoom out to, in seconds (about 30 years).
const MAX_TIME_SPAN_S: f64 = 1e9;

/// Vertical: time runs top to bottom, value left to right (the default).
/// Horizontal: time runs left to right, value bottom to top.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orientation {
    Vertical,
    Horizontal,
}

impl Orientation {
    pub fn toggled(self) -> Orientation {
        match self {
            Orientation::Vertical => Orientation::Horizontal,
            Orientation::Horizontal => Orientation::Vertical,
        }
    }
}

/// A visible range on one axis, `from < to`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub from: f64,
    pub to: f64,
}

impl Span {
    pub fn new(from: f64, to: f64) -> Span {
        Span { from, to }
    }

    pub fn len(&self) -> f64 {
        self.to - self.from
    }

    /// A fraction `u` of the way from `a` to `b`, each end in a straight line.
    pub fn lerp(a: Span, b: Span, u: f64) -> Span {
        Span::new(a.from + (b.from - a.from) * u, a.to + (b.to - a.to) * u)
    }

    pub fn shifted(&self, by: f64) -> Span {
        Span::new(self.from + by, self.to + by)
    }

    /// The span that shows `min..=max` with a small margin on both sides. A
    /// flat signal still gets a visible span.
    pub fn fitted(min: f64, max: f64) -> Span {
        let len = max - min;
        let pad = if len > 0.0 {
            len * 0.025
        } else if max != 0.0 {
            max.abs() * 0.05
        } else {
            1.0
        };
        Span::new(min - pad, max + pad)
    }
}

/// What the widget shows: a time span along, a value span across.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub time: Span,
    pub value: Span,
}

impl View {
    /// The view a fraction `u` of the way from `a` to `b`, each end of each
    /// span moving in a straight line. A zoom about a point keeps that point
    /// fixed all the way.
    pub fn lerp(a: View, b: View, u: f64) -> View {
        View {
            time: Span::lerp(a.time, b.time, u),
            value: Span::lerp(a.value, b.value, u),
        }
    }
}

/// A view laid onto a screen rectangle in one orientation: the only place
/// that knows which screen axis is which.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mapping {
    pub rect: Rect,
    pub orientation: Orientation,
    pub view: View,
}

impl Mapping {
    pub fn new(rect: Rect, orientation: Orientation, view: View) -> Mapping {
        Mapping {
            rect,
            orientation,
            view,
        }
    }

    /// Pixels along the time axis.
    pub fn along_px(&self) -> f32 {
        match self.orientation {
            Orientation::Vertical => self.rect.height(),
            Orientation::Horizontal => self.rect.width(),
        }
    }

    /// Pixels across the value axis.
    pub fn across_px(&self) -> f32 {
        match self.orientation {
            Orientation::Vertical => self.rect.width(),
            Orientation::Horizontal => self.rect.height(),
        }
    }

    pub fn time_per_px(&self) -> f64 {
        self.view.time.len() / self.along_px() as f64
    }

    pub fn value_per_px(&self) -> f64 {
        self.view.value.len() / self.across_px() as f64
    }

    /// The screen coordinate along time of instant `t`: y when vertical, x
    /// when horizontal. Time grows down and to the right.
    pub fn time_to_along(&self, t: f64) -> f32 {
        let start = match self.orientation {
            Orientation::Vertical => self.rect.min.y,
            Orientation::Horizontal => self.rect.min.x,
        };
        start + ((t - self.view.time.from) / self.time_per_px()) as f32
    }

    /// The screen coordinate across value of `v`: x growing right when
    /// vertical, y growing up when horizontal.
    pub fn value_to_across(&self, v: f64) -> f32 {
        let offset = ((v - self.view.value.from) / self.value_per_px()) as f32;
        match self.orientation {
            Orientation::Vertical => self.rect.min.x + offset,
            Orientation::Horizontal => self.rect.max.y - offset,
        }
    }

    /// Where `(t, v)` falls on screen.
    pub fn screen_pos(self, t: f64, v: f64) -> Pos2 {
        match self.orientation {
            Orientation::Vertical => pos2(self.value_to_across(v), self.time_to_along(t)),
            Orientation::Horizontal => pos2(self.time_to_along(t), self.value_to_across(v)),
        }
    }

    pub fn along_to_time(&self, along: f32) -> f64 {
        let start = match self.orientation {
            Orientation::Vertical => self.rect.min.y,
            Orientation::Horizontal => self.rect.min.x,
        };
        self.view.time.from + (along - start) as f64 * self.time_per_px()
    }

    pub fn across_to_value(&self, across: f32) -> f64 {
        let offset = match self.orientation {
            Orientation::Vertical => across - self.rect.min.x,
            Orientation::Horizontal => self.rect.max.y - across,
        };
        self.view.value.from + offset as f64 * self.value_per_px()
    }

    /// `(time, value)` under a screen position.
    pub fn data_at(&self, p: Pos2) -> (f64, f64) {
        match self.orientation {
            Orientation::Vertical => (self.along_to_time(p.y), self.across_to_value(p.x)),
            Orientation::Horizontal => (self.along_to_time(p.x), self.across_to_value(p.y)),
        }
    }

    /// The view after the content moved by `delta` screen pixels: what a
    /// two-finger scroll reports, content following the fingers.
    pub fn panned(&self, delta: Vec2) -> View {
        let (d_along, d_across) = match self.orientation {
            Orientation::Vertical => (delta.y, delta.x),
            Orientation::Horizontal => (delta.x, delta.y),
        };
        let dt = -(d_along as f64) * self.time_per_px();
        let dv = match self.orientation {
            Orientation::Vertical => -(d_across as f64) * self.value_per_px(),
            Orientation::Horizontal => d_across as f64 * self.value_per_px(),
        };
        View {
            time: self.view.time.shifted(dt),
            value: self.view.value.shifted(dv),
        }
    }

    /// The view after zooming time by `factor` (above 1 zooms in) about the
    /// instant under `cursor`, which stays where it is on screen.
    pub fn zoomed_time(&self, factor: f64, cursor: Pos2) -> View {
        let (t_cursor, _) = self.data_at(cursor);
        let len = self.view.time.len();
        let frac = (t_cursor - self.view.time.from) / len;
        let new_len = (len / factor).clamp(MIN_TIME_SPAN_S, MAX_TIME_SPAN_S);
        let from = t_cursor - frac * new_len;
        View {
            time: Span::new(from, from + new_len),
            value: self.view.value,
        }
    }

    /// The view after zooming value by `factor` (above 1 zooms in) about the
    /// value under `cursor`, which stays where it is on screen.
    pub fn zoomed_value(&self, factor: f64, cursor: Pos2) -> View {
        let (_, v_cursor) = self.data_at(cursor);
        let len = self.view.value.len();
        let frac = (v_cursor - self.view.value.from) / len;
        let new_len = len / factor;
        let from = v_cursor - frac * new_len;
        View {
            time: self.view.time,
            value: Span::new(from, from + new_len),
        }
    }

    /// The view that shows exactly what `rect` covers on screen: what a box
    /// drawn there fits to.
    pub fn view_of(&self, rect: Rect) -> View {
        let (t0, v0) = self.data_at(rect.min);
        let (t1, v1) = self.data_at(rect.max);
        View {
            time: Span::new(t0.min(t1), t0.max(t1)),
            value: Span::new(v0.min(v1), v0.max(v1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Rect, pos2, vec2};

    fn rect() -> Rect {
        Rect::from_min_max(pos2(100.0, 50.0), pos2(300.0, 150.0))
    }

    fn view() -> View {
        View {
            time: Span::new(0.0, 10.0),
            value: Span::new(-1.0, 1.0),
        }
    }

    #[test]
    fn vertical_runs_time_top_to_bottom_and_value_left_to_right() {
        let m = Mapping::new(rect(), Orientation::Vertical, view());
        assert_eq!(m.screen_pos(0.0, -1.0), pos2(100.0, 50.0));
        assert_eq!(m.screen_pos(10.0, 1.0), pos2(300.0, 150.0));
        assert_eq!(m.screen_pos(5.0, 0.0), pos2(200.0, 100.0));
        assert_eq!(m.along_px(), 100.0);
        assert_eq!(m.time_per_px(), 0.1);
    }

    #[test]
    fn horizontal_runs_time_left_to_right_and_value_bottom_to_top() {
        let m = Mapping::new(rect(), Orientation::Horizontal, view());
        assert_eq!(m.screen_pos(0.0, -1.0), pos2(100.0, 150.0));
        assert_eq!(m.screen_pos(10.0, 1.0), pos2(300.0, 50.0));
        assert_eq!(m.along_px(), 200.0);
        assert_eq!(m.time_per_px(), 0.05);
    }

    #[test]
    fn screen_to_data_inverts_data_to_screen_in_both_orientations() {
        for orientation in [Orientation::Vertical, Orientation::Horizontal] {
            let m = Mapping::new(rect(), orientation, view());
            let (t, v) = m.data_at(m.screen_pos(3.3, 0.25));
            assert!(
                (t - 3.3).abs() < 1e-5 && (v - 0.25).abs() < 1e-6,
                "{orientation:?}"
            );
        }
    }

    #[test]
    fn panning_moves_the_content_with_the_fingers_in_both_orientations() {
        // Vertical: time is y. Content moved down 10 px (1 s) and right 20 px (0.2).
        let m = Mapping::new(rect(), Orientation::Vertical, view());
        let v = m.panned(vec2(20.0, 10.0));
        assert_eq!((v.time.from, v.time.to), (-1.0, 9.0));
        assert!((v.value.from + 1.2).abs() < 1e-9 && (v.value.to - 0.8).abs() < 1e-9);

        // Horizontal: time is x; content moved right 20 px (1 s), down 10 px:
        // value grows upward, so the visible values move up by 0.2.
        let m = Mapping::new(rect(), Orientation::Horizontal, view());
        let v = m.panned(vec2(20.0, 10.0));
        assert_eq!((v.time.from, v.time.to), (-1.0, 9.0));
        assert!((v.value.from - (-0.8)).abs() < 1e-9 && (v.value.to - 1.2).abs() < 1e-9);
    }

    #[test]
    fn zooming_time_keeps_the_instant_under_the_cursor_where_it_is() {
        for orientation in [Orientation::Vertical, Orientation::Horizontal] {
            let m = Mapping::new(rect(), orientation, view());
            let cursor = m.screen_pos(2.0, 0.0);
            let zoomed = m.zoomed_time(2.0, cursor);
            assert_eq!(zoomed.time.to - zoomed.time.from, 5.0, "{orientation:?}");
            let after = Mapping::new(rect(), orientation, zoomed);
            let (t, _) = after.data_at(cursor);
            assert!((t - 2.0).abs() < 1e-9, "{orientation:?}: {t}");
            assert_eq!((zoomed.time.from, zoomed.time.to), (1.0, 6.0));
        }
    }

    #[test]
    fn zooming_out_past_the_signal_is_allowed_but_the_span_never_collapses() {
        let m = Mapping::new(rect(), Orientation::Vertical, view());
        let z = m.zoomed_time(1e12, m.screen_pos(5.0, 0.0));
        assert!(z.time.to > z.time.from);
        let z = m.zoomed_time(0.1, m.screen_pos(5.0, 0.0));
        assert_eq!((z.time.from, z.time.to), (-45.0, 55.0));
    }

    #[test]
    fn zooming_value_keeps_the_value_under_the_cursor_where_it_is() {
        for orientation in [Orientation::Vertical, Orientation::Horizontal] {
            let m = Mapping::new(rect(), orientation, view());
            let cursor = m.screen_pos(7.0, 0.5);
            let zoomed = m.zoomed_value(4.0, cursor);
            assert_eq!(zoomed.time, view().time, "{orientation:?}");
            assert!((zoomed.value.len() - 0.5).abs() < 1e-12, "{orientation:?}");
            let (_, v) = Mapping::new(rect(), orientation, zoomed).data_at(cursor);
            assert!((v - 0.5).abs() < 1e-6, "{orientation:?}: {v}");
        }
    }

    #[test]
    fn a_box_drawn_on_screen_in_any_direction_becomes_the_view() {
        for orientation in [Orientation::Vertical, Orientation::Horizontal] {
            let m = Mapping::new(rect(), orientation, view());
            let (a, b) = (m.screen_pos(2.0, -0.5), m.screen_pos(6.0, 0.25));
            for (from, to) in [(a, b), (b, a)] {
                let boxed = m.view_of(Rect::from_two_pos(from, to));
                assert!(
                    (boxed.time.from - 2.0).abs() < 1e-5 && (boxed.time.to - 6.0).abs() < 1e-5,
                    "{orientation:?}: {boxed:?}"
                );
                assert!(
                    (boxed.value.from + 0.5).abs() < 1e-6 && (boxed.value.to - 0.25).abs() < 1e-6,
                    "{orientation:?}: {boxed:?}"
                );
            }
        }
    }

    #[test]
    fn a_view_between_two_others_interpolates_both_axes_end_by_end() {
        let a = view();
        let b = View {
            time: Span::new(4.0, 6.0),
            value: Span::new(0.0, 3.0),
        };
        assert_eq!(View::lerp(a, b, 0.0), a);
        assert_eq!(View::lerp(a, b, 1.0), b);
        let mid = View::lerp(a, b, 0.5);
        assert_eq!((mid.time.from, mid.time.to), (2.0, 8.0));
        assert_eq!((mid.value.from, mid.value.to), (-0.5, 2.0));
    }

    #[test]
    fn toggling_orientation_leaves_the_view_untouched() {
        assert_eq!(Orientation::Vertical.toggled(), Orientation::Horizontal);
        assert_eq!(Orientation::Horizontal.toggled(), Orientation::Vertical);
        let v = view();
        let a = Mapping::new(rect(), Orientation::Vertical, v).view;
        let b = Mapping::new(rect(), Orientation::Horizontal, v).view;
        assert_eq!(a, b);
    }

    #[test]
    fn a_span_fitted_to_a_range_adds_a_small_margin_and_never_degenerates() {
        let s = Span::fitted(0.0, 10.0);
        assert!(s.from < 0.0 && s.to > 10.0 && s.to - s.from < 11.0);
        let s = Span::fitted(3.0, 3.0);
        assert!(s.from < 3.0 && s.to > 3.0);
        let s = Span::fitted(0.0, 0.0);
        assert!(s.from < 0.0 && s.to > 0.0);
    }
}
