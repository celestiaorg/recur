//! App identity and the dogfood instance label.

/// Product name: menu bar, notifications, window title fallback.
pub const APP_NAME: &str = "Recur";

/// macOS bundle identifier.
pub const BUNDLE_ID: &str = "build.recur.app";

/// Linux application id. The window's Wayland app id and the desktop entry
/// name must match it so docks pair the window with its icon.
pub const LINUX_APP_ID: &str = "build.recur.app";

/// Command the CLI symlink installs as.
pub const CLI_LINK_NAME: &str = "recur";

/// Where the Linux .deb/.rpm install the app.
/// The package owns the PATH entry and menu entry itself.
pub const LINUX_PACKAGED_DIR: &str = "/opt/recur";

/// Short description shown by the desktop entry.
pub const APP_DESCRIPTION: &str = "Run autonomous AI coding loops locally.";

/// About panel copyright. Matches `NSHumanReadableCopyright` in `build/darwin/Info.plist`.
pub const APP_COPYRIGHT: &str = "© 2026, recur.build";

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

    fn plist_string<'a>(plist: &'a str, key: &str) -> &'a str {
        let rest = &plist[plist.find(&format!("<key>{key}</key>")).expect(key)..];
        let start = rest.find("<string>").expect(key) + "<string>".len();
        &rest[start..start + rest[start..].find("</string>").expect(key)]
    }

    #[test]
    fn info_plist_carries_this_identity() {
        let plist = include_str!("../../../build/darwin/Info.plist");
        assert_eq!(plist_string(plist, "CFBundleIdentifier"), BUNDLE_ID);
        assert_eq!(plist_string(plist, "CFBundleName"), APP_NAME);
    }

    #[test]
    fn env_reader_matches_pure_helper() {
        let want = label_from(std::env::var(INSTANCE_LABEL_ENV).ok().as_deref());
        assert_eq!(instance_label(), want);
        assert_eq!(window_title(), title_from(&want));
    }
}
