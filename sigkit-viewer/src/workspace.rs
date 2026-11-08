//! The workspace: a `.sigws.json` file holding what belongs to no single
//! recording: which recordings are open together, the panels and their
//! channels, orientation, the view ranges and the mode. Saved explicitly.
//! Nothing a recording's header holds (corrections, calibration) is in it.

use crate::layout::{ChannelRef, Dragged, Drop, Layout};
use crate::plot::Mode;
use crate::view::{Orientation, Span};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The format version this viewer reads and writes.
pub const VERSION: u32 = 1;

/// A workspace as the file holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub sigws: u32,
    /// Recording paths, relative to the workspace's folder when inside it.
    pub recordings: Vec<PathBuf>,
    #[serde(default)]
    pub orientation: Orientation,
    #[serde(default)]
    pub mode: Mode,
    /// The time every panel shows; the whole first recording when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub view: Option<TimeView>,
    #[serde(default)]
    pub panels: Vec<PanelSpec>,
}

/// The display time span every panel shows, in seconds.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TimeView {
    pub time_from_s: f64,
    pub time_to_s: f64,
}

/// One panel: its channels overlaid in order and its value span, when kept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PanelSpec {
    pub channels: Vec<ChannelSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_from: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_to: Option<f64>,
}

/// A channel of one of the workspace's recordings, by the recording's place
/// in `recordings` and the ids its header gives (`residual` for a stream's
/// residual).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelSpec {
    pub recording: usize,
    pub stream: String,
    pub channel: String,
}

/// Whether `path` names a workspace file rather than a recording.
pub fn is_workspace(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n.to_string_lossy().ends_with(EXTENSION))
}

/// What a workspace file's name ends with.
pub const EXTENSION: &str = ".sigws.json";

/// What a workspace is taken from: the viewer as it is.
pub struct Session<'a> {
    /// The open recordings, in the tree's order.
    pub opened: Vec<Opened<'a>>,
    /// Recordings a workspace listed that could not be opened, kept so a
    /// save does not drop them.
    pub missing: Vec<&'a Path>,
    pub layout: &'a Layout,
    pub time: Span,
    pub orientation: Orientation,
    pub mode: Mode,
}

/// An open recording: where it is, and each of its entries (the channels a
/// `ChannelRef` counts, residuals included) as `(stream, channel)`.
pub struct Opened<'a> {
    pub path: &'a Path,
    pub entries: Vec<(&'a str, &'a str)>,
}

/// One of the workspace's recordings as it opened: its index among the open
/// recordings and its entries as `(stream, channel)`.
pub struct Found<'a> {
    pub index: usize,
    pub entries: Vec<(&'a str, &'a str)>,
}

/// A workspace laid onto the recordings that opened.
pub struct Restored {
    pub layout: Layout,
    /// The time to show; `None` when the file keeps none worth showing.
    pub time: Option<Span>,
    pub orientation: Orientation,
    pub mode: Mode,
    /// Channels the file names that none of its recordings has.
    pub problems: Vec<String>,
}

impl Workspace {
    /// The workspace of `session`, to be saved at `file`: the open
    /// recordings, then the missing ones.
    pub fn capture(file: &Path, session: &Session) -> Workspace {
        let dir = folder(file);
        let recordings = session
            .opened
            .iter()
            .map(|o| o.path)
            .chain(session.missing.iter().copied())
            .map(|path| path.strip_prefix(dir).unwrap_or(path).to_path_buf())
            .collect();
        Workspace {
            sigws: VERSION,
            recordings,
            orientation: session.orientation,
            mode: session.mode,
            view: Some(TimeView {
                time_from_s: session.time.from,
                time_to_s: session.time.to,
            }),
            panels: session
                .layout
                .panels()
                .iter()
                .map(|panel| PanelSpec {
                    channels: panel
                        .channels
                        .iter()
                        .map(|c| {
                            let (stream, channel) = session.opened[c.recording].entries[c.entry];
                            ChannelSpec {
                                recording: c.recording,
                                stream: stream.into(),
                                channel: channel.into(),
                            }
                        })
                        .collect(),
                    value_from: Some(panel.value.from),
                    value_to: Some(panel.value.to),
                })
                .collect(),
        }
    }

    /// Lay the panels onto what opened: `found[i]` is the file's `i`-th
    /// recording, `None` when it is missing. A missing recording's channels
    /// are left out and a panel left with none is gone; everything else is as
    /// saved. A panel with no value span kept shows `fit` of its channels.
    pub fn restore(
        &self,
        found: &[Option<Found>],
        mut fit: impl FnMut(&[ChannelRef]) -> Span,
    ) -> Restored {
        let mut problems = Vec::new();
        let mut layout = Layout::default();
        for panel in &self.panels {
            let mut channels: Vec<ChannelRef> = Vec::new();
            for spec in &panel.channels {
                let Some(slot) = found.get(spec.recording) else {
                    problems.push(format!("no recording {} in the workspace", spec.recording));
                    continue;
                };
                // Missing: the tree says so.
                let Some(found) = slot else { continue };
                let entry = found
                    .entries
                    .iter()
                    .position(|&(s, c)| s == spec.stream && c == spec.channel);
                match entry {
                    Some(entry) => channels.push(ChannelRef {
                        recording: found.index,
                        entry,
                    }),
                    None => problems.push(format!(
                        "{}: no channel {}/{}",
                        self.recordings[spec.recording].display(),
                        spec.stream,
                        spec.channel
                    )),
                }
            }
            let Some(&first) = channels.first() else {
                continue;
            };
            let value = match (panel.value_from, panel.value_to) {
                (Some(from), Some(to)) => span(from, to),
                _ => None,
            }
            .unwrap_or_else(|| fit(&channels));
            let index = layout.panels().len();
            let dragged = |channel| Dragged {
                channel,
                from: None,
            };
            layout.drop(dragged(first), Drop::NewPanel(index), value);
            for &channel in &channels[1..] {
                layout.drop(dragged(channel), Drop::Overlay(index), value);
            }
        }
        Restored {
            layout,
            time: self.time(),
            orientation: self.orientation,
            mode: self.mode,
            problems,
        }
    }

    /// The time to show, when the file keeps one that can be shown.
    pub fn time(&self) -> Option<Span> {
        self.view.and_then(|v| span(v.time_from_s, v.time_to_s))
    }

    /// Where each recording is, for the workspace at `file`.
    pub fn recording_paths(&self, file: &Path) -> Vec<PathBuf> {
        let dir = folder(file);
        self.recordings.iter().map(|p| dir.join(p)).collect()
    }

    /// Read a workspace file's text; an error says why it cannot be used.
    pub fn parse(text: &str) -> Result<Workspace, String> {
        // The version first, so a newer file is refused for what it is and
        // not for a shape this viewer does not know.
        let value: serde_json::Value =
            serde_json::from_str(text).map_err(|e| format!("not a workspace: {e}"))?;
        match value.get("sigws").and_then(|v| v.as_u64()) {
            Some(v) if v == VERSION as u64 => {}
            Some(v) => {
                return Err(format!(
                    "workspace version {v}, this viewer reads version {VERSION}"
                ));
            }
            None => return Err("not a workspace: no `sigws` version".into()),
        }
        serde_json::from_value(value).map_err(|e| format!("not a workspace: {e}"))
    }

    /// The file's text.
    pub fn to_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self).expect("plain data");
        text.push('\n');
        text
    }
}

/// `from..to` when it can be shown: finite, and not empty or reversed.
fn span(from: f64, to: f64) -> Option<Span> {
    (from.is_finite() && to.is_finite() && from < to).then(|| Span::new(from, to))
}

/// The folder a workspace file is in.
fn folder(file: &Path) -> &Path {
    file.parent().unwrap_or(Path::new(""))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The example in the viewer spec, verbatim.
    const SPEC_EXAMPLE: &str = r#"{
      "sigws": 1,
      "recordings": ["pixel7-drive.sigrec", "iphone-drive.sigrec"],
      "orientation": "vertical",
      "mode": "pan",
      "view": { "time_from_s": 12.0, "time_to_s": 48.0 },
      "panels": [
        { "channels": [ { "recording": 0, "stream": "accel", "channel": "z" },
                        { "recording": 1, "stream": "accel", "channel": "z" } ],
          "value_from": -12.0, "value_to": 12.0 },
        { "channels": [ { "recording": 0, "stream": "accel", "channel": "residual" } ] }
      ]
    }"#;

    fn ch(recording: usize, stream: &str, channel: &str) -> ChannelSpec {
        ChannelSpec {
            recording,
            stream: stream.into(),
            channel: channel.into(),
        }
    }

    #[test]
    fn the_spec_example_reads_as_written_and_reads_back_the_same_after_writing() {
        let ws = Workspace::parse(SPEC_EXAMPLE).unwrap();
        assert_eq!(
            ws,
            Workspace {
                sigws: 1,
                recordings: vec!["pixel7-drive.sigrec".into(), "iphone-drive.sigrec".into()],
                orientation: Orientation::Vertical,
                mode: Mode::Pan,
                view: Some(TimeView {
                    time_from_s: 12.0,
                    time_to_s: 48.0
                }),
                panels: vec![
                    PanelSpec {
                        channels: vec![ch(0, "accel", "z"), ch(1, "accel", "z")],
                        value_from: Some(-12.0),
                        value_to: Some(12.0),
                    },
                    PanelSpec {
                        channels: vec![ch(0, "accel", "residual")],
                        value_from: None,
                        value_to: None,
                    },
                ],
            }
        );
        assert_eq!(Workspace::parse(&ws.to_json()).unwrap(), ws);
        let horizontal = Workspace {
            orientation: Orientation::Horizontal,
            mode: Mode::Align,
            ..ws
        };
        let text = horizontal.to_json();
        assert!(text.contains(r#""orientation": "horizontal""#), "{text}");
        assert!(text.contains(r#""mode": "align""#), "{text}");
        assert_eq!(Workspace::parse(&text).unwrap(), horizontal);
    }

    #[test]
    fn a_recording_inside_the_workspace_folder_is_found_after_the_folder_moves_and_one_outside_stays_put()
     {
        let layout = Layout::default();
        let session = Session {
            opened: vec![
                Opened {
                    path: Path::new("/w/data/a.sigrec"),
                    entries: vec![],
                },
                Opened {
                    path: Path::new("/elsewhere/b.sigrec"),
                    entries: vec![],
                },
            ],
            missing: vec![Path::new("/w/old/c.sigrec")],
            layout: &layout,
            time: Span::new(0.0, 1.0),
            orientation: Orientation::Vertical,
            mode: Mode::Pan,
        };
        let ws = Workspace::capture(Path::new("/w/x.sigws.json"), &session);
        let written: Vec<PathBuf> = vec![
            "data/a.sigrec".into(),
            "/elsewhere/b.sigrec".into(),
            "old/c.sigrec".into(),
        ];
        assert_eq!(ws.recordings, written);
        let moved: Vec<PathBuf> = vec![
            "/moved/w/data/a.sigrec".into(),
            "/elsewhere/b.sigrec".into(),
            "/moved/w/old/c.sigrec".into(),
        ];
        assert_eq!(
            ws.recording_paths(Path::new("/moved/w/x.sigws.json")),
            moved
        );
    }

    #[test]
    fn a_missing_recording_costs_only_its_own_channels_and_the_rest_comes_back_as_saved() {
        let ws = Workspace {
            sigws: 1,
            recordings: vec!["a.sigrec".into(), "gone.sigrec".into(), "c.sigrec".into()],
            orientation: Orientation::Horizontal,
            mode: Mode::Box,
            view: Some(TimeView {
                time_from_s: 3.0,
                time_to_s: 4.5,
            }),
            panels: vec![
                PanelSpec {
                    channels: vec![ch(0, "s", "x"), ch(1, "s", "x")],
                    value_from: Some(-2.0),
                    value_to: Some(2.0),
                },
                PanelSpec {
                    channels: vec![ch(1, "s", "x")],
                    value_from: Some(0.0),
                    value_to: Some(1.0),
                },
                PanelSpec {
                    channels: vec![ch(2, "s", "x"), ch(0, "s", "residual")],
                    value_from: None,
                    value_to: None,
                },
                PanelSpec {
                    channels: vec![ch(0, "s", "renamed"), ch(7, "s", "x")],
                    value_from: Some(0.0),
                    value_to: Some(1.0),
                },
            ],
        };
        let found = [
            Some(Found {
                index: 0,
                entries: vec![("s", "x"), ("s", "residual")],
            }),
            None,
            Some(Found {
                index: 1,
                entries: vec![("s", "x")],
            }),
        ];
        let at = |recording, entry| ChannelRef { recording, entry };
        let mut fitted = Vec::new();
        let restored = ws.restore(&found, |channels| {
            fitted.push(channels.to_vec());
            Span::new(5.0, 6.0)
        });
        let panels: Vec<(Vec<ChannelRef>, Span)> = restored
            .layout
            .panels()
            .iter()
            .map(|p| (p.channels.clone(), p.value))
            .collect();
        assert_eq!(
            panels,
            vec![
                (vec![at(0, 0)], Span::new(-2.0, 2.0)),
                (vec![at(1, 0), at(0, 1)], Span::new(5.0, 6.0)),
            ]
        );
        assert_eq!(fitted, vec![vec![at(1, 0), at(0, 1)]]);
        assert_eq!(restored.time, Some(Span::new(3.0, 4.5)));
        assert_eq!(restored.orientation, Orientation::Horizontal);
        assert_eq!(restored.mode, Mode::Box);
        // The missing recording is the tree's to show; a channel no
        // recording has is said.
        assert_eq!(restored.problems.len(), 2, "{:?}", restored.problems);
        assert!(
            restored.problems[0].contains("s/renamed"),
            "{:?}",
            restored.problems
        );
    }

    #[test]
    fn a_span_that_cannot_be_shown_is_fitted_instead() {
        let ws = Workspace {
            view: Some(TimeView {
                time_from_s: 2.0,
                time_to_s: 2.0,
            }),
            panels: vec![PanelSpec {
                channels: vec![ch(0, "s", "x")],
                value_from: Some(1.0),
                value_to: Some(f64::NAN),
            }],
            ..Workspace::parse(r#"{ "sigws": 1, "recordings": ["a.sigrec"] }"#).unwrap()
        };
        let found = [Some(Found {
            index: 0,
            entries: vec![("s", "x")],
        })];
        let restored = ws.restore(&found, |_| Span::new(5.0, 6.0));
        assert_eq!(restored.time, None);
        assert_eq!(restored.layout.panels()[0].value, Span::new(5.0, 6.0));
    }

    #[test]
    fn a_workspace_from_a_newer_viewer_or_not_a_workspace_at_all_is_refused_with_a_reason() {
        let newer = SPEC_EXAMPLE.replace(r#""sigws": 1"#, r#""sigws": 2"#);
        let error = Workspace::parse(&newer).unwrap_err();
        assert!(error.contains("version 2"), "{error}");
        assert!(Workspace::parse("{}").is_err());
        assert!(Workspace::parse("not json").is_err());
    }
}
