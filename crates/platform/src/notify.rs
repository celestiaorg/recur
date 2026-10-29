//! OS notifications for the engine's `notify` event (escalations, PR ready).
//!
//! A bundled macOS app posts through `UNUserNotificationCenter`, whose click
//! responses carry the inbox `target` back through [`on_response`].
//! Unbundled runs (dev builds) and Linux use `osascript` / `notify-send`.
//! `UNUserNotificationCenter` aborts unbundled binaries, so every native call
//! checks for a bundle identifier first.

use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::identity::APP_NAME;

/// `actionIdentifier` for a plain click on the notification body.
pub const DEFAULT_ACTION_IDENTIFIER: &str = "DEFAULT_ACTION";

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum NotifyError {
    #[error("notifications are not available on this system")]
    Unavailable,
    #[error("notifications require a valid bundle identifier")]
    NoBundleIdentifier,
    #[error("notification ID cannot be empty")]
    EmptyId,
    #[error("notification title cannot be empty")]
    EmptyTitle,
    #[error("{0}")]
    Native(String),
    #[error("{0}")]
    Timeout(String),
    #[error("sending notification failed")]
    SendFailed,
}

/// A native notification. `data` becomes the notification's userInfo.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NotificationOptions {
    pub id: String,
    pub title: String,
    pub body: String,
    pub data: Vec<(String, String)>,
}

/// What a user interaction with a notification reports.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct NotificationResponse {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub action_identifier: String,
    #[serde(
        rename = "categoryIdentifier",
        skip_serializing_if = "String::is_empty"
    )]
    pub category_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub subtitle: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub user_text: String,
    #[serde(skip_serializing_if = "serde_json::Map::is_empty")]
    pub user_info: serde_json::Map<String, serde_json::Value>,
}

impl NotificationResponse {
    /// The inbox target Recur attaches to every notification it sends.
    pub fn target(&self) -> &str {
        self.user_info
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
    }
}

pub type NotificationResult = Result<NotificationResponse, NotifyError>;
type Callback = Arc<dyn Fn(NotificationResult) + Send + Sync>;

static CALLBACK: Mutex<Option<Callback>> = Mutex::new(None);

/// Registers the handler for notification clicks. On macOS, clicks are
/// delivered here when the OS activates this process instead of launching
/// another: focus the window and route to `response.target()`.
pub fn on_response(callback: impl Fn(NotificationResult) + Send + Sync + 'static) {
    *CALLBACK.lock() = Some(Arc::new(callback));
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn deliver(result: NotificationResult) {
    let cb = CALLBACK.lock().clone();
    if let Some(cb) = cb {
        cb(result);
    }
}

/// Installs the notification-center delegate so clicks reach
/// [`on_response`]. Call early at launch from a bundled app; fails for
/// unbundled runs and on Linux.
pub fn start() -> Result<(), NotifyError> {
    imp::start()
}

/// Whether notifications are currently authorized.
pub fn check_authorization() -> Result<bool, NotifyError> {
    imp::check_authorization()
}

/// Asks for alert, sound and badge permission (badge is what makes the dock
/// badge show on modern macOS).
pub fn request_authorization() -> Result<bool, NotifyError> {
    imp::request_authorization()
}

/// Posts a native notification.
pub fn send_native(opts: &NotificationOptions) -> Result<(), NotifyError> {
    if opts.id.is_empty() {
        return Err(NotifyError::EmptyId);
    }
    if opts.title.is_empty() {
        return Err(NotifyError::EmptyTitle);
    }
    imp::send(opts)
}

/// Delivers a "needs you" / "PR ready" notification. A bundled macOS app
/// uses the native center; otherwise (or when that fails) `osascript` on
/// macOS and `notify-send` on Linux. Bounded and best effort: failures are
/// logged. Blocking; run it off the event bus.
pub fn send_os_notification(title: &str, body: &str, target: &str) {
    if title.is_empty() {
        return;
    }
    if crate::running_in_app_bundle() {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
        let opts = NotificationOptions {
            id: format!("recur-{nanos}"),
            title: title.to_string(),
            body: body.to_string(),
            data: vec![("target".to_string(), target.to_string())],
        };
        if send_native(&opts).is_ok() {
            return;
        }
    }
    let mut cmd = fallback_command(title, body);
    match crate::run_with_timeout(&mut cmd, Duration::from_secs(3), true) {
        Some(done) if done.success => {}
        Some(done) => tracing::warn!(
            "notification failed: exit status: {}",
            String::from_utf8_lossy(&done.output)
        ),
        None => tracing::warn!("notification failed: helper did not finish"),
    }
}

fn fallback_command(title: &str, body: &str) -> Command {
    if cfg!(target_os = "macos") {
        let script = format!(
            "display notification {} with title {}",
            go_quote(body),
            go_quote(title)
        );
        let mut cmd = Command::new("osascript");
        cmd.args(["-e", &script]);
        cmd
    } else {
        let mut cmd = Command::new("notify-send");
        cmd.arg(format!("--app-name={APP_NAME}"))
            .arg(title)
            .arg(body);
        cmd
    }
}

/// A double-quoted literal with backslash escapes, which AppleScript string
/// literals accept for quotes, backslashes and controls.
fn go_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{07}' => out.push_str("\\a"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0b}' => out.push_str("\\v"),
            c if (c as u32) < 0x20 || c == '\u{7f}' => {
                out.push_str(&format!("\\x{:02x}", c as u32));
            }
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}' | '\u{feff}') => {
                let n = c as u32;
                if n <= 0xffff {
                    out.push_str(&format!("\\u{n:04x}"));
                } else {
                    out.push_str(&format!("\\U{n:08x}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(target_os = "macos")]
mod imp {
    // UserNotifications has no safe Rust wrapper for the delegate class or
    // for reading the `NSError*` its completion blocks hand back; this module
    // is the only place that touches those raw Objective-C pointers.
    #![allow(unsafe_code)]

    use std::ptr::NonNull;
    use std::sync::Once;
    use std::sync::mpsc;
    use std::time::Duration;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyObject, Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AnyThread, define_class, msg_send};
    use objc2_foundation::{
        NSBundle, NSDictionary, NSError, NSJSONSerialization, NSJSONWritingOptions, NSString,
    };
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent,
        UNNotification, UNNotificationPresentationOptions, UNNotificationRequest,
        UNNotificationResponse, UNNotificationSettings, UNNotificationSound,
        UNTextInputNotificationResponse, UNUserNotificationCenter,
        UNUserNotificationCenterDelegate,
    };

    use super::{NotificationOptions, NotificationResponse, NotifyError, deliver};

    const APPLE_DEFAULT_ACTION_IDENTIFIER: &str = "com.apple.UNNotificationDefaultActionIdentifier";

    define_class!(
        // SAFETY: NSObject has no subclassing requirements and the class does
        // not implement Drop.
        #[unsafe(super(NSObject))]
        #[name = "RecurNotificationsDelegate"]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                deliver(read_response(response));
                completion.call(());
            }
        }
    );

    impl Delegate {
        fn new() -> Retained<Self> {
            let this = Self::alloc().set_ivars(());
            // SAFETY: NSObject's designated initializer.
            unsafe { msg_send![super(this), init] }
        }
    }

    fn read_response(response: &UNNotificationResponse) -> super::NotificationResult {
        let request = response.notification().request();
        let content = request.content();
        let mut action = response.actionIdentifier().to_string();
        if action == APPLE_DEFAULT_ACTION_IDENTIFIER {
            action = super::DEFAULT_ACTION_IDENTIFIER.to_string();
        }
        let user_info = content.userInfo();
        let user_info = if user_info.count() == 0 {
            serde_json::Map::new()
        } else {
            json_object(&user_info).map_err(|e| {
                NotifyError::Native(format!("notification response error: Error: {e}"))
            })?
        };
        Ok(NotificationResponse {
            id: request.identifier().to_string(),
            action_identifier: action,
            category_id: content.categoryIdentifier().to_string(),
            title: content.title().to_string(),
            subtitle: content.subtitle().to_string(),
            body: content.body().to_string(),
            user_text: response
                .downcast_ref::<UNTextInputNotificationResponse>()
                .map(|t| t.userText().to_string())
                .unwrap_or_default(),
            user_info,
        })
    }

    fn json_object(
        dict: &NSDictionary,
    ) -> Result<serde_json::Map<String, serde_json::Value>, String> {
        let obj: &AnyObject = dict.as_ref();
        // SAFETY: `obj` is an NSDictionary, a valid top-level JSON container.
        if !unsafe { NSJSONSerialization::isValidJSONObject(obj) } {
            return Err("userInfo is not JSON-serializable".into());
        }
        // SAFETY: validated above.
        let data = unsafe {
            NSJSONSerialization::dataWithJSONObject_options_error(
                obj,
                NSJSONWritingOptions::empty(),
            )
        }
        .map_err(|e| e.localizedDescription().to_string())?;
        serde_json::from_slice(&data.to_vec()).map_err(|e| e.to_string())
    }

    fn has_bundle_identifier() -> bool {
        NSBundle::mainBundle().bundleIdentifier().is_some()
    }

    fn center() -> Result<Retained<UNUserNotificationCenter>, NotifyError> {
        if !has_bundle_identifier() {
            return Err(NotifyError::NoBundleIdentifier);
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        static DELEGATE: Once = Once::new();
        DELEGATE.call_once(|| {
            let delegate = Delegate::new();
            center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            // The center holds its delegate weakly; keep ours for the life
            // of the process.
            std::mem::forget(delegate);
        });
        Ok(center)
    }

    pub(super) fn start() -> Result<(), NotifyError> {
        center().map(|_| ())
    }

    fn error_text(err: *mut NSError) -> Option<String> {
        // SAFETY: completion handlers pass either null or a valid NSError.
        let err = unsafe { err.as_ref() }?;
        Some(format!("Error: {}", err.localizedDescription()))
    }

    fn wait<T>(
        rx: mpsc::Receiver<Result<T, NotifyError>>,
        timeout: Duration,
        what: &str,
    ) -> Result<T, NotifyError> {
        rx.recv_timeout(timeout)
            .unwrap_or_else(|_| Err(NotifyError::Timeout(what.to_string())))
    }

    pub(super) fn check_authorization() -> Result<bool, NotifyError> {
        let center = center()?;
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |settings: NonNull<UNNotificationSettings>| {
            // SAFETY: the settings object is valid for the duration of the call.
            let status = unsafe { settings.as_ref() }.authorizationStatus();
            let _ = tx.send(Ok(status == UNAuthorizationStatus::Authorized));
        });
        center.getNotificationSettingsWithCompletionHandler(&block);
        wait(
            rx,
            Duration::from_secs(15),
            "notification authorization timed out after 15s",
        )
    }

    pub(super) fn request_authorization() -> Result<bool, NotifyError> {
        let center = center()?;
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |granted: Bool, err: *mut NSError| {
            let _ = tx.send(match error_text(err) {
                Some(msg) => Err(NotifyError::Native(msg)),
                None => Ok(granted.as_bool()),
            });
        });
        center.requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert
                | UNAuthorizationOptions::Sound
                | UNAuthorizationOptions::Badge,
            &block,
        );
        wait(
            rx,
            Duration::from_secs(180),
            "notification authorization timed out after 3 minutes",
        )
    }

    pub(super) fn send(opts: &NotificationOptions) -> Result<(), NotifyError> {
        let center = center()?;
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&opts.title));
        content.setBody(&NSString::from_str(&opts.body));
        content.setSound(Some(&UNNotificationSound::defaultSound()));
        if !opts.data.is_empty() {
            let keys: Vec<Retained<NSString>> = opts
                .data
                .iter()
                .map(|(k, _)| NSString::from_str(k))
                .collect();
            let vals: Vec<Retained<NSString>> = opts
                .data
                .iter()
                .map(|(_, v)| NSString::from_str(v))
                .collect();
            let key_refs: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
            let val_refs: Vec<&AnyObject> = vals.iter().map(|v| v.as_ref()).collect();
            let dict = NSDictionary::<NSString, AnyObject>::from_slices(&key_refs, &val_refs);
            // SAFETY: userInfo must be a property-list dictionary; it holds
            // only NSString keys and values.
            unsafe { content.setUserInfo(Retained::cast_unchecked::<NSDictionary>(dict).as_ref()) };
        }
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&opts.id),
            &content,
            None,
        );
        let (tx, rx) = mpsc::channel();
        let block = RcBlock::new(move |err: *mut NSError| {
            let _ = tx.send(match error_text(err) {
                Some(msg) => Err(NotifyError::Native(msg)),
                None => Ok(()),
            });
        });
        center.addNotificationRequest_withCompletionHandler(&request, Some(&block));
        wait(rx, Duration::from_secs(5), "sending notification timed out")
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::{NotificationOptions, NotifyError};

    pub(super) fn start() -> Result<(), NotifyError> {
        Err(NotifyError::Unavailable)
    }

    pub(super) fn check_authorization() -> Result<bool, NotifyError> {
        Err(NotifyError::Unavailable)
    }

    pub(super) fn request_authorization() -> Result<bool, NotifyError> {
        Err(NotifyError::Unavailable)
    }

    pub(super) fn send(_opts: &NotificationOptions) -> Result<(), NotifyError> {
        Err(NotifyError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quotes_like_go() {
        assert_eq!(go_quote("plain"), r#""plain""#);
        assert_eq!(go_quote(r#"say "hi" \ now"#), r#""say \"hi\" \\ now""#);
        assert_eq!(go_quote("a\nb\tc\u{1}"), r#""a\nb\tc\x01""#);
        assert_eq!(go_quote("caf\u{e9} \u{2028}"), "\"caf\u{e9} \\u2028\"");
    }

    #[test]
    fn fallback_commands() {
        let cmd = fallback_command("Title \"x\"", "Body");
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        if cfg!(target_os = "macos") {
            assert_eq!(cmd.get_program(), "osascript");
            assert_eq!(
                args,
                [
                    "-e",
                    r#"display notification "Body" with title "Title \"x\"""#
                ]
            );
        } else {
            assert_eq!(cmd.get_program(), "notify-send");
            assert_eq!(
                args,
                [
                    format!("--app-name={APP_NAME}").as_str(),
                    "Title \"x\"",
                    "Body"
                ]
            );
        }
    }

    #[test]
    fn validation() {
        let mut opts = NotificationOptions {
            title: "t".into(),
            ..NotificationOptions::default()
        };
        assert_eq!(send_native(&opts), Err(NotifyError::EmptyId));
        opts.id = "id".into();
        opts.title.clear();
        assert_eq!(send_native(&opts), Err(NotifyError::EmptyTitle));
    }

    #[test]
    fn unbundled_native_calls_fail_cleanly() {
        // Test binaries are never inside an app bundle.
        let err = start().expect_err("unbundled");
        assert!(
            matches!(
                err,
                NotifyError::NoBundleIdentifier | NotifyError::Unavailable
            ),
            "{err}"
        );
        assert!(check_authorization().is_err());
    }

    #[test]
    fn response_wire_format_and_target() {
        let res: NotificationResponse = serde_json::from_str(
            r#"{"id":"recur-1","actionIdentifier":"DEFAULT_ACTION","categoryIdentifier":"c",
                "title":"T","userInfo":{"target":"inbox:42"}}"#,
        )
        .expect("parse");
        assert_eq!(res.target(), "inbox:42");
        assert_eq!(res.category_id, "c");
        assert_eq!(NotificationResponse::default().target(), "");
        assert_eq!(
            serde_json::to_string(&NotificationResponse {
                id: "x".into(),
                ..NotificationResponse::default()
            })
            .expect("json"),
            r#"{"id":"x"}"#
        );
    }

    #[test]
    fn callback_receives_results() {
        let got = Arc::new(Mutex::new(Vec::new()));
        on_response({
            let got = Arc::clone(&got);
            move |r: NotificationResult| got.lock().push(r.map(|r| r.target().to_string()))
        });
        let mut info = serde_json::Map::new();
        info.insert("target".into(), "t1".into());
        deliver(Ok(NotificationResponse {
            user_info: info,
            ..NotificationResponse::default()
        }));
        deliver(Err(NotifyError::Native("boom".into())));
        assert_eq!(
            *got.lock(),
            [
                Ok("t1".to_string()),
                Err(NotifyError::Native("boom".into()))
            ]
        );
    }
}
