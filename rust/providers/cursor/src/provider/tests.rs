//! The provider surface: availability and caps, the router's Cursor
//! self-heal and turned-off wrap (Go `runAgent` cases of
//! `provider_test.go`), the Settings probes (`cursor_service.go`), the
//! catalog, worktree release and settings changes, against a fake bridge.

use std::sync::Arc;
use std::time::Duration;

use recur_agent::{AgentEvent, CredentialDraft, ErrorKind, Provider, TurnContext, TurnRequest};
use recur_domain::Settings;
use tokio_util::sync::CancellationToken;

use super::*;
use crate::testserver::write_fake_cursor_install;
use crate::turn::tests::{Rec, kinds};

struct Fixture {
    data: tempfile::TempDir,
    home: tempfile::TempDir,
    p: CursorProvider,
}

fn keyed() -> Settings {
    Settings {
        cursor_api_key: "sk-test".into(),
        ..Default::default()
    }
}

fn disabled() -> Settings {
    Settings {
        disabled_providers: vec![PROVIDER_CURSOR.into()],
        ..keyed()
    }
}

fn fixture(install: bool, set: Settings) -> Fixture {
    let data = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    if install {
        write_fake_cursor_install(data.path());
    }
    let p = CursorProvider::new(CursorConfig {
        data_dir: data.path().to_path_buf(),
        settings: set,
        home: Some(home.path().to_path_buf()),
    });
    Fixture { data, home, p }
}

fn marker(f: &Fixture, name: &str, body: &str) {
    std::fs::write(f.data.path().join("cursor-bridge/bin").join(name), body).unwrap();
}

async fn turn(
    p: &CursorProvider,
    wt: &Path,
    cancel: CancellationToken,
) -> (Result<TurnResult, ProviderError>, Vec<AgentEvent>) {
    let rec = Rec::default();
    let req = TurnRequest {
        worktree: wt.to_path_buf(),
        model: "composer-2.5".into(),
        prompt: "go".into(),
        ..Default::default()
    };
    let res = p.run_turn(req, TurnContext::new(cancel, rec.sink())).await;
    (res, rec.all())
}

#[test]
fn identity_and_caps() {
    let f = fixture(false, Settings::default());
    assert_eq!((f.p.id(), f.p.label()), ("cursor", "Cursor"));
    let c = f.p.caps();
    assert_eq!(c.permission, recur_agent::PermissionRoute::None);
    assert_eq!(c.steer, recur_agent::SteerSupport::LiveOrRestart);
    assert!(
        !c.can_disable_tools && !c.native_fork && c.images && c.user_mcp.http && c.user_mcp.stdio
    );
}

#[test]
fn availability_wording() {
    let f = fixture(false, Settings::default());
    assert_eq!(
        f.p.availability(&Settings::default()),
        Availability::Unavailable(
            "a role is set to a Cursor model but Cursor is not connected (Settings → Providers → Connect)".into()
        )
    );
    assert_eq!(
        f.p.availability(&disabled()),
        Availability::Unavailable("Cursor is turned off in Settings → Providers".into())
    );
    assert_eq!(f.p.availability(&keyed()), Availability::Ready);
}

#[test]
fn model_helpers() {
    let f = fixture(false, Settings::default());
    let flags: Flags = [("context".to_owned(), "1m".to_owned())].into();
    assert_eq!(f.p.resolve_model("composer-2.5", &flags), "composer-2.5");
    assert_eq!(f.p.context_window("composer-2.5", &Flags::new()), 200_000);
    assert_eq!(f.p.context_window("gpt-5.6-sol", &flags), 1_000_000);
    assert!(
        f.p.oneshot_flags("composer-2.5").is_empty(),
        "no default effort for Cursor"
    );
    assert_eq!(f.p.model_label("composer-2.5"), "composer-2.5");
}

#[test]
fn resume_handles() {
    let f = fixture(false, Settings::default());
    assert!(f.p.resume_handle_fits(""));
    assert!(f.p.resume_handle_fits("agent-123"));
    assert!(!f.p.resume_handle_fits("0b9a6c2e-uuid"));
}

#[test]
fn display_delegates_to_cursor_vocabulary() {
    let f = fixture(false, Settings::default());
    assert_eq!(
        f.p.display_tool("shell", r#"{"command":"ls"}"#).name,
        "Bash"
    );
    assert_eq!(
        recur_agent::display::display_tool_result(
            &f.p,
            "read",
            r#"{"status":"success","value":{"content":"x"}}"#
        ),
        "x"
    );
    assert_eq!(
        recur_agent::display::summarize_display_input(&f.p, "read", r#"{"path":"/wt/a.go"}"#),
        "/wt/a.go"
    );
}

#[test]
fn died_before_turn() {
    assert!(cursor_died_before_turn(&Error::msg(
        "cursor create agent: dial tcp: connection refused"
    )));
    assert!(!cursor_died_before_turn(&Error::msg(
        "cursor run: dial tcp: connection refused"
    )));
    assert!(!cursor_died_before_turn(&Error::msg(
        "cursor auth failed: connection refused"
    )));
    assert!(!cursor_died_before_turn(&Error::msg("some other error")));
}

#[tokio::test]
async fn run_turn_unavailable_without_key() {
    let f = fixture(true, Settings::default());
    let wt = tempfile::tempdir().unwrap();
    let (res, _) = turn(&f.p, wt.path(), CancellationToken::new()).await;
    let e = res.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
    assert!(e.message.contains("Connect"), "{}", e.message);
}

#[tokio::test]
async fn run_turn_through_the_pool() {
    let f = fixture(true, keyed());
    let wt = tempfile::tempdir().unwrap();
    let (res, evs) = turn(&f.p, wt.path(), CancellationToken::new()).await;
    let r = res.unwrap();
    assert!(r.text.starts_with("fake ok "), "{r:?}");
    assert!(r.session_id.starts_with("agent-fake-"));
    assert_eq!(r.duration_ms, 7);
    assert_eq!(kinds(&evs), ["init", "result"]);
    // The reservation is released after the turn.
    let p = f.p.pool.state.lock().await;
    let e = p.entries.values().next().unwrap();
    assert_eq!(
        e.runner
            .as_ref()
            .unwrap()
            .in_use
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    drop(p);
    // The bridge was started with the attribution config in place.
    assert!(f.data.path().join("cursor-attribution.json").is_file());
    f.p.shutdown().await;
}

/// A bridge whose port dies before anything streamed (CreateAgent refused)
/// is replaced and the turn retried once.
#[tokio::test]
async fn run_turn_self_heals_a_dead_port() {
    let f = fixture(true, keyed());
    marker(&f, "refuse-once", "");
    let wt = tempfile::tempdir().unwrap();
    let (res, evs) = turn(&f.p, wt.path(), CancellationToken::new()).await;
    let r = res.unwrap_or_else(|e| panic!("self-heal failed: {}", e.message));
    assert!(r.text.starts_with("fake ok "));
    assert!(!f.data.path().join("cursor-bridge/bin/refuse-once").exists());
    assert_eq!(
        kinds(&evs),
        ["init", "result"],
        "nothing streamed from the dead bridge"
    );
    f.p.shutdown().await;
}

/// Cursor turned off mid-turn: the failure says so and the bridge is dropped.
#[tokio::test]
async fn run_turn_wraps_cursor_off_error() {
    let f = Arc::new(fixture(true, keyed()));
    marker(&f, "send-mode", "slowfail");
    let wt = tempfile::tempdir().unwrap();
    let f2 = f.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        f2.p.settings_changed(&keyed(), &disabled()).await;
    });
    let (res, _) = turn(&f.p, wt.path(), CancellationToken::new()).await;
    let e = res.unwrap_err();
    assert_eq!(e.kind, ErrorKind::Unavailable);
    assert!(
        e.message.starts_with(
            "Cursor is turned off in Settings → Providers: cursor run ended without a result"
        ),
        "{}",
        e.message
    );
    assert!(f.p.pool.state.lock().await.entries.is_empty());
}

#[tokio::test]
async fn run_turn_preserves_cancel_when_cursor_off() {
    let f = Arc::new(fixture(true, keyed()));
    marker(&f, "send-mode", "slow");
    let wt = tempfile::tempdir().unwrap();
    let cancel = CancellationToken::new();
    let (f2, c2) = (f.clone(), cancel.clone());
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        f2.p.settings_changed(&keyed(), &disabled()).await;
        c2.cancel();
    });
    let (res, _) = turn(&f.p, wt.path(), cancel).await;
    let e = res.unwrap_err();
    assert!(e.is_canceled(), "{e:?}");
    f.p.shutdown().await;
}

#[tokio::test]
async fn status_probes() {
    let f = fixture(false, keyed());
    let s = f.p.cursor_status(&keyed()).await;
    assert!(
        !s.bridge_installed && s.error.starts_with("cursor bridge not found"),
        "{s:?}"
    );

    let f = fixture(true, keyed());
    std::fs::write(
        f.data.path().join("cursor-bridge/manifest.json"),
        r#"{"sdkVersion":"1.0.27"}"#,
    )
    .unwrap();
    // No Connect session file: no probe, just the install and the pin.
    let s = f.p.cursor_status(&keyed()).await;
    assert!(s.bridge_installed && !s.authenticated);
    assert_eq!(
        (s.bridge_version.as_str(), s.pinned_version.as_str()),
        ("1.0.27", crate::PINNED_SDK_VERSION)
    );
    assert!(s.update_available);
    assert!(f.p.cached_models(&keyed()).is_empty());

    std::fs::write(f.data.path().join(crate::CURSOR_USAGE_AUTH_FILE), "{}").unwrap();
    let s = f.p.cursor_status(&keyed()).await;
    assert!(s.authenticated, "{s:?}");
    assert_eq!(s.model_count, 3);
    assert_eq!(
        (
            s.email.as_str(),
            s.account.as_str(),
            s.api_key_name.as_str(),
            s.name.as_str()
        ),
        ("dev@example.com", "dev@example.com", "Recur laptop", "Ada")
    );
    // The probe warms the offline catalog.
    let cached: Vec<String> =
        f.p.cached_models(&keyed())
            .into_iter()
            .map(|m| m.model_ref)
            .collect();
    assert_eq!(cached, ["cursor/composer-2.5", "cursor/gpt-5.6-sol"]);
    assert!(f.p.cached_models(&disabled()).is_empty());

    let bad = Settings {
        cursor_api_key: "sk-bad".into(),
        ..Default::default()
    };
    let s = f.p.cursor_status(&bad).await;
    assert_eq!(s.error, "that API key was rejected by Cursor");
    let ps: ProviderStatus = f.p.status(&bad).await;
    assert_eq!(ps.error, "that API key was rejected by Cursor");
}

#[tokio::test]
async fn test_credentials() {
    let f = fixture(true, Settings::default());
    let s =
        f.p.test_credentials(&Settings::default(), &CredentialDraft::default())
            .await;
    assert_eq!(s.error, "Enter an API key first.");
    let s =
        f.p.test_credentials(
            &Settings::default(),
            &CredentialDraft {
                api_key: "sk-draft".into(),
                ..Default::default()
            },
        )
        .await;
    assert!(
        s.authenticated && s.installed && s.model_count == 3,
        "{s:?}"
    );
    // An empty draft falls back to the saved key.
    let s =
        f.p.test_credentials(&keyed(), &CredentialDraft::default())
            .await;
    assert!(s.authenticated);
}

#[tokio::test]
async fn live_models() {
    let f = fixture(true, keyed());
    assert!(
        f.p.models(&Settings::default()).await.unwrap().is_empty(),
        "unconnected: no group"
    );
    assert!(f.p.models(&disabled()).await.unwrap().is_empty());
    let refs: Vec<String> =
        f.p.models(&keyed())
            .await
            .unwrap()
            .into_iter()
            .map(|m| m.model_ref)
            .collect();
    assert_eq!(refs, ["cursor/composer-2.5", "cursor/gpt-5.6-sol"]);
    assert_eq!(f.p.cached_models(&keyed()).len(), 2);
    let none = fixture(false, keyed());
    assert!(
        none.p.models(&keyed()).await.unwrap().is_empty(),
        "not installed: no group"
    );
}

#[tokio::test]
async fn release_worktree_drops_bridge_and_project_dirs() {
    let f = fixture(true, keyed());
    let wt = tempfile::tempdir().unwrap();
    let (res, _) = turn(&f.p, wt.path(), CancellationToken::new()).await;
    res.unwrap();
    let slug = crate::cursor_project_slug(&wt.path().to_string_lossy());
    let proj = f.home.path().join(".cursor/projects").join(&slug);
    assert!(proj.join("terminals").is_dir());
    f.p.release_worktree(wt.path()).await.unwrap();
    assert!(!proj.exists());
    assert!(f.p.pool.state.lock().await.entries.is_empty());
    f.p.release_worktree(wt.path()).await.unwrap();

    // A failure is the sentence the workspace feed shows.
    use std::os::unix::fs::PermissionsExt;
    let projects = f.home.path().join(".cursor/projects");
    std::fs::create_dir_all(projects.join(&slug).join("x")).unwrap();
    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o500)).unwrap();
    let err = f.p.release_worktree(wt.path()).await.unwrap_err();
    std::fs::set_permissions(&projects, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        err.message
            .starts_with("Could not remove Cursor project dirs: "),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn settings_changed_writes_attribution_and_closes_when_off() {
    let f = fixture(true, keyed());
    let wt = tempfile::tempdir().unwrap();
    turn(&f.p, wt.path(), CancellationToken::new())
        .await
        .0
        .unwrap();
    let on = Settings {
        cursor_attribute_commits: true,
        ..keyed()
    };
    f.p.settings_changed(&keyed(), &on).await;
    let body = std::fs::read_to_string(f.data.path().join("cursor-attribution.json")).unwrap();
    assert_eq!(
        body,
        "{\n  \"attributeCommitsToAgent\": true,\n  \"attributePRsToAgent\": false\n}\n"
    );
    assert_eq!(
        f.p.pool.state.lock().await.entries.len(),
        1,
        "a toggle keeps warm bridges"
    );
    f.p.settings_changed(&on, &disabled()).await;
    assert!(
        f.p.pool.state.lock().await.entries.is_empty(),
        "off with nothing running closes now"
    );
    assert_eq!(
        f.p.availability(&f.p.settings()),
        Availability::Unavailable("Cursor is turned off in Settings → Providers".into())
    );
}

#[tokio::test]
async fn on_idle_finishes_a_deferred_close() {
    let f = fixture(true, keyed());
    let wt = tempfile::tempdir().unwrap();
    let held =
        f.p.pool
            .ensure_runner(&CancellationToken::new(), &keyed(), wt.path())
            .await
            .unwrap();
    f.p.settings_changed(&keyed(), &disabled()).await;
    assert_eq!(
        f.p.pool.state.lock().await.entries.len(),
        1,
        "a turn in flight keeps its bridge"
    );
    drop(held);
    // Turning Cursor back on cancels the deferred close.
    f.p.settings_changed(&disabled(), &keyed()).await;
    f.p.on_idle().await;
    assert_eq!(f.p.pool.state.lock().await.entries.len(), 1);
    let held =
        f.p.pool
            .ensure_runner(&CancellationToken::new(), &keyed(), wt.path())
            .await
            .unwrap();
    f.p.settings_changed(&keyed(), &disabled()).await;
    drop(held);
    f.p.on_idle().await;
    assert!(f.p.pool.state.lock().await.entries.is_empty());
}
