//! Dock badge (D-attention). macOS sets `NSApp.dockTile.badgeLabel`; Linux
//! has no standard dock badge, so it is a no-op there.
//!
//! Only call once the app's run loop is live: AppKit work is dispatched
//! synchronously to the main queue, which never drains before then. Modern
//! macOS ignores the badge unless notification Badge authorization was
//! granted (see [`crate::notify::request_authorization`]).

#[derive(Debug, thiserror::Error)]
pub enum DockError {
    /// The app is not in the regular activation policy (no dock icon).
    #[error("failed to set badge")]
    Failed,
}

/// Sets the badge label. An empty label shows the default dot badge.
pub fn set_badge(label: &str) -> Result<(), DockError> {
    let label = if label.is_empty() { "●" } else { label };
    imp::set_badge(Some(label))
}

/// Removes the badge.
pub fn remove_badge() -> Result<(), DockError> {
    imp::set_badge(None)
}

/// The documented dock arithmetic: unseen chats plus other inbox unread.
/// Chat rows live in the inbox, so pass non-chat unread here (or 0 when the
/// caller already counted chats via unseen).
pub fn badge_count(inbox_unread: i64, unseen_chats: i64) -> i64 {
    inbox_unread.max(0) + unseen_chats.max(0)
}

/// Applies an attention count: removes the badge at zero, else shows it.
/// Failures are ignored, as in the Go app.
pub fn sync_badge(count: i64) {
    let _ = if count <= 0 {
        remove_badge()
    } else {
        set_badge(&count.to_string())
    };
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
    use objc2_foundation::NSString;

    use super::DockError;

    pub(super) fn set_badge(label: Option<&str>) -> Result<(), DockError> {
        let label = label.map(str::to_owned);
        let apply = move || {
            let mtm = MainThreadMarker::new().expect("main queue runs on the main thread");
            let app = NSApplication::sharedApplication(mtm);
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                return false;
            }
            let tile = app.dockTile();
            tile.setBadgeLabel(label.as_deref().map(NSString::from_str).as_deref());
            tile.display();
            true
        };
        // dispatch_sync onto the main queue from the main thread deadlocks.
        let ok = if MainThreadMarker::new().is_some() {
            apply()
        } else {
            let mut ok = false;
            dispatch2::DispatchQueue::main().exec_sync(|| ok = apply());
            ok
        };
        if ok { Ok(()) } else { Err(DockError::Failed) }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::DockError;

    pub(super) fn set_badge(_label: Option<&str>) -> Result<(), DockError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_arithmetic() {
        assert_eq!(badge_count(3, 2), 5);
        assert_eq!(badge_count(-1, 2), 2);
        assert_eq!(badge_count(4, -3), 4);
        assert_eq!(badge_count(0, 0), 0);
    }
}
