//! The application: one recording open at a time, one of its channels (or a
//! timestamped stream's residual) on the plot, and a bar at the bottom with
//! the mode control, the channel picker, the readouts and the key hints.
//! Pyramids are built on a background thread so a first open of a huge
//! recording shows progress instead of a frozen window.

use crate::plot::{self, Mode, PlotState, Series};
use crate::trace::{Clock, Placement, Trace, Values};
use crate::view::{Orientation, Span, View};
use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
use egui::{
    Align, ColorImage, Context, FontData, FontFamily, Key, Layout, ScreenshotCallback,
    ViewportCommand,
};
use sigkit_format::wallclock::rfc3339_utc_ms;
use sigkit_format::{PyramidSource, RateFit, Recording, Residual, Stream, StreamKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// Name of a timestamped stream's virtual residual channel.
const RESIDUAL: &str = "residual";

/// What the command line asked for.
pub struct Options {
    pub recording: Option<PathBuf>,
    pub orientation: Orientation,
    /// Initial time window; the whole recording when `None`.
    pub time: Option<Span>,
    /// The `stream/channel` to draw first; the first channel when `None`.
    pub channel: Option<String>,
    /// Draw a residual against the fitted rate.
    pub subtract_fit: bool,
    /// Save a PNG of the window once the recording is drawn, then quit.
    pub screenshot: Option<PathBuf>,
}

/// The viewer window: at most one open recording, drawn by the plot widget.
pub struct App {
    open: Option<Loaded>,
    loading: Option<Loading>,
    error: Option<String>,
    plot: PlotState,
    initial_time: Option<Span>,
    initial_channel: Option<String>,
    subtract_fit: bool,
    screenshot: Option<Screenshot>,
    /// Display time under the pointer, for the cursor readout.
    hover_time: Option<f64>,
    /// CPU time of the last plot frame, for checking the frame budget by eye.
    plot_ms: f64,
}

/// A pending `--screenshot`: taken a few frames after the recording is drawn.
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

/// An open recording: every channel ready to draw, and which one is drawn.
struct Loaded {
    name: String,
    entries: Vec<Entry>,
    selected: usize,
    /// Draw a residual against the fitted rate instead of the nominal one.
    subtract_fit: bool,
    /// Streams that could not be opened, said in the status bar.
    problems: Vec<String>,
    /// Pyramids built (not from the cache) when opening, and how long it took.
    built: usize,
    took: Duration,
}

/// One channel of the recording, or one stream's residual.
struct Entry {
    /// `stream/channel`.
    label: String,
    unit: String,
    /// Index into the channel palette: the entry's place in the recording.
    color: usize,
    trace: Trace,
    /// True for a stream's residual.
    residual: bool,
    /// A residual's trace against the fitted rate, when there is a fit.
    fitted: Option<Trace>,
    /// The stream's rates, for a timestamped stream.
    rates: Option<Rates>,
}

impl Entry {
    fn trace(&self, subtract_fit: bool) -> &Trace {
        match (&self.fitted, subtract_fit) {
            (Some(fitted), true) => fitted,
            _ => &self.trace,
        }
    }
}

impl Loaded {
    fn selected(&self) -> &Entry {
        &self.entries[self.selected]
    }

    /// The trace on the plot.
    fn selected_trace(&self) -> &Trace {
        self.selected().trace(self.subtract_fit)
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
        let placeholder = View {
            time: Span::new(0.0, 1.0),
            value: Span::new(-1.0, 1.0),
        };
        let mut app = App {
            open: None,
            loading: None,
            error: None,
            plot: PlotState::new(placeholder, options.orientation),
            initial_time: options.time,
            initial_channel: options.channel,
            subtract_fit: options.subtract_fit,
            screenshot: options.screenshot.map(|path| Screenshot {
                path,
                frames_drawn: 0,
                requested: false,
                saved: Arc::new(AtomicBool::new(false)),
            }),
            hover_time: None,
            plot_ms: 0.0,
        };
        if let Some(path) = options.recording {
            app.start_load(path);
        }
        app
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
        if self.open.is_none() || self.plot.is_moving() {
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

    fn start_load(&mut self, path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Progress::default());
        let worker = (path.clone(), progress.clone());
        std::thread::spawn(move || {
            let (path, progress) = worker;
            let _ = tx.send(load(&path, &progress));
        });
        self.loading = Some(Loading { path, rx, progress });
        self.error = None;
    }

    fn poll_loading(&mut self, ctx: &Context) {
        let Some(loading) = &self.loading else {
            return;
        };
        match loading.rx.try_recv() {
            Ok(Ok(mut loaded)) => {
                if let Some(label) = self.initial_channel.take() {
                    match loaded.entries.iter().position(|e| e.label == label) {
                        Some(i) => loaded.selected = i,
                        None => loaded.problems.push(format!("no channel {label}")),
                    }
                }
                loaded.subtract_fit = self.subtract_fit;
                let trace = loaded.selected_trace();
                self.plot.view = initial_view(trace);
                if let Some(time) = self.initial_time.take() {
                    self.plot.view.time = time;
                }
                ctx.send_viewport_cmd(ViewportCommand::Title(format!("{} — sigkit", loaded.name)));
                self.open = Some(loaded);
                self.loading = None;
            }
            Ok(Err(message)) => {
                self.error = Some(message);
                self.loading = None;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.error = Some(format!("{}: opening failed", loading.path.display()));
                self.loading = None;
            }
        }
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

    /// The channel picker, the fit readout and the toggle that subtracts it.
    fn channel_controls(&mut self, ui: &mut egui::Ui, now: f64) {
        let Some(open) = &mut self.open else {
            return;
        };
        let before = (open.selected, open.subtract_fit);
        egui::ComboBox::from_id_salt("channel")
            .selected_text(&open.selected().label)
            .show_ui(ui, |ui| {
                for (i, entry) in open.entries.iter().enumerate() {
                    ui.selectable_value(&mut open.selected, i, &entry.label);
                }
            });
        let entry = &open.entries[open.selected];
        if entry.residual {
            ui.add_enabled(
                entry.fitted.is_some(),
                egui::Checkbox::new(&mut open.subtract_fit, "subtract fitted rate"),
            );
        }
        if let Some(rates) = entry.rates {
            match rates.fit {
                Some(fit) => {
                    let ppm = fit.ppm(rates.nominal_hz);
                    let sd = fit.ppm_sd(rates.nominal_hz);
                    ui.label(format!(
                        "fitted {:.4} Hz  ·  {ppm:+.2} ± {sd:.2} ppm  ·  {} missing",
                        fit.rate_hz, fit.missing
                    ));
                }
                None => {
                    ui.label(format!(
                        "nominal {} Hz, too few samples to fit",
                        rates.nominal_hz
                    ));
                }
            }
        }
        if (open.selected, open.subtract_fit) != before {
            // Another channel: same time, its own values, eased in.
            let trace = open.selected_trace();
            if let Some((lo, hi)) = trace.whole_signal() {
                let target = View {
                    value: Span::fitted(lo, hi),
                    ..self.plot.target()
                };
                self.plot.animate_to(target, now);
            }
        }
    }

    /// The display time under the pointer and, on a stream with a UTC
    /// anchor, the wall-clock time of that instant.
    fn cursor_readout(&self, ui: &mut egui::Ui) {
        let (Some(open), Some(t)) = (&self.open, self.hover_time) else {
            return;
        };
        let trace = open.selected_trace();
        let utc = trace
            .utc_ns_at(t)
            .map(|ns| format!("  ·  UTC {}", rfc3339_utc_ms(ns)))
            .unwrap_or_default();
        ui.monospace(format!("t {t:.4} s{utc}"));
    }

    fn status_line(&self, ui: &mut egui::Ui) {
        if let Some(loading) = &self.loading {
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
        } else if let Some(open) = &self.open {
            let trace = open.selected_trace();
            let mut text = format!(
                "{}  ·  {} samples at {} Hz  ·  {} pyramids, {} built, in {:.0} ms  ·  plot {:.1} ms",
                open.name,
                trace.len(),
                trace.rate_hz(),
                open.entries.len() + open.entries.iter().filter(|e| e.fitted.is_some()).count(),
                open.built,
                open.took.as_secs_f64() * 1e3,
                self.plot_ms
            );
            for problem in &open.problems {
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
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.poll_loading(ctx);
        let dropped = ctx.input(|i| {
            i.raw
                .dropped_files
                .first()
                .map(|file| file.path().to_path_buf())
        });
        if let Some(path) = dropped {
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
                ui.add_space(8.0);
                self.channel_controls(ui, now);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    self.cursor_readout(ui)
                });
            });
            ui.horizontal(|ui| self.status_line(ui));
            ui.horizontal(|ui| ui.weak(hints(self.plot.mode)));
            ui.add_space(2.0);
        });
        egui::CentralPanel::no_frame().show(ui, |ui| match &self.open {
            Some(open) => {
                let entry = open.selected();
                let started = Instant::now();
                let series = Series {
                    trace: open.selected_trace(),
                    label: &entry.label,
                    unit: &entry.unit,
                    color: plot::channel_color(entry.color, ui.visuals().dark_mode),
                };
                self.hover_time = plot::show(ui, &series, &mut self.plot).hover_time;
                self.plot_ms = started.elapsed().as_secs_f64() * 1e3;
            }
            None => {
                ui.centered_and_justified(|ui| {
                    ui.weak(if self.loading.is_some() {
                        "opening…"
                    } else {
                        "no recording open"
                    })
                });
            }
        });
    }
}

/// Every gesture with its key, for the mode in use.
fn hints(mode: Mode) -> &'static str {
    match mode {
        Mode::Pan => {
            "pinch or +/− zoom time  ·  ⇧ value  ·  scroll, drag or arrows pan  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
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
        name: header.name.clone(),
        entries: Vec::new(),
        selected: 0,
        subtract_fit: false,
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
        loaded.entries.push(Entry {
            label: format!("{}/{}", stream.id, channel.id),
            unit: channel.unit.clone(),
            color: loaded.entries.len(),
            trace,
            residual: false,
            fitted: None,
            rates,
        });
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
        loaded.entries.push(Entry {
            label: format!("{}/{RESIDUAL}", stream.id),
            unit: "ms".into(),
            color: loaded.entries.len(),
            trace: nominal,
            residual: true,
            fitted,
            rates: Some(rates),
        });
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

/// The whole stream along time, the whole signal across value.
fn initial_view(trace: &Trace) -> View {
    let value = trace
        .whole_signal()
        .map(|(lo, hi)| Span::fitted(lo, hi))
        .unwrap_or(Span::new(-1.0, 1.0));
    View {
        time: trace.display_span().unwrap_or(Span::new(0.0, 1.0)),
        value,
    }
}
