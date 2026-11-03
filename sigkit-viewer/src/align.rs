//! Align: the active stream dragged directly on the plot. Along time a drag
//! shifts it and `Shift+drag` stretches it (changing `k`) about the drag
//! anchor; across value a drag offsets the channel and `Shift+drag` scales it
//! (changing `gain`) about the anchor. A stretch is sized so the stream's far
//! end follows the pointer: zoomed out it is a rubber band, zoomed in a fine
//! adjustment. Every change is one step of the session's undo history.

/// The least a stretch may scale by, so `k` and `gain` never reach zero or
/// flip when the pointer crosses the anchor.
const MIN_FACTOR: f64 = 1e-3;

/// A stream's time correction: display time is `k · t + shift_s`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Correction {
    pub k: f64,
    pub shift_s: f64,
}

impl Correction {
    /// Every display instant `by` seconds later.
    pub fn shifted(self, by: f64) -> Correction {
        Correction {
            shift_s: self.shift_s + by,
            ..self
        }
    }

    /// Stretched by `factor` about display instant `anchor`, which stays put.
    pub fn stretched(self, anchor: f64, factor: f64) -> Correction {
        Correction {
            k: self.k * factor,
            shift_s: anchor + factor * (self.shift_s - anchor),
        }
    }
}

/// A channel's calibration: display value is `gain · raw + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Calibration {
    pub gain: f64,
    pub offset: f64,
}

impl Calibration {
    /// Every display value `by` higher.
    pub fn offset_by(self, by: f64) -> Calibration {
        Calibration {
            offset: self.offset + by,
            ..self
        }
    }

    /// Scaled by `factor` about display value `anchor`, which stays put.
    pub fn stretched(self, anchor: f64, factor: f64) -> Calibration {
        Calibration {
            gain: self.gain * factor,
            offset: anchor + factor * (self.offset - anchor),
        }
    }
}

/// The factor that stretches about `anchor` so that `far` moves by `moved`,
/// in display units; never below [`MIN_FACTOR`]. `None` when `far` is the
/// anchor itself, so there is nothing to pull.
pub fn stretch_factor(anchor: f64, far: f64, moved: f64) -> Option<f64> {
    ((far - anchor).abs() > 1e-12).then(|| (1.0 + moved / (far - anchor)).max(MIN_FACTOR))
}

/// What one change sets, from what to what.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// A stream's time correction.
    Time {
        stream: String,
        from: Correction,
        to: Correction,
    },
    /// A channel's calibration.
    Calibration {
        stream: String,
        channel: String,
        from: Calibration,
        to: Calibration,
    },
}

/// One change to one open recording's header.
#[derive(Debug, Clone, PartialEq)]
pub struct Edit {
    /// The recording's index among the open ones.
    pub recording: usize,
    pub change: Change,
}

impl Edit {
    /// The edit that takes this one back.
    pub fn reversed(&self) -> Edit {
        let change = match &self.change {
            Change::Time { stream, from, to } => Change::Time {
                stream: stream.clone(),
                from: *to,
                to: *from,
            },
            Change::Calibration {
                stream,
                channel,
                from,
                to,
            } => Change::Calibration {
                stream: stream.clone(),
                channel: channel.clone(),
                from: *to,
                to: *from,
            },
        };
        Edit {
            recording: self.recording,
            change,
        }
    }

    /// True when it sets what was already there.
    pub fn changes_nothing(&self) -> bool {
        match &self.change {
            Change::Time { from, to, .. } => from == to,
            Change::Calibration { from, to, .. } => from == to,
        }
    }
}

/// The session's header edits, for `Cmd+Z`.
#[derive(Debug, Default)]
pub struct History {
    done: Vec<Edit>,
}

impl History {
    /// Remember a change made; one that changed nothing is not a step.
    pub fn push(&mut self, edit: Edit) {
        if !edit.changes_nothing() {
            self.done.push(edit);
        }
    }

    /// The edit that takes back the latest change, now forgotten; `None`
    /// when there is nothing left to undo.
    pub fn undo(&mut self) -> Option<Edit> {
        self.done.pop().map(|edit| edit.reversed())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stretch_keeps_the_anchor_where_it_is_and_brings_the_far_end_along_with_the_drag() {
        let start = Correction {
            k: 1.0,
            shift_s: 0.0,
        };
        // Anchor at display 2 s, the stream ends at 6 s, the drag moved 2 s.
        let f = stretch_factor(2.0, 6.0, 2.0).unwrap();
        assert_eq!(f, 1.5);
        let stretched = start.stretched(2.0, f);
        assert_eq!(
            stretched,
            Correction {
                k: 1.5,
                shift_s: -1.0
            }
        );
        // Display time is `k · u + shift` for `u` seconds from the zero.
        let display = |c: Correction, u: f64| c.k * u + c.shift_s;
        assert_eq!(display(stretched, 2.0), 2.0);
        assert_eq!(display(stretched, 6.0), 8.0);
    }

    #[test]
    fn a_gain_stretch_keeps_the_anchor_value_and_an_offset_moves_every_value() {
        let start = Calibration {
            gain: 2.0,
            offset: 1.0,
        };
        let value = |c: Calibration, raw: f64| c.gain * raw + c.offset;
        // Raw 1 shows at 3: the anchor. Raw 5 shows at 11: the far extreme,
        // dragged from 11 to 7, so the signal halves about 3.
        let f = stretch_factor(3.0, 11.0, -4.0).unwrap();
        let scaled = start.stretched(3.0, f);
        assert_eq!(
            scaled,
            Calibration {
                gain: 1.0,
                offset: 2.0
            }
        );
        assert_eq!(value(scaled, 1.0), 3.0);
        assert_eq!(value(scaled, 5.0), 7.0);
        assert_eq!(
            start.offset_by(-0.5),
            Calibration {
                gain: 2.0,
                offset: 0.5
            }
        );
    }

    #[test]
    fn a_drag_past_the_anchor_never_collapses_or_flips_the_stream() {
        let f = stretch_factor(2.0, 6.0, -10.0).unwrap();
        assert!(f > 0.0 && f <= 1e-3, "{f}");
        assert_eq!(stretch_factor(2.0, 2.0, 1.0), None);
        let c = Correction {
            k: 1.0,
            shift_s: 0.0,
        }
        .stretched(2.0, f);
        assert!(c.k > 0.0);
    }

    fn time(from: f64, to: f64) -> Edit {
        Edit {
            recording: 1,
            change: Change::Time {
                stream: "s".into(),
                from: Correction {
                    k: 1.0,
                    shift_s: from,
                },
                to: Correction {
                    k: 1.0,
                    shift_s: to,
                },
            },
        }
    }

    #[test]
    fn undo_steps_back_through_the_changes_newest_first_and_skips_changes_that_changed_nothing() {
        let mut history = History::default();
        history.push(time(0.0, 0.25));
        history.push(time(0.25, 0.25));
        let gain = Edit {
            recording: 0,
            change: Change::Calibration {
                stream: "s".into(),
                channel: "v".into(),
                from: Calibration {
                    gain: 1.0,
                    offset: 0.0,
                },
                to: Calibration {
                    gain: 2.0,
                    offset: 0.0,
                },
            },
        };
        history.push(gain.clone());
        history.push(time(0.25, 0.5));

        assert_eq!(history.undo(), Some(time(0.5, 0.25)));
        let Some(Edit {
            recording: 0,
            change: Change::Calibration { from, to, .. },
        }) = history.undo()
        else {
            panic!("the gain change comes second");
        };
        assert_eq!((from.gain, to.gain), (2.0, 1.0));
        assert_eq!(history.undo(), Some(time(0.25, 0.0)));
        assert_eq!(history.undo(), None);
    }
}
