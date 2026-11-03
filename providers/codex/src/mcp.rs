//! `codex -c key=value` overrides: MCP servers plus the automation defaults.
//! Values are TOML-encoded.

use std::collections::BTreeMap;

/// One MCP server injected through overrides.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    /// Streamable HTTP URL.
    pub url: String,
    pub required: bool,
    /// Stdio servers (Playwright, custom).
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    /// Streamable-HTTP headers (Linear, custom). Never log.
    pub http_headers: BTreeMap<String, String>,
}

/// The overrides for `servers`, then the automation defaults. The runner
/// appends the real `approval_policy` after these, so the `never` here is
/// only the failsafe.
pub fn config_overrides(servers: &[McpServer]) -> Vec<String> {
    let mut out = Vec::new();
    for s in servers {
        let name = s.name.trim();
        if name.is_empty() {
            continue;
        }
        let prefix = format!("mcp_servers.{name}");
        out.push(format!("{prefix}.enabled=true"));
        if s.required {
            out.push(format!("{prefix}.required=true"));
        }
        // Auto-approve MCP tools so automation does not stall on
        // elicitations (still answered as a failsafe).
        out.push(format!(r#"{prefix}.default_tools_approval_mode="approve""#));
        if !s.url.is_empty() {
            out.push(format!(r#"{prefix}.url="{}""#, escape_toml_string(&s.url)));
        }
        if !s.command.is_empty() {
            out.push(format!(
                r#"{prefix}.command="{}""#,
                escape_toml_string(&s.command)
            ));
            if !s.args.is_empty() {
                let parts: Vec<String> = s
                    .args
                    .iter()
                    .map(|a| format!(r#""{}""#, escape_toml_string(a)))
                    .collect();
                out.push(format!("{prefix}.args=[{}]", parts.join(",")));
            }
        }
        for (k, v) in &s.http_headers {
            let k = k.trim();
            if !k.is_empty() {
                out.push(format!(
                    r#"{prefix}.http_headers.{k}="{}""#,
                    escape_toml_string(v)
                ));
            }
        }
        for (k, v) in &s.env {
            let k = k.trim();
            if !k.is_empty() {
                out.push(format!(r#"{prefix}.env.{k}="{}""#, escape_toml_string(v)));
            }
        }
    }
    out.extend(
        [
            r#"approval_policy="never""#,
            r#"sandbox_mode="workspace-write""#,
            // Readable reasoning summaries stream as thinking blocks.
            r#"model_reasoning_summary="auto""#,
            "model_supports_reasoning_summaries=true",
            // Recur owns the skill catalog and activation over MCP. The
            // isolated CODEX_HOME already drops ~/.codex/skills;
            // include_instructions=false drops the ## Skills catalog that
            // ~/.agents/skills and repo .agents/skills still injected, and
            // remote_plugin=false drops <recommended_plugins>.
            "skills.bundled.enabled=false",
            "skills.include_instructions=false",
            "features.skip_host_skill_discovery=true",
            "features.remote_plugin=false",
            // Delegation goes through Recur child workspaces, never Codex's
            // own spawn-agent tools.
            "features.multi_agent=false",
            "features.multi_agent_v2=false",
        ]
        .map(String::from),
    );
    out
}

/// A TOML basic-string body: backslash, quote and control characters
/// escaped, so a multi-line value stays on one argv line.
pub fn escape_toml_string(s: &str) -> String {
    let mut b = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => b.push_str(r"\\"),
            '"' => b.push_str("\\\""),
            '\u{8}' => b.push_str(r"\b"),
            '\t' => b.push_str(r"\t"),
            '\n' => b.push_str(r"\n"),
            '\u{c}' => b.push_str(r"\f"),
            '\r' => b.push_str(r"\r"),
            c if (c as u32) < 0x20 => b.push_str(&format!("\\u{:04x}", c as u32)),
            c => b.push(c),
        }
    }
    b
}

/// Interleaved `-c`, `key=value` argv pairs.
pub fn config_args(overrides: &[String]) -> Vec<String> {
    overrides
        .iter()
        .flat_map(|o| ["-c".to_owned(), o.clone()])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_url() {
        let ov = config_overrides(&[McpServer {
            name: "recur".into(),
            url: "http://127.0.0.1:9/tok/mcp".into(),
            required: true,
            ..Default::default()
        }]);
        let joined = ov.join("\n");
        for want in [
            r#"mcp_servers.recur.url="http://127.0.0.1:9/tok/mcp""#,
            "mcp_servers.recur.required=true",
            r#"mcp_servers.recur.default_tools_approval_mode="approve""#,
            r#"approval_policy="never""#,
            r#"model_reasoning_summary="auto""#,
            "model_supports_reasoning_summaries=true",
            "skills.bundled.enabled=false",
            "skills.include_instructions=false",
            "features.skip_host_skill_discovery=true",
            "features.remote_plugin=false",
            "features.multi_agent=false",
            "features.multi_agent_v2=false",
        ] {
            assert!(joined.contains(want), "missing {want}\n{joined}");
        }
        assert!(!joined.contains("network_access"));
        let args = config_args(&ov);
        assert_eq!(args.len(), ov.len() * 2);
        assert!(args.iter().step_by(2).all(|a| a == "-c"));
    }

    #[test]
    fn playwright_stdio() {
        let joined = config_overrides(&[McpServer {
            name: "playwright".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "@playwright/mcp@latest".into()],
            ..Default::default()
        }])
        .join("\n");
        assert!(joined.contains(r#"mcp_servers.playwright.command="npx""#));
        assert!(joined.contains(r#"mcp_servers.playwright.args=["-y","@playwright/mcp@latest"]"#));
    }

    #[test]
    fn http_headers() {
        let joined = config_overrides(&[McpServer {
            name: "linear".into(),
            url: "https://mcp.linear.app/mcp".into(),
            http_headers: [("Authorization".into(), "Bearer lin_api_x".into())].into(),
            ..Default::default()
        }])
        .join("\n");
        assert!(joined.contains(r#"mcp_servers.linear.url="https://mcp.linear.app/mcp""#));
        assert!(
            joined.contains(r#"mcp_servers.linear.http_headers.Authorization="Bearer lin_api_x""#)
        );
        assert!(!joined.contains("mcp_servers.linear.required=true"));
    }

    #[test]
    fn stdio_env() {
        let joined = config_overrides(&[McpServer {
            name: "fs".into(),
            command: "npx".into(),
            args: vec!["-y".into(), "mcp-fs".into()],
            env: [("FOO".into(), "bar".into())].into(),
            ..Default::default()
        }])
        .join("\n");
        assert!(joined.contains(r#"mcp_servers.fs.command="npx""#));
        assert!(joined.contains(r#"mcp_servers.fs.env.FOO="bar""#));
        assert!(!joined.contains("mcp_servers.fs.required=true"));
    }

    #[test]
    fn stdio_env_multiline_round_trips_through_toml() {
        let pem = "-----BEGIN KEY-----\nline2\n-----END KEY-----";
        let ov = config_overrides(&[McpServer {
            name: "fs".into(),
            command: "npx".into(),
            env: [("PRIVATE_KEY".into(), pem.into())].into(),
            ..Default::default()
        }]);
        let raw = ov
            .iter()
            .find_map(|o| o.strip_prefix("mcp_servers.fs.env.PRIVATE_KEY="))
            .expect("PRIVATE_KEY override");
        assert!(!raw.contains('\n'));
        #[derive(serde::Deserialize)]
        struct Doc {
            v: String,
        }
        let doc: Doc = toml::from_str(&format!("v = {raw}")).unwrap();
        assert_eq!(doc.v, pem);
        assert_eq!(escape_toml_string("a\u{1}\"\\"), "a\\u0001\\\"\\\\");
    }
}
