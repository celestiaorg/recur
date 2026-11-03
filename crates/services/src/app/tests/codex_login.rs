//! Codex sign-in over the services: a successful browser sign-in
//! switches from an API key to ChatGPT auth; a failed or cancelled one leaves
//! the saved auth alone. The sign-in worker is tested in
//! `recur_provider_codex::login`.

use recur_provider_codex::login::fake_login_server;

use super::Fx;

async fn fixture(mode: &str) -> Fx {
    let fx = Fx::new().await;
    let bin = fake_login_server(&fx.root, mode);
    let mut set = fx.s.settings.get_settings().await.unwrap();
    set.codex_bin_path = bin.to_string_lossy().into_owned();
    set.codex_api_key = "saved-api-key".into();
    set.codex_chat_gpt_auth = false;
    fx.s.settings.set_settings(&set).await.unwrap();
    fx
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_browser_login() {
    let fx = fixture("success").await;
    let login = fx.s.settings.start_codex_login().await.unwrap();
    assert_eq!(login.auth_url, "https://auth.openai.com/test-login");
    let status =
        fx.s.settings
            .wait_codex_login(&login.login_id)
            .await
            .unwrap();
    assert!(
        status.authenticated && status.email == "recur@example.com",
        "{status:?}"
    );
    let set = fx.s.settings.get_settings().await.unwrap();
    assert!(
        set.codex_chat_gpt_auth && set.codex_api_key.is_empty(),
        "successful login did not switch from API key to ChatGPT auth"
    );
    assert!(fx.root.join("codex-home/auth.json").exists());
    // A new probe process must work with only Recur's saved credentials.
    let status = fx.s.settings.get_codex_status().await.unwrap();
    assert!(
        status.authenticated,
        "saved login could not be reused: {status:?}"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn codex_browser_login_failure_and_cancel_keep_saved_auth() {
    let fx = fixture("failed").await;
    let login = fx.s.settings.start_codex_login().await.unwrap();
    let err =
        fx.s.settings
            .wait_codex_login(&login.login_id)
            .await
            .unwrap_err();
    assert!(err.message.contains("Login rejected"), "{}", err.message);
    std::fs::write(fx.root.join("mode"), "pending").unwrap();
    let login = fx.s.settings.start_codex_login().await.unwrap();
    fx.s.settings
        .cancel_codex_login(&login.login_id)
        .await
        .unwrap();
    assert!(
        fx.s.settings
            .wait_codex_login(&login.login_id)
            .await
            .is_err()
    );
    let set = fx.s.settings.get_settings().await.unwrap();
    assert!(
        !set.codex_chat_gpt_auth && set.codex_api_key == "saved-api-key",
        "a failed or cancelled login changed the saved auth"
    );
    let mut off = set.clone();
    off.disabled_providers.push("codex".into());
    fx.s.settings.set_settings(&off).await.unwrap();
    assert!(
        fx.s.settings.start_codex_login().await.is_err(),
        "started a login with Codex off"
    );
    fx.close().await;
}
