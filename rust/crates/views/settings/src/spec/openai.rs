//! `chatgpt-login.spec.ts` and `codex-login.spec.ts`.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_services::mock::{MockOptions, Reply};
use recur_ui::widgets::ToastKind;
use serde_json::{Value, json};

use super::harness::with;

fn signed_out() -> Value {
    json!({ "authenticated": false, "accountId": "", "email": "", "accounts": [], "modelCount": 0, "error": "" })
}

fn signed_in() -> Value {
    json!({
        "authenticated": true, "accountId": "recur-registration", "email": "direct@example.com",
        "accounts": [{ "id": "recur-registration", "email": "direct@example.com" }],
        "modelCount": 1, "error": "",
    })
}

/// A resolver whose replies stay pending for the rest of the test.
fn held()
-> impl Fn(&[Value], &mut recur_services::mock::Seed, &recur_services::mock::Table) -> Reply
+ Send
+ Sync
+ 'static {
    let kept: Arc<Mutex<Vec<recur_services::mock::Release>>> = Arc::default();
    move |_, _, _| {
        let (reply, release) = Reply::deferred();
        kept.lock().push(release);
        reply
    }
}

const BROWSER_URL: &str =
    "https://auth.openai.com/oauth/authorize?client_id=app_EMoamEEZ73f0CkXaXp7hrann";

#[gpui::test]
fn direct_chatgpt_sign_in_works_with_every_cli_provider_disabled(cx: &mut TestAppContext) {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let start_args: Arc<Mutex<Vec<Value>>> = Arc::default();
    let (r, a) = (refreshes.clone(), start_args.clone());
    let opts = MockOptions::default()
        .resolver("GetChatGPTStatus", |_, s, _| {
            let on = s.settings["chatGPTAccountId"]
                .as_str()
                .is_some_and(|a| !a.is_empty());
            Reply::value(if on { signed_in() } else { signed_out() })
        })
        .resolver("StartChatGPTLogin", move |args, _, _| {
            *a.lock() = args.to_vec();
            Reply::value(
                json!({ "loginId": "direct-login", "authUrl": BROWSER_URL, "method": "browser" }),
            )
        })
        .resolver("WaitChatGPTLogin", |_, s, _| {
            s.settings["chatGPTAccountId"] = json!("recur-registration");
            s.settings["chatGPTModels"] = json!([{ "id": "gpt-direct", "label": "Direct GPT" }]);
            Reply::value(signed_in())
        })
        .resolver("RefreshChatGPTModels", move |_, _, _| {
            r.fetch_add(1, Ordering::SeqCst);
            Reply::value(signed_in())
        })
        .resolver("DisconnectChatGPT", |_, s, _| {
            s.settings["chatGPTAccountId"] = json!("");
            s.settings["chatGPTModels"] = json!([]);
            let mut out = signed_out();
            out["accounts"] = signed_in()["accounts"].clone();
            Reply::value(out)
        });
    let mut s = with(
        cx,
        |sd| sd.settings["disabledProviders"] = json!(["claude-code", "cursor", "codex"]),
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.has("override:Enable OpenAI:true"));
    assert!(s.has("override:Enable ChatGPT:true"));
    assert!(s.has("override:Enable Codex:false"));
    assert!(s.until(|s| s.has(
        "chatgpt-status=Use your ChatGPT plan directly. Recur runs the coding tools. No Codex installation or API key is needed."
    )));
    s.click("btn:Continue with ChatGPT");
    assert!(s.until(|s| s.has("chatgpt-status=direct@example.com · 1 model available")));
    assert_eq!(*start_args.lock(), [json!(""), json!("browser")]);
    assert!(s.toasted(ToastKind::Success, "ChatGPT connected"));
    assert_eq!(s.saved()["chatGPTAccountId"], "recur-registration");
    s.click("btn:Refresh ChatGPT models");
    assert!(s.until(|_| refreshes.load(Ordering::SeqCst) == 1));
    s.click("btn:Sign out of ChatGPT");
    assert!(s.until(|s| s.has("btn:Continue with ChatGPT") && !s.has("btn:Sign out of ChatGPT")));
    assert!(s.has("sel:ChatGPT sign-in account=direct@example.com (recur-registration)"));
}

#[gpui::test]
fn openai_cannot_be_turned_off_when_it_is_the_only_ready_provider(cx: &mut TestAppContext) {
    let opts =
        MockOptions::default().resolver("GetChatGPTStatus", |_, _, _| Reply::value(signed_in()));
    let mut s = with(
        cx,
        |sd| {
            sd.settings["disabledProviders"] = json!(["claude-code", "cursor", "codex"]);
            sd.settings["chatGPTAccountId"] = json!("recur-registration");
            sd.settings["chatGPTModels"] = json!([{ "id": "gpt-direct" }]);
        },
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.until(|s| s.has("btn:Sign out of ChatGPT")));
    s.click("override:Enable OpenAI:true");
    assert!(s.toasted(ToastKind::Error, "Keep at least one provider on"));
    assert!(s.has("override:Enable OpenAI:true"));
}

#[gpui::test]
fn chatgpt_login_cancels_on_cancel_and_on_leaving_providers(cx: &mut TestAppContext) {
    for (method, leave) in [
        ("browser", false),
        ("browser", true),
        ("headless", false),
        ("headless", true),
    ] {
        let cancelled: Arc<Mutex<Vec<Value>>> = Arc::default();
        let c = cancelled.clone();
        let opts = MockOptions::default()
            .resolver("StartChatGPTLogin", move |_, _, _| {
                Reply::value(json!({
                    "loginId": "pending-login", "authUrl": BROWSER_URL, "method": method,
                    "userCode": if method == "headless" { "ABCD-1234" } else { "" },
                }))
            })
            .resolver("WaitChatGPTLogin", held())
            .resolver("CancelChatGPTLogin", move |args, _, _| {
                c.lock().extend(args.iter().cloned());
                Reply::null()
            });
        let mut s = with(cx, |_| {}, opts);
        s.open_settings();
        s.section("Providers");
        if method == "headless" {
            s.pick(
                "ChatGPT sign-in method",
                "Browser",
                "Headless (device code)",
            );
        }
        s.click("btn:Continue with ChatGPT");
        assert!(s.until(|s| s.has("btn:Cancel ChatGPT sign-in")), "{method}");
        assert!(s.has("inp:ChatGPT sign-in link"));
        if leave {
            s.section("General");
        } else {
            s.click("btn:Cancel ChatGPT sign-in");
            assert!(s.has("btn:Continue with ChatGPT"));
        }
        assert!(s.until(|_| *cancelled.lock() == [json!("pending-login")]));
    }
}

#[gpui::test]
fn chatgpt_login_reports_an_authorization_failure_and_allows_retry(cx: &mut TestAppContext) {
    let opts = MockOptions::default()
        .resolver("StartChatGPTLogin", |_, _, _| {
            Reply::value(
                json!({ "loginId": "failed-login", "authUrl": BROWSER_URL, "method": "browser" }),
            )
        })
        .resolver("WaitChatGPTLogin", |_, _, _| {
            Reply::error("OpenAI rejected the session (HTTP 401)")
        });
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Providers");
    s.click("btn:Continue with ChatGPT");
    assert!(s.until(|s| s.has("chatgpt-err=OpenAI rejected the session (HTTP 401)")));
    assert!(s.has("btn:Continue with ChatGPT"));
}

#[gpui::test]
fn headless_chatgpt_sign_in_shows_the_device_url_and_code(cx: &mut TestAppContext) {
    let start_args: Arc<Mutex<Vec<Value>>> = Arc::default();
    let a = start_args.clone();
    let opts = MockOptions::default()
        .resolver("StartChatGPTLogin", move |args, _, _| {
            *a.lock() = args.to_vec();
            Reply::value(json!({
                "loginId": "device-login", "authUrl": "https://auth.openai.com/codex/device",
                "method": "headless", "userCode": "ABCD-1234",
            }))
        })
        .resolver("WaitChatGPTLogin", held());
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Providers");
    s.pick(
        "ChatGPT sign-in method",
        "Browser",
        "Headless (device code)",
    );
    s.click("btn:Continue with ChatGPT");
    assert!(s.until(|s| s.has("chatgpt-code=ABCD-1234")));
    assert_eq!(*start_args.lock(), [json!(""), json!("headless")]);
    assert!(s.has(
        "chatgpt-status=Open the sign-in link in any browser and enter the one-time code. Recur will connect automatically."
    ));
    assert!(s.has("btn:Open sign-in page"));
}

#[gpui::test]
fn codex_sign_in_reports_a_missing_binary_instead_of_disabling_the_button(cx: &mut TestAppContext) {
    let error = "Codex binary not found. Install Codex or set its Binary path.";
    let opts = MockOptions::default()
        .resolver("GetCodexStatus", move |_, _, _| {
            Reply::value(json!({ "binInstalled": false, "binPath": "", "authenticated": false, "error": error }))
        })
        .resolver("StartCodexLogin", move |_, _, _| Reply::error(error));
    let mut s = with(
        cx,
        |sd| sd.settings["codexChatGPTAuth"] = json!(false),
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.until(|s| s.has(&format!("status-head:Bad:{error}"))));
    s.click("btn:Sign in with ChatGPT");
    assert!(s.until(|s| s.has(&format!("codex-login-err={error}"))));
}

#[gpui::test]
fn codex_browser_sign_in_saves_chatgpt_auth(cx: &mut TestAppContext) {
    let signed = Arc::new(AtomicUsize::new(0));
    let g = signed.clone();
    let opts = MockOptions::default()
        .resolver("GetCodexStatus", move |_, _, _| {
            let on = g.load(Ordering::SeqCst) > 0;
            Reply::value(json!({
                "binInstalled": true, "binPath": "/usr/local/bin/codex", "authenticated": on,
                "loginMethod": if on { "ChatGPT account" } else { "" },
                "email": if on { "browser@example.com" } else { "" }, "error": "",
            }))
        })
        .resolver("StartCodexLogin", |_, _, _| {
            Reply::value(json!({ "loginId": "codex-login", "authUrl": "https://auth.openai.com/test-login" }))
        })
        .resolver("WaitCodexLogin", move |_, s, _| {
            signed.fetch_add(1, Ordering::SeqCst);
            s.settings["codexChatGPTAuth"] = json!(true);
            s.settings["codexApiKey"] = json!("");
            Reply::value(json!({
                "binInstalled": true, "binPath": "/usr/local/bin/codex", "authenticated": true,
                "loginMethod": "ChatGPT account", "email": "browser@example.com", "plan": "Plus", "error": "",
            }))
        });
    let mut s = with(
        cx,
        |sd| {
            sd.settings["codexChatGPTAuth"] = json!(false);
            sd.settings["codexApiKey"] = json!("sk-old");
        },
        opts,
    );
    s.open_settings();
    s.section("Providers");
    s.click("btn:Sign in with ChatGPT");
    assert!(s.until(|s| s.toasted(ToastKind::Success, "Signed in to Codex")));
    assert!(s.has("status-head:Ok:Connected"));
    assert!(s.has("btn:Switch ChatGPT account"));
    assert!(s.has("tog:toggle Codex ChatGPT auth:true"));
}

#[gpui::test]
fn codex_browser_sign_in_cancels_on_cancel_and_on_leaving_settings(cx: &mut TestAppContext) {
    for leave in [false, true] {
        let cancelled: Arc<Mutex<Vec<Value>>> = Arc::default();
        let c = cancelled.clone();
        let opts = MockOptions::default()
            .resolver("StartCodexLogin", |_, _, _| {
                Reply::value(json!({ "loginId": "codex-login", "authUrl": "https://auth.openai.com/test-login" }))
            })
            .resolver("WaitCodexLogin", held())
            .resolver("CancelCodexLogin", move |args, _, _| {
                c.lock().extend(args.iter().cloned());
                Reply::null()
            });
        let mut s = with(
            cx,
            |sd| sd.settings["codexChatGPTAuth"] = json!(false),
            opts,
        );
        s.open_settings();
        s.section("Providers");
        s.click("btn:Sign in with ChatGPT");
        assert!(s.until(|s| s.has("btn:Cancel sign-in")));
        assert!(s.has("tog:toggle Codex ChatGPT auth:false"));
        if leave {
            s.section("General");
        } else {
            s.click("btn:Cancel sign-in");
            assert!(s.has("btn:Sign in with ChatGPT"));
        }
        assert!(s.until(|_| *cancelled.lock() == [json!("codex-login")]));
        assert_eq!(s.saved()["codexChatGPTAuth"], json!(false));
    }
}

#[gpui::test]
fn codex_browser_sign_in_shows_login_errors_and_allows_retry(cx: &mut TestAppContext) {
    let tries = Arc::new(AtomicUsize::new(0));
    let t = tries.clone();
    let opts = MockOptions::default()
        .resolver("StartCodexLogin", |_, _, _| {
            Reply::value(json!({ "loginId": "codex-login", "authUrl": "https://auth.openai.com/test-login" }))
        })
        .resolver("WaitCodexLogin", move |_, _, _| {
            if t.fetch_add(1, Ordering::SeqCst) == 0 {
                return Reply::error("Codex sign-in failed: Login rejected");
            }
            Reply::value(json!({ "binInstalled": true, "authenticated": true, "email": "browser@example.com" }))
        });
    let mut s = with(
        cx,
        |sd| sd.settings["codexChatGPTAuth"] = json!(false),
        opts,
    );
    s.open_settings();
    s.section("Providers");
    s.click("btn:Sign in with ChatGPT");
    assert!(s.until(|s| s.has("codex-login-err=Codex sign-in failed: Login rejected")));
    s.click("btn:Sign in with ChatGPT");
    assert!(s.until(|s| s.toasted(ToastKind::Success, "Signed in to Codex")));
    assert!(!s.has("codex-login-err=Codex sign-in failed: Login rejected"));
}

#[gpui::test]
fn saving_the_codex_binary_path_rechecks_discovery_without_leaving_providers(
    cx: &mut TestAppContext,
) {
    let opts = MockOptions::default().resolver("GetCodexStatus", |_, s, _| {
        let path = s.settings["codexBinPath"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        Reply::value(if path.is_empty() {
            json!({ "binInstalled": false, "binPath": "", "error": "Codex binary not found" })
        } else {
            json!({ "binInstalled": true, "binPath": path, "error": "" })
        })
    });
    let mut s = with(
        cx,
        |sd| {
            sd.settings["codexChatGPTAuth"] = json!(false);
            sd.settings["codexBinPath"] = json!("");
        },
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.until(|s| s.has("status-head:Bad:Codex binary not found")));
    s.fill("inp:Codex binary path", "/opt/codex/bin/codex");
    s.keys("enter");
    assert!(s.until(|s| !s.has("status-head:Bad:Codex binary not found")));
    assert_eq!(s.saved()["codexBinPath"], "/opt/codex/bin/codex");
}
