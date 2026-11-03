//! Motion: what makes the plot feel like an iPhone. A flick keeps panning
//! with exponentially decaying speed until it comes to rest; a jump of the
//! view (a zoom step, an autoscale, a box) eases to its target over about
//! 150 ms. Both are pure functions of time, so a frame drawn late lands where
//! it should and nothing accumulates rounding.

use crate::layout::Views;
use crate::view::View;
use egui::Vec2;

/// Time constant of a flick's decay: speed falls to 1/e every this many
/// seconds, close to iOS's normal scroll deceleration (0.998 per ms).
pub const DECELERATION_S: f64 = 0.5;
/// A flick slower than this, in points per second, is not a flick.
const MIN_FLICK_SPEED: f32 = 60.0;
/// A flick has come to rest below this speed, in points per second: what is
/// left to travel, `speed · DECELERATION_S`, is under two points.
const REST_SPEED: f64 = 4.0;
/// Velocity is measured over the last this many seconds of movement.
const VELOCITY_WINDOW_S: f64 = 0.1;
/// A finger that held still this long before lifting did not flick.
const VELOCITY_STALE_S: f64 = 0.05;
/// How long a view change animates.
pub const ANIMATION_S: f64 = 0.15;
/// How long the flash of an edge the view ran into takes to fade.
pub const FLASH_S: f64 = 0.6;

/// The speed of a pan gesture, from the movements it reported.
#[derive(Debug, Default)]
pub struct VelocityTracker {
    /// `(time, movement)` of the recent events, oldest first.
    recent: Vec<(f64, Vec2)>,
}

impl VelocityTracker {
    pub fn add(&mut self, now: f64, delta: Vec2) {
        self.recent.retain(|(t, _)| now - t <= VELOCITY_WINDOW_S);
        self.recent.push((now, delta));
    }

    pub fn clear(&mut self) {
        self.recent.clear();
    }

    /// Points per second over the last [`VELOCITY_WINDOW_S`]; zero when the
    /// movement stopped before `now` or was a single event.
    pub fn velocity(&self, now: f64) -> Vec2 {
        let recent: Vec<&(f64, Vec2)> = self
            .recent
            .iter()
            .filter(|(t, _)| now - t <= VELOCITY_WINDOW_S)
            .collect();
        let (Some(first), Some(last)) = (recent.first(), recent.last()) else {
            return Vec2::ZERO;
        };
        let span = last.0 - first.0;
        if now - last.0 > VELOCITY_STALE_S || span <= 0.0 {
            return Vec2::ZERO;
        }
        // The first event's movement happened before the window began.
        let moved = recent[1..].iter().fold(Vec2::ZERO, |sum, (_, d)| sum + *d);
        moved / span as f32
    }
}

/// A pan coasting after a flick: velocity `v · e^(−t/τ)`, so it has
/// travelled `v · τ · (1 − e^(−t/τ))` after `t` seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Inertia {
    velocity: Vec2,
    started: f64,
    travelled: Vec2,
}

impl Inertia {
    /// Coast from `velocity` (points per second); `None` when it is too slow
    /// to be a flick.
    pub fn flick(velocity: Vec2, now: f64) -> Option<Inertia> {
        (velocity.length() >= MIN_FLICK_SPEED).then_some(Inertia {
            velocity,
            started: now,
            travelled: Vec2::ZERO,
        })
    }

    /// The movement since the previous step, in points.
    pub fn step(&mut self, now: f64) -> Vec2 {
        let t = (now - self.started).max(0.0);
        let travelled =
            self.velocity * (DECELERATION_S * (1.0 - (-t / DECELERATION_S).exp())) as f32;
        let delta = travelled - self.travelled;
        self.travelled = travelled;
        delta
    }

    /// True once the speed has decayed below what the eye can follow.
    pub fn is_done(&self, now: f64) -> bool {
        let decay = (-(now - self.started).max(0.0) / DECELERATION_S).exp();
        self.velocity.length() as f64 * decay < REST_SPEED
    }
}

/// How strongly an edge still flashes `age_s` seconds after the view ran
/// into it: fully at once, fading out over [`FLASH_S`], gone after.
pub fn flash(age_s: f64) -> f32 {
    let u = (age_s / FLASH_S).clamp(0.0, 1.0);
    ((1.0 - u) * (1.0 - u)) as f32
}

/// What a [`Tween`] can ease: anything with a straight line between two
/// values of it.
pub trait Ease: Clone {
    fn lerp(a: &Self, b: &Self, u: f64) -> Self;
}

impl Ease for Views {
    fn lerp(a: &Views, b: &Views, u: f64) -> Views {
        Views::lerp(a, b, u)
    }
}

impl Ease for View {
    fn lerp(a: &View, b: &View, u: f64) -> View {
        View::lerp(*a, *b, u)
    }
}

/// A view easing from where it was to a target, interruptible: whoever
/// interrupts takes [`Tween::at`] as the current view and drops the tween.
#[derive(Debug, Clone, PartialEq)]
pub struct Tween<T> {
    from: T,
    to: T,
    started: f64,
}

impl<T: Ease> Tween<T> {
    pub fn new(from: T, to: T, now: f64) -> Tween<T> {
        Tween {
            from,
            to,
            started: now,
        }
    }

    pub fn target(&self) -> &T {
        &self.to
    }

    /// The view at `now`: ease-out cubic from `from` to `to`, exactly `to`
    /// from [`ANIMATION_S`] on.
    pub fn at(&self, now: f64) -> T {
        if self.is_done(now) {
            return self.to.clone();
        }
        let u = ((now - self.started) / ANIMATION_S).max(0.0);
        T::lerp(&self.from, &self.to, 1.0 - (1.0 - u).powi(3))
    }

    pub fn is_done(&self, now: f64) -> bool {
        now >= self.started + ANIMATION_S
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::Span;
    use egui::vec2;

    #[test]
    fn velocity_is_the_recent_movement_per_second_and_zero_once_the_finger_rests() {
        let mut tracker = VelocityTracker::default();
        for frame in 0..30 {
            tracker.add(frame as f64 / 60.0, vec2(10.0, -5.0));
        }
        let now = 29.0 / 60.0;
        let v = tracker.velocity(now);
        assert!((v - vec2(600.0, -300.0)).length() < 1e-2, "{v:?}");
        // Held still for 60 ms before lifting: no flick.
        assert_eq!(tracker.velocity(now + 0.06), Vec2::ZERO);
        tracker.clear();
        assert_eq!(tracker.velocity(now), Vec2::ZERO);
    }

    #[test]
    fn a_flick_coasts_in_its_direction_slowing_down_every_frame_and_comes_to_rest() {
        let mut inertia = Inertia::flick(vec2(1200.0, 0.0), 10.0).unwrap();
        let (mut now, mut total, mut last) = (10.0, Vec2::ZERO, f32::INFINITY);
        while !inertia.is_done(now) {
            now += 1.0 / 60.0;
            let step = inertia.step(now);
            assert!(step.y == 0.0 && step.x > 0.0, "{step:?}");
            assert!(step.x < last, "sped up: {} after {last}", step.x);
            last = step.x;
            total += step;
            assert!(now < 15.0, "never came to rest");
        }
        // It travels v · τ, less the couple of points left at rest.
        assert!(
            (600.0 - total.x) > 0.0 && (600.0 - total.x) < 2.0,
            "{total:?}"
        );
        assert!(last < 0.1, "last frame moved {last} pt");
    }

    #[test]
    fn a_flick_is_where_its_clock_says_however_unevenly_the_frames_come() {
        let mut even = Inertia::flick(vec2(0.0, -900.0), 0.0).unwrap();
        let mut uneven = even;
        let mut a = Vec2::ZERO;
        for frame in 1..=30 {
            a += even.step(frame as f64 / 60.0);
        }
        let b = uneven.step(0.2) + uneven.step(0.21) + uneven.step(0.5);
        assert!((a - b).length() < 1e-3, "{a:?} vs {b:?}");
    }

    #[test]
    fn a_slow_release_is_not_a_flick() {
        assert!(Inertia::flick(vec2(30.0, 20.0), 0.0).is_none());
        assert!(Inertia::flick(Vec2::ZERO, 0.0).is_none());
    }

    fn view(t: (f64, f64), v: (f64, f64)) -> View {
        View {
            time: Span::new(t.0, t.1),
            value: Span::new(v.0, v.1),
        }
    }

    #[test]
    fn an_edge_flashes_at_full_strength_then_fades_out_for_good() {
        assert_eq!(flash(0.0), 1.0);
        assert_eq!(flash(FLASH_S), 0.0);
        assert_eq!(flash(10.0), 0.0);
        let mut previous = 1.0;
        for ms in 1..=600 {
            let now = flash(ms as f64 / 1000.0);
            assert!(now <= previous && now >= 0.0, "{ms} ms: {now}");
            previous = now;
        }
        // Most of the red is gone by half time, so it reads as a flash.
        assert!(flash(FLASH_S / 2.0) < 0.5);
    }

    #[test]
    fn a_tween_eases_out_to_its_target_in_about_150_ms() {
        let from = view((0.0, 10.0), (-1.0, 1.0));
        let to = view((2.0, 4.0), (0.0, 5.0));
        let tween = Tween::new(from, to, 1.0);
        assert_eq!(tween.at(1.0), from);
        assert_eq!(tween.at(1.0 + ANIMATION_S), to);
        assert_eq!(tween.at(9.0), to);
        assert!(!tween.is_done(1.1) && tween.is_done(1.0 + ANIMATION_S));
        // Ease-out: more than half way at half time, and never overshooting.
        let half = tween.at(1.0 + ANIMATION_S / 2.0);
        assert!(half.time.from > 1.0 && half.time.from < 2.0, "{half:?}");
        let mut previous = from.value.to;
        for ms in 1..=150 {
            let now = tween.at(1.0 + ms as f64 / 1000.0).value.to;
            assert!(now >= previous && now <= 5.0, "{ms} ms: {now}");
            previous = now;
        }
    }
}
