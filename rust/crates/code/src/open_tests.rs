//! Port of `open_in_test.go`.

use std::cell::RefCell;

use super::*;
use crate::testutil::tmp;

#[derive(Default)]
struct FakeSys {
    installed: &'static [&'static str],
    on_path: bool,
    fail_start: bool,
    started: RefCell<Vec<(String, Vec<String>)>>,
}

impl OpenSys for FakeSys {
    fn mac_app_installed(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self.installed.contains(n))
    }
    fn look_path(&self, bin: &str) -> Option<String> {
        self.on_path.then(|| format!("/usr/bin/{bin}"))
    }
    fn start_detached(&self, name: &str, args: &[String]) -> std::result::Result<(), String> {
        if self.fail_start {
            return Err("boom".into());
        }
        self.started.borrow_mut().push((name.into(), args.to_vec()));
        Ok(())
    }
}

#[test]
fn list_open_in_targets_includes_file_manager() {
    let targets = list_open_in_targets();
    if !IS_DARWIN {
        assert_eq!(targets[0].id, "finder");
        assert_eq!(targets[0].name, "Files");
        return;
    }
    assert_eq!((targets[0].id.as_str(), targets[0].name.as_str()), ("finder", "Finder"));
    assert!(
        targets.iter().any(|t| t.id == "terminal"),
        "darwin catalog should include Terminal"
    );
}

#[test]
fn list_open_in_targets_filters_installed() {
    let sys = FakeSys {
        installed: &["Cursor.app"],
        ..Default::default()
    };
    let ids: Vec<String> = list_open_in_targets_with(&sys)
        .into_iter()
        .map(|t| t.id)
        .collect();
    if !IS_DARWIN {
        // .app bundles only count on macOS; nothing is on PATH here.
        assert_eq!(ids, ["finder"]);
        return;
    }
    assert!(ids.contains(&"finder".into()));
    assert!(ids.contains(&"cursor".into()), "Cursor.app is present");
    for gone in ["vscode", "zed", "ghostty"] {
        assert!(!ids.contains(&gone.into()), "uninstalled {gone} leaked: {ids:?}");
    }
    assert!(ids.contains(&"terminal".into()), "terminal is always-on on darwin");
}

#[test]
fn list_open_in_targets_includes_os_app_icons() {
    if !IS_DARWIN {
        return;
    }
    let targets = list_open_in_targets();
    assert_eq!(targets[0].id, "finder");
    assert!(
        targets[0].icon.starts_with("data:image/png;base64,"),
        "Finder icon: {:.40}",
        targets[0].icon
    );
    let again = list_open_in_targets();
    assert_eq!(again[0].icon, targets[0].icon, "icon cache should be stable");
}

#[test]
fn open_in_finder() {
    let want = match std::env::consts::OS {
        "macos" => "open",
        "linux" => "xdg-open",
        _ => return,
    };
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let sys = FakeSys::default();
    open_in_with(&sys, d, "finder").unwrap();
    assert_eq!(*sys.started.borrow(), [(want.to_string(), vec![d.to_string()])]);
}

#[test]
fn open_in_cursor_uses_open_a() {
    if !IS_DARWIN {
        return;
    }
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let sys = FakeSys {
        installed: &["Cursor.app"],
        ..Default::default()
    };
    open_in_with(&sys, d, "cursor").unwrap();
    assert_eq!(
        *sys.started.borrow(),
        [(
            "open".to_string(),
            vec!["-a".to_string(), "Cursor".into(), d.into()]
        )]
    );
}

#[test]
fn open_in_unknown_and_missing() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let sys = FakeSys::default();
    assert_eq!(
        open_in_with(&sys, d, "nope").unwrap_err().to_string(),
        r#"unknown open-in target "nope""#
    );
    let missing = format!("{d}/missing");
    assert_eq!(
        open_in_with(&sys, &missing, "finder").unwrap_err().to_string(),
        format!("cannot open {missing}: stat {missing}: no such file or directory")
    );
    let file = format!("{d}/file.txt");
    std::fs::write(&file, "x").unwrap();
    assert_eq!(
        open_in_with(&sys, &file, "finder").unwrap_err().to_string(),
        format!("{file} is not a directory")
    );
}

#[test]
fn open_in_propagates_start_error() {
    let dir = tmp();
    let sys = FakeSys {
        fail_start: true,
        ..Default::default()
    };
    assert!(open_in_with(&sys, dir.path().to_str().unwrap(), "finder").is_err());
}

#[test]
fn open_in_ghostty_bin_passes_working_directory() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let sys = FakeSys {
        on_path: true,
        ..Default::default()
    };
    open_in_with(&sys, d, "ghostty").unwrap();
    assert_eq!(
        *sys.started.borrow(),
        [(
            "/usr/bin/ghostty".to_string(),
            vec![format!("--working-directory={d}")]
        )]
    );
}

#[test]
fn open_in_reports_missing_app() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    let sys = FakeSys::default();
    assert_eq!(
        open_in_with(&sys, d, "zed").unwrap_err().to_string(),
        r#"open in Zed: exec: "zed": executable file not found in $PATH"#
    );
    if IS_DARWIN {
        assert_eq!(
            open_in_with(&sys, d, "xcode").unwrap_err().to_string(),
            "Xcode is not installed"
        );
    }
}

#[test]
fn open_in_editor_and_reveal_reject_bad_paths() {
    let dir = tmp();
    let d = dir.path().to_str().unwrap();
    assert_eq!(
        open_in_editor(d, 3).unwrap_err().to_string(),
        format!("cannot open {d}")
    );
    let gone = format!("{d}/nested/gone.txt");
    assert!(reveal_in_finder(&gone).is_err());
}
