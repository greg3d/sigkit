//! The application: one recording open at a time, its first fixed-rate
//! channel on the plot, a status bar with the key hints. The pyramid is
//! built on a background thread so a first open of a huge recording shows
//! progress instead of a frozen window.

use crate::plot::{self, PlotState};
use crate::trace::Trace;
use crate::view::{Orientation, Span, View};
use egui::{Align, ColorImage, Context, Key, Layout, ScreenshotCallback, ViewportCommand};
use sigkit_format::{PyramidSource, Recording, StreamKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

/// What the command line asked for.
pub struct Options {
    pub recording: Option<PathBuf>,
    pub orientation: Orientation,
    /// Initial time window; the whole recording when `None`.
    pub time: Option<Span>,
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
    screenshot: Option<Screenshot>,
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
    done: Arc<AtomicUsize>,
    total: Arc<AtomicUsize>,
}

/// An open recording: the channel on the plot and where its pyramid came from.
struct Loaded {
    name: String,
    stream: String,
    channel: String,
    unit: String,
    trace: Trace,
    source: PyramidSource,
    took: Duration,
}

impl App {
    pub fn new(_cc: &eframe::CreationContext<'_>, options: Options) -> App {
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
            screenshot: options.screenshot.map(|path| Screenshot {
                path,
                frames_drawn: 0,
                requested: false,
                saved: Arc::new(AtomicBool::new(false)),
            }),
            plot_ms: 0.0,
        };
        if let Some(path) = options.recording {
            app.start_load(path);
        }
        app
    }

    /// Drive a pending `--screenshot`: wait for the plot to be drawn, ask the
    /// backend for the frame, save it, quit. Quit with the error if opening
    /// failed, so a scripted run never hangs.
    fn drive_screenshot(&mut self, ctx: &Context) {
        let Some(shot) = &mut self.screenshot else {
            return;
        };
        if self.error.is_some() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
            return;
        }
        if self.open.is_none() {
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
        let done = Arc::new(AtomicUsize::new(0));
        let total = Arc::new(AtomicUsize::new(0));
        let worker = (path.clone(), done.clone(), total.clone());
        std::thread::spawn(move || {
            let (path, done, total) = worker;
            let _ = tx.send(load(&path, &done, &total));
        });
        self.loading = Some(Loading {
            path,
            rx,
            done,
            total,
        });
        self.error = None;
    }

    fn poll_loading(&mut self, ctx: &Context) {
        let Some(loading) = &self.loading else {
            return;
        };
        match loading.rx.try_recv() {
            Ok(Ok(loaded)) => {
                self.plot.view = initial_view(&loaded.trace);
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

    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if let Some(loading) = &self.loading {
                let done = loading.done.load(Ordering::Relaxed);
                let total = loading.total.load(Ordering::Relaxed);
                let percent = (100 * done).checked_div(total).unwrap_or(0);
                ui.label(format!(
                    "building pyramid for {}  ·  {percent}%",
                    loading.path.display()
                ));
            } else if let Some(open) = &self.open {
                let source = match open.source {
                    PyramidSource::Cache => "from cache",
                    PyramidSource::Built => "built",
                };
                ui.label(format!(
                    "{}  ·  {}/{}  ·  {} samples at {} Hz  ·  pyramid {source} in {:.0} ms  ·  plot {:.1} ms",
                    open.name,
                    open.stream,
                    open.channel,
                    open.trace.len(),
                    open.trace.rate_hz(),
                    open.took.as_secs_f64() * 1e3,
                    self.plot_ms
                ));
            } else {
                ui.label("drop a .sigrec recording here, or pass it on the command line");
            }
            if let Some(error) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, error);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.weak("O orientation  ·  A autoscale visible  ·  ⇧A autoscale all  ·  pinch zoom time  ·  scroll pan");
            });
        });
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
        if ctx.input(|i| i.key_pressed(Key::O)) {
            self.plot.orientation = self.plot.orientation.toggled();
        }
        self.drive_screenshot(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::bottom("status").show(ui, |ui| self.status_bar(ui));
        egui::CentralPanel::no_frame().show(ui, |ui| match &self.open {
            Some(open) => {
                let started = Instant::now();
                plot::show(ui, &open.trace, &mut self.plot, &open.unit);
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

/// Open the recording and build or load the pyramid of its first fixed-rate
/// channel, reporting progress through `done` and `total`.
fn load(path: &Path, done: &AtomicUsize, total: &AtomicUsize) -> Result<Loaded, String> {
    let started = Instant::now();
    let rec = Recording::open(path).map_err(|e| e.to_string())?;
    let header = rec.header();
    let (stream, channel) = header
        .streams
        .iter()
        .filter(|s| s.kind == StreamKind::FixedRate)
        .find_map(|s| s.channels.first().map(|c| (s, c)))
        .ok_or_else(|| {
            format!(
                "{}: no fixed-rate channel to draw (timestamped streams come with ticket 04)",
                header.name
            )
        })?;
    let column = rec
        .map_column(&stream.id, &channel.id)
        .map_err(|e| e.to_string())?;
    let (pyramid, source) = rec
        .pyramid_with_progress(&stream.id, &channel.id, &mut |d, t| {
            done.store(d, Ordering::Relaxed);
            total.store(t, Ordering::Relaxed);
        })
        .map_err(|e| e.to_string())?;
    let rate_hz = stream.rate_hz.expect("fixed-rate stream validated at open");
    Ok(Loaded {
        name: header.name.clone(),
        stream: stream.id.clone(),
        channel: channel.id.clone(),
        unit: channel.unit.clone(),
        trace: Trace::new(column, pyramid, rate_hz, channel.gain, channel.offset),
        source,
        took: started.elapsed(),
    })
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

/// The whole recording along time, the whole signal across value.
fn initial_view(trace: &Trace) -> View {
    let value = trace
        .whole_signal()
        .map(|(lo, hi)| Span::fitted(lo, hi))
        .unwrap_or(Span::new(-1.0, 1.0));
    View {
        time: Span::new(0.0, trace.duration_s().max(1e-3)),
        value,
    }
}
