//! The layout of the plot area: panels side by side along the stacking axis,
//! each holding the channels overlaid on it and its own value span. Laid out
//! by dragging: a channel dropped on a panel overlays it there, dropped on a
//! panel's edge makes a new panel, dragged back to the tree leaves its panel.
//! A panel with no channel left is gone.

use crate::view::{Span, View};

/// One channel of one open recording: a recording's index among the open
/// ones and the channel's place in it (a residual counts as a channel).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChannelRef {
    pub recording: usize,
    pub entry: usize,
}

/// A panel's identity, kept while panels come and go around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PanelId(u64);

/// One plotting area with one value axis.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub id: PanelId,
    pub channels: Vec<ChannelRef>,
    pub value: Span,
}

/// Where a dragged channel lands, by panel index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Drop {
    /// Onto this panel, overlaid on what is there.
    Overlay(usize),
    /// Into a new panel at this index.
    NewPanel(usize),
}

/// What is being dragged: a channel, from a panel's legend or from the tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dragged {
    pub channel: ChannelRef,
    pub from: Option<PanelId>,
}

#[derive(Debug, Default)]
pub struct Layout {
    panels: Vec<Panel>,
    next_id: u64,
}

impl Layout {
    pub fn panels(&self) -> &[Panel] {
        &self.panels
    }

    /// Drop `dragged` at `drop`; a new panel shows `value`. False, and
    /// nothing changes, when the drop would leave the layout as it was or put
    /// a channel twice on one panel.
    /// A channel dragged from a panel leaves it; dropped back on the tree, that
    /// is all that happens to it.
    pub fn drop(&mut self, dragged: Dragged, drop: Drop, value: Span) -> bool {
        let channel = dragged.channel;
        let source = dragged
            .from
            .and_then(|id| self.index_of(id))
            .filter(|&i| self.panels[i].channels.contains(&channel));
        match drop {
            Drop::Overlay(mut index) => {
                match self.panels.get(index) {
                    Some(panel) if !panel.channels.contains(&channel) => {}
                    _ => return false,
                }
                if let Some(s) = source
                    && self.take(s, channel)
                    && s < index
                {
                    index -= 1;
                }
                self.panels[index].channels.push(channel);
            }
            Drop::NewPanel(index) => {
                let mut index = index.min(self.panels.len());
                if let Some(s) = source {
                    let alone = self.panels[s].channels.len() == 1;
                    if alone && (index == s || index == s + 1) {
                        return false;
                    }
                    if self.take(s, channel) && s < index {
                        index -= 1;
                    }
                }
                self.insert(index, channel, value);
            }
        }
        true
    }

    /// Take a channel dragged from a panel off that panel: what dropping it
    /// on the tree does. False when it came from the tree or is not there.
    pub fn remove(&mut self, dragged: Dragged) -> bool {
        let Some(index) = dragged.from.and_then(|id| self.index_of(id)) else {
            return false;
        };
        if !self.panels[index].channels.contains(&dragged.channel) {
            return false;
        }
        self.take(index, dragged.channel);
        true
    }

    fn index_of(&self, id: PanelId) -> Option<usize> {
        self.panels.iter().position(|p| p.id == id)
    }

    /// Take `channel` off panel `index`; true when that emptied the panel and
    /// it is gone.
    fn take(&mut self, index: usize, channel: ChannelRef) -> bool {
        self.panels[index].channels.retain(|&c| c != channel);
        let emptied = self.panels[index].channels.is_empty();
        if emptied {
            self.panels.remove(index);
        }
        emptied
    }

    /// How many panels show `channel`.
    pub fn count(&self, channel: ChannelRef) -> usize {
        self.panels
            .iter()
            .filter(|p| p.channels.contains(&channel))
            .count()
    }

    fn insert(&mut self, index: usize, channel: ChannelRef, value: Span) {
        let id = PanelId(self.next_id);
        self.next_id += 1;
        let panel = Panel {
            id,
            channels: vec![channel],
            value,
        };
        self.panels.insert(index.min(self.panels.len()), panel);
    }
}

/// What every panel shows: one time span for all, a value span each. What a
/// jump of the view animates, so time and the values ease together.
#[derive(Debug, Clone, PartialEq)]
pub struct Views {
    pub time: Span,
    values: Vec<(PanelId, Span)>,
}

impl Views {
    pub fn of(time: Span, layout: &Layout) -> Views {
        Views {
            time,
            values: layout.panels.iter().map(|p| (p.id, p.value)).collect(),
        }
    }

    /// What panel `id` shows.
    pub fn panel(&self, id: PanelId) -> Option<View> {
        let &(_, value) = self.values.iter().find(|(i, _)| *i == id)?;
        Some(View {
            time: self.time,
            value,
        })
    }

    /// Panel `id` shows `view`: its time becomes every panel's.
    pub fn with_panel(&self, id: PanelId, view: View) -> Views {
        let values = self
            .values
            .iter()
            .map(|&(i, value)| (i, if i == id { view.value } else { value }))
            .collect();
        Views {
            time: view.time,
            values,
        }
    }

    /// A fraction `u` of the way from `a` to `b`, each end of each span
    /// moving in a straight line. A panel `a` does not know starts at `b`.
    pub fn lerp(a: &Views, b: &Views, u: f64) -> Views {
        let values = b
            .values
            .iter()
            .map(|&(id, to)| match a.values.iter().find(|(i, _)| *i == id) {
                Some(&(_, from)) => (id, Span::lerp(from, to, u)),
                None => (id, to),
            })
            .collect();
        Views {
            time: Span::lerp(a.time, b.time, u),
            values,
        }
    }

    /// Write the value spans into `layout`'s panels; false, and nothing
    /// written, when its panels are no longer the ones these views are of.
    pub fn apply_to(&self, layout: &mut Layout) -> bool {
        let same = layout.panels.len() == self.values.len()
            && layout
                .panels
                .iter()
                .zip(&self.values)
                .all(|(p, (id, _))| p.id == *id);
        if same {
            for (panel, &(_, value)) in layout.panels.iter_mut().zip(&self.values) {
                panel.value = value;
            }
        }
        same
    }
}

/// How close to a panel's edge, in points along the stacking axis, a drop
/// makes a new panel there instead of overlaying.
pub const EDGE_PX: f32 = 22.0;

/// Where a drop at `pos` along the stacking axis lands, given each panel's
/// `(start, end)` there in order. Within [`EDGE_PX`] of an edge (a quarter
/// of a narrow panel) it is a new panel at that edge; further in, an overlay.
/// Anywhere on an empty plot it is the first panel; outside every panel,
/// nothing.
pub fn drop_target(spans: &[(f32, f32)], pos: f32) -> Option<Drop> {
    if spans.is_empty() {
        return Some(Drop::NewPanel(0));
    }
    let index = spans.iter().position(|&(a, b)| pos >= a && pos < b)?;
    let (a, b) = spans[index];
    let edge = EDGE_PX.min((b - a) / 4.0);
    Some(if pos < a + edge {
        Drop::NewPanel(index)
    } else if pos >= b - edge {
        Drop::NewPanel(index + 1)
    } else {
        Drop::Overlay(index)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(recording: usize, entry: usize) -> ChannelRef {
        ChannelRef { recording, entry }
    }

    fn from_tree(channel: ChannelRef) -> Dragged {
        Dragged {
            channel,
            from: None,
        }
    }

    fn channels(layout: &Layout) -> Vec<Vec<ChannelRef>> {
        layout.panels().iter().map(|p| p.channels.clone()).collect()
    }

    const SPAN: Span = Span {
        from: -1.0,
        to: 1.0,
    };

    #[test]
    fn a_channel_dropped_on_an_empty_plot_gets_a_panel_of_its_own_with_the_given_value_span() {
        let mut layout = Layout::default();
        assert!(layout.drop(from_tree(ch(0, 2)), Drop::NewPanel(0), SPAN));
        assert_eq!(channels(&layout), vec![vec![ch(0, 2)]]);
        assert_eq!(layout.panels()[0].value, SPAN);
    }

    #[test]
    fn a_channel_dropped_on_a_panel_joins_its_value_axis_but_never_twice() {
        let mut layout = Layout::default();
        layout.drop(from_tree(ch(0, 0)), Drop::NewPanel(0), SPAN);
        let other = Span::new(5.0, 6.0);
        assert!(layout.drop(from_tree(ch(1, 0)), Drop::Overlay(0), other));
        assert_eq!(channels(&layout), vec![vec![ch(0, 0), ch(1, 0)]]);
        assert_eq!(layout.panels()[0].value, SPAN);
        assert!(!layout.drop(from_tree(ch(1, 0)), Drop::Overlay(0), other));
        assert_eq!(channels(&layout), vec![vec![ch(0, 0), ch(1, 0)]]);
    }

    #[test]
    fn the_same_channel_can_sit_alone_in_a_panel_of_its_own_next_to_an_overlay() {
        let mut layout = Layout::default();
        layout.drop(from_tree(ch(0, 0)), Drop::NewPanel(0), SPAN);
        layout.drop(from_tree(ch(1, 0)), Drop::Overlay(0), SPAN);
        let alone = Span::new(9.0, 10.0);
        assert!(layout.drop(from_tree(ch(0, 0)), Drop::NewPanel(1), alone));
        assert_eq!(
            channels(&layout),
            vec![vec![ch(0, 0), ch(1, 0)], vec![ch(0, 0)]]
        );
        assert_eq!(layout.panels()[1].value, alone);
        assert_ne!(layout.panels()[0].id, layout.panels()[1].id);
        assert_eq!(layout.count(ch(0, 0)), 2);
        assert_eq!(layout.count(ch(1, 0)), 1);
        assert_eq!(layout.count(ch(1, 1)), 0);
    }

    /// Three panels: `[a, b]`, `[c]`, `[d]`.
    fn three() -> (Layout, [ChannelRef; 4]) {
        let [a, b, c, d] = [ch(0, 0), ch(0, 1), ch(0, 2), ch(1, 0)];
        let mut layout = Layout::default();
        layout.drop(from_tree(a), Drop::NewPanel(0), SPAN);
        layout.drop(from_tree(b), Drop::Overlay(0), SPAN);
        layout.drop(from_tree(c), Drop::NewPanel(1), Span::new(2.0, 3.0));
        layout.drop(from_tree(d), Drop::NewPanel(2), Span::new(4.0, 5.0));
        (layout, [a, b, c, d])
    }

    fn moved(layout: &Layout, channel: ChannelRef, panel: usize) -> Dragged {
        Dragged {
            channel,
            from: Some(layout.panels()[panel].id),
        }
    }

    #[test]
    fn a_legend_dropped_on_another_panel_moves_its_channel_there_and_an_emptied_panel_is_gone() {
        let (mut layout, [a, b, c, d]) = three();
        let kept = layout.panels()[2].id;
        assert!(layout.drop(moved(&layout, b, 0), Drop::Overlay(1), SPAN));
        assert_eq!(channels(&layout), vec![vec![a], vec![c, b], vec![d]]);
        assert!(layout.drop(moved(&layout, c, 1), Drop::Overlay(2), SPAN));
        assert!(layout.drop(moved(&layout, b, 1), Drop::Overlay(0), SPAN));
        assert_eq!(channels(&layout), vec![vec![a, b], vec![d, c]]);
        // The panel that took `c` is the same panel, value span and all.
        assert_eq!(layout.panels()[1].id, kept);
        assert_eq!(layout.panels()[1].value, Span::new(4.0, 5.0));
    }

    #[test]
    fn a_legend_dropped_on_a_panel_edge_moves_its_channel_into_a_new_panel_there() {
        let (mut layout, [a, b, c, d]) = three();
        let fresh = Span::new(7.0, 8.0);
        assert!(layout.drop(moved(&layout, a, 0), Drop::NewPanel(3), fresh));
        assert_eq!(channels(&layout), vec![vec![b], vec![c], vec![d], vec![a]]);
        assert_eq!(layout.panels()[3].value, fresh);
        // Out of a panel that empties, to an edge past it.
        assert!(layout.drop(moved(&layout, c, 1), Drop::NewPanel(3), fresh));
        assert_eq!(channels(&layout), vec![vec![b], vec![d], vec![c], vec![a]]);
    }

    #[test]
    fn a_drop_that_would_change_nothing_changes_nothing() {
        let (mut layout, [a, _, c, _]) = three();
        let before = layout.panels().to_vec();
        // Onto its own panel, or a lone channel to either edge of its panel.
        assert!(!layout.drop(moved(&layout, a, 0), Drop::Overlay(0), SPAN));
        assert!(!layout.drop(moved(&layout, c, 1), Drop::NewPanel(1), SPAN));
        assert!(!layout.drop(moved(&layout, c, 1), Drop::NewPanel(2), SPAN));
        // Onto a panel already showing it.
        layout.drop(from_tree(c), Drop::Overlay(0), SPAN);
        let before_c = layout.panels().to_vec();
        assert!(!layout.drop(moved(&layout, c, 1), Drop::Overlay(0), SPAN));
        assert_eq!(layout.panels(), before_c.as_slice());
        assert_ne!(before, before_c);
    }

    #[test]
    fn a_legend_dragged_back_to_the_tree_leaves_its_panel_and_only_that_one() {
        let (mut layout, [a, b, c, d]) = three();
        layout.drop(from_tree(a), Drop::Overlay(2), SPAN);
        assert!(layout.remove(moved(&layout, a, 0)));
        assert_eq!(channels(&layout), vec![vec![b], vec![c], vec![d, a]]);
        assert!(layout.remove(moved(&layout, c, 1)));
        assert_eq!(channels(&layout), vec![vec![b], vec![d, a]]);
        // From the tree, or no longer on that panel: nothing to remove.
        assert!(!layout.remove(from_tree(b)));
        assert!(!layout.remove(moved(&layout, c, 0)));
        assert_eq!(channels(&layout), vec![vec![b], vec![d, a]]);
    }

    #[test]
    fn near_a_panel_edge_a_drop_makes_a_new_panel_and_inside_it_overlays() {
        // Three panels of 300 px from 10 px along the stacking axis.
        let spans = [(10.0, 310.0), (310.0, 610.0), (610.0, 910.0)];
        let at = |pos: f32| drop_target(&spans, pos);
        assert_eq!(at(12.0), Some(Drop::NewPanel(0)));
        assert_eq!(at(10.0 + EDGE_PX - 1.0), Some(Drop::NewPanel(0)));
        assert_eq!(at(10.0 + EDGE_PX + 1.0), Some(Drop::Overlay(0)));
        assert_eq!(at(160.0), Some(Drop::Overlay(0)));
        assert_eq!(at(300.0), Some(Drop::NewPanel(1)));
        assert_eq!(at(320.0), Some(Drop::NewPanel(1)));
        assert_eq!(at(460.0), Some(Drop::Overlay(1)));
        assert_eq!(at(905.0), Some(Drop::NewPanel(3)));
        assert_eq!(at(2.0), None);
        assert_eq!(at(1000.0), None);
    }

    #[test]
    fn a_narrow_panel_keeps_its_middle_for_overlay_and_an_empty_plot_takes_a_new_panel_anywhere() {
        let spans = [(0.0, 40.0), (40.0, 80.0)];
        assert_eq!(drop_target(&spans, 20.0), Some(Drop::Overlay(0)));
        assert_eq!(drop_target(&spans, 60.0), Some(Drop::Overlay(1)));
        assert_eq!(drop_target(&spans, 41.0), Some(Drop::NewPanel(1)));
        assert_eq!(drop_target(&[], 500.0), Some(Drop::NewPanel(0)));
    }

    #[test]
    fn a_view_change_in_one_panel_moves_time_in_every_panel_and_value_only_in_that_one() {
        let (mut layout, _) = three();
        let ids: Vec<PanelId> = layout.panels().iter().map(|p| p.id).collect();
        let views = Views::of(Span::new(0.0, 10.0), &layout);
        let middle = views.panel(ids[1]).unwrap();
        assert_eq!(middle.time, Span::new(0.0, 10.0));
        assert_eq!(middle.value, Span::new(2.0, 3.0));

        let panned = View {
            time: Span::new(1.0, 11.0),
            value: Span::new(2.5, 3.5),
        };
        let after = views.with_panel(ids[1], panned);
        assert_eq!(after.time, Span::new(1.0, 11.0));
        assert_eq!(after.panel(ids[0]).unwrap().value, SPAN);
        assert_eq!(after.panel(ids[1]).unwrap(), panned);
        assert_eq!(
            after.panel(ids[2]).unwrap(),
            View {
                time: Span::new(1.0, 11.0),
                value: Span::new(4.0, 5.0),
            }
        );

        assert!(after.apply_to(&mut layout));
        let values: Vec<Span> = layout.panels().iter().map(|p| p.value).collect();
        assert_eq!(values, vec![SPAN, Span::new(2.5, 3.5), Span::new(4.0, 5.0)]);
    }

    #[test]
    fn views_ease_end_by_end_and_never_land_on_a_layout_that_changed_under_them() {
        let (mut layout, [a, ..]) = three();
        let id = layout.panels()[0].id;
        let from = Views::of(Span::new(0.0, 10.0), &layout);
        let to = from.with_panel(
            id,
            View {
                time: Span::new(2.0, 4.0),
                value: Span::new(0.0, 3.0),
            },
        );
        let mid = Views::lerp(&from, &to, 0.5);
        assert_eq!(mid.time, Span::new(1.0, 7.0));
        assert_eq!(mid.panel(id).unwrap().value, Span::new(-0.5, 2.0));
        assert_eq!(
            mid.panel(layout.panels()[1].id).unwrap().value,
            Span::new(2.0, 3.0)
        );

        // The first panel's channels move out: a view of the old layout no
        // longer fits, and applying it changes nothing.
        layout.drop(moved(&layout, a, 0), Drop::Overlay(1), SPAN);
        layout.drop(moved(&layout, ch(0, 1), 0), Drop::NewPanel(2), SPAN);
        let before = layout.panels().to_vec();
        assert_eq!(layout.panels().len(), 3);
        assert!(!to.apply_to(&mut layout));
        assert_eq!(layout.panels(), before.as_slice());
    }
}
