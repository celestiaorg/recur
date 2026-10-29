//! Handing paths to other apps: the external editor, the file manager, and
//! the workspace "Open In" picker.

use std::collections::HashMap;
use std::fs;
use std::process::{Command, Stdio};
use std::sync::LazyLock;

use parking_lot::Mutex;
use recur_domain::OpenInTarget;
use recur_domain::path;

use crate::error::{Error, Result};
use crate::rg::look_path;

/// Starts `name args…` without waiting; the child is reaped in the
/// background. Errors carry Go's `exec` text.
fn start_detached_cmd(name: &str, args: &[String]) -> std::result::Result<(), String> {
    let program = if name.contains('/') {
        name.to_string()
    } else {
        look_path(name)
            .ok_or_else(|| format!("exec: {:?}: executable file not found in $PATH", name))?
    };
    let mut child = Command::new(&program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Opens `abs_path:line` in an external editor: `$RECUR_EDITOR`, then Cursor,
/// then VS Code, then `$VISUAL` / `$EDITOR`, then the OS opener.
pub fn open_in_editor(abs_path: &str, line: i64) -> Result<()> {
    if !fs::metadata(abs_path).is_ok_and(|m| !m.is_dir()) {
        return Err(Error::msg(format!("cannot open {abs_path}")));
    }
    let target = if line > 0 {
        format!("{abs_path}:{line}")
    } else {
        abs_path.to_string()
    };
    let mut candidates: Vec<Vec<String>> = Vec::new();
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let recur_editor = env("RECUR_EDITOR");
    if !recur_editor.is_empty() {
        candidates.push(vec![recur_editor, target.clone()]);
    }
    // Cursor and VS Code understand path:line.
    candidates.push(vec!["cursor".into(), "-g".into(), target.clone()]);
    candidates.push(vec!["code".into(), "-g".into(), target]);
    let ed = [env("VISUAL"), env("EDITOR")]
        .into_iter()
        .find(|v| !v.is_empty())
        .unwrap_or_default();
    if !ed.is_empty() {
        if line > 0 {
            candidates.push(vec![ed, format!("+{line}"), abs_path.into()]);
        } else {
            candidates.push(vec![ed, abs_path.into()]);
        }
    }
    match std::env::consts::OS {
        "macos" => candidates.push(vec!["open".into(), abs_path.into()]),
        "linux" => candidates.push(vec!["xdg-open".into(), abs_path.into()]),
        _ => {}
    }
    let mut last = None;
    for c in candidates {
        match start_detached_cmd(&c[0], &c[1..]) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::msg(
        last.unwrap_or_else(|| "no editor available".into()),
    ))
}

/// Shows a file or folder in the OS file manager; a missing path reveals its
/// parent folder when that exists.
pub fn reveal_in_finder(abs_path: &str) -> Result<()> {
    let mut target = abs_path.to_string();
    if let Err(e) = fs::metadata(abs_path) {
        let parent = path::dir(abs_path);
        if !fs::metadata(&parent).is_ok_and(|m| m.is_dir()) {
            return Err(Error::path(abs_path, e));
        }
        target = parent;
    }
    let res = match std::env::consts::OS {
        "macos" => start_detached_cmd("open", &["-R".into(), target]),
        "linux" => {
            // xdg-open cannot select a file; open its folder.
            if fs::metadata(&target).is_ok_and(|m| !m.is_dir()) {
                target = path::dir(&target);
            }
            start_detached_cmd("xdg-open", &[target])
        }
        os => return Err(Error::msg(format!("reveal not supported on {}", go_os(os)))),
    };
    res.map_err(Error::Msg)
}

fn go_os(os: &str) -> &str {
    if os == "macos" { "darwin" } else { os }
}

/// One "Open In" catalog entry. Only installed apps are listed.
struct OpenInApp {
    id: &'static str,
    name: &'static str,
    /// `.app` bundle names.
    darwin: &'static [&'static str],
    /// `PATH` fallbacks.
    bins: &'static [&'static str],
    /// Prefix for the folder argument when a binary takes no positional path.
    dir_flag: &'static str,
    darwin_only: bool,
    /// Skip the install check (system apps).
    always: bool,
}

const IS_DARWIN: bool = cfg!(target_os = "macos");

const CATALOG: &[OpenInApp] = &[
    app(
        "cursor",
        "Cursor",
        &["Cursor.app"],
        &["cursor"],
        "",
        false,
        false,
    ),
    app(
        "vscode",
        "Visual Studio Code",
        &["Visual Studio Code.app", "Code.app"],
        &["code"],
        "",
        false,
        false,
    ),
    app("zed", "Zed", &["Zed.app"], &["zed"], "", false, false),
    app("xcode", "Xcode", &["Xcode.app"], &[], "", true, false),
    app(
        "ghostty",
        "Ghostty",
        &["Ghostty.app"],
        &["ghostty"],
        "--working-directory=",
        false,
        false,
    ),
    app(
        "iterm",
        "iTerm",
        &["iTerm.app", "iTerm2.app"],
        &[],
        "",
        true,
        false,
    ),
    app(
        "terminal",
        "Terminal",
        &["Terminal.app"],
        &[],
        "",
        true,
        IS_DARWIN,
    ),
    app("warp", "Warp", &["Warp.app"], &["warp"], "", false, false),
    app(
        "kaleidoscope",
        "Kaleidoscope",
        &["Kaleidoscope.app"],
        &[],
        "",
        true,
        false,
    ),
    app(
        "sublime",
        "Sublime Text",
        &["Sublime Text.app"],
        &["subl"],
        "",
        false,
        false,
    ),
];

const fn app(
    id: &'static str,
    name: &'static str,
    darwin: &'static [&'static str],
    bins: &'static [&'static str],
    dir_flag: &'static str,
    darwin_only: bool,
    always: bool,
) -> OpenInApp {
    OpenInApp {
        id,
        name,
        darwin,
        bins,
        dir_flag,
        darwin_only,
        always,
    }
}

/// The OS seams Open In touches, replaceable in tests.
pub(crate) trait OpenSys {
    fn mac_app_installed(&self, names: &[&str]) -> bool {
        !mac_app_path(names).is_empty()
    }
    fn look_path(&self, bin: &str) -> Option<String> {
        look_path(bin)
    }
    fn start_detached(&self, name: &str, args: &[String]) -> std::result::Result<(), String> {
        start_detached_cmd(name, args)
    }
}

struct RealSys;
impl OpenSys for RealSys {}

fn mac_app_path(names: &[&str]) -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        "/Applications".to_string(),
        "/System/Applications".into(),
        "/System/Applications/Utilities".into(),
        "/Applications/Utilities".into(),
        "/System/Library/CoreServices".into(),
    ];
    if !home.is_empty() {
        dirs.push(path::join(&[&home, "Applications"]));
    }
    for name in names {
        for dir in &dirs {
            let p = path::join(&[dir, name]);
            if fs::metadata(&p).is_ok_and(|m| m.is_dir()) {
                return p;
            }
        }
    }
    String::new()
}

static ICONS: LazyLock<Mutex<HashMap<String, String>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The app's icon as a PNG data URL (cached per app path; may be empty).
fn cached_app_icon(app_path: &str) -> String {
    if app_path.is_empty() {
        return String::new();
    }
    if let Some(url) = ICONS.lock().get(app_path) {
        return url.clone();
    }
    let url = icon::app_icon_data_url(app_path);
    ICONS.lock().insert(app_path.to_string(), url.clone());
    url
}

fn app_present(sys: &dyn OpenSys, app: &OpenInApp) -> bool {
    if app.always {
        return true;
    }
    if app.darwin_only && !IS_DARWIN {
        return false;
    }
    if IS_DARWIN && !app.darwin.is_empty() && sys.mac_app_installed(app.darwin) {
        return true;
    }
    app.bins.iter().any(|b| sys.look_path(b).is_some())
}

/// Installed apps that can open a workspace folder, the file manager first.
/// Copy-path is a UI-only action. Off macOS only apps with a `PATH` binary
/// are listed.
pub fn list_open_in_targets() -> Vec<OpenInTarget> {
    list_open_in_targets_with(&RealSys)
}

pub(crate) fn list_open_in_targets_with(sys: &dyn OpenSys) -> Vec<OpenInTarget> {
    if !IS_DARWIN && std::env::consts::OS != "linux" {
        return Vec::new();
    }
    let mut out = vec![OpenInTarget {
        id: "finder".into(),
        name: if IS_DARWIN { "Finder" } else { "Files" }.into(),
        kind: "app".into(),
        icon: cached_app_icon(&mac_app_path(&["Finder.app"])),
    }];
    for app in CATALOG {
        if !app_present(sys, app) {
            continue;
        }
        out.push(OpenInTarget {
            id: app.id.into(),
            name: app.name.into(),
            kind: "app".into(),
            icon: cached_app_icon(&mac_app_path(app.darwin)),
        });
    }
    out
}

/// Opens a directory in the named target (`finder`, `cursor`, …).
pub fn open_in(abs_path: &str, target_id: &str) -> Result<()> {
    open_in_with(&RealSys, abs_path, target_id)
}

pub(crate) fn open_in_with(sys: &dyn OpenSys, abs_path: &str, target_id: &str) -> Result<()> {
    let st =
        fs::metadata(abs_path).map_err(|e| Error::msg(format!("cannot open {abs_path}: {e}")))?;
    if !st.is_dir() {
        return Err(Error::msg(format!("{abs_path} is not a directory")));
    }
    if target_id == "finder" {
        return match std::env::consts::OS {
            "macos" => sys.start_detached("open", &[abs_path.into()]),
            "linux" => sys.start_detached("xdg-open", &[abs_path.into()]),
            os => {
                return Err(Error::msg(format!(
                    "open folder not supported on {}",
                    go_os(os)
                )));
            }
        }
        .map_err(Error::Msg);
    }
    let Some(app) = CATALOG.iter().find(|a| a.id == target_id) else {
        return Err(Error::msg(format!(
            "unknown open-in target {:?}",
            target_id
        )));
    };
    open_with_app(sys, abs_path, app)
}

fn open_with_app(sys: &dyn OpenSys, path: &str, app: &OpenInApp) -> Result<()> {
    if IS_DARWIN {
        for bundle in app.darwin {
            if !app.always && !sys.mac_app_installed(&[bundle]) {
                continue;
            }
            let name = bundle.strip_suffix(".app").unwrap_or(bundle);
            return sys
                .start_detached("open", &["-a".into(), name.into(), path.into()])
                .map_err(Error::Msg);
        }
    }
    let mut last = None;
    for bin in app.bins {
        let Some(p) = sys.look_path(bin) else {
            last = Some(format!(
                "exec: {:?}: executable file not found in $PATH",
                bin
            ));
            continue;
        };
        match sys.start_detached(&p, &[format!("{}{path}", app.dir_flag)]) {
            Ok(()) => return Ok(()),
            Err(e) => last = Some(e),
        }
    }
    Err(Error::msg(match last {
        Some(e) => format!("open in {}: {e}", app.name),
        None => format!("{} is not installed", app.name),
    }))
}

#[cfg(target_os = "macos")]
mod icon {
    // NSBitmapImageRep's designated initializer and the NSData-returning
    // representation call have no safe wrappers; this module is the only
    // place that calls them.
    #![allow(unsafe_code)]

    use base64::Engine;
    use objc2::AnyThread;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{
        NSBitmapImageFileType, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSColor,
        NSCompositingOperation, NSGraphicsContext, NSRectFill, NSWorkspace,
    };
    use objc2_foundation::{NSDictionary, NSPoint, NSRect, NSSize, NSString};

    const SIZE: f64 = 64.0;

    /// The app's 64×64 icon from NSWorkspace as a PNG data URL, or `""`.
    pub(super) fn app_icon_data_url(app_path: &str) -> String {
        autoreleasepool(|_| png(app_path))
            .map(|b| {
                format!(
                    "data:image/png;base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(b)
                )
            })
            .unwrap_or_default()
    }

    fn png(app_path: &str) -> Option<Vec<u8>> {
        if app_path.is_empty() {
            return None;
        }
        let icon = NSWorkspace::sharedWorkspace().iconForFile(&NSString::from_str(app_path));
        icon.setSize(NSSize::new(SIZE, SIZE));
        // SAFETY: null planes ask AppKit to allocate the pixel buffer; the
        // color space name is AppKit's own constant.
        let rep = unsafe {
            NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bytesPerRow_bitsPerPixel(
                NSBitmapImageRep::alloc(),
                std::ptr::null_mut(),
                SIZE as isize,
                SIZE as isize,
                8,
                4,
                true,
                false,
                NSCalibratedRGBColorSpace,
                0,
                0,
            )
        }?;
        let ctx = NSGraphicsContext::graphicsContextWithBitmapImageRep(&rep)?;
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(SIZE, SIZE));
        NSGraphicsContext::saveGraphicsState_class();
        NSGraphicsContext::setCurrentContext(Some(&ctx));
        NSColor::clearColor().set();
        NSRectFill(rect);
        // SAFETY: no hints dictionary.
        unsafe {
            icon.drawInRect_fromRect_operation_fraction_respectFlipped_hints(
                rect,
                NSRect::ZERO,
                NSCompositingOperation::SourceOver,
                1.0,
                true,
                None,
            );
        }
        NSGraphicsContext::restoreGraphicsState_class();
        // SAFETY: an empty properties dictionary.
        let data = unsafe {
            rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }?;
        Some(data.to_vec())
    }
}

#[cfg(not(target_os = "macos"))]
mod icon {
    pub(super) fn app_icon_data_url(_: &str) -> String {
        String::new()
    }
}

#[cfg(test)]
#[path = "open_tests.rs"]
mod tests;
