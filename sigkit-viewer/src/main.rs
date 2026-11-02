//! `sigkit-viewer`: look at `.sigrec` recordings honestly. Candles at every
//! zoom, gestures first. See `docs/specs/` and `GLOSSARY.md`.

mod app;
mod plot;
mod ticks;
mod trace;
mod view;

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "sigkit-viewer",
    version,
    about = "Look at .sigrec recordings: min/max candles at every zoom"
)]
struct Cli {
    /// A .sigrec recording to open (or drop one on the window)
    recording: Option<PathBuf>,
    /// Time runs left to right instead of top to bottom
    #[arg(long)]
    horizontal: bool,
    /// Initial time window in seconds, as `from:to` (default: the whole recording)
    #[arg(long, value_parser = parse_window, value_name = "FROM:TO")]
    time: Option<(f64, f64)>,
    /// Save a PNG of the window once the recording is drawn, then quit
    #[arg(long, hide = true, value_name = "PNG")]
    screenshot: Option<PathBuf>,
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
        recording: cli.recording,
        orientation: if cli.horizontal {
            view::Orientation::Horizontal
        } else {
            view::Orientation::Vertical
        },
        time: cli.time.map(|(from, to)| view::Span::new(from, to)),
        screenshot: cli.screenshot,
    };
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("sigkit")
            .with_inner_size([1100.0, 760.0])
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
