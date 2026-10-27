//! App identity while the Rust app runs side by side with the Go app, and the
//! dogfood instance label (`internal/app/instance.go`).

/// Product name: menu bar, notifications, window title fallback.
pub const APP_NAME: &str = "Recur Next";

/// macOS bundle identifier (`build.recur.app` for the Go app).
pub const BUNDLE_ID: &str = "build.recur.next";

/// Linux application id. The window's Wayland app id and the desktop entry
/// name must match it so docks pair the window with its icon.
pub const LINUX_APP_ID: &str = "build.recur.next";

/// Command the CLI symlink installs as (`recur` for the Go app).
pub const CLI_LINK_NAME: &str = "recur-next";

/// Where the Linux .deb/.rpm install the app (`/opt/recur` for the Go app).
/// The package owns the PATH entry and menu entry itself.
pub const LINUX_PACKAGED_DIR: &str = "/opt/recur-next";

/// Short description shown by the desktop entry.
pub const APP_DESCRIPTION: &str = "Run autonomous AI coding loops locally.";

const INSTANCE_LABEL_ENV: &str = "RECUR_INSTANCE_LABEL";

/// The optional process env Recur paints in the titlebar so dogfood windows
/// are distinguishable. Empty means production chrome.
pub fn instance_label() -> String {
    label_from(std::env::var(INSTANCE_LABEL_ENV).ok().as_deref())
}

/// The native window title. When the instance label is set, the title is
/// that label only (no app name prefix).
pub fn window_title() -> String {
    title_from(&instance_label())
}

fn label_from(raw: Option<&str>) -> String {
    raw.unwrap_or_default().trim().to_string()
}

fn title_from(label: &str) -> String {
    if label.is_empty() {
        APP_NAME.to_string()
    } else {
        label.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_label_trims() {
        assert_eq!(label_from(None), "");
        assert_eq!(label_from(Some("")), "");
        assert_eq!(label_from(Some("  \t\n")), "");
        assert_eq!(
            label_from(Some("  mycodecrafting/smoky-shortcake  ")),
            "mycodecrafting/smoky-shortcake"
        );
    }

    #[test]
    fn window_title_uses_label_or_app_name() {
        assert_eq!(title_from(&label_from(Some(""))), APP_NAME);
        assert_eq!(title_from(&label_from(Some("   "))), APP_NAME);
        assert_eq!(
            title_from(&label_from(Some("mycodecrafting/smoky-shortcake"))),
            "mycodecrafting/smoky-shortcake"
        );
    }

    fn plist_keys(plist: &str) -> Vec<&str> {
        plist
            .split("<key>")
            .skip(1)
            .filter_map(|s| s.split("</key>").next())
            .collect()
    }

    fn plist_string<'a>(plist: &'a str, key: &str) -> &'a str {
        let rest = &plist[plist.find(&format!("<key>{key}</key>")).expect(key)..];
        let start = rest.find("<string>").expect(key) + "<string>".len();
        &rest[start..start + rest[start..].find("</string>").expect(key)]
    }

    #[test]
    fn info_plist_carries_the_go_apps_keys_under_this_identity() {
        let go = include_str!("../../../../build/darwin/Info.plist");
        let next = include_str!("../../../build/darwin/Info.plist");
        assert_eq!(plist_keys(next), plist_keys(go));
        assert_eq!(plist_string(next, "CFBundleIdentifier"), BUNDLE_ID);
        assert_eq!(plist_string(next, "CFBundleName"), APP_NAME);
        for key in [
            "CFBundleExecutable",
            "CFBundleIconFile",
            "CFBundleIconName",
            "LSMinimumSystemVersion",
        ] {
            assert_eq!(plist_string(next, key), plist_string(go, key), "{key}");
        }
    }

    #[test]
    fn env_reader_matches_pure_helper() {
        let want = label_from(std::env::var(INSTANCE_LABEL_ENV).ok().as_deref());
        assert_eq!(instance_label(), want);
        assert_eq!(window_title(), title_from(&want));
    }
}
