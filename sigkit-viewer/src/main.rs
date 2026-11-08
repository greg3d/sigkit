//! `sigkit-viewer`: look at `.sigrec` recordings honestly. Candles at every
//! zoom, gestures first. See `docs/specs/` and `GLOSSARY.md`.

mod align;
mod app;
mod layout;
mod motion;
mod plot;
mod ticks;
mod trace;
mod view;
mod workspace;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sigkit-viewer",
    version,
    about = "Look at .sigrec recordings: min/max candles at every zoom"
)]
struct Cli {
    /// .sigrec recordings to open, side by side (or drop them on the window)
    recordings: Vec<PathBuf>,
    /// Time runs left to right instead of top to bottom
    #[arg(long)]
    horizontal: bool,
    /// Initial time window in seconds, as `from:to` (default: the whole recording)
    #[arg(long, value_parser = parse_window, value_name = "FROM:TO")]
    time: Option<(f64, f64)>,
    /// The channel of the first recording to draw first, as `stream/channel`
    /// (`stream/residual` for a timestamped stream's residual)
    #[arg(long, value_name = "STREAM/CHANNEL")]
    channel: Option<String>,
    /// Draw a residual against the fitted rate rather than the nominal one
    #[arg(long, hide = true)]
    subtract_fit: bool,
    /// Save a PNG of the window once the recordings are drawn, then quit
    #[arg(long, hide = true, value_name = "PNG")]
    screenshot: Option<PathBuf>,
    /// Show the cursor at this display time, in seconds, while the pointer is
    /// off the plot
    #[arg(long, hide = true, value_name = "SECONDS")]
    cursor: Option<f64>,
}

fn parse_window(text: &str) -> Result<(f64, f64), String> {
    let (from, to) = text
        .split_once(':')
        .ok_or_else(|| "expected `from:to` in seconds".to_string())?;
    let parse = |s: &str| s.trim().parse::<f64>().map_err(|e| format!("{s:?}: {e}"));
    let (from, to) = (parse(from)?, parse(to)?);
    if !(from.is_finite() && to.is_finite()) || to <= from {
        return Err(format!("`to` must be greater than `from`, got {from}:{to}"));
    }
    Ok((from, to))
}

fn main() -> eframe::Result {
    let cli = Cli::parse();
    let options = app::Options {
        recordings: cli.recordings,
        orientation: if cli.horizontal {
            view::Orientation::Horizontal
        } else {
            view::Orientation::Vertical
        },
        time: cli.time.map(|(from, to)| view::Span::new(from, to)),
        channel: cli.channel,
        subtract_fit: cli.subtract_fit,
        screenshot: cli.screenshot,
        cursor: cli.cursor,
    };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sigkit")
            .with_inner_size([1280.0, 800.0])
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "sigkit-viewer",
        native,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, options)))),
    )
}
