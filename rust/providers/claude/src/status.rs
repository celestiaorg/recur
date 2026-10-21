//! The Settings → Providers card for Claude Code (Go `app/claude_status.go`):
//! `claude --version` and `claude auth status`, each bounded on its own so a
//! slow version probe cannot starve the auth probe and misreport a logged-in
//! user. Never invents a success state.

use std::time::Duration;

use recur_agent::ProviderStatus;
use serde::Deserialize;
use tokio::process::Command;

use crate::proc::look_path;

/// Each CLI probe's budget.
pub const CMD_TIMEOUT: Duration = Duration::from_secs(5);

/// Probes the local CLI. `claude auth status` exits non-zero when logged out
/// but still prints JSON: that reads as "Not logged in", not "not detected",
/// which is reserved for a missing binary.
pub async fn probe(bin: &str) -> ProviderStatus {
    let path = look_path(bin)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let version = match tokio::time::timeout(CMD_TIMEOUT, output(bin, &["--version"])).await {
        Ok(Ok((out, true))) => parse_version(&String::from_utf8_lossy(&out)),
        _ => String::new(),
    };
    match tokio::time::timeout(CMD_TIMEOUT, output(bin, &["auth", "status"])).await {
        Ok(Ok((out, _))) => classify(&path, &version, &out, false, false),
        Ok(Err(e)) => classify(
            &path,
            &version,
            &[],
            e.kind() == std::io::ErrorKind::NotFound,
            false,
        ),
        Err(_) => classify(&path, &version, &[], false, true),
    }
}

/// stdout and whether the command exited 0. The child is killed if the
/// caller's timeout drops this future.
async fn output(bin: &str, args: &[&str]) -> std::io::Result<(Vec<u8>, bool)> {
    let out = Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    Ok((out.stdout, out.status.success()))
}

/// Turns path, version and the auth-status output into the card.
/// `binary_missing` is a spawn failure for a missing `claude`; `timed_out`
/// is the auth probe hitting [`CMD_TIMEOUT`].
pub fn classify(
    path: &str,
    version: &str,
    out: &[u8],
    binary_missing: bool,
    timed_out: bool,
) -> ProviderStatus {
    if let Some(mut st) = parse_auth_status(out) {
        st.path = path.into();
        st.installed = !path.is_empty();
        st.version = version.into();
        return st;
    }
    if path.is_empty() && version.is_empty() && binary_missing {
        return ProviderStatus {
            error: "Claude Code not detected".into(),
            ..Default::default()
        };
    }
    ProviderStatus {
        installed: !path.is_empty(),
        path: path.into(),
        version: version.into(),
        error: if timed_out {
            "Status check timed out"
        } else {
            "Not logged in"
        }
        .into(),
        ..Default::default()
    }
}

/// Whether a spawn error means `claude` is not installed.
pub fn binary_missing(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::NotFound
}

/// The version token of `2.1.224 (Claude Code)`.
pub fn parse_version(raw: &str) -> String {
    let s = raw.trim();
    s.split([' ', '\t', '\n'])
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// A logged-in `claude auth status` payload; `None` when logged out or not
/// JSON.
pub fn parse_auth_status(out: &[u8]) -> Option<ProviderStatus> {
    #[derive(Default, Deserialize)]
    #[serde(default, rename_all = "camelCase")]
    struct Auth {
        logged_in: Option<bool>,
        auth_method: Option<String>,
        api_provider: Option<String>,
        email: Option<String>,
        org_name: Option<String>,
        subscription_type: Option<String>,
    }
    let info: Auth = serde_json::from_slice(out).ok()?;
    if info.logged_in != Some(true) {
        return None;
    }
    let s = |v: Option<String>| v.unwrap_or_default();
    let email = s(info.email).trim().to_owned();
    Some(ProviderStatus {
        authenticated: true,
        provider: format_provider(&s(info.api_provider)),
        login_method: format_login_method(&s(info.auth_method), &s(info.subscription_type)),
        organization: s(info.org_name).trim().to_owned(),
        account: email.clone(),
        email,
        ..Default::default()
    })
}

pub fn format_provider(api_provider: &str) -> String {
    match api_provider.trim().to_lowercase().as_str() {
        "" | "firstparty" => "Anthropic API",
        "bedrock" => "Amazon Bedrock",
        "vertex" => "Google Vertex AI",
        "foundry" => "Microsoft Foundry",
        _ => return api_provider.to_owned(),
    }
    .to_owned()
}

pub fn format_login_method(auth_method: &str, subscription_type: &str) -> String {
    let by_plan = match subscription_type.trim().to_lowercase().as_str() {
        "team" => "Claude Team account",
        "enterprise" => "Claude Enterprise account",
        "pro" => "Claude Pro account",
        "max" => "Claude Max account",
        "free" => "Claude Free account",
        _ => "",
    };
    if !by_plan.is_empty() {
        return by_plan.to_owned();
    }
    match auth_method.trim().to_lowercase().as_str() {
        "claude.ai" | "claude_ai" | "claudeai" => "Claude.ai account",
        "api_key" | "apikey" | "api-key" => "API key",
        "" => "",
        _ => return auth_method.to_owned(),
    }
    .to_owned()
}

/// The welcome screen's connected-agent line (Go `DetectAgent`): empty
/// unless logged in, never a fake success.
pub fn detect_agent(st: &ProviderStatus) -> String {
    if !st.authenticated {
        return String::new();
    }
    match (st.email.is_empty(), st.organization.is_empty()) {
        (false, false) => format!("Claude Code · {} ({})", st.email, st.organization),
        (false, true) => format!("Claude Code · {}", st.email),
        _ => "Claude Code".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_status_logged_in() {
        let raw = br#"{
            "loggedIn": true,
            "authMethod": "claude.ai",
            "apiProvider": "firstParty",
            "email": "dev@example.com",
            "orgName": "Acme",
            "subscriptionType": "team"
        }"#;
        let st = parse_auth_status(raw).expect("authenticated");
        assert!(st.authenticated);
        assert_eq!(st.provider, "Anthropic API");
        assert_eq!(st.login_method, "Claude Team account");
        assert_eq!(
            (st.organization.as_str(), st.email.as_str()),
            ("Acme", "dev@example.com")
        );
    }

    #[test]
    fn auth_status_logged_out() {
        let raw = br#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#;
        assert!(parse_auth_status(raw).is_none());
        assert!(parse_auth_status(b"not-json").is_none());
    }

    #[test]
    fn missing_binary() {
        assert!(binary_missing(&std::io::Error::from(
            std::io::ErrorKind::NotFound
        )));
        assert!(!binary_missing(&std::io::Error::other("exit status 1")));
    }

    #[test]
    fn logged_out_still_detected() {
        let out = br#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#;
        let st = classify("/usr/local/bin/claude", "2.1.224", out, false, false);
        assert!(!st.authenticated);
        assert_eq!(st.error, "Not logged in");
        assert_eq!(
            (st.version.as_str(), st.path.as_str()),
            ("2.1.224", "/usr/local/bin/claude")
        );
        assert!(st.installed);
    }

    #[test]
    fn missing_and_timed_out() {
        assert_eq!(
            classify("", "", &[], true, false).error,
            "Claude Code not detected"
        );
        let st = classify("/usr/local/bin/claude", "2.1.224", &[], false, true);
        assert_eq!(st.error, "Status check timed out");
        assert!(!st.authenticated);
    }

    #[test]
    fn versions() {
        assert_eq!(parse_version("2.1.224 (Claude Code)\n"), "2.1.224");
        assert_eq!(parse_version("  3.0.0\n"), "3.0.0");
        assert_eq!(parse_version(""), "");
    }

    #[test]
    fn login_method_and_provider() {
        assert_eq!(format_login_method("claude.ai", ""), "Claude.ai account");
        assert_eq!(format_login_method("api_key", ""), "API key");
        for (input, want) in [
            ("firstParty", "Anthropic API"),
            ("bedrock", "Amazon Bedrock"),
            ("vertex", "Google Vertex AI"),
            ("custom", "custom"),
        ] {
            assert_eq!(format_provider(input), want);
        }
    }

    #[test]
    fn detect_agent_lines() {
        let mut st = ProviderStatus::default();
        assert_eq!(detect_agent(&st), "");
        st.authenticated = true;
        assert_eq!(detect_agent(&st), "Claude Code");
        st.email = "a@b.c".into();
        assert_eq!(detect_agent(&st), "Claude Code · a@b.c");
        st.organization = "Acme".into();
        assert_eq!(detect_agent(&st), "Claude Code · a@b.c (Acme)");
    }
}
