//! The application: the open recordings in a sidebar tree, their channels laid
//! out on panels by dragging, and a bar at the bottom with the mode control,
//! the cursor readout, the status line and the key hints. Pyramids are built
//! on a background thread so a first open of a huge recording shows progress
//! instead of a frozen window.

use crate::layout::{ChannelRef, Dragged, Drop, Layout};
use crate::plot::{self, Mode, PlotState, Series, Shown};
use crate::trace::{Clock, Placement, Trace, Values};
use crate::view::{Orientation, Span};
use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{
    Align, Color32, ColorImage, Context, DragAndDrop, FontData, FontFamily, FontId, Id, Key,
    LayerId, Layout as Flow, Order, Pos2, Rect, ScreenshotCallback, Sense, Stroke, ViewportCommand,
    pos2, vec2,
};
use sigkit_format::wallclock::rfc3339_utc_ms;
use sigkit_format::{PyramidSource, RateFit, Recording, Residual, Stream, StreamKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Name of a timestamped stream's virtual residual channel.
const RESIDUAL: &str = "residual";
/// Width of the sidebar tree.
const TREE_WIDTH: f32 = 260.0;

/// What the command line asked for.
pub struct Options {
    pub recordings: Vec<PathBuf>,
    pub orientation: Orientation,
    /// Initial time window; the whole first recording when `None`.
    pub time: Option<Span>,
    /// The `stream/channel` of the first recording to draw first; its first
    /// channel when `None`.
    pub channel: Option<String>,
    /// Draw a residual against the fitted rate.
    pub subtract_fit: bool,
    /// Save a PNG of the window once the recordings are drawn, then quit.
    pub screenshot: Option<PathBuf>,
    /// Show the cursor at this display time while the pointer is away.
    pub cursor: Option<f64>,
}

/// The viewer window: the open recordings and how their channels are laid
/// out on the plot.
pub struct App {
    recordings: Vec<Loaded>,
    /// Recordings being opened, in the order they were asked for; each joins
    /// the tree in that order.
    loading: Vec<Loading>,
    error: Option<String>,
    layout: Layout,
    plot: PlotState,
    initial_time: Option<Span>,
    initial_channel: Option<String>,
    subtract_fit: bool,
    screenshot: Option<Screenshot>,
    /// Channels whose trace changed (a residual's fit toggled); their panels
    /// fit them next frame.
    refit: Vec<ChannelRef>,
    /// Display time under the pointer, for the cursor readout.
    hover_time: Option<f64>,
    /// CPU time of the last plot frame, for checking the frame budget by eye.
    plot_ms: f64,
}

/// A pending `--screenshot`: taken a few frames after the recordings are drawn.
struct Screenshot {
    path: PathBuf,
    frames_drawn: u32,
    requested: bool,
    saved: Arc<AtomicBool>,
}

/// A recording being opened on a background thread.
struct Loading {
    path: PathBuf,
    rx: mpsc::Receiver<Result<Loaded, String>>,
    progress: Arc<Progress>,
}

/// How far opening has got: pyramid `step` of `steps`, and `done` of
/// `total` samples into it.
#[derive(Default)]
struct Progress {
    step: AtomicUsize,
    steps: AtomicUsize,
    done: AtomicUsize,
    total: AtomicUsize,
}

/// An open recording: every channel ready to draw.
struct Loaded {
    path: PathBuf,
    name: String,
    entries: Vec<Entry>,
    /// Streams that could not be opened, said in the status bar.
    problems: Vec<String>,
    /// Pyramids built (not from the cache) when opening, and how long it took.
    built: usize,
    took: Duration,
}

/// One channel of a recording, or one stream's residual.
struct Entry {
    stream: String,
    /// The channel's id, or `residual`.
    channel: String,
    /// `stream/channel`.
    label: String,
    /// `recording · stream/channel`, for when several recordings are open.
    full_label: String,
    unit: String,
    /// The unit of the stream's raw stamps; empty at a fixed rate.
    stamp_unit: String,
    trace: Trace,
    /// True for a stream's residual.
    residual: bool,
    /// A residual's trace against the fitted rate, when there is a fit.
    fitted: Option<Trace>,
    /// Draw a residual against the fitted rate instead of the nominal one.
    subtract_fit: bool,
    /// The stream's rates, for a timestamped stream.
    rates: Option<Rates>,
}

impl Entry {
    /// The trace on the plot.
    fn trace(&self) -> &Trace {
        match (&self.fitted, self.subtract_fit) {
            (Some(fitted), true) => fitted,
            _ => &self.trace,
        }
    }
}

#[derive(Clone, Copy)]
struct Rates {
    nominal_hz: f64,
    fit: Option<RateFit>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, options: Options) -> App {
        cc.egui_ctx.add_font(FontInsert::new(
            "Inter",
            FontData::from_static(include_bytes!("../assets/Inter-Regular.otf")),
            vec![InsertFontFamily {
                family: FontFamily::Proportional,
                priority: FontPriority::Highest,
            }],
        ));
        let mut app = App {
            recordings: Vec::new(),
            loading: Vec::new(),
            error: None,
            layout: Layout::default(),
            plot: PlotState::new(Span::new(0.0, 1.0), options.orientation),
            initial_time: options.time,
            initial_channel: options.channel,
            subtract_fit: options.subtract_fit,
            screenshot: options.screenshot.map(|path| Screenshot {
                path,
                frames_drawn: 0,
                requested: false,
                saved: Arc::new(AtomicBool::new(false)),
            }),
            refit: Vec::new(),
            hover_time: None,
            plot_ms: 0.0,
        };
        app.plot.pinned_cursor = options.cursor;
        for path in options.recordings {
            app.start_load(path);
        }
        app
    }

    fn entry(&self, channel: ChannelRef) -> &Entry {
        entry(&self.recordings, channel)
    }

    fn color(&self, channel: ChannelRef, dark_mode: bool) -> Color32 {
        color(&self.recordings, channel, dark_mode)
    }

    fn label(&self, channel: ChannelRef) -> &str {
        label(&self.recordings, channel)
    }

    /// Drive a pending `--screenshot`: wait for the plot to be drawn and
    /// still, ask the backend for the frame, save it, quit. Quit with the
    /// error if opening failed, so a scripted run never hangs.
    fn drive_screenshot(&mut self, ctx: &Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        if self.error.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if self.recordings.is_empty() || !self.loading.is_empty() || self.plot.is_moving() {
            return;
        }
        shot.frames_drawn += 1;
        if shot.frames_drawn >= 3 && !shot.requested {
            shot.requested = true;
            let (path, saved) = (shot.path.clone(), shot.saved.clone());
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(ScreenshotCallback::new(
                move |image| {
                    if let Err(e) = save_png(&path, &image) {
                        eprintln!("sigkit-viewer: {}: {e}", path.display());
                    }
                    saved.store(true, Ordering::Relaxed);
                },
            )));
        }
        if shot.saved.load(Ordering::Relaxed) {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        ctx.request_repaint();
    }

    /// Say what went wrong in the status bar, after anything already said
    /// since the last drop.
    fn report(&mut self, message: String) {
        self.error = Some(match self.error.take() {
            Some(before) => format!("{before}; {message}"),
            None => message,
        });
    }

    fn start_load(&mut self, path: PathBuf) {
        // One recording by any spelling of its path is open once.
        let path = path.canonicalize().unwrap_or(path);
        let open = self.recordings.iter().map(|r| &r.path);
        if open
            .chain(self.loading.iter().map(|l| &l.path))
            .any(|p| *p == path)
        {
            self.report(format!("{}: already open", path.display()));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Progress::default());
        let worker = (path.clone(), progress.clone());
        std::thread::spawn(move || {
            let (path, progress) = worker;
            let _ = tx.send(load(&path, &progress));
        });
        self.loading.push(Loading { path, rx, progress });
    }

    /// Take finished recordings in the order they were asked for.
    fn poll_loading(&mut self, ctx: &Context) {
        while let Some(loading) = self.loading.first() {
            match loading.rx.try_recv() {
                Ok(Ok(loaded)) => {
                    self.loading.remove(0);
                    self.add_recording(ctx, loaded);
                }
                Ok(Err(message)) => {
                    self.loading.remove(0);
                    self.report(message);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    return;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let path = self.loading.remove(0).path;
                    self.report(format!("{}: opening failed", path.display()));
                }
            }
        }
    }

    /// Add a recording to the tree and one of its channels in a new panel:
    /// the first recording's first channel (or `--channel`), a later one's
    /// channel matching the first panel's, so two recordings of the same
    /// thing open side by side.
    fn add_recording(&mut self, ctx: &Context, mut loaded: Loaded) {
        let first = self.recordings.is_empty();
        let wanted = if first {
            self.initial_channel.take()
        } else {
            self.layout
                .panels()
                .first()
                .and_then(|p| p.channels.first())
                .map(|&c| self.entry(c).label.clone())
        };
        let mut entry = 0;
        if let Some(label) = wanted {
            match loaded.entries.iter().position(|e| e.label == label) {
                Some(i) => entry = i,
                None if first => loaded.problems.push(format!("no channel {label}")),
                None => {}
            }
        }
        if first {
            for e in &mut loaded.entries {
                e.subtract_fit = self.subtract_fit && e.fitted.is_some();
            }
        }
        self.recordings.push(loaded);
        let channel = ChannelRef {
            recording: self.recordings.len() - 1,
            entry,
        };
        if first {
            let whole = self.entry(channel).trace().display_span();
            self.plot.time = self
                .initial_time
                .take()
                .or(whole)
                .unwrap_or(Span::new(0.0, 1.0));
        }
        let value = fitted_value(self.entry(channel).trace(), self.plot.time);
        let end = self.layout.panels().len();
        self.layout.drop(
            Dragged {
                channel,
                from: None,
            },
            Drop::NewPanel(end),
            value,
        );
        self.plot.interrupt();
        let names: Vec<&str> = self.recordings.iter().map(|r| r.name.as_str()).collect();
        ctx.send_viewport_cmd(ViewportCommand::Title(format!(
            "{} — sigkit",
            names.join(", ")
        )));
    }

    /// The mode as a segment control, with its key on each segment.
    fn mode_control(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style())
            .inner_margin(2.0)
            .corner_radius(7.0)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for mode in Mode::ALL {
                    let key = mode.key().symbol_or_name();
                    let text = egui::RichText::new(format!("{}  {key}", mode.label()));
                    if ui.selectable_label(self.plot.mode == mode, text).clicked() {
                        self.plot.mode = mode;
                    }
                }
            });
    }

    /// The tree: recordings, their streams with the fitted rate, and each
    /// stream's channels plus, for a timestamped stream, its residual. A row
    /// drags its channel onto the plot; its dot is filled while the channel is
    /// on a panel.
    fn tree(&mut self, ui: &mut egui::Ui) {
        let dark = ui.visuals().dark_mode;
        let weak = ui.visuals().weak_text_color();
        let mut toggled = None;
        for r in 0..self.recordings.len() {
            let rec = &self.recordings[r];
            egui::CollapsingHeader::new(egui::RichText::new(&rec.name).strong())
                .id_salt(("recording", r))
                .default_open(true)
                .show(ui, |ui| {
                    let mut stream: Option<&str> = None;
                    for (e, entry) in rec.entries.iter().enumerate() {
                        if stream != Some(entry.stream.as_str()) {
                            stream = Some(&entry.stream);
                            stream_row(ui, entry, weak);
                        }
                        let channel = ChannelRef {
                            recording: r,
                            entry: e,
                        };
                        let row = ChannelRow {
                            entry,
                            color: self.color(channel, dark),
                            count: self.layout.count(channel),
                            weak,
                        };
                        if row.show(ui, channel) {
                            toggled = Some(channel);
                        }
                    }
                });
        }
        if let Some(channel) = toggled {
            let entry = &mut self.recordings[channel.recording].entries[channel.entry];
            entry.subtract_fit = !entry.subtract_fit;
            self.refit.push(channel);
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "drag a channel onto the plot  ·  drop a .sigrec on the window to add it",
            )
            .small()
            .color(weak),
        );
    }

    /// The display time under the pointer and, for each recording with a UTC
    /// anchor, the wall-clock time of that instant.
    fn cursor_readout(&self, ui: &mut egui::Ui) {
        let Some(t) = self.hover_time else {
            return;
        };
        let mut text = format!("t {t:.4} s");
        let several = self.recordings.len() > 1;
        for rec in &self.recordings {
            if let Some(ns) = rec.entries.iter().find_map(|e| e.trace.utc_ns_at(t)) {
                let name = if several {
                    format!("{} ", rec.name)
                } else {
                    String::new()
                };
                text.push_str(&format!("  ·  {name}UTC {}", rfc3339_utc_ms(ns)));
            }
        }
        ui.monospace(text);
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        if let Some(loading) = self.loading.first() {
            let p = &loading.progress;
            let (step, steps) = (
                p.step.load(Ordering::Relaxed),
                p.steps.load(Ordering::Relaxed),
            );
            let (done, total) = (
                p.done.load(Ordering::Relaxed),
                p.total.load(Ordering::Relaxed),
            );
            let percent = (100 * done).checked_div(total).unwrap_or(0);
            ui.weak(format!(
                "building pyramids for {}  ·  {}/{steps}  ·  {percent}%",
                loading.path.display(),
                (step + 1).min(steps.max(1)),
            ));
        } else if !self.recordings.is_empty() {
            let names: Vec<&str> = self.recordings.iter().map(|r| r.name.as_str()).collect();
            let pyramids: usize = self
                .recordings
                .iter()
                .flat_map(|r| &r.entries)
                .map(|e| 1 + e.fitted.is_some() as usize)
                .sum();
            let built: usize = self.recordings.iter().map(|r| r.built).sum();
            let took: Duration = self.recordings.iter().map(|r| r.took).sum();
            let panels = self.layout.panels().len();
            let mut text = format!(
                "{}  ·  {panels} panel{}  ·  {pyramids} pyramids, {built} built, in {:.0} ms  ·  plot {:.1} ms",
                names.join(", "),
                if panels == 1 { "" } else { "s" },
                took.as_secs_f64() * 1e3,
                self.plot_ms
            );
            for problem in self.recordings.iter().flat_map(|r| &r.problems) {
                text.push_str("  ·  ");
                text.push_str(problem);
            }
            ui.weak(text);
        } else {
            ui.weak("drop a .sigrec recording here, or pass it on the command line");
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    /// While a channel is dragged: say where it would land, and on release
    /// put it there. Onto the plot it overlays or opens a panel; a legend
    /// dragged back onto the tree leaves its panel.
    fn drag_and_drop(&mut self, ctx: &Context, shown: &Shown, tree: Rect) {
        let Some(dragged) = DragAndDrop::payload::<Dragged>(ctx).map(|d| *d) else {
            return;
        };
        let Some(pointer) = ctx.input(|i| i.pointer.latest_pos()) else {
            return;
        };
        let released = ctx.input(|i| i.pointer.any_released());
        let dark = ctx.global_style().visuals.dark_mode;
        let color = self.color(dragged.channel, dark);
        let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::unique("drop")));
        let orientation = self.plot.orientation;
        if tree.contains(pointer) && dragged.from.is_some() {
            let remove = if dark {
                Color32::from_rgb(0xf0, 0x7a, 0x8a)
            } else {
                Color32::from_rgb(0xd1, 0x49, 0x5b)
            };
            draw_remove(&painter, tree.shrink(6.0), remove);
            if released && self.layout.remove(dragged) {
                self.plot.interrupt();
            }
        } else if let Some(drop) = shown.drop_target(orientation, pointer) {
            let text = match drop {
                Drop::Overlay(i) if self.layout.panels()[i].channels.contains(&dragged.channel) => {
                    "already on this panel"
                }
                Drop::Overlay(_) => "overlay",
                Drop::NewPanel(_) => "new panel",
            };
            plot::draw_drop(&painter, shown, orientation, drop, color, text);
            if released {
                let time = self.plot.time;
                let value = fitted_value(self.entry(dragged.channel).trace(), time);
                if self.layout.drop(dragged, drop, value) {
                    self.plot.interrupt();
                }
            }
        }
        if !released {
            ghost(&painter, pointer, self.label(dragged.channel), color, dark);
            ctx.request_repaint();
        } else {
            DragAndDrop::clear_payload(ctx);
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll_loading(ctx);
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            // A new drop: the last one's errors have been read.
            self.error = None;
        }
        for path in dropped {
            self.start_load(path);
        }
        ctx.input(|i| {
            if i.key_pressed(Key::O) {
                self.plot.orientation = self.plot.orientation.toggled();
            }
            for mode in Mode::ALL {
                if i.key_pressed(mode.key()) {
                    self.plot.mode = mode;
                }
            }
        });
        self.drive_screenshot(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let now = ui.input(|i| i.time);
        egui::Panel::bottom("bar").show(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                self.mode_control(ui);
                ui.with_layout(Flow::right_to_left(Align::Center), |ui| {
                    self.cursor_readout(ui)
                });
            });
            ui.horizontal(|ui| self.status_line(ui));
            ui.horizontal(|ui| ui.weak(hints(self.plot.mode)));
            ui.add_space(2.0);
        });
        let tree = egui::Panel::left("tree")
            .resizable(false)
            .exact_size(TREE_WIDTH)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(6.0);
                    self.tree(ui);
                    ui.allocate_space(ui.available_size());
                });
            })
            .response
            .rect;
        let shown = egui::CentralPanel::no_frame()
            .show(ui, |ui| {
                let dark = ui.visuals().dark_mode;
                let series = series(&self.recordings, &self.layout, dark);
                if !self.refit.is_empty() {
                    let which: Vec<usize> = (0..series.len())
                        .filter(|&i| series[i].iter().any(|s| self.refit.contains(&s.channel)))
                        .collect();
                    plot::autoscale(&mut self.plot, &self.layout, &series, &which, false, now);
                    self.refit.clear();
                }
                let started = Instant::now();
                let shown = plot::show(ui, &mut self.layout, &series, &mut self.plot);
                self.plot_ms = started.elapsed().as_secs_f64() * 1e3;
                shown
            })
            .inner;
        self.hover_time = shown.hover_time;
        self.drag_and_drop(ui.ctx(), &shown, tree);
    }
}

fn entry(recordings: &[Loaded], channel: ChannelRef) -> &Entry {
    &recordings[channel.recording].entries[channel.entry]
}

/// The channel's colour in the channel palette, by its place in the tree
/// across every open recording.
fn color(recordings: &[Loaded], channel: ChannelRef, dark_mode: bool) -> Color32 {
    let before: usize = recordings[..channel.recording]
        .iter()
        .map(|r| r.entries.len())
        .sum();
    plot::channel_color(before + channel.entry, dark_mode)
}

/// `stream/channel`, with the recording's name when several are open.
fn label(recordings: &[Loaded], channel: ChannelRef) -> &str {
    let entry = entry(recordings, channel);
    if recordings.len() > 1 {
        &entry.full_label
    } else {
        &entry.label
    }
}

/// What each panel draws, in layout order.
fn series<'a>(recordings: &'a [Loaded], layout: &Layout, dark_mode: bool) -> Vec<Vec<Series<'a>>> {
    layout
        .panels()
        .iter()
        .map(|panel| {
            panel
                .channels
                .iter()
                .map(|&channel| {
                    let entry = entry(recordings, channel);
                    Series {
                        trace: entry.trace(),
                        label: label(recordings, channel),
                        unit: &entry.unit,
                        stamp_unit: &entry.stamp_unit,
                        color: color(recordings, channel, dark_mode),
                        channel,
                    }
                })
                .collect()
        })
        .collect()
}

/// A stream's row in the tree: its id, kind and rate, and for a timestamped
/// stream the fitted rate under it.
fn stream_row(ui: &mut egui::Ui, entry: &Entry, weak: Color32) {
    ui.horizontal(|ui| {
        ui.label(&entry.stream);
        let kind = match entry.rates {
            Some(rates) => format!("timestamped · {} Hz nominal", rates.nominal_hz),
            None => format!("fixed · {} Hz", entry.trace.rate_hz()),
        };
        ui.label(egui::RichText::new(kind).small().color(weak));
    });
    if let Some(rates) = entry.rates {
        let text = match rates.fit {
            Some(fit) => format!(
                "fitted {:.4} Hz  ·  {:+.2} ± {:.2} ppm  ·  {} missing",
                fit.rate_hz,
                fit.ppm(rates.nominal_hz),
                fit.ppm_sd(rates.nominal_hz),
                fit.missing
            ),
            None => "too few samples to fit a rate".into(),
        };
        ui.indent(("fit", &entry.stream), |ui| {
            ui.label(egui::RichText::new(text).small().color(weak));
        });
    }
}

/// A channel's row in the tree.
struct ChannelRow<'a> {
    entry: &'a Entry,
    color: Color32,
    /// How many panels show the channel.
    count: usize,
    weak: Color32,
}

impl ChannelRow<'_> {
    const HEIGHT: f32 = 20.0;
    const INDENT: f32 = 22.0;

    /// Draw the row and let it be dragged; true when its residual's fit
    /// toggle was clicked.
    fn show(&self, ui: &mut egui::Ui, channel: ChannelRef) -> bool {
        let (rect, response) = ui.allocate_exact_size(
            vec2(ui.available_width(), Self::HEIGHT),
            Sense::click_and_drag(),
        );
        response.dnd_set_drag_payload(Dragged {
            channel,
            from: None,
        });
        if response.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals();
        if response.hovered() || response.dragged() {
            painter.rect_filled(rect, 5.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let mid = rect.center().y;
        let dot = pos2(rect.min.x + Self::INDENT, mid);
        if self.count > 0 {
            painter.circle_filled(dot, 4.5, self.color);
        } else {
            painter.circle_stroke(dot, 4.0, Stroke::new(1.5, self.color));
        }
        let font = FontId::proportional(13.0);
        let id = if self.entry.residual {
            egui::RichText::new(&self.entry.channel).italics()
        } else {
            egui::RichText::new(&self.entry.channel)
        };
        let id = id.font(font.clone()).color(visuals.text_color());
        let galley = egui::WidgetText::from(id).into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            rect.width() - 120.0,
            font.clone(),
        );
        painter.galley(
            pos2(dot.x + 12.0, mid - galley.size().y / 2.0),
            galley,
            visuals.text_color(),
        );
        // Right to left: the count, the unit, the fit toggle.
        let small = FontId::proportional(11.5);
        let mut x = rect.max.x - 6.0;
        if self.count > 1 {
            let r = painter.text(
                pos2(x, mid),
                egui::Align2::RIGHT_CENTER,
                format!("×{}", self.count),
                small.clone(),
                self.weak,
            );
            x = r.min.x - 8.0;
        }
        let r = painter.text(
            pos2(x, mid),
            egui::Align2::RIGHT_CENTER,
            &self.entry.unit,
            small,
            self.weak,
        );
        x = r.min.x - 8.0;
        if self.entry.residual && self.entry.fitted.is_some() {
            let toggle = Rect::from_min_max(pos2(x - 30.0, mid - 8.0), pos2(x, mid + 8.0));
            return ui
                .put(
                    toggle,
                    egui::Button::new(egui::RichText::new("fit").small())
                        .selected(self.entry.subtract_fit)
                        .frame(true),
                )
                .on_hover_text("subtract the fitted rate")
                .clicked();
        }
        false
    }
}

/// The tree tinted as the place a dragged legend leaves its panel.
fn draw_remove(painter: &egui::Painter, rect: Rect, color: Color32) {
    plot::draw_outline(painter, rect, color);
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        "drop to remove from its panel",
        FontId::proportional(13.0),
        color,
    );
}

/// The dragged channel's name following the pointer.
fn ghost(painter: &egui::Painter, pointer: Pos2, label: &str, color: Color32, dark: bool) {
    let text = if dark {
        Color32::from_gray(225)
    } else {
        Color32::from_gray(40)
    };
    let fill = if dark {
        Color32::from_rgba_unmultiplied(44, 44, 44, 235)
    } else {
        Color32::from_rgba_unmultiplied(255, 255, 255, 235)
    };
    let galley = painter.layout_no_wrap(label.to_string(), FontId::proportional(12.0), text);
    let min = pointer + vec2(14.0, 10.0);
    let rect = Rect::from_min_size(min, galley.size() + vec2(36.0, 8.0));
    painter.rect_filled(rect, 6.0, fill);
    painter.hline(
        rect.min.x + 8.0..=rect.min.x + 22.0,
        rect.center().y,
        Stroke::new(2.0, color),
    );
    painter.galley(pos2(rect.min.x + 28.0, rect.min.y + 4.0), galley, text);
}

/// The value span a new panel shows `trace` in: fitted over display `time`,
/// or over its whole signal when nothing of it is in view.
fn fitted_value(trace: &Trace, time: Span) -> Span {
    plot::fitted_value([trace], Some(time))
        .or_else(|| plot::fitted_value([trace], None))
        .unwrap_or(Span::new(-1.0, 1.0))
}

/// Every gesture with its key, for the mode in use.
fn hints(mode: Mode) -> &'static str {
    match mode {
        Mode::Pan => {
            "drag a channel onto a panel to overlay, to an edge for a new panel, a legend back to the tree to remove  ·  pinch or +/− zoom time  ·  ⇧ value  ·  scroll, drag or arrows pan  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
        }
        Mode::Align => {
            "dragging to align is not wired up yet  ·  pinch or +/− zoom time  ·  ⇧ value  ·  scroll pans  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
        }
        Mode::Box => {
            "drag a box to fit it  ·  pinch or +/− zoom time  ·  ⇧ value  ·  scroll or arrows pan  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
        }
    }
}

/// Open the recording and make every channel ready to draw: its pyramid
/// built or loaded, and for each timestamped stream two residual pyramids
/// (against the nominal and the fitted rate). A stream that cannot be read
/// is reported and skipped; a recording with nothing to draw is an error.
fn load(path: &Path, progress: &Progress) -> Result<Loaded, String> {
    let started = Instant::now();
    let rec = Recording::open(path).map_err(|e| e.to_string())?;
    let header = rec.header();
    let steps = header
        .streams
        .iter()
        .map(|s| s.channels.len() + 2 * (s.kind == StreamKind::Timestamped) as usize)
        .sum();
    progress.steps.store(steps, Ordering::Relaxed);
    let mut loaded = Loaded {
        path: path.to_path_buf(),
        name: header.name.clone(),
        entries: Vec::new(),
        problems: Vec::new(),
        built: 0,
        took: Duration::ZERO,
    };
    for stream in &header.streams {
        if let Err(e) = load_stream(&rec, stream, &mut loaded, progress) {
            loaded.problems.push(format!("{}: {e}", stream.id));
        }
    }
    if loaded.entries.is_empty() {
        let mut message = format!("{}: no channel to draw", header.name);
        for problem in &loaded.problems {
            message.push_str("; ");
            message.push_str(problem);
        }
        return Err(message);
    }
    loaded.took = started.elapsed();
    Ok(loaded)
}

/// Add a stream's channels, and its residual when it is timestamped.
fn load_stream(
    rec: &Recording,
    stream: &Stream,
    loaded: &mut Loaded,
    progress: &Progress,
) -> sigkit_format::Result<()> {
    let report = &mut |done: usize, total: usize| {
        progress.done.store(done, Ordering::Relaxed);
        progress.total.store(total, Ordering::Relaxed);
    };
    let next_step = || {
        progress.step.fetch_add(1, Ordering::Relaxed);
    };
    let tc = &stream.time_correction;
    let placement = Placement {
        start_s: rec.start_s(&stream.id)?,
        k: tc.k,
        shift_s: tc.shift_s,
    };
    let invalid = |reason: &str| sigkit_format::Error::InvalidStream {
        stream: stream.id.clone(),
        reason: reason.into(),
    };
    // A clock per trace: each maps the time column for itself.
    let clock = || -> sigkit_format::Result<Clock> {
        match stream.kind {
            StreamKind::FixedRate => Ok(Clock::Fixed {
                rate_hz: stream.rate_hz.expect("validated at open"),
            }),
            StreamKind::Timestamped => Ok(Clock::stamped(
                rec.map_time(&stream.id)?.expect("timestamped"),
                stream
                    .seconds_per_tick()
                    .ok_or_else(|| invalid("unknown clock unit"))?,
                stream.rate_nominal_hz.expect("validated at open"),
            )),
        }
    };
    let rates = match stream.kind {
        StreamKind::FixedRate => None,
        StreamKind::Timestamped => Some(Rates {
            nominal_hz: stream.rate_nominal_hz.expect("validated at open"),
            fit: rec.fit_rate(&stream.id)?,
        }),
    };
    let stamp_unit = stream
        .time
        .as_ref()
        .map(|t| t.unit.clone())
        .unwrap_or_default();
    let entry = |channel: &str, unit: &str, trace: Trace, residual: bool, fitted: Option<Trace>| {
        let label = format!("{}/{channel}", stream.id);
        Entry {
            stream: stream.id.clone(),
            channel: channel.into(),
            full_label: format!("{} · {label}", rec.header().name),
            label,
            unit: unit.into(),
            stamp_unit: stamp_unit.clone(),
            trace,
            residual,
            fitted,
            subtract_fit: false,
            rates,
        }
    };

    for channel in &stream.channels {
        let (pyramid, source) = rec.pyramid_with_progress(&stream.id, &channel.id, report)?;
        loaded.built += (source == PyramidSource::Built) as usize;
        next_step();
        let trace = Trace::new(
            Values::Column(rec.map_column(&stream.id, &channel.id)?),
            clock()?,
            pyramid,
        )
        .calibrated(channel.gain, channel.offset)
        .placed(placement)
        .anchored(stream.utc_anchor.clone());
        loaded
            .entries
            .push(entry(&channel.id, &channel.unit, trace, false, None));
    }

    if let (Some(rates), Some(time)) = (rates, rec.map_time(&stream.id)?) {
        let Some(first) = time.get_i64(0) else {
            return Ok(());
        };
        let seconds_per_tick = stream
            .seconds_per_tick()
            .ok_or_else(|| invalid("unknown clock unit"))?;
        // Residuals are drawn in milliseconds.
        let residual_trace = |residual: Residual,
                              report: &mut dyn FnMut(usize, usize)|
         -> sigkit_format::Result<Trace> {
            let pyramid = rec.residual_pyramid(&stream.id, &residual, report)?;
            next_step();
            Ok(Trace::new(Values::Residual(residual), clock()?, pyramid)
                .calibrated(1e3, 0.0)
                .placed(placement)
                .anchored(stream.utc_anchor.clone()))
        };
        let nominal = residual_trace(
            Residual::against_nominal(first, seconds_per_tick, rates.nominal_hz),
            report,
        )?;
        let fitted = match rates.fit {
            Some(fit) => Some(residual_trace(
                Residual::against_fit(first, seconds_per_tick, &fit),
                report,
            )?),
            None => None,
        };
        loaded.built += 1 + fitted.is_some() as usize;
        loaded
            .entries
            .push(entry(RESIDUAL, "ms", nominal, true, fitted));
    }
    Ok(())
}

fn save_png(path: &Path, image: &ColorImage) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::io::BufWriter::new(std::fs::File::create(path)?);
    let [width, height] = image.size;
    let mut encoder = png::Encoder::new(file, width as u32, height as u32);
    // The window is opaque; the framebuffer's alpha is a blending leftover.
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let bytes: Vec<u8> = image
        .pixels
        .iter()
        .flat_map(|c| [c.r(), c.g(), c.b()])
        .collect();
    encoder.write_header()?.write_image_data(&bytes)?;
    Ok(())
}
