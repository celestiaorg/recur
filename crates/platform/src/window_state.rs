//! Main-window geometry persisted as `window.json` in the data dir, and the
//! restore rules applied when the window opens.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_WIDTH: i32 = 1380;
pub const DEFAULT_HEIGHT: i32 = 880;
pub const MIN_WIDTH: i32 = 1100;
pub const MIN_HEIGHT: i32 = 720;
/// Matches the `.titlebar` height and the hidden-inset drag strip
/// (DECISIONS D-window).
pub const TITLEBAR_HEIGHT: i32 = 52;
/// Window background (`--bg` = #0A0C10).
pub const BACKGROUND_RGB: (u8, u8, u8) = (10, 12, 16);
pub const STATE_FILE: &str = "window.json";
/// Resize/move events are coalesced for this long before a save.
pub const SAVE_DEBOUNCE: Duration = Duration::from_millis(250);

/// The last known main-window geometry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub has_pos: bool,
    pub maximised: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            width: DEFAULT_WIDTH,
            height: DEFAULT_HEIGHT,
            x: 0,
            y: 0,
            has_pos: false,
            maximised: false,
        }
    }
}

impl WindowState {
    /// Undersized dimensions fall back to the defaults.
    pub fn clamped(mut self) -> Self {
        if self.width < MIN_WIDTH {
            self.width = DEFAULT_WIDTH;
        }
        if self.height < MIN_HEIGHT {
            self.height = DEFAULT_HEIGHT;
        }
        self
    }
}

pub fn state_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

/// Reads `window.json`; a missing or malformed file yields the defaults.
pub fn load(data_dir: &Path) -> WindowState {
    let Ok(data) = std::fs::read(state_path(data_dir)) else {
        return WindowState::default();
    };
    // Absent fields are 0, not the defaults.
    let zero = WindowState {
        width: 0,
        height: 0,
        ..WindowState::default()
    };
    match serde_json::from_slice::<Partial>(&data) {
        Ok(p) => p.over(zero).clamped(),
        Err(_) => WindowState::default(),
    }
}

/// Field-optional mirror of [`WindowState`] for decoding.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Partial {
    width: Option<i32>,
    height: Option<i32>,
    x: Option<i32>,
    y: Option<i32>,
    has_pos: Option<bool>,
    maximised: Option<bool>,
}

impl Partial {
    fn over(self, base: WindowState) -> WindowState {
        WindowState {
            width: self.width.unwrap_or(base.width),
            height: self.height.unwrap_or(base.height),
            x: self.x.unwrap_or(base.x),
            y: self.y.unwrap_or(base.y),
            has_pos: self.has_pos.unwrap_or(base.has_pos),
            maximised: self.maximised.unwrap_or(base.maximised),
        }
    }
}

/// Writes `window.json` (two-space indented). Best effort.
pub fn save(data_dir: &Path, st: WindowState) {
    let st = st.clamped();
    if let Ok(data) = serde_json::to_vec_pretty(&st) {
        let _ = std::fs::write(state_path(data_dir), data);
    }
}

/// Creation options for the main window, derived from the saved state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowOptions {
    pub title: String,
    pub width: i32,
    pub height: i32,
    pub min_width: i32,
    pub min_height: i32,
    /// `None` centres the window.
    pub position: Option<(i32, i32)>,
    pub background: (u8, u8, u8),
    /// Hidden-inset titlebar with this invisible drag height (macOS).
    pub titlebar_height: i32,
    /// Finder drops reach the composer.
    pub file_drop: bool,
}

impl WindowOptions {
    /// Maximised is deliberately not part of the creation options: zooming
    /// during create is undone by the initial position on macOS. Restore it
    /// with [`GeometryTracker::should_zoom_on_show`] once the window shows.
    pub fn from_state(st: &WindowState) -> Self {
        Self {
            title: crate::identity::window_title(),
            width: st.width,
            height: st.height,
            min_width: MIN_WIDTH,
            min_height: MIN_HEIGHT,
            position: st.has_pos.then_some((st.x, st.y)),
            background: BACKGROUND_RGB,
            titlebar_height: TITLEBAR_HEIGHT,
            file_drop: true,
        }
    }
}

/// What the window reports at the time of a snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WindowFrame {
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    pub maximised: bool,
    pub minimised: bool,
    pub fullscreen: bool,
}

/// Tracks the last normal geometry. Size is only recorded while the window
/// is normal (not maximised, fullscreen or minimised), so a maximised
/// session does not clobber the last restored dimensions.
///
/// The window layer drives it: on resize/move, debounce for
/// [`SAVE_DEBOUNCE`] then `save(data_dir, tracker.snapshot(frame))`; on
/// close, cancel the pending save and save immediately; on the first show,
/// zoom when [`GeometryTracker::should_zoom_on_show`] says so.
#[derive(Debug)]
pub struct GeometryTracker {
    normal: WindowState,
    zoom_pending: bool,
}

impl GeometryTracker {
    pub fn new(loaded: WindowState) -> Self {
        Self {
            normal: loaded,
            zoom_pending: loaded.maximised,
        }
    }

    pub fn snapshot(&mut self, frame: &WindowFrame) -> WindowState {
        if !frame.minimised && !frame.fullscreen && !frame.maximised {
            self.normal = WindowState {
                width: frame.width,
                height: frame.height,
                x: frame.x,
                y: frame.y,
                has_pos: true,
                maximised: false,
            };
            return self.normal;
        }
        WindowState {
            maximised: frame.maximised,
            ..self.normal
        }
    }

    /// True once, on the first show, when the saved state was maximised and
    /// the window is not already.
    pub fn should_zoom_on_show(&mut self, currently_maximised: bool) -> bool {
        std::mem::take(&mut self.zoom_pending) && !currently_maximised
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp() {
        let st = WindowState {
            width: 800,
            height: 600,
            ..WindowState::default()
        }
        .clamped();
        assert_eq!((st.width, st.height), (DEFAULT_WIDTH, DEFAULT_HEIGHT));
        let valid = WindowState {
            width: 1600,
            height: 1000,
            has_pos: true,
            x: 10,
            y: 20,
            maximised: false,
        };
        assert_eq!(valid.clamped(), valid);
    }

    #[test]
    fn load_save_round_trip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let want = WindowState {
            width: 1500,
            height: 900,
            x: 40,
            y: 60,
            has_pos: true,
            maximised: true,
        };
        save(dir.path(), want);
        let raw = std::fs::read_to_string(state_path(dir.path())).expect("read");
        assert_eq!(
            raw,
            "{\n  \"width\": 1500,\n  \"height\": 900,\n  \"x\": 40,\n  \"y\": 60,\n  \
             \"hasPos\": true,\n  \"maximised\": true\n}"
        );
        let file: WindowState = serde_json::from_str(&raw).expect("parse");
        assert_eq!(file, want);
        assert_eq!(load(dir.path()), want);
    }

    #[test]
    fn load_fallbacks() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(load(dir.path()), WindowState::default(), "missing file");
        std::fs::write(state_path(dir.path()), "{nope").expect("write");
        assert_eq!(load(dir.path()), WindowState::default(), "malformed");
        std::fs::write(state_path(dir.path()), r#"{"x":5,"hasPos":true}"#).expect("write");
        assert_eq!(
            load(dir.path()),
            WindowState {
                x: 5,
                has_pos: true,
                ..WindowState::default()
            },
            "absent sizes decode as zero and clamp to defaults"
        );
    }

    #[test]
    fn options_from_state() {
        let opts = WindowOptions::from_state(&WindowState {
            width: 1500,
            height: 900,
            x: 12,
            y: 34,
            has_pos: true,
            maximised: true,
        });
        assert_eq!((opts.width, opts.height), (1500, 900));
        assert_eq!(opts.position, Some((12, 34)));
        assert!(opts.file_drop, "Finder drops reach the composer");
        assert_eq!((opts.min_width, opts.min_height), (MIN_WIDTH, MIN_HEIGHT));
        assert_eq!(opts.title, crate::identity::window_title());
        assert_eq!(
            WindowOptions::from_state(&WindowState::default()).position,
            None
        );
    }

    #[test]
    fn tracker_records_normal_geometry_only() {
        let loaded = WindowState {
            width: 1400,
            height: 900,
            x: 1,
            y: 2,
            has_pos: true,
            maximised: false,
        };
        let mut t = GeometryTracker::new(loaded);
        let maxed = WindowFrame {
            width: 3000,
            height: 2000,
            maximised: true,
            ..WindowFrame::default()
        };
        assert_eq!(
            t.snapshot(&maxed),
            WindowState {
                maximised: true,
                ..loaded
            }
        );

        let normal = WindowFrame {
            width: 1200,
            height: 800,
            x: 7,
            y: 8,
            ..WindowFrame::default()
        };
        let want = WindowState {
            width: 1200,
            height: 800,
            x: 7,
            y: 8,
            has_pos: true,
            maximised: false,
        };
        assert_eq!(t.snapshot(&normal), want);
        let full = WindowFrame {
            fullscreen: true,
            ..maxed
        };
        assert_eq!(
            t.snapshot(&WindowFrame {
                maximised: false,
                ..full
            }),
            want
        );
        let mini = WindowFrame {
            minimised: true,
            ..normal
        };
        assert_eq!(t.snapshot(&mini), want);
    }

    #[test]
    fn zoom_on_first_show_only() {
        let mut t = GeometryTracker::new(WindowState {
            maximised: true,
            ..WindowState::default()
        });
        assert!(t.should_zoom_on_show(false));
        assert!(!t.should_zoom_on_show(false));
        let mut t = GeometryTracker::new(WindowState {
            maximised: true,
            ..WindowState::default()
        });
        assert!(!t.should_zoom_on_show(true));
        assert!(!GeometryTracker::new(WindowState::default()).should_zoom_on_show(false));
    }
}
