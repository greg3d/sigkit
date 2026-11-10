//! The application: the open recordings in a sidebar tree, their channels laid
//! out on panels by dragging, and a bar at the bottom with the mode control,
//! the cursor readout, the status line and the key hints. In Align a sidebar
//! on the right holds the active stream's corrections as fields; every
//! change is written to the recording's header at once and `Cmd+Z` takes it
//! back. The whole comparison (recordings, panels, orientation, time and
//! value spans, mode) is saved as a workspace with `Cmd+S` and reopened by
//! dropping the `.sigws.json` on the window. Pyramids are built
//! on a background thread so a first open of a huge recording shows progress
//! instead of a frozen window.

use crate::align::{Calibration, Change, Correction, Edit, History, stretch_factor};
use crate::import::{ImportSheet, Outcome};
use crate::layout::{ChannelRef, Dragged, Drop, Layout};
use crate::plot::{self, AlignDrag, Mode, PlotState, Series, Shown};
use crate::sheet::Sheet;
use crate::trace::{Clock, Placement, Trace, Values};
use crate::view::{Orientation, Span};
use crate::workspace::{self, Found, Opened, Restored, Session, Workspace};
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
/// Width of the Align sidebar.
const ALIGN_WIDTH: f32 = 250.0;

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
    /// The session's header edits, for `Cmd+Z`.
    history: History,
    /// The Align drag in progress, from where it started.
    drag: Option<DragStart>,
    /// The Align field being typed into, and what is typed so far.
    typing: Option<(Field, String)>,
    /// The workspace file this session was opened from or last saved to.
    workspace: Option<PathBuf>,
    /// What was last saved to or opened from `workspace`, to tell whether
    /// the session has changed since.
    saved: Option<Workspace>,
    /// Recordings a workspace lists that could not be opened.
    missing: Vec<Missing>,
    /// A workspace being opened: laid out once its recordings have opened.
    restoring: Option<Restoring>,
    /// The sheet asking where to save the workspace.
    sheet: Sheet<SaveSheet>,
    /// The sheet importing a raw file with no header.
    import: Sheet<ImportSheet>,
}

/// A recording a workspace lists that could not be opened: shown in the tree
/// and kept in the workspace when it is saved again.
struct Missing {
    path: PathBuf,
    reason: String,
}

/// A workspace whose recordings are opening.
struct Restoring {
    workspace: Workspace,
    /// What became of each of the workspace's recordings.
    slots: Vec<Slot>,
}

/// One of a workspace's recordings while the workspace opens.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Slot {
    Opening,
    /// Open, at this index among the open recordings.
    Open(usize),
    Missing,
    /// Listed again: the same recording as this earlier slot.
    SameAs(usize),
}

/// The sheet that asks where to save the workspace.
struct SaveSheet {
    path: String,
    /// Put the keyboard in the field on the next frame.
    focus: bool,
}

/// What the save sheet asks of the app.
enum SaveAction {
    Save(PathBuf),
    Cancel,
}

/// The save sheet: where the workspace goes, and what it would list. Enter
/// saves.
fn save_sheet(ui: &mut egui::Ui, sheet: &mut SaveSheet, session: &Session) -> Option<SaveAction> {
    let mut save = false;
    let mut cancel = false;
    ui.horizontal(|ui| {
        ui.strong("Save workspace");
        let response = ui.add(
            egui::TextEdit::singleline(&mut sheet.path)
                .desired_width(ui.available_width() - 160.0)
                .font(egui::TextStyle::Monospace),
        );
        if std::mem::take(&mut sheet.focus) {
            response.request_focus();
        }
        save |= response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
        save |= ui.button("Save  ⏎").clicked();
        cancel |= ui.button("Cancel  esc").clicked();
    });
    // What the file would list, as typed: relative inside its folder.
    let weak = ui.visuals().weak_text_color();
    let file = canonical_file(&with_extension(&sheet.path));
    for listed in Workspace::capture(&file, session).recordings {
        let how = if listed.is_absolute() {
            "absolute: outside the folder"
        } else {
            "relative: moves with the folder"
        };
        ui.label(
            egui::RichText::new(format!("{}    {how}", listed.display()))
                .small()
                .monospace()
                .color(weak),
        );
    }
    ui.label(
        egui::RichText::new("corrections and calibration stay in each recording's header")
            .small()
            .color(weak),
    );
    ui.add_space(6.0);
    if save {
        Some(SaveAction::Save(with_extension(&sheet.path)))
    } else if cancel {
        Some(SaveAction::Cancel)
    } else {
        None
    }
}

/// Where an Align drag started: the active channel's corrections then, and
/// the far ends a stretch pulls along.
struct DragStart {
    channel: ChannelRef,
    correction: Correction,
    /// `None` for a residual: it has no calibration in the header.
    calibration: Option<Calibration>,
    /// The display span end farther from the anchor.
    far_time: f64,
    /// The whole signal's extreme farther from the anchor.
    far_value: Option<f64>,
    /// What the drag has changed so far.
    edit: Option<Edit>,
}

/// A number in the Align sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Shift,
    K,
    Ppm,
    Rate,
    Gain,
    Offset,
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
    /// The recording's place in a workspace being opened.
    slot: Option<usize>,
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

/// An open recording: every channel ready to draw, and its header, which
/// Align edits.
struct Loaded {
    path: PathBuf,
    name: String,
    rec: Recording,
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

impl Loaded {
    /// The stream's time correction, as the header holds it.
    fn correction(&self, stream: &str) -> Correction {
        let tc = &self
            .rec
            .header()
            .stream(stream)
            .expect("loaded")
            .time_correction;
        Correction {
            k: tc.k,
            shift_s: tc.shift_s,
        }
    }

    /// The channel's calibration, as the header holds it.
    fn calibration(&self, stream: &str, channel: &str) -> Calibration {
        let stream = self.rec.header().stream(stream).expect("loaded");
        let channel = stream.channel(channel).expect("loaded");
        Calibration {
            gain: channel.gain,
            offset: channel.offset,
        }
    }

    /// Make `change` in the header and on every trace it moves: a time
    /// correction moves each channel of the stream and its residual. Persist
    /// with [`Loaded::save`].
    fn set(&mut self, change: &Change) {
        match change {
            Change::Time { stream, to, .. } => {
                let tc = &mut self
                    .rec
                    .header_mut()
                    .stream_mut(stream)
                    .expect("loaded")
                    .time_correction;
                tc.k = to.k;
                tc.shift_s = to.shift_s;
                for entry in self.entries.iter_mut().filter(|e| e.stream == *stream) {
                    for trace in std::iter::once(&mut entry.trace).chain(&mut entry.fitted) {
                        trace.set_placement(Placement {
                            k: to.k,
                            shift_s: to.shift_s,
                            ..trace.placement()
                        });
                    }
                }
            }
            Change::Calibration {
                stream,
                channel,
                to,
                ..
            } => {
                let header = self.rec.header_mut().stream_mut(stream).expect("loaded");
                let c = header
                    .channels
                    .iter_mut()
                    .find(|c| c.id == *channel)
                    .expect("loaded");
                c.gain = to.gain;
                c.offset = to.offset;
                for entry in &mut self.entries {
                    if entry.stream == *stream && entry.channel == *channel && !entry.residual {
                        entry.trace.set_calibration(to.gain, to.offset);
                    }
                }
            }
        }
    }

    /// Rewrite `header.json`; the columns are never touched.
    fn save(&self) -> Result<(), String> {
        self.rec
            .write_header()
            .map_err(|e| format!("{}: {e}", self.name))
    }
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
            history: History::default(),
            drag: None,
            typing: None,
            workspace: None,
            saved: None,
            missing: Vec::new(),
            restoring: None,
            sheet: Sheet::default(),
            import: Sheet::default(),
        };
        app.plot.pinned_cursor = options.cursor;
        app.open_paths(options.recordings);
        app
    }

    /// Open what was passed or dropped: a workspace in place of what is open,
    /// then the recordings, which join it.
    fn open_paths(&mut self, paths: Vec<PathBuf>) {
        let (workspaces, recordings): (Vec<PathBuf>, Vec<PathBuf>) =
            paths.into_iter().partition(|p| workspace::is_workspace(p));
        if let Some(file) = workspaces.first() {
            self.open_workspace(file);
        }
        for file in workspaces.iter().skip(1) {
            self.report(format!("{}: one workspace at a time", file.display()));
        }
        for path in recordings {
            if path.is_dir() {
                self.start_load(path, None);
            } else if let Some(dir) = sigkit_format::raw::recording_of(&path) {
                // Already imported, or a column of a recording: open that.
                self.start_load(dir, None);
            } else if self.import.is_open() {
                self.report(format!("{}: one import at a time", path.display()));
            } else {
                match ImportSheet::new(path) {
                    Ok(import) => {
                        self.sheet.close();
                        self.import.open(import);
                    }
                    Err(e) => self.report(e),
                }
            }
        }
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
        let nothing =
            self.recordings.is_empty() && self.missing.is_empty() && !self.import.is_shown();
        if nothing || !self.loading.is_empty() || self.restoring.is_some() || self.plot.is_moving()
        {
            return;
        }
        shot.frames_drawn += 1;
        // A sheet slides up for a few frames first.
        let settle = if self.import.is_shown() { 30 } else { 3 };
        if shot.frames_drawn >= settle && !shot.requested {
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

    /// Open a recording on a background thread; `slot` is its place in a
    /// workspace being opened.
    fn start_load(&mut self, path: PathBuf, slot: Option<usize>) {
        // One recording by any spelling of its path is open once.
        let path = path.canonicalize().unwrap_or(path);
        let open = self.recordings.iter().map(|r| &r.path);
        if open
            .chain(self.loading.iter().map(|l| &l.path))
            .any(|p| *p == path)
        {
            self.report(format!("{}: already open", path.display()));
            self.fill_slot(slot, Slot::Missing);
            return;
        }
        let (tx, rx) = mpsc::channel();
        let progress = Arc::new(Progress::default());
        let worker = (path.clone(), progress.clone());
        std::thread::spawn(move || {
            let (path, progress) = worker;
            let _ = tx.send(load(&path, &progress));
        });
        self.loading.push(Loading {
            path,
            slot,
            rx,
            progress,
        });
    }

    /// What became of the workspace recording `slot`.
    fn fill_slot(&mut self, slot: Option<usize>, to: Slot) {
        if let (Some(slot), Some(restoring)) = (slot, &mut self.restoring) {
            restoring.slots[slot] = to;
        }
    }

    /// A recording could not be opened, for `reason` (which names it):
    /// missing from a workspace, or said.
    fn failed(&mut self, path: PathBuf, slot: Option<usize>, reason: String) {
        if slot.is_some() {
            self.missing.push(Missing { path, reason });
            self.fill_slot(slot, Slot::Missing);
        } else {
            self.report(reason);
        }
    }

    /// Open the workspace at `file` in place of everything open now. Its
    /// recordings open in the background; the panels follow once they have.
    fn open_workspace(&mut self, file: &Path) {
        let (workspace, paths) = match read_workspace(file) {
            Ok(read) => read,
            Err(e) => return self.report(e),
        };
        self.recordings.clear();
        self.loading.clear();
        self.missing.clear();
        self.layout = Layout::default();
        self.history = History::default();
        self.refit.clear();
        self.drag = None;
        self.typing = None;
        self.sheet.close();
        self.initial_time = None;
        self.initial_channel = None;
        self.plot.active = None;
        self.plot.let_go();
        self.plot.interrupt();
        self.workspace = Some(canonical_file(file));
        self.saved = Some(workspace.clone());
        // A recording listed twice opens once; both places show it.
        let paths: Vec<PathBuf> = paths
            .into_iter()
            .map(|p| p.canonicalize().unwrap_or(p))
            .collect();
        let slots = (0..paths.len())
            .map(|i| match paths[..i].iter().position(|p| *p == paths[i]) {
                Some(first) => Slot::SameAs(first),
                None => Slot::Opening,
            })
            .collect();
        self.restoring = Some(Restoring { slots, workspace });
        for (slot, path) in paths.into_iter().enumerate() {
            if self.restoring.as_ref().expect("just set").slots[slot] == Slot::Opening {
                self.start_load(path, Some(slot));
            }
        }
    }

    /// Lay out a workspace once every one of its recordings has opened or
    /// failed to.
    fn finish_restoring(&mut self, ctx: &Context) {
        let Some(restoring) = &self.restoring else {
            return;
        };
        let Some(slots) = opened(&restoring.slots) else {
            return;
        };
        let restoring = self.restoring.take().expect("checked");
        let Restored {
            layout,
            time,
            orientation,
            mode,
            problems,
        } = restore(&restoring.workspace, &self.recordings, &slots);
        self.layout = layout;
        self.plot.time = time
            .or_else(|| {
                self.recordings
                    .first()?
                    .entries
                    .first()?
                    .trace()
                    .display_span()
            })
            .unwrap_or(Span::new(0.0, 1.0));
        self.plot.orientation = orientation;
        self.plot.mode = mode;
        self.plot.interrupt();
        for problem in problems {
            self.report(problem);
        }
        // What opened is unchanged until the user changes it, even where it
        // differs from the file (a missing recording's channels left out).
        if let Some(file) = &self.workspace {
            self.saved = Some(self.captured(file));
        }
        self.update_title(ctx);
    }

    /// The window's title: the workspace's name, or the open recordings'.
    fn update_title(&self, ctx: &Context) {
        let title = match &self.workspace {
            Some(file) => file_name(file),
            None => {
                let names: Vec<&str> = self.recordings.iter().map(|r| r.name.as_str()).collect();
                names.join(", ")
            }
        };
        ctx.send_viewport_cmd(ViewportCommand::Title(format!("{title} — sigkit")));
    }

    /// The workspace of the session as it is now, as it would be saved at
    /// `file`.
    fn captured(&self, file: &Path) -> Workspace {
        Workspace::capture(
            file,
            &session(&self.recordings, &self.missing, &self.layout, &self.plot),
        )
    }

    /// Save the workspace at `file`, which becomes the session's.
    fn save_workspace(&mut self, file: PathBuf) {
        if !self.loading.is_empty() || self.restoring.is_some() {
            return self.report("wait for the recordings to open before saving".into());
        }
        match write_workspace(
            &file,
            &self.recordings,
            &self.missing,
            &self.layout,
            &self.plot,
        ) {
            Ok(file) => {
                self.saved = Some(self.captured(&file));
                self.workspace = Some(file);
                self.sheet.close();
            }
            Err(e) => self.report(e),
        }
    }

    /// `Cmd+S`: save to the session's workspace, or ask where with the sheet.
    /// `Cmd+Shift+S` always asks.
    fn save(&mut self, ask: bool) {
        match &self.workspace {
            Some(file) if !ask => self.save_workspace(file.clone()),
            _ => {
                let path = self.workspace.clone().unwrap_or_else(|| {
                    let first = self.recordings.first().map(|r| r.path.as_path());
                    default_workspace_path(first)
                });
                self.import.close();
                self.sheet.open(SaveSheet {
                    path: path.display().to_string(),
                    focus: true,
                });
            }
        }
    }

    /// The sheets at the bottom: saving the workspace, importing a raw file.
    fn sheets(&mut self, ui: &mut egui::Ui) {
        let (recordings, missing, layout, plot) =
            (&self.recordings, &self.missing, &self.layout, &self.plot);
        let save = self
            .sheet
            .show(ui, "save sheet", |ui, sheet| {
                save_sheet(ui, sheet, &session(recordings, missing, layout, plot))
            })
            .flatten();
        match save {
            Some(SaveAction::Save(path)) => self.save_workspace(path),
            Some(SaveAction::Cancel) => self.sheet.close(),
            None => {}
        }
        let imported = self
            .import
            .show(ui, "import sheet", |ui, import| import.ui(ui))
            .flatten();
        if let Some(Outcome::Imported(dir)) = imported {
            self.import.close();
            self.start_load(dir, None);
        }
    }

    /// Take finished recordings in the order they were asked for.
    fn poll_loading(&mut self, ctx: &Context) {
        while let Some(loading) = self.loading.first() {
            match loading.rx.try_recv() {
                Ok(Ok(loaded)) => {
                    let slot = self.loading.remove(0).slot;
                    if slot.is_some() {
                        self.recordings.push(loaded);
                        self.fill_slot(slot, Slot::Open(self.recordings.len() - 1));
                    } else {
                        self.add_recording(ctx, loaded);
                    }
                }
                Ok(Err(reason)) => {
                    let Loading { path, slot, .. } = self.loading.remove(0);
                    self.failed(path, slot, reason);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    let Loading { path, slot, .. } = self.loading.remove(0);
                    let reason = format!("{}: opening failed", path.display());
                    self.failed(path, slot, reason);
                }
            }
            // Before any recording queued after the workspace's joins it.
            self.finish_restoring(ctx);
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
        self.update_title(ctx);
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
        for missing in &self.missing {
            missing_row(ui, missing, weak);
        }
        if let Some(channel) = toggled {
            let entry = &mut self.recordings[channel.recording].entries[channel.entry];
            entry.subtract_fit = !entry.subtract_fit;
            self.refit.push(channel);
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(
                "drag a channel onto the plot  ·  drop a .sigrec on the window to add it, a .sigws.json to open a workspace  ·  ⌘S saves the workspace, ⇧⌘S somewhere else",
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
            let mut text = match &self.workspace {
                Some(file) => {
                    let changed = self.saved.as_ref() != Some(&self.captured(file));
                    let state = if changed {
                        "changed since saved"
                    } else {
                        "saved"
                    };
                    format!("{} ({state})  ·  ", file_name(file))
                }
                None => String::new(),
            };
            text += &format!(
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
        } else if self.missing.is_empty() {
            ui.weak("drop a .sigrec recording or a .sigws.json workspace here, or pass it on the command line");
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }

    /// Make `edit` on screen and write the header.
    fn write(&mut self, edit: &Edit) {
        let loaded = &mut self.recordings[edit.recording];
        loaded.set(&edit.change);
        if let Err(e) = loaded.save() {
            self.report(e);
        }
    }

    /// Make `edit`, write the header and remember it for `Cmd+Z`.
    fn commit(&mut self, edit: Edit) {
        if edit.changes_nothing() {
            return;
        }
        self.write(&edit);
        self.history.push(edit);
    }

    /// Take back the latest change, on screen and on disk. A drag in
    /// progress is a change of its own: it ends here and is what is undone.
    fn undo(&mut self) {
        self.end_drag();
        self.plot.let_go();
        if let Some(edit) = self.history.undo() {
            self.write(&edit);
        }
    }

    /// The edit that sets the active channel's stream's time correction.
    fn time_edit(&self, channel: ChannelRef, from: Correction, to: Correction) -> Edit {
        Edit {
            recording: channel.recording,
            change: Change::Time {
                stream: self.entry(channel).stream.clone(),
                from,
                to,
            },
        }
    }

    /// The edit that sets the channel's calibration.
    fn calibration_edit(&self, channel: ChannelRef, from: Calibration, to: Calibration) -> Edit {
        let entry = self.entry(channel);
        Edit {
            recording: channel.recording,
            change: Change::Calibration {
                stream: entry.stream.clone(),
                channel: entry.channel.clone(),
                from,
                to,
            },
        }
    }

    /// The channel's stream's time correction and, unless it is a residual,
    /// its calibration.
    fn corrections(&self, channel: ChannelRef) -> (Correction, Option<Calibration>) {
        let (loaded, entry) = (&self.recordings[channel.recording], self.entry(channel));
        let calibration =
            (!entry.residual).then(|| loaded.calibration(&entry.stream, &entry.channel));
        (loaded.correction(&entry.stream), calibration)
    }

    /// Follow an Align drag of the active channel: move it live, and on
    /// release write the header once and make the drag one undo step.
    fn align_drag(&mut self, drag: AlignDrag) {
        let Some(channel) = self.plot.active else {
            return;
        };
        if drag.started || self.drag.as_ref().is_none_or(|d| d.channel != channel) {
            self.end_drag();
            let (correction, calibration) = self.corrections(channel);
            let trace = self.entry(channel).trace();
            let farther = |anchor: f64, (a, b): (f64, f64)| {
                if (a - anchor).abs() >= (b - anchor).abs() {
                    a
                } else {
                    b
                }
            };
            let span = trace.display_span().map_or((0.0, 0.0), |s| (s.from, s.to));
            self.drag = Some(DragStart {
                channel,
                correction,
                calibration,
                far_time: farther(drag.anchor.0, span),
                far_value: trace
                    .whole_signal()
                    .map(|range| farther(drag.anchor.1, range)),
                edit: None,
            });
        }
        let start = self.drag.as_ref().expect("just started");
        let (anchor_t, anchor_v) = drag.anchor;
        let (dt, dv) = drag.moved;
        let edit = if dt != 0.0 {
            let to = if drag.stretch {
                stretch_factor(anchor_t, start.far_time, dt)
                    .map(|f| start.correction.stretched(anchor_t, f))
            } else {
                Some(start.correction.shifted(dt))
            };
            to.map(|to| self.time_edit(channel, start.correction, to))
        } else if dv != 0.0
            && let Some(from) = start.calibration
        {
            let to = if drag.stretch {
                start
                    .far_value
                    .and_then(|far| stretch_factor(anchor_v, far, dv))
                    .map(|f| from.stretched(anchor_v, f))
            } else {
                Some(from.offset_by(dv))
            };
            to.map(|to| self.calibration_edit(channel, from, to))
        } else {
            None
        };
        if let Some(edit) = edit {
            self.recordings[channel.recording].set(&edit.change);
            self.drag.as_mut().expect("just started").edit = Some(edit);
        }
        if drag.released {
            self.end_drag();
        }
    }

    /// A drag that is over: write what it changed and make it one step.
    fn end_drag(&mut self) {
        if let Some(DragStart {
            edit: Some(edit), ..
        }) = self.drag.take()
        {
            let loaded = &self.recordings[edit.recording];
            if let Err(e) = loaded.save() {
                self.report(e);
            }
            self.history.push(edit);
        }
    }

    /// The arrows in Align: move the active stream along time and its
    /// channel across value, each a change of its own.
    fn nudge(&mut self, (dt, dv): (f64, f64)) {
        let Some(channel) = self.plot.active else {
            return;
        };
        self.end_drag();
        let (correction, calibration) = self.corrections(channel);
        if dt != 0.0 {
            self.commit(self.time_edit(channel, correction, correction.shifted(dt)));
        }
        if dv != 0.0
            && let Some(calibration) = calibration
        {
            let to = calibration.offset_by(dv);
            self.commit(self.calibration_edit(channel, calibration, to));
        }
    }

    /// The Align sidebar: the active channel's shift, `k` (also as ppm and as
    /// the corrected rate), gain and offset. Each takes a typed value on
    /// Enter or when it loses focus; Escape leaves it as it was.
    fn align_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.strong("Align");
        ui.add_space(4.0);
        let weak = ui.visuals().weak_text_color();
        let Some(channel) = self.plot.active else {
            ui.label(
                egui::RichText::new("click a legend on the plot to pick the stream to align")
                    .small()
                    .color(weak),
            );
            return;
        };
        let dark = ui.visuals().dark_mode;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
            ui.painter().hline(
                rect.x_range(),
                rect.center().y,
                Stroke::new(2.0, self.color(channel, dark)),
            );
            ui.label(self.label(channel));
        });
        let entry = self.entry(channel);
        let (correction, calibration) = self.corrections(channel);
        // The rate the stream's samples arrive at on its own clock: fitted
        // when there is a fit.
        let base_hz = match entry.rates {
            Some(Rates { fit: Some(fit), .. }) => fit.rate_hz,
            Some(rates) => rates.nominal_hz,
            None => entry.trace.rate_hz(),
        };
        let stream = entry.stream.clone();
        let channel_id = entry.channel.clone();

        let mut time = None;
        let mut values = None;
        ui.add_space(8.0);
        section(ui, &format!("time · {stream}"), weak);
        egui::Grid::new("time fields")
            .num_columns(3)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                let typing = &mut self.typing;
                let c = correction;
                if let Some(v) = field(
                    ui,
                    typing,
                    Field::Shift,
                    "shift",
                    "s",
                    format!("{:.6}", c.shift_s),
                ) {
                    time = Some(Correction { shift_s: v, ..c });
                }
                if let Some(v) = field(ui, typing, Field::K, "k", "", format!("{:.9}", c.k)) {
                    time = (v > 0.0).then_some(Correction { k: v, ..c });
                }
                let ppm = (c.k - 1.0) * 1e6;
                if let Some(v) = field(ui, typing, Field::Ppm, "", "ppm", format!("{ppm:+.3}")) {
                    let k = 1.0 + v * 1e-6;
                    time = (k > 0.0).then_some(Correction { k, ..c });
                }
                let rate = base_hz / c.k;
                if let Some(v) = field(ui, typing, Field::Rate, "rate", "Hz", format!("{rate:.6}"))
                {
                    time = (v > 0.0).then_some(Correction {
                        k: base_hz / v,
                        ..c
                    });
                }
            });
        ui.label(
            egui::RichText::new(format!("rate is {base_hz:.4} Hz on its own clock ÷ k"))
                .small()
                .color(weak),
        );
        ui.add_space(8.0);
        section(ui, &format!("calibration · {channel_id}"), weak);
        match calibration {
            Some(c) => {
                egui::Grid::new("value fields")
                    .num_columns(3)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        let typing = &mut self.typing;
                        if let Some(v) = field(ui, typing, Field::Gain, "gain", "", plain(c.gain)) {
                            values = Some(Calibration { gain: v, ..c });
                        }
                        if let Some(v) =
                            field(ui, typing, Field::Offset, "offset", "", plain(c.offset))
                        {
                            values = Some(Calibration { offset: v, ..c });
                        }
                    });
            }
            None => {
                ui.label(
                    egui::RichText::new("a residual has no calibration")
                        .small()
                        .color(weak),
                );
            }
        }
        if let Some(to) = time {
            self.end_drag();
            self.commit(self.time_edit(channel, correction, to));
        }
        if let (Some(to), Some(from)) = (values, calibration) {
            self.end_drag();
            self.commit(self.calibration_edit(channel, from, to));
        }
        ui.add_space(12.0);
        ui.label(
            egui::RichText::new("saved to the recording's header.json on every change  ·  ⌘Z undo")
                .small()
                .color(weak),
        );
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
        self.open_paths(dropped);
        // Keys typed into a field are the field's.
        if !ctx.egui_wants_keyboard_input() {
            let save = ctx.input(|i| {
                (i.modifiers.command && i.key_pressed(Key::S)).then_some(i.modifiers.shift)
            });
            if let Some(ask) = save {
                self.save(ask);
            }
            let undo = ctx.input(|i| {
                if i.key_pressed(Key::O) {
                    self.plot.orientation = self.plot.orientation.toggled();
                }
                for mode in Mode::ALL {
                    if i.key_pressed(mode.key()) {
                        self.plot.mode = mode;
                    }
                }
                i.modifiers.command && !i.modifiers.shift && i.key_pressed(Key::Z)
            });
            if undo {
                self.undo();
            }
        }
        if self.plot.mode != Mode::Align {
            self.end_drag();
        }
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
        self.sheets(ui);
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
        if self.plot.mode == Mode::Align {
            egui::Panel::right("align")
                .resizable(false)
                .exact_size(ALIGN_WIDTH)
                .show(ui, |ui| self.align_panel(ui));
        }
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
        if let Some(channel) = shown.clicked {
            self.end_drag();
            self.plot.active = Some(channel);
        }
        if let Some(drag) = shown.align {
            self.align_drag(drag);
            ui.ctx().request_repaint();
        }
        if let Some(nudge) = shown.nudge {
            self.nudge(nudge);
        }
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

/// `v` to 15 significant digits and no trailing zeros: what was typed reads
/// back as typed, without the noise of the arithmetic behind it.
fn plain(v: f64) -> String {
    let v: f64 = format!("{v:.14e}").parse().unwrap_or(v);
    format!("{}", if v == 0.0 { 0.0 } else { v })
}

/// A heading in the Align sidebar.
fn section(ui: &mut egui::Ui, text: &str, weak: Color32) {
    ui.label(egui::RichText::new(text).small().color(weak));
}

/// One number in the Align sidebar, as a row of a three-column grid: label,
/// text field, unit. Shows `shown` until typed into; returns the typed value
/// once it is committed with Enter or by leaving the field, unless it is
/// unchanged, not a finite number, or Escape left it.
fn field(
    ui: &mut egui::Ui,
    typing: &mut Option<(Field, String)>,
    which: Field,
    label: &str,
    unit: &str,
    shown: String,
) -> Option<f64> {
    ui.label(label);
    let mut text = match typing {
        Some((field, text)) if *field == which => text.clone(),
        _ => shown.clone(),
    };
    let response = ui.add(
        egui::TextEdit::singleline(&mut text)
            .desired_width(130.0)
            .font(egui::TextStyle::Monospace),
    );
    ui.label(unit);
    ui.end_row();
    if response.has_focus() {
        *typing = Some((which, text));
        return None;
    }
    if !response.lost_focus() {
        return None;
    }
    *typing = None;
    if ui.input(|i| i.key_pressed(Key::Escape)) || text == shown {
        return None;
    }
    text.trim().parse::<f64>().ok().filter(|v| v.is_finite())
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
            "drag a channel onto a panel to overlay, to an edge for a new panel, a legend back to the tree to remove  ·  pinch or +/− zoom time  ·  ⇧ value  ·  scroll pans time  ·  ⇧scroll value  ·  drag or arrows pan  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
        }
        Mode::Align => {
            "click a legend to pick a stream  ·  drag along time shifts it, ⇧ stretches it about the point pressed  ·  drag across value offsets, ⇧ scales  ·  arrows nudge a pixel, ⇧ ten  ·  ⌘Z undo  ·  pinch or +/− zoom time  ·  ⇧ value  ·  scroll pans time  ·  ⇧scroll value  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
        }
        Mode::Box => {
            "drag a box to fit it  ·  pinch or +/− zoom time  ·  ⇧ value  ·  scroll pans time  ·  ⇧scroll value  ·  arrows pan  ·  A or double-click fit  ·  ⇧A or double-click legend fit all  ·  O orientation"
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
    let (mut entries, mut problems, mut built) = (Vec::new(), Vec::new(), 0);
    for stream in &header.streams {
        if let Err(e) = load_stream(&rec, stream, &mut entries, &mut built, progress) {
            problems.push(format!("{}: {e}", stream.id));
        }
    }
    if entries.is_empty() {
        let mut message = format!("{}: no channel to draw", header.name);
        for problem in &problems {
            message.push_str("; ");
            message.push_str(problem);
        }
        return Err(message);
    }
    Ok(Loaded {
        path: path.to_path_buf(),
        name: header.name.clone(),
        rec,
        entries,
        problems,
        built,
        took: started.elapsed(),
    })
}

/// Add a stream's channels, and its residual when it is timestamped.
fn load_stream(
    rec: &Recording,
    stream: &Stream,
    entries: &mut Vec<Entry>,
    built: &mut usize,
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
        *built += (source == PyramidSource::Built) as usize;
        next_step();
        let trace = Trace::new(
            Values::Column(rec.map_column(&stream.id, &channel.id)?),
            clock()?,
            pyramid,
        )
        .calibrated(channel.gain, channel.offset)
        .placed(placement)
        .anchored(stream.utc_anchor.clone());
        entries.push(entry(&channel.id, &channel.unit, trace, false, None));
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
        *built += 1 + fitted.is_some() as usize;
        entries.push(entry(RESIDUAL, "ms", nominal, true, fitted));
    }
    Ok(())
}

/// Each workspace recording's index among the open recordings, `None` when
/// it is missing; `None` overall while any is still opening.
fn opened(slots: &[Slot]) -> Option<Vec<Option<usize>>> {
    slots
        .iter()
        .map(|&slot| match slot {
            Slot::Opening => None,
            Slot::Open(index) => Some(Some(index)),
            Slot::Missing => Some(None),
            Slot::SameAs(first) => match slots[first] {
                Slot::Opening => None,
                Slot::Open(index) => Some(Some(index)),
                _ => Some(None),
            },
        })
        .collect()
}

/// A recording a workspace lists that could not be opened: its name, marked
/// missing, and why on hover.
fn missing_row(ui: &mut egui::Ui, missing: &Missing, weak: Color32) {
    let response = ui
        .horizontal(|ui| {
            ui.label(
                egui::RichText::new(file_name(&missing.path))
                    .strong()
                    .color(weak),
            );
            ui.label(
                egui::RichText::new("missing")
                    .small()
                    .color(ui.visuals().error_fg_color),
            );
        })
        .response;
    response.on_hover_text(format!("{}\n{}", missing.path.display(), missing.reason));
}

/// A path's last component, for showing.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// A typed workspace path, ending in the workspace extension.
fn with_extension(typed: &str) -> PathBuf {
    let mut path = typed.trim().to_string();
    if !path.ends_with(workspace::EXTENSION) {
        path.push_str(workspace::EXTENSION);
    }
    PathBuf::from(path)
}

/// `file` with its folder spelt canonically, so recording paths (canonical
/// once opened) can be told to be inside it.
fn canonical_file(file: &Path) -> PathBuf {
    let dir = match file.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    match (dir.canonicalize(), file.file_name()) {
        (Ok(dir), Some(name)) => dir.join(name),
        _ => file.to_path_buf(),
    }
}

/// Where the save sheet suggests a new workspace goes: beside the first
/// recording, named after it.
fn default_workspace_path(first: Option<&Path>) -> PathBuf {
    let Some(first) = first else {
        return PathBuf::from(format!("workspace{}", workspace::EXTENSION));
    };
    let stem = first
        .file_stem()
        .map_or_else(|| "workspace".into(), |s| s.to_string_lossy().into_owned());
    first.with_file_name(format!("{stem}{}", workspace::EXTENSION))
}

/// What a workspace is taken from.
fn session<'a>(
    recordings: &'a [Loaded],
    missing: &'a [Missing],
    layout: &'a Layout,
    plot: &PlotState,
) -> Session<'a> {
    Session {
        opened: recordings
            .iter()
            .map(|r| Opened {
                path: &r.path,
                entries: entry_ids(r),
            })
            .collect(),
        missing: missing.iter().map(|m| m.path.as_path()).collect(),
        layout,
        time: plot.time,
        orientation: plot.orientation,
        mode: plot.mode,
    }
}

/// A recording's entries as `(stream, channel)`.
fn entry_ids(loaded: &Loaded) -> Vec<(&str, &str)> {
    loaded
        .entries
        .iter()
        .map(|e| (e.stream.as_str(), e.channel.as_str()))
        .collect()
}

/// Write the workspace of the session at `file`, atomically; the file as
/// the session now knows it.
fn write_workspace(
    file: &Path,
    recordings: &[Loaded],
    missing: &[Missing],
    layout: &Layout,
    plot: &PlotState,
) -> Result<PathBuf, String> {
    let file = canonical_file(file);
    let workspace = Workspace::capture(&file, &session(recordings, missing, layout, plot));
    sigkit_format::write_atomic(&file, workspace.to_json().as_bytes())
        .map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(file)
}

/// Read the workspace at `file`, and where each of its recordings is.
fn read_workspace(file: &Path) -> Result<(Workspace, Vec<PathBuf>), String> {
    let said = |e: String| format!("{}: {e}", file.display());
    let text = std::fs::read_to_string(file).map_err(|e| said(e.to_string()))?;
    let workspace = Workspace::parse(&text).map_err(said)?;
    let paths = workspace.recording_paths(&canonical_file(file));
    Ok((workspace, paths))
}

/// Lay `workspace` onto the recordings that opened: `slots[i]` is the index
/// among `recordings` of its `i`-th recording, `None` when it is missing. A
/// panel without a value span kept is fitted to its channels in the time
/// shown.
fn restore(workspace: &Workspace, recordings: &[Loaded], slots: &[Option<usize>]) -> Restored {
    let found: Vec<Option<Found>> = slots
        .iter()
        .map(|slot| {
            slot.map(|index| Found {
                index,
                entries: entry_ids(&recordings[index]),
            })
        })
        .collect();
    let time = workspace.time();
    workspace.restore(&found, |channels| {
        let traces = channels.iter().map(|&c| entry(recordings, c).trace());
        plot::fitted_value(traces.clone(), time)
            .or_else(|| plot::fitted_value(traces, None))
            .unwrap_or(Span::new(-1.0, 1.0))
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align::{Calibration, Change, Correction, Edit};
    use sigkit_format::synth::{self, PairSpec, Signal, TimestampedSpec};
    use std::time::SystemTime;

    fn opened(dir: &Path) -> Loaded {
        load(dir, &Progress::default()).unwrap()
    }

    /// Every column file's name, bytes and modification time.
    fn columns(dir: &Path) -> Vec<(String, Vec<u8>, SystemTime)> {
        let mut out: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|x| x == "bin"))
            .map(|p| {
                let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                (name, std::fs::read(&p).unwrap(), mtime)
            })
            .collect();
        out.sort();
        out
    }

    fn on_disk(dir: &Path) -> (f64, f64, f64, f64) {
        let rec = Recording::open(dir).unwrap();
        let stream = rec.header().stream("signal").unwrap();
        let tc = &stream.time_correction;
        let ch = &stream.channels[0];
        (tc.k, tc.shift_s, ch.gain, ch.offset)
    }

    #[test]
    fn an_align_change_moves_the_whole_stream_and_rewrites_the_header_and_nothing_else() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("t.sigrec");
        synth::timestamped(&TimestampedSpec {
            name: "t".into(),
            rate_nominal_hz: 100.0,
            duration_s: 10.0,
            jitter_s: 0.0,
            drop_fraction: 0.0,
            ppm: 0.0,
            signal: Signal::default(),
            seed: 1,
        })
        .write(&dir)
        .unwrap();
        let mut loaded = opened(&dir);
        let before = columns(&dir);
        // The channel and the stream's residual.
        assert_eq!(loaded.entries.len(), 2);
        let span = |l: &Loaded, e: usize| l.entries[e].trace().display_span().unwrap();
        assert_eq!(span(&loaded, 1).from, 0.0);
        let first = loaded.entries[0].trace().nearest(0.0).unwrap().value;

        let shift = Edit {
            recording: 0,
            change: Change::Time {
                stream: "signal".into(),
                from: Correction {
                    k: 1.0,
                    shift_s: 0.0,
                },
                to: Correction {
                    k: 2.0,
                    shift_s: 0.25,
                },
            },
        };
        let gain = Edit {
            recording: 0,
            change: Change::Calibration {
                stream: "signal".into(),
                channel: "value".into(),
                from: Calibration {
                    gain: 1.0,
                    offset: 0.0,
                },
                to: Calibration {
                    gain: -3.0,
                    offset: 0.5,
                },
            },
        };
        for edit in [&shift, &gain] {
            loaded.set(&edit.change);
            loaded.save().unwrap();
        }
        assert_eq!(on_disk(&dir), (2.0, 0.25, -3.0, 0.5));
        assert_eq!(
            loaded.correction("signal"),
            Correction {
                k: 2.0,
                shift_s: 0.25
            }
        );
        assert_eq!(
            loaded.calibration("signal", "value"),
            Calibration {
                gain: -3.0,
                offset: 0.5
            }
        );
        for e in 0..2 {
            assert_eq!(span(&loaded, e).from, 0.25, "entry {e}");
            assert!((span(&loaded, e).to - 20.25).abs() < 1e-9, "entry {e}");
        }
        let reading = loaded.entries[0].trace().nearest(0.25).unwrap();
        assert_eq!((reading.index, reading.value), (0, -3.0 * first + 0.5));
        assert_eq!(columns(&dir), before, "column bytes and mtimes");

        for (edit, expected) in [
            (&gain, (2.0, 0.25, 1.0, 0.0)),
            (&shift, (1.0, 0.0, 1.0, 0.0)),
        ] {
            loaded.set(&edit.reversed().change);
            loaded.save().unwrap();
            assert_eq!(on_disk(&dir), expected);
        }
        assert_eq!(span(&loaded, 0).from, 0.0);
        assert_eq!(columns(&dir), before, "column bytes and mtimes");
    }

    /// A layout's panels as their channels' labels and value spans.
    fn laid_out(recordings: &[Loaded], layout: &Layout) -> Vec<(Vec<String>, Span)> {
        layout
            .panels()
            .iter()
            .map(|p| {
                let labels = p.channels.iter().map(|&c| label(recordings, c).into());
                (labels.collect(), p.value)
            })
            .collect()
    }

    #[test]
    fn a_saved_workspace_reopens_as_left_after_its_folder_moves_and_without_a_missing_recording() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        let pair = synth::pair(&PairSpec {
            name: "pair".into(),
            rate_nominal_hz: 100.0,
            duration_s: 10.0,
            jitter_s: 0.0,
            drop_fraction: 0.0,
            ppm_a: 0.0,
            ppm_b: 100.0,
            shift_s: 0.25,
            signal: Signal::default(),
            seed: 1,
        });
        let folder = root.join("w");
        let paths = ["a", "b"].map(|n| folder.join("data").join(format!("pair-{n}.sigrec")));
        for (recording, path) in pair.iter().zip(&paths) {
            recording.write(path).unwrap();
        }
        let recordings: Vec<Loaded> = paths.iter().map(|p| opened(p)).collect();
        // a's value and b's value overlaid, a's residual alone; entry 1 is a
        // residual.
        let ch = |recording, entry| ChannelRef { recording, entry };
        let mut layout = Layout::default();
        let from_tree = |channel| Dragged {
            channel,
            from: None,
        };
        layout.drop(from_tree(ch(0, 0)), Drop::NewPanel(0), Span::new(-2.0, 2.0));
        layout.drop(from_tree(ch(1, 0)), Drop::Overlay(0), Span::new(-2.0, 2.0));
        layout.drop(from_tree(ch(0, 1)), Drop::NewPanel(1), Span::new(-0.1, 0.1));
        let mut plot = PlotState::new(Span::new(1.0, 3.0), Orientation::Horizontal);
        plot.mode = Mode::Align;
        let before = laid_out(&recordings, &layout);
        let headers = paths
            .clone()
            .map(|p| std::fs::read(p.join("header.json")).unwrap());

        let file = folder.join("pair.sigws.json");
        write_workspace(&file, &recordings, &[], &layout, &plot).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains(r#""data/pair-a.sigrec""#), "{text}");
        for header_fact in ["gain", "offset", "\"k\"", "shift", "rate", "unit"] {
            assert!(!text.contains(header_fact), "{header_fact} in {text}");
        }
        for (path, header) in paths.iter().zip(&headers) {
            assert_eq!(&std::fs::read(path.join("header.json")).unwrap(), header);
        }

        // The folder moves as a whole; the workspace still finds everything.
        let moved = root.join("moved");
        std::fs::rename(&folder, &moved).unwrap();
        let file = moved.join("pair.sigws.json");
        let reopen = |file: &Path| {
            let (ws, paths) = read_workspace(file).unwrap();
            let mut recordings = Vec::new();
            let mut missing = Vec::new();
            let mut slots = Vec::new();
            for path in paths {
                match load(&path, &Progress::default()) {
                    Ok(loaded) => {
                        slots.push(Some(recordings.len()));
                        recordings.push(loaded);
                    }
                    Err(reason) => {
                        slots.push(None);
                        missing.push(Missing { path, reason });
                    }
                }
            }
            let restored = restore(&ws, &recordings, &slots);
            (recordings, missing, restored)
        };
        let (recordings, missing, restored) = reopen(&file);
        assert!(missing.is_empty());
        assert_eq!(laid_out(&recordings, &restored.layout), before);
        assert_eq!(restored.time, Some(Span::new(1.0, 3.0)));
        assert_eq!(restored.orientation, Orientation::Horizontal);
        assert_eq!(restored.mode, Mode::Align);
        assert!(restored.problems.is_empty(), "{:?}", restored.problems);

        // Without b, a's channels come back, b is missing and a save keeps it.
        std::fs::remove_dir_all(moved.join("data/pair-b.sigrec")).unwrap();
        let (recordings, missing, restored) = reopen(&file);
        assert_eq!(recordings.len(), 1);
        assert_eq!(missing.len(), 1);
        assert_eq!(missing[0].path, moved.join("data/pair-b.sigrec"));
        assert_eq!(
            laid_out(&recordings, &restored.layout),
            vec![
                (vec!["signal/value".to_string()], Span::new(-2.0, 2.0)),
                (vec!["signal/residual".to_string()], Span::new(-0.1, 0.1)),
            ]
        );
        write_workspace(&file, &recordings, &missing, &restored.layout, &plot).unwrap();
        let (ws, _) = read_workspace(&file).unwrap();
        let listed: Vec<PathBuf> = vec!["data/pair-a.sigrec".into(), "data/pair-b.sigrec".into()];
        assert_eq!(ws.recordings, listed);
    }

    #[test]
    fn the_pair_lines_up_when_k_and_shift_are_the_injected_drift_and_shift() {
        let tmp = tempfile::tempdir().unwrap();
        let (ppm_a, ppm_b, shift_s) = (-500.0, 2000.0, 0.25);
        let pair = synth::pair(&PairSpec {
            name: "pair".into(),
            rate_nominal_hz: 416.0,
            duration_s: 10.0,
            jitter_s: 0.0,
            drop_fraction: 0.0,
            ppm_a,
            ppm_b,
            shift_s,
            signal: Signal {
                noise_sd: 0.0,
                ..Signal::default()
            },
            seed: 1,
        });
        let [a, b] = ["a", "b"].map(|name| tmp.path().join(format!("{name}.sigrec")));
        pair[0].write(&a).unwrap();
        pair[1].write(&b).unwrap();
        let (a, mut b) = (opened(&a), opened(&b));

        // Most a and b disagree on one display instant, nearest sample
        // against nearest sample.
        let worst = |b: &Loaded| {
            (1..19)
                .map(|i| {
                    let t = 0.5 * i as f64;
                    let value = |l: &Loaded| l.entries[0].trace().nearest(t).unwrap().value;
                    (value(&a) - value(b)).abs()
                })
                .fold(0.0, f64::max)
        };
        // A clock running `ppm` fast stamps `1 + ppm · 1e-6` times too few
        // seconds per physical second; b starts `shift_s` physical seconds
        // after a.
        let scale = |ppm: f64| 1.0 / (1.0 + ppm * 1e-6);
        let truth = Correction {
            k: scale(ppm_a) / scale(ppm_b),
            shift_s: shift_s * scale(ppm_a),
        };
        let mut set = |to: Correction| {
            let from = b.correction("signal");
            b.set(&Change::Time {
                stream: "signal".into(),
                from,
                to,
            });
            worst(&b)
        };
        // Two samples' worth of a 1 Hz unit sine is about 0.03.
        assert!(set(truth) < 0.03, "aligned: {}", set(truth));
        let shift_only = truth.stretched(truth.shift_s, 1.0 / truth.k);
        assert!(set(shift_only) > 0.1, "drift left in: {}", set(shift_only));
        assert!(
            set(Correction {
                k: 1.0,
                shift_s: 0.0
            }) > 0.5
        );
    }
}
