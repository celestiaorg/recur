//! ChatGPT and Codex sign-in.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use gpui::TestAppContext;
use parking_lot::Mutex;
use recur_domain::CompatibleModel;
use recur_services::api::{
    ChatGPTAccount, ChatGPTAuth, ChatGPTLogin, ChatGPTStatus, CodexLogin, CodexStatus,
};
use recur_services::fake::{Answer, Overrides, Release, Seed, hold};
use recur_services::{Error, Result};
use recur_ui::widgets::ToastKind;

use super::harness::with;

fn signed_out() -> ChatGPTStatus {
    ChatGPTStatus::default()
}

fn account() -> ChatGPTAccount {
    ChatGPTAccount {
        id: "recur-registration".into(),
        email: "direct@example.com".into(),
    }
}

fn signed_in() -> ChatGPTStatus {
    ChatGPTStatus {
        status: ChatGPTAuth {
            authenticated: true,
            account_id: "recur-registration".into(),
            email: "direct@example.com".into(),
            accounts: vec![account()],
        },
        model_count: 1,
        error: String::new(),
    }
}

fn chatgpt_login(login_id: &str, auth_url: &str, method: &str, user_code: &str) -> ChatGPTLogin {
    ChatGPTLogin {
        login_id: login_id.into(),
        auth_url: auth_url.into(),
        method: method.into(),
        user_code: user_code.into(),
    }
}

fn codex_login() -> CodexLogin {
    CodexLogin {
        login_id: "codex-login".into(),
        auth_url: "https://auth.openai.com/test-login".into(),
    }
}

fn no_chatgpt_auth(sd: &mut Seed) {
    sd.settings.codex_chat_gpt_auth = false;
}

/// An override whose answers stay pending for the rest of the test.
fn held<T: Send + 'static>() -> impl Fn(&mut Seed, String) -> Answer<Result<T>> + Send + Sync {
    let kept: Arc<Mutex<Vec<Release<Result<T>>>>> = Arc::default();
    move |_, _| {
        let (answer, release) = hold();
        kept.lock().push(release);
        answer
    }
}

fn disabled(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|s| (*s).to_owned()).collect()
}

const BROWSER_URL: &str =
    "https://auth.openai.com/oauth/authorize?client_id=app_EMoamEEZ73f0CkXaXp7hrann";

#[gpui::test]
fn direct_chatgpt_sign_in_works_with_every_cli_provider_disabled(cx: &mut TestAppContext) {
    let refreshes = Arc::new(AtomicUsize::new(0));
    let start_args: Arc<Mutex<Option<(String, String)>>> = Arc::default();
    let (r, a) = (refreshes.clone(), start_args.clone());
    let opts = Overrides::default()
        .get_chat_gpt_status(|s, ()| {
            let on = !s.settings.chat_gpt_account_id.is_empty();
            Ok(if on { signed_in() } else { signed_out() })
        })
        .start_chat_gpt_login(move |_, args| {
            *a.lock() = Some(args);
            Ok(chatgpt_login("direct-login", BROWSER_URL, "browser", ""))
        })
        .wait_chat_gpt_login(|s, _| {
            s.settings.chat_gpt_account_id = "recur-registration".into();
            s.settings.chat_gpt_models = vec![CompatibleModel {
                id: "gpt-direct".into(),
                label: "Direct GPT".into(),
                ..Default::default()
            }];
            Ok(signed_in())
        })
        .refresh_chat_gpt_models(move |_, ()| {
            r.fetch_add(1, Ordering::SeqCst);
            Ok(signed_in())
        })
        .disconnect_chat_gpt(|s, ()| {
            s.settings.chat_gpt_account_id.clear();
            s.settings.chat_gpt_models.clear();
            let mut out = signed_out();
            out.status.accounts = vec![account()];
            Ok(out)
        });
    let mut s = with(
        cx,
        |sd| sd.settings.disabled_providers = disabled(&["claude-code", "cursor", "codex"]),
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
    assert!(s.has("status-head:Bad:Not connected"));
    s.click("btn:Continue with ChatGPT");
    assert!(s.until(|s| s.has("chatgpt-status=direct@example.com · 1 model available")));
    assert!(s.has("status-head:Ok:Connected"));
    assert_eq!(
        *start_args.lock(),
        Some((String::new(), "browser".to_owned()))
    );
    assert!(s.toasted(ToastKind::Success, "ChatGPT connected"));
    assert_eq!(s.saved().chat_gpt_account_id, "recur-registration");
    s.click("btn:Refresh ChatGPT models");
    assert!(s.until(|_| refreshes.load(Ordering::SeqCst) == 1));
    s.click("btn:Sign out of ChatGPT");
    assert!(s.until(|s| s.has("btn:Continue with ChatGPT") && !s.has("btn:Sign out of ChatGPT")));
    assert!(s.has("status-head:Bad:Not connected"));
    assert!(s.has("sel:ChatGPT sign-in account=direct@example.com (recur-registration)"));
}

#[gpui::test]
fn openai_contains_chatgpt_codex_and_api_setup_under_one_provider_toggle(cx: &mut TestAppContext) {
    let mut s = with(
        cx,
        |sd| {
            sd.settings.disabled_providers.clear();
            sd.settings.compatible_providers = vec![recur_domain::CompatibleProvider {
                id: "openai".into(),
                name: "OpenAI".into(),
                base_url: "https://api.openai.com".into(),
                models: vec![CompatibleModel {
                    id: "gpt-api".into(),
                    label: "API GPT".into(),
                    ..Default::default()
                }],
                ..Default::default()
            }];
        },
        Overrides::default(),
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.has("override:Enable ChatGPT:true"));
    assert!(s.has("override:Enable Codex:true"));
    assert!(s.has("override:Enable OpenAI API endpoint:true"));
    let off = |s: &mut super::harness::S<'_>| {
        let off = s.saved().disabled_providers;
        ["chatgpt", "codex", "openai"].map(|p| off.iter().any(|o| o == p))
    };
    s.click("override:Enable OpenAI:true");
    assert!(s.until(|s| off(s) == [true; 3]));
    s.click("override:Enable OpenAI:false");
    assert!(s.until(|s| off(s) == [false; 3]));
}

#[gpui::test]
fn openai_cannot_be_turned_off_when_it_is_the_only_ready_provider(cx: &mut TestAppContext) {
    let opts = Overrides::default().get_chat_gpt_status(|_, ()| Ok(signed_in()));
    let mut s = with(
        cx,
        |sd| {
            sd.settings.disabled_providers = disabled(&["claude-code", "cursor", "codex"]);
            sd.settings.chat_gpt_account_id = "recur-registration".into();
            sd.settings.chat_gpt_models = vec![CompatibleModel {
                id: "gpt-direct".into(),
                ..Default::default()
            }];
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
        let cancelled: Arc<Mutex<Vec<String>>> = Arc::default();
        let c = cancelled.clone();
        let opts = Overrides::default()
            .start_chat_gpt_login(move |_, _| {
                let code = if method == "headless" {
                    "ABCD-1234"
                } else {
                    ""
                };
                Ok(chatgpt_login("pending-login", BROWSER_URL, method, code))
            })
            .wait_chat_gpt_login(held())
            .cancel_chat_gpt_login(move |_, id| {
                c.lock().push(id);
                Ok(())
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
        assert!(s.until(|_| *cancelled.lock() == ["pending-login"]));
    }
}

#[gpui::test]
fn chatgpt_login_reports_an_authorization_failure_and_allows_retry(cx: &mut TestAppContext) {
    let opts = Overrides::default()
        .start_chat_gpt_login(|_, _| Ok(chatgpt_login("failed-login", BROWSER_URL, "browser", "")))
        .wait_chat_gpt_login(|_, _| Err(Error::new("OpenAI rejected the session (HTTP 401)")));
    let mut s = with(cx, |_| {}, opts);
    s.open_settings();
    s.section("Providers");
    s.click("btn:Continue with ChatGPT");
    assert!(s.until(|s| s.has("chatgpt-err=OpenAI rejected the session (HTTP 401)")));
    assert!(s.has("btn:Continue with ChatGPT"));
}

#[gpui::test]
fn headless_chatgpt_sign_in_shows_the_device_url_and_code(cx: &mut TestAppContext) {
    let start_args: Arc<Mutex<Option<(String, String)>>> = Arc::default();
    let a = start_args.clone();
    let opts = Overrides::default()
        .start_chat_gpt_login(move |_, args| {
            *a.lock() = Some(args);
            Ok(chatgpt_login(
                "device-login",
                "https://auth.openai.com/codex/device",
                "headless",
                "ABCD-1234",
            ))
        })
        .wait_chat_gpt_login(held());
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
    assert_eq!(
        *start_args.lock(),
        Some((String::new(), "headless".to_owned()))
    );
    assert!(s.has(
        "chatgpt-status=Open the sign-in link in any browser and enter the one-time code. Recur will connect automatically."
    ));
    assert!(s.has("btn:Open sign-in page"));
}

#[gpui::test]
fn codex_sign_in_reports_a_missing_binary_instead_of_disabling_the_button(cx: &mut TestAppContext) {
    let error = "Codex binary not found. Install Codex or set its Binary path.";
    let opts = Overrides::default()
        .get_codex_status(move |_, ()| {
            Ok(CodexStatus {
                error: error.into(),
                ..Default::default()
            })
        })
        .start_codex_login(move |_, ()| Err(Error::new(error)));
    let mut s = with(cx, no_chatgpt_auth, opts);
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
    let codex = |on: bool| CodexStatus {
        bin_installed: true,
        bin_path: "/usr/local/bin/codex".into(),
        authenticated: on,
        login_method: if on { "ChatGPT account" } else { "" }.into(),
        email: if on { "browser@example.com" } else { "" }.into(),
        ..Default::default()
    };
    let opts = Overrides::default()
        .get_codex_status(move |_, ()| Ok(codex(g.load(Ordering::SeqCst) > 0)))
        .start_codex_login(|_, ()| Ok(codex_login()))
        .wait_codex_login(move |s, _| {
            signed.fetch_add(1, Ordering::SeqCst);
            s.settings.codex_chat_gpt_auth = true;
            s.settings.codex_api_key.clear();
            Ok(CodexStatus {
                plan: "Plus".into(),
                ..codex(true)
            })
        });
    let mut s = with(
        cx,
        |sd| {
            sd.settings.codex_chat_gpt_auth = false;
            sd.settings.codex_api_key = "sk-old".into();
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
        let cancelled: Arc<Mutex<Vec<String>>> = Arc::default();
        let c = cancelled.clone();
        let opts = Overrides::default()
            .start_codex_login(|_, ()| Ok(codex_login()))
            .wait_codex_login(held())
            .cancel_codex_login(move |_, id| {
                c.lock().push(id);
                Ok(())
            });
        let mut s = with(cx, no_chatgpt_auth, opts);
        s.open_settings();
        s.section("Providers");
        s.click("btn:Sign in with ChatGPT");
        assert!(s.until(|s| s.has("btn:Cancel sign-in")));
        assert!(s.has("tog:toggle Codex ChatGPT auth:false"));
        s.fill("inp:Codex API key", "sk-typed");
        if leave {
            s.section("General");
        } else {
            s.click("btn:Cancel sign-in");
            assert!(s.has("btn:Sign in with ChatGPT"));
        }
        assert!(s.until(|_| *cancelled.lock() == ["codex-login"]));
        assert!(!s.saved().codex_chat_gpt_auth);
        if !leave {
            s.click("btn:Save");
            assert_ne!(s.saved().codex_api_key, "sk-typed");
        }
    }
}

#[gpui::test]
fn codex_browser_sign_in_shows_login_errors_and_allows_retry(cx: &mut TestAppContext) {
    let tries = Arc::new(AtomicUsize::new(0));
    let t = tries.clone();
    let opts = Overrides::default()
        .start_codex_login(|_, ()| Ok(codex_login()))
        .wait_codex_login(move |_, _| {
            if t.fetch_add(1, Ordering::SeqCst) == 0 {
                return Err(Error::new("Codex sign-in failed: Login rejected"));
            }
            Ok(CodexStatus {
                bin_installed: true,
                authenticated: true,
                email: "browser@example.com".into(),
                ..Default::default()
            })
        });
    let mut s = with(cx, no_chatgpt_auth, opts);
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
    let opts = Overrides::default().get_codex_status(|s, ()| {
        let path = s.settings.codex_bin_path.clone();
        Ok(if path.is_empty() {
            CodexStatus {
                error: "Codex binary not found".into(),
                ..Default::default()
            }
        } else {
            CodexStatus {
                bin_installed: true,
                bin_path: path,
                ..Default::default()
            }
        })
    });
    let mut s = with(
        cx,
        |sd| {
            sd.settings.codex_chat_gpt_auth = false;
            sd.settings.codex_bin_path.clear();
        },
        opts,
    );
    s.open_settings();
    s.section("Providers");
    assert!(s.until(|s| s.has("status-head:Bad:Codex binary not found")));
    s.fill("inp:Codex binary path", "/opt/codex/bin/codex");
    s.keys("enter");
    assert!(s.until(|s| !s.has("status-head:Bad:Codex binary not found")));
    assert_eq!(s.saved().codex_bin_path, "/opt/codex/bin/codex");
}
