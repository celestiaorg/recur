//! `settings_mcp_test.go`, `settings_prefix_test.go` and the service cases
//! of `provider_usage_test.go` (the usage poller itself is covered in
//! recur-usage; Go's nil-poller case cannot occur here).

use recur_domain::{
    BranchPrefixMode, CompatibleFlagDim, CompatibleFlagOption, CompatibleModel, CompatibleProvider,
    DEFAULT_BRANCH_NAME_PREFIX, DEFAULT_MAX_LOOPS, McpServer, Settings,
};

use super::Fx;

impl Fx {
    async fn settings(&self) -> Settings {
        self.s.settings.get_settings().await.unwrap()
    }

    async fn save(&self, set: &Settings) -> Settings {
        self.s.settings.set_settings(set).await.unwrap()
    }
}

fn mcp(id: &str, name: &str, url: &str) -> McpServer {
    McpServer {
        id: id.into(),
        name: name.into(),
        enabled: true,
        url: url.into(),
        ..Default::default()
    }
}

fn openai(base_url: &str, models: Vec<CompatibleModel>) -> CompatibleProvider {
    CompatibleProvider {
        id: "openai".into(),
        name: "OpenAI".into(),
        base_url: base_url.into(),
        api_key: "sk-keep".into(),
        models,
        ..Default::default()
    }
}

fn model(id: &str, label: &str, flags: Vec<CompatibleFlagDim>) -> CompatibleModel {
    CompatibleModel {
        id: id.into(),
        label: label.into(),
        context_window: 128_000,
        flags,
    }
}

fn opt(value: &str, label: &str, default: bool) -> CompatibleFlagOption {
    CompatibleFlagOption {
        value: value.into(),
        label: label.into(),
        default,
    }
}

fn flag(
    id: &str,
    label: &str,
    wire: &str,
    options: Vec<CompatibleFlagOption>,
) -> CompatibleFlagDim {
    CompatibleFlagDim {
        id: id.into(),
        label: label.into(),
        wire: wire.into(),
        options,
    }
}

/// Saves `good`, then asserts `bad` is refused with `want` in the error and
/// the stored providers are still the saved `good`.
async fn refuses_compatible(good: CompatibleProvider, bad: CompatibleProvider, want: &str) {
    let fx = Fx::new().await;
    let mut set = fx.settings().await;
    set.compatible_providers = vec![good];
    let saved = fx.save(&set).await.compatible_providers;
    let mut set = fx.settings().await;
    set.compatible_providers = vec![bad];
    let e = fx.s.settings.set_settings(&set).await.unwrap_err();
    assert!(e.message.contains(want), "{e}");
    assert_eq!(fx.settings().await.compatible_providers, saved);
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_rejects_duplicate_mcp() {
    let fx = Fx::new().await;
    let keep = mcp("acme", "Acme", "https://keep.example/mcp");
    let mut set = fx.settings().await;
    set.mcp_servers = vec![keep.clone()];
    fx.save(&set).await;
    let mut set = fx.settings().await;
    set.mcp_servers = vec![keep, mcp("acme", "Acme 2", "https://clobber.example/mcp")];
    let e = fx.s.settings.set_settings(&set).await.unwrap_err();
    assert!(e.message.contains("duplicate"), "{e}");
    let stored = fx.settings().await.mcp_servers;
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].url, "https://keep.example/mcp");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_rejects_reserved_mcp() {
    let fx = Fx::new().await;
    let mut set = fx.settings().await;
    set.mcp_servers = vec![mcp("acme", "Acme", "https://keep.example/mcp")];
    fx.save(&set).await;
    let mut set = fx.settings().await;
    set.mcp_servers.push(McpServer {
        id: "recur".into(),
        enabled: true,
        ..Default::default()
    });
    assert!(fx.s.settings.set_settings(&set).await.is_err());
    let stored = fx.settings().await.mcp_servers;
    assert_eq!(
        stored.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
        ["acme"]
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_rejects_invalid_compatible_provider() {
    let good = openai(
        "https://api.openai.com",
        vec![model("gpt-5", "GPT-5", vec![])],
    );
    let bad = openai("not-a-url", vec![model("gpt-5", "", vec![])]);
    refuses_compatible(good, bad, "").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_rejects_duplicate_compatible_model() {
    let good = openai(
        "https://api.openai.com",
        vec![
            model("gpt-5", "GPT-5", vec![]),
            model("gpt-4o", "GPT-4o", vec![]),
        ],
    );
    let mut clobber = model("gpt-5", "Clobber", vec![]);
    clobber.context_window = 1;
    let bad = openai(
        "https://api.openai.com",
        vec![model("gpt-5", "GPT-5", vec![]), clobber],
    );
    refuses_compatible(good, bad, "duplicate model id").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_rejects_duplicate_compatible_flag_id() {
    let opts = || vec![opt("low", "", false), opt("medium", "", true)];
    let with = |second: &str| {
        openai(
            "https://api.openai.com",
            vec![model(
                "gpt-5",
                "GPT-5",
                vec![
                    flag("effort", "Effort", "reasoning.effort", opts()),
                    flag(second, "Verbosity", "text.verbosity", opts()),
                ],
            )],
        )
    };
    refuses_compatible(with("verbosity"), with("effort"), "duplicate flag id").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_rejects_duplicate_compatible_flag_option() {
    let with = |second: &str| {
        openai(
            "https://api.openai.com",
            vec![model(
                "gpt-5",
                "GPT-5",
                vec![flag(
                    "effort",
                    "Effort",
                    "reasoning.effort",
                    vec![
                        opt("low", "Low", false),
                        opt(second, "Medium", true),
                        opt("high", "High", false),
                    ],
                )],
            )],
        )
    };
    refuses_compatible(with("medium"), with("low"), "duplicate flag option").await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_normalizes_branch_name_prefix() {
    let fx = Fx::new().await;
    let mut set = fx.settings().await;
    assert_eq!(
        set.branch_name_prefix.as_deref(),
        Some(DEFAULT_BRANCH_NAME_PREFIX)
    );
    set.branch_name_prefix = Some("My Team".into());
    let mut got = fx.save(&set).await;
    assert_eq!(got.branch_name_prefix.as_deref(), Some("my-team"));
    got.branch_name_prefix = Some("!!!".into());
    let mut got = fx.save(&got).await;
    assert_eq!(got.branch_name_prefix.as_deref(), Some(""));
    assert_eq!(got.branch_prefix_mode, BranchPrefixMode::None);
    got.branch_name_prefix = Some(String::new());
    let got = fx.save(&got).await;
    assert_eq!(got.branch_name_prefix.as_deref(), Some(""));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_governor_caps() {
    let fx = Fx::new().await;
    let mut set = fx.settings().await;
    set.max_loops = 0;
    set.max_agents = 128;
    let mut got = fx.save(&set).await;
    assert_eq!(
        (got.max_loops, got.max_agents),
        (0, 128),
        "unlimited, no upper clamp"
    );
    got.max_loops = -1;
    got.max_agents = 3;
    let got = fx.save(&got).await;
    assert_eq!((got.max_loops, got.max_agents), (DEFAULT_MAX_LOOPS, 3));
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_settings_turning_cursor_off_clears_key() {
    let fx = Fx::new().await;
    let mut set = fx.store().settings();
    set.cursor_api_key = "crsr_on".into();
    fx.save(&set).await;
    let mut set = fx.store().settings();
    set.disabled_providers.push("cursor".into());
    let got = fx.save(&set).await;
    assert_eq!(got.cursor_api_key, "");
    assert_eq!(fx.store().settings().cursor_api_key, "");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn disconnect_cursor_usage_clears_settings_key() {
    let fx = Fx::new().await;
    let mut set = fx.store().settings();
    set.cursor_api_key = "crsr_leftover".into();
    fx.save(&set).await;
    fx.s.settings.disconnect_cursor_usage().await.unwrap();
    assert_eq!(fx.store().settings().cursor_api_key, "");
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn drop_orphan_cursor_key() {
    let fx = Fx::new().await;
    let mut set = fx.store().settings();
    set.cursor_api_key = "crsr_leftover".into();
    fx.save(&set).await;
    let empty = fx.temp();
    let got = fx
        .svc
        .usage
        .drop_orphan_cursor_key(fx.store().settings(), std::path::Path::new(&empty))
        .await;
    assert_eq!(got.cursor_api_key, "");
    assert_eq!(fx.store().settings().cursor_api_key, "");

    let dir = fx.temp();
    super::write(
        format!("{dir}/cursor-usage-auth.json"),
        r#"{"accessToken":"sess"}"#,
    );
    let mut set = fx.store().settings();
    set.cursor_api_key = "crsr_kept".into();
    fx.save(&set).await;
    let got = fx
        .svc
        .usage
        .drop_orphan_cursor_key(fx.store().settings(), std::path::Path::new(&dir))
        .await;
    assert_eq!(
        got.cursor_api_key, "crsr_kept",
        "a Connect file keeps the key"
    );
    fx.close().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn resolve_runner_compatible() {
    let fx = Fx::new().await;
    let reg = fx.svc.engine().providers();
    assert!(reg.get("openai").is_none());
    let mut set = fx.settings().await;
    set.compatible_providers = vec![openai(
        "https://api.openai.com",
        vec![model("gpt-5", "GPT-5", vec![])],
    )];
    fx.save(&set).await;
    let p = reg.get("openai").expect("a saved endpoint is routable");
    assert_eq!((p.id(), p.label()), ("openai", "OpenAI"));
    fx.close().await;
}
