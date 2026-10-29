//! Terminal color decisions.

use std::os::unix::fs::FileTypeExt;

/// Standard streams, by file descriptor.
#[derive(Debug, Clone, Copy)]
pub enum Stream {
    Stdout = 1,
    Stderr = 2,
}

/// `NO_COLOR` / `FORCE_COLOR` / `CLICOLOR_FORCE`, else whether the stream is a
/// character device (so `/dev/null` counts, unlike `isatty`). `FORCE_COLOR`
/// wins over `NO_COLOR`.
pub fn want_color(stream: Stream) -> bool {
    if let Some(force) = color_force_env() {
        return force;
    }
    if std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    std::fs::metadata(format!("/dev/fd/{}", stream as i32))
        .is_ok_and(|m| m.file_type().is_char_device())
}

fn color_force_env() -> Option<bool> {
    for key in ["FORCE_COLOR", "CLICOLOR_FORCE"] {
        let Ok(v) = std::env::var(key) else { continue };
        if v.is_empty() {
            continue;
        }
        return Some(!matches!(v.as_str(), "0" | "false" | "off"));
    }
    None
}

pub fn paint_red(s: &str, color: bool) -> String {
    if color {
        format!("\x1b[31m{s}\x1b[0m")
    } else {
        s.to_string()
    }
}
