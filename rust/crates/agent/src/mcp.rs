//! Settings → MCP / Project → MCP list rules (frontend `lib/mcp.ts`), over
//! the stored [`McpServer`] rows. Enablement matches
//! [`recur_domain::mcp_global_disabled`].

use recur_domain::{MCP_ID_LINEAR, MCP_TRANSPORT_HTTP, McpServer};

/// `^[a-z][a-z0-9-]{0,40}$`, not reserved (`recur`, `playwright`). Unlike
/// [`recur_domain::valid_mcp_id`] the id is not trimmed first.
pub fn valid_mcp_id(id: &str) -> bool {
    let b = id.as_bytes();
    !matches!(id, "recur" | "playwright")
        && (1..=41).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A display name as an id slug: lowercase, non-alphanumeric runs to `-`,
/// trimmed of dashes, at most 41 bytes.
pub fn slug_mcp_id(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let mut out = String::with_capacity(lower.len());
    let mut dash = false;
    for c in lower.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            if dash && !out.is_empty() {
                out.push('-');
            }
            dash = false;
            out.push(c);
        } else {
            dash = true;
        }
    }
    out.truncate(41);
    out
}

/// The global off-list: Linear unless enabled, then every disabled custom
/// server.
pub fn mcp_global_disabled(servers: &[McpServer]) -> Vec<String> {
    let mut linear_on = false;
    let mut extra_off = Vec::new();
    for s in servers {
        let id = s.id.trim();
        if id.is_empty() {
            continue;
        }
        if s.enabled {
            linear_on |= id == MCP_ID_LINEAR;
        } else if id != MCP_ID_LINEAR {
            extra_off.push(id.to_owned());
        }
    }
    let mut off = Vec::with_capacity(extra_off.len() + 1);
    if !linear_on {
        off.push(MCP_ID_LINEAR.to_owned());
    }
    off.extend(extra_off);
    off
}

pub fn linear_mcp_on(servers: &[McpServer]) -> bool {
    servers.iter().any(|s| s.id == MCP_ID_LINEAR && s.enabled)
}

/// Every server but the Linear preset.
pub fn custom_mcp_servers(servers: &[McpServer]) -> Vec<McpServer> {
    servers
        .iter()
        .filter(|s| s.id != MCP_ID_LINEAR && s.preset != MCP_ID_LINEAR)
        .cloned()
        .collect()
}

/// Stores the Linear preset first while enabled; drops it when disabled.
pub fn upsert_linear_enabled(servers: &[McpServer], enabled: bool) -> Vec<McpServer> {
    let rest = custom_mcp_servers(servers);
    if !enabled {
        return rest;
    }
    let mut out = vec![McpServer {
        id: MCP_ID_LINEAR.to_owned(),
        name: "Linear".to_owned(),
        preset: MCP_ID_LINEAR.to_owned(),
        enabled: true,
        transport: MCP_TRANSPORT_HTTP.to_owned(),
        ..Default::default()
    }];
    out.extend(rest);
    out
}

pub fn set_custom_mcp_enabled(servers: &[McpServer], id: &str, enabled: bool) -> Vec<McpServer> {
    servers
        .iter()
        .map(|s| {
            let mut s = s.clone();
            if s.id == id {
                s.enabled = enabled;
            }
            s
        })
        .collect()
}

pub fn remove_mcp_server(servers: &[McpServer], id: &str) -> Vec<McpServer> {
    servers.iter().filter(|s| s.id != id).cloned().collect()
}

/// Patches a stored server in place; the id (and project override lists
/// keyed on it) stays put whatever the patch does.
pub fn update_mcp_server(
    servers: &[McpServer],
    id: &str,
    patch: impl Fn(&mut McpServer),
) -> Vec<McpServer> {
    servers
        .iter()
        .map(|s| {
            let mut s = s.clone();
            if s.id == id {
                let keep = s.id.clone();
                patch(&mut s);
                s.id = keep;
            }
            s
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server(id: &str, enabled: bool) -> McpServer {
        McpServer {
            id: id.into(),
            enabled,
            ..Default::default()
        }
    }

    #[test]
    fn ids() {
        for bad in [
            "recur",
            "playwright",
            "Linear",
            "",
            " linear",
            &"a".repeat(42),
        ] {
            assert!(!valid_mcp_id(bad), "{bad:?}");
        }
        assert!(
            valid_mcp_id("linear") && valid_mcp_id("acme-jira") && valid_mcp_id(&"a".repeat(41))
        );
        assert_eq!(slug_mcp_id("Acme Jira"), "acme-jira");
        assert_eq!(slug_mcp_id("  --Hello, World!!--  "), "hello-world");
        assert_eq!(slug_mcp_id(&"ab ".repeat(20)).len(), 41);
    }

    #[test]
    fn linear_off_until_enabled() {
        assert_eq!(mcp_global_disabled(&[]), [MCP_ID_LINEAR]);
        assert!(!linear_mcp_on(&[]));
        assert!(mcp_global_disabled(&[server(MCP_ID_LINEAR, true)]).is_empty());
        assert!(linear_mcp_on(&[server(MCP_ID_LINEAR, true)]));
        assert_eq!(
            mcp_global_disabled(&[server("acme", false), server(" ", false), server("b", true)]),
            [MCP_ID_LINEAR, "acme"]
        );
    }

    #[test]
    fn upsert_linear() {
        let acme = McpServer {
            url: "https://x".into(),
            ..server("acme", true)
        };
        let on = upsert_linear_enabled(&[acme], true);
        assert_eq!(
            (on[0].id.as_str(), on[0].enabled, on[0].preset.as_str()),
            (MCP_ID_LINEAR, true, MCP_ID_LINEAR)
        );
        let ids: Vec<_> = custom_mcp_servers(&on).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["acme"]);
        let ids: Vec<_> = upsert_linear_enabled(&on, false)
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["acme"]);
    }

    #[test]
    fn update_keeps_the_id() {
        let mut old = server("acme", true);
        old.name = "Acme".into();
        old.url = "https://old".into();
        old.headers
            .insert("Authorization".into(), "Bearer old".into());
        let got = update_mcp_server(&[old], "acme", |s| {
            s.id = "other".into();
            s.name = "Acme API".into();
            s.url = "https://new".into();
            s.headers = [("Authorization".to_owned(), "Bearer new".to_owned())].into();
        });
        assert_eq!(got[0].id, "acme");
        assert_eq!(got[0].name, "Acme API");
        assert_eq!(got[0].url, "https://new");
        assert_eq!(got[0].headers["Authorization"], "Bearer new");
        assert!(got[0].enabled);
        let toggled = set_custom_mcp_enabled(&got, "acme", false);
        assert!(!toggled[0].enabled);
        assert!(remove_mcp_server(&toggled, "acme").is_empty());
    }
}
