//! The import sheet: a legacy raw file with no header was dropped. It shows
//! dtype, byte order, header offset, channel count, rate, gain and offset,
//! starting from a guess, and re-renders a preview of the decoded channels on
//! every change. Confirming writes a `header.json` beside the file; the data
//! is not copied.

use crate::plot;
use egui::{Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use sigkit_format::raw::{Guess, RawImport, guess};
use sigkit_format::recording::HEADER_FILE;
use sigkit_format::{Bin, ByteOrder, Dtype, Recording};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Bytes of the file the dtype guess looks at.
const GUESS_BYTES: usize = 64 * 1024;
/// Frames the preview reads at most, from the start of the file.
const PREVIEW_FRAMES: usize = 1 << 21;
/// Candles in the preview, merged down to its width when drawn.
const PREVIEW_BINS: usize = 2048;
/// Height of the fields and the preview.
const SHEET_BODY_HEIGHT: f32 = 200.0;
/// Rate a raw file is assumed to have until one is typed: nothing in the file
/// says.
const DEFAULT_RATE_HZ: f64 = 100.0;

/// A raw file being imported.
pub struct ImportSheet {
    pub file: PathBuf,
    file_bytes: u64,
    guess: Guess,
    pub import: RawImport,
    /// Why confirming cannot work here, whatever the fields say.
    blocked: Option<String>,
    /// The last confirm's failure.
    error: Option<String>,
    preview: Preview,
}

/// The decoded channels as min/max candles, for the fields they were
/// decoded with.
struct Preview {
    of: Option<RawImport>,
    frames: usize,
    trailing: u64,
    /// Per channel, [`PREVIEW_BINS`] candles (fewer for a short file).
    channels: Vec<Vec<Bin>>,
    /// Frames the candles cover.
    covered: usize,
    error: Option<String>,
}

/// What the sheet asks of the app.
pub enum Outcome {
    /// Imported: open the recording in this folder.
    Imported(PathBuf),
}

impl ImportSheet {
    /// Open `file` for import: guess what it holds from its start.
    pub fn new(file: PathBuf) -> Result<ImportSheet, String> {
        let fail = |e: std::io::Error| format!("{}: {e}", file.display());
        let file_bytes = fs::metadata(&file).map_err(fail)?.len();
        let mut prefix = Vec::with_capacity(GUESS_BYTES);
        fs::File::open(&file)
            .map_err(fail)?
            .take(GUESS_BYTES as u64)
            .read_to_end(&mut prefix)
            .map_err(fail)?;
        let guess = guess(&prefix);
        let header = file
            .parent()
            .map(|dir| dir.join(HEADER_FILE))
            .filter(|h| h.exists());
        Ok(ImportSheet {
            blocked: header.map(|h| {
                format!(
                    "{} already holds a recording's header; move the file into a folder of its own to import it",
                    h.parent().map(Path::display).map(|d| d.to_string()).unwrap_or_default()
                )
            }),
            file,
            file_bytes,
            guess,
            import: RawImport {
                dtype: guess.dtype,
                byte_order: guess.byte_order,
                offset_bytes: 0,
                channels: guess.channels,
                rate_hz: DEFAULT_RATE_HZ,
                gain: 1.0,
                offset: 0.0,
            },
            error: None,
            preview: Preview {
                of: None,
                frames: 0,
                trailing: 0,
                channels: Vec::new(),
                covered: 0,
                error: None,
            },
        })
    }

    fn file_name(&self) -> String {
        self.file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Decode the preview again when a field changed since the last one.
    fn refresh(&mut self) {
        // Rate, gain and offset only change how the candles are labelled
        // and drawn, not what is decoded.
        let layout = |i: &RawImport| (i.dtype, i.byte_order, i.offset_bytes, i.channels);
        if self.preview.of.as_ref().map(layout) == Some(layout(&self.import)) {
            return;
        }
        self.preview = preview(&self.file, &self.import, self.file_bytes);
    }

    fn confirm(&mut self) -> Option<Outcome> {
        match Recording::import_raw(&self.file, &self.import) {
            Ok(rec) => Some(Outcome::Imported(rec.dir().to_path_buf())),
            Err(e) => {
                self.error = Some(e.to_string());
                None
            }
        }
    }

    /// The sheet's contents. Returns what the app should do, if anything.
    pub fn ui(&mut self, ui: &mut Ui) -> Option<Outcome> {
        self.refresh();
        let weak = ui.visuals().weak_text_color();
        let mut confirm = false;
        let before = self.import;
        ui.horizontal(|ui| {
            ui.strong(format!("Import {}", self.file_name()));
            ui.label(
                egui::RichText::new(format!(
                    "{} bytes, no header · guessed {} {}-endian{}",
                    self.file_bytes,
                    self.guess.dtype,
                    self.guess.byte_order.name(),
                    match self.guess.channels {
                        1 => String::new(),
                        n => format!(", {n} channels interleaved"),
                    }
                ))
                .small()
                .color(weak),
            );
        });
        ui.add_space(4.0);
        // A fixed height, so the vertical separator does not stretch the
        // sheet over the window.
        let body = vec2(ui.available_width(), SHEET_BODY_HEIGHT);
        ui.allocate_ui_with_layout(body, egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.set_height(SHEET_BODY_HEIGHT);
            ui.vertical(|ui| {
                ui.set_width(300.0);
                self.fields(ui);
            });
            ui.separator();
            ui.vertical(|ui| self.preview_ui(ui));
        });
        if self.import != before {
            self.error = None;
        }
        ui.horizontal(|ui| {
            let ready = self.blocked.is_none() && self.preview.frames > 0;
            confirm |= ui
                .add_enabled(ready, egui::Button::new("Import  ⏎"))
                .clicked();
            confirm |= ready
                && !ui.ctx().egui_wants_keyboard_input()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let note = self
                .error
                .as_deref()
                .or(self.blocked.as_deref())
                .map(|e| egui::RichText::new(e).color(ui.visuals().error_fg_color))
                .unwrap_or_else(|| {
                    egui::RichText::new(
                        "writes header.json beside the file; the data is not copied · swipe down or esc to cancel",
                    )
                    .color(weak)
                });
            ui.label(note.small());
        });
        ui.add_space(6.0);
        if confirm { self.confirm() } else { None }
    }

    fn fields(&mut self, ui: &mut Ui) {
        let import = &mut self.import;
        egui::Grid::new("import fields")
            .num_columns(2)
            .spacing(vec2(10.0, 6.0))
            .show(ui, |ui| {
                ui.label("dtype");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for dtype in Dtype::ALL {
                        ui.selectable_value(&mut import.dtype, dtype, dtype.name());
                    }
                });
                ui.end_row();
                ui.label("byte order");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    for order in ByteOrder::ALL {
                        ui.selectable_value(&mut import.byte_order, order, order.name());
                    }
                });
                ui.end_row();
                ui.label("header offset");
                ui.add(egui::DragValue::new(&mut import.offset_bytes).suffix(" bytes"));
                ui.end_row();
                ui.label("channels");
                ui.add(
                    egui::DragValue::new(&mut import.channels)
                        .range(1..=64)
                        .speed(0.05)
                        .suffix(" interleaved"),
                );
                ui.end_row();
                ui.label("rate");
                ui.add(
                    egui::DragValue::new(&mut import.rate_hz)
                        .range(1e-6..=1e9)
                        .speed(1.0)
                        .suffix(" Hz"),
                );
                ui.end_row();
                ui.label("gain");
                ui.add(egui::DragValue::new(&mut import.gain).speed(0.001));
                ui.end_row();
                ui.label("offset");
                ui.add(egui::DragValue::new(&mut import.offset).speed(0.01));
                ui.end_row();
            });
    }

    fn preview_ui(&mut self, ui: &mut Ui) {
        let weak = ui.visuals().weak_text_color();
        let p = &self.preview;
        let rate = self.import.rate_hz;
        let summary = match &p.error {
            Some(e) => e.clone(),
            None => {
                let mut s = format!(
                    "{} frames × {} channel{} · {}",
                    p.frames,
                    self.import.channels,
                    if self.import.channels == 1 { "" } else { "s" },
                    seconds(p.frames as f64 / rate)
                );
                if p.covered < p.frames {
                    s.push_str(&format!(
                        " · preview: the first {}",
                        seconds(p.covered as f64 / rate)
                    ));
                }
                if p.trailing > 0 {
                    s.push_str(&format!(" · {} trailing bytes left out", p.trailing));
                }
                s
            }
        };
        ui.label(egui::RichText::new(summary).small().color(weak));
        let size = vec2(ui.available_width().max(200.0), 170.0);
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        draw_preview(ui, rect, p, self.import.gain, self.import.offset);
    }
}

/// `v` to five significant digits, without trailing zeros.
fn short(v: f64) -> String {
    let v: f64 = format!("{v:.4e}").parse().unwrap_or(v);
    format!("{v}")
}

/// `s` seconds as the time axis would say it.
fn seconds(s: f64) -> String {
    if s >= 120.0 {
        format!("{:.1} min", s / 60.0)
    } else if s >= 1.0 {
        format!("{s:.2} s")
    } else {
        format!("{:.1} ms", s * 1e3)
    }
}

/// Decode the start of `file` as `import` says, into min/max candles.
fn preview(file: &Path, import: &RawImport, file_bytes: u64) -> Preview {
    let (frames, trailing) = import.frames(file_bytes);
    let mut out = Preview {
        of: Some(*import),
        frames,
        trailing,
        channels: Vec::new(),
        covered: 0,
        error: None,
    };
    if file_bytes < import.offset_bytes {
        out.error = Some(format!(
            "the header offset is past the end of the file ({file_bytes} bytes)"
        ));
        return out;
    }
    if frames == 0 {
        out.error = Some("no whole frame after the header offset".into());
        return out;
    }
    let columns = match import.map(file) {
        Ok(columns) => columns,
        Err(e) => {
            out.error = Some(e.to_string());
            return out;
        }
    };
    let covered = frames.min(PREVIEW_FRAMES);
    let bins = covered.min(PREVIEW_BINS);
    out.covered = covered;
    let mut buffer = Vec::new();
    for column in &columns {
        let mut candles = vec![Bin::EMPTY; bins];
        for (b, candle) in candles.iter_mut().enumerate() {
            column.read_into(b * covered / bins..(b + 1) * covered / bins, &mut buffer);
            for &v in &buffer {
                candle.include(v);
            }
        }
        out.channels.push(candles);
    }
    out
}

/// The candles of every channel across `rect`, time left to right, value
/// fitted to what is shown, calibrated.
fn draw_preview(ui: &Ui, rect: Rect, p: &Preview, gain: f64, offset: f64) {
    let painter = ui.painter_at(rect);
    let visuals = ui.visuals();
    painter.rect_filled(rect, 4.0, visuals.extreme_bg_color);
    let calibrated = |v: f32| gain * v as f64 + offset;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for candle in p.channels.iter().flatten().filter(|c| !c.is_empty()) {
        for v in [calibrated(candle.min), calibrated(candle.max)] {
            if v.is_finite() {
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return;
    }
    if hi - lo <= f64::EPSILON * hi.abs().max(1.0) {
        (lo, hi) = (lo - 0.5, hi + 0.5);
    }
    let inner = rect.shrink2(vec2(4.0, 8.0));
    let y = |v: f64| {
        let t = ((v - lo) / (hi - lo)).clamp(0.0, 1.0) as f32;
        inner.bottom() - t * inner.height()
    };
    let dark = visuals.dark_mode;
    let columns = inner.width().floor().max(1.0) as usize;
    for (j, candles) in p.channels.iter().enumerate() {
        let color: Color32 = plot::channel_color(j, dark);
        let stroke = Stroke::new(1.0, color);
        let mut previous: Option<(f32, f32)> = None;
        for x in 0..columns {
            let from = x * candles.len() / columns;
            let to = ((x + 1) * candles.len() / columns)
                .max(from + 1)
                .min(candles.len());
            let merged = candles[from..to]
                .iter()
                .fold(Bin::EMPTY, |a, b| a.merge(*b));
            if merged.is_empty() {
                previous = None;
                continue;
            }
            // A negative gain turns the candle over.
            let (a, b) = (y(calibrated(merged.max)), y(calibrated(merged.min)));
            let (mut top, mut bottom) = (a.min(b), a.max(b));
            // Join to the column before, so a line is a line.
            if let Some((pt, pb)) = previous {
                top = top.min(pb);
                bottom = bottom.max(pt);
            }
            let px = inner.left() + x as f32 + 0.5;
            painter.line_segment([pos2(px, top), pos2(px, bottom.max(top + 1.0))], stroke);
            previous = Some((a.min(b), a.max(b)));
        }
    }
    let label = |text: String, at: egui::Pos2, align: egui::Align2| {
        painter.text(
            at,
            align,
            text,
            egui::FontId::proportional(11.0),
            visuals.weak_text_color(),
        );
    };
    label(
        short(hi),
        rect.left_top() + vec2(6.0, 2.0),
        egui::Align2::LEFT_TOP,
    );
    label(
        short(lo),
        rect.left_bottom() + vec2(6.0, -2.0),
        egui::Align2::LEFT_BOTTOM,
    );
}
