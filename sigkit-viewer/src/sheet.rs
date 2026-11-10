//! The sheet: the bottom panel that slides up for anything needing input. It
//! animates in and out, and goes away with a swipe down (a drag, or a
//! two-finger scroll, anywhere on it but its fields) or Escape. Never modal:
//! the plot above keeps working while it is up.

use egui::{Color32, Id, Key, Sense, Stroke, Ui, pos2, vec2};

/// Points a swipe must travel down to dismiss the sheet.
const DISMISS: f32 = 48.0;

/// A sheet holding `T` while it is up or sliding away.
pub struct Sheet<T> {
    content: Option<T>,
    open: bool,
}

impl<T> Default for Sheet<T> {
    fn default() -> Self {
        Sheet {
            content: None,
            open: false,
        }
    }
}

impl<T> Sheet<T> {
    /// Slide up showing `content`, in place of anything shown now.
    pub fn open(&mut self, content: T) {
        self.content = Some(content);
        self.open = true;
    }

    /// Slide away. The content stays drawn until the sheet is gone.
    pub fn close(&mut self) {
        self.open = false;
    }

    /// Up, and not on its way out.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Up or sliding away.
    pub fn is_shown(&self) -> bool {
        self.content.is_some()
    }

    /// Draw the sheet at the bottom of `ui`, its contents by `add`.
    pub fn show<R>(
        &mut self,
        ui: &mut Ui,
        id: &str,
        add: impl FnOnce(&mut Ui, &mut T) -> R,
    ) -> Option<R> {
        let content = self.content.as_mut()?;
        // Escape in a field leaves the field, not the sheet.
        let typing = ui.memory(|m| m.focused().is_some());
        if self.open && !typing && ui.input(|i| i.key_pressed(Key::Escape)) {
            self.open = false;
        }
        let swipe_id = Id::unique(id).with("swipe");
        let mut swiped = false;
        let shown = egui::Panel::bottom(id)
            .resizable(false)
            .drag_to_open(false)
            .show_collapsible(ui, &mut self.open, |ui| {
                // Behind the fields, so they keep their own drags.
                let whole = ui.max_rect();
                let back = ui.interact(whole, swipe_id, Sense::drag());
                let mut travel: f32 = ui.data(|d| d.get_temp(swipe_id)).unwrap_or(0.0);
                if back.dragged() {
                    travel += back.drag_delta().y;
                } else if back.drag_stopped() {
                    // Released: judged below on how far it went.
                } else if back.hovered() {
                    // A scroll up takes the swipe back; a pause keeps it.
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    travel = if scroll < 0.0 { 0.0 } else { travel + scroll };
                } else {
                    travel = 0.0;
                }
                if !back.dragged() {
                    swiped = travel > DISMISS;
                    if back.drag_stopped() || swiped {
                        travel = 0.0;
                    }
                }
                ui.data_mut(|d| d.insert_temp(swipe_id, travel));

                grabber(ui, travel.max(0.0) / DISMISS);
                add(ui, content)
            });
        if swiped {
            self.open = false;
        }
        match shown {
            Some(inner) => Some(inner.inner),
            None => {
                if !self.open {
                    self.content = None;
                }
                None
            }
        }
    }
}

/// The handle at the top of a sheet, drawn stronger as a swipe nears
/// dismissing it.
fn grabber(ui: &mut Ui, pulled: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().strong_text_color();
    let color = lerp_color(weak, strong, pulled.clamp(0.0, 1.0));
    let c = rect.center();
    ui.painter().line_segment(
        [pos2(c.x - 18.0, c.y), pos2(c.x + 18.0, c.y)],
        Stroke::new(4.0, color),
    );
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), 255)
}
