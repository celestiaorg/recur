//! The user MCP catalog (Settings → MCP). Recur MCP and Playwright stay
//! engine-owned and are never stored here.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::session::{CHAT_MODE_AGENT, CHAT_MODE_LOOP, normalize_chat_mode};
use crate::text::{eq_fold, kebab_slug};

pub const MCP_ID_LINEAR: &str = "linear";
pub const MCP_PRESET_LINEAR: &str = "linear";
pub const MCP_TRANSPORT_HTTP: &str = "http";
pub const MCP_TRANSPORT_STDIO: &str = "stdio";
pub const LINEAR_MCP_WRITE_URL: &str = "https://mcp.linear.app/mcp";
pub const LINEAR_MCP_READ_ONLY_URL: &str = "https://mcp.linear.app/mcp/readonly";

/// One extra MCP server the user added or enabled (a shipped preset).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServer {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub preset: String,
    pub enabled: bool,
    /// http | stdio.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub transport: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub url: String,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub command: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub env: BTreeMap<String, String>,
}

/// A rejected MCP catalog at the settings save boundary.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum McpError {
    #[error("mcp server id is required")]
    MissingId,
    #[error("mcp server id {0:?} is reserved or invalid")]
    InvalidId(String),
    #[error("duplicate mcp server id {0:?}")]
    Duplicate(String),
}

/// Reports whether id can be an MCP config key (`mcp__<id>__*`): lowercase
/// kebab `^[a-z][a-z0-9-]{0,40}$`. recur and playwright are reserved for
/// engine-owned servers.
pub fn valid_mcp_id(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || id == "recur" || id == "playwright" {
        return false;
    }
    kebab_slug(id, 40)
}

/// Rejects reserved, malformed, or duplicate ids, so a bad catalog cannot
/// replace a stored server.
pub fn validate_mcp_servers(servers: &[McpServer]) -> Result<(), McpError> {
    let mut seen = HashSet::new();
    for s in servers {
        let id = s.id.trim().to_lowercase();
        if !valid_mcp_id(&id) {
            let raw = s.id.trim();
            if raw.is_empty() {
                return Err(McpError::MissingId);
            }
            return Err(McpError::InvalidId(raw.to_owned()));
        }
        if !seen.insert(id.clone()) {
            return Err(McpError::Duplicate(id));
        }
    }
    Ok(())
}

/// Drops invalid/reserved ids, forces the Linear preset shape, and dedupes by
/// id (last wins, keeping the first position). Runtime catalog only; saves
/// must [`validate_mcp_servers`].
pub fn normalize_mcp_servers(servers: &[McpServer]) -> Vec<McpServer> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<McpServer> = Vec::new();
    for s in servers {
        let Some(s) = normalize_mcp_server(s.clone()) else {
            continue;
        };
        if let Some(&i) = seen.get(&s.id) {
            out[i] = s;
            continue;
        }
        seen.insert(s.id.clone(), out.len());
        out.push(s);
    }
    out
}

fn normalize_mcp_server(mut s: McpServer) -> Option<McpServer> {
    s.id = s.id.trim().to_lowercase();
    if !valid_mcp_id(&s.id) {
        return None;
    }
    s.name = s.name.trim().to_owned();
    s.preset = s.preset.to_lowercase().trim().to_owned();
    s.transport = s.transport.trim().to_lowercase();
    s.url = s.url.trim().to_owned();
    s.command = s.command.trim().to_owned();
    if s.preset == MCP_PRESET_LINEAR || s.id == MCP_ID_LINEAR {
        s.id = MCP_ID_LINEAR.to_owned();
        s.preset = MCP_PRESET_LINEAR.to_owned();
        s.transport = MCP_TRANSPORT_HTTP.to_owned();
        s.url.clear();
        s.headers.clear();
        s.command.clear();
        s.args.clear();
        s.env.clear();
        if s.name.is_empty() || eq_fold(&s.name, MCP_ID_LINEAR) {
            s.name = "Linear".to_owned();
        }
        return Some(s);
    }
    if s.name.is_empty() {
        s.name = s.id.clone();
    }
    if s.transport != MCP_TRANSPORT_STDIO {
        s.transport = MCP_TRANSPORT_HTTP.to_owned();
    }
    Some(s)
}

/// Shipped presets ∪ stored servers (stored wins on the same id). Linear is
/// always first so Settings/Project can list it without a separate row.
pub fn mcp_catalog(stored: &[McpServer]) -> Vec<McpServer> {
    let mut linear = McpServer {
        id: MCP_ID_LINEAR.to_owned(),
        name: "Linear".to_owned(),
        preset: MCP_PRESET_LINEAR.to_owned(),
        transport: MCP_TRANSPORT_HTTP.to_owned(),
        ..Default::default()
    };
    let mut custom = Vec::new();
    for s in normalize_mcp_servers(stored) {
        if s.id == MCP_ID_LINEAR {
            linear = s;
        } else {
            custom.push(s);
        }
    }
    let mut out = vec![linear];
    out.extend(custom);
    out
}

/// The off-list for [`crate::effective_skill_enabled`]: shipped presets
/// default off; a stored server is off when Enabled is false.
pub fn mcp_global_disabled(stored: &[McpServer]) -> Vec<String> {
    let mut linear_on = false;
    let mut extra_off = Vec::new();
    for s in normalize_mcp_servers(stored) {
        if s.enabled {
            linear_on |= s.id == MCP_ID_LINEAR;
        } else if s.id != MCP_ID_LINEAR {
            extra_off.push(s.id);
        }
    }
    let mut off = Vec::with_capacity(1 + extra_off.len());
    if !linear_on {
        off.push(MCP_ID_LINEAR.to_owned());
    }
    off.extend(extra_off);
    off
}

/// The official Linear MCP endpoint for a chat mode. Ask/Plan/Review get the
/// readonly URL so a provider without a per-run allowlist cannot see write
/// tools; Agent and Loop (Maestro) get the write URL for triage.
pub fn linear_mcp_url(mode: &str) -> &'static str {
    match normalize_chat_mode(mode) {
        CHAT_MODE_AGENT | CHAT_MODE_LOOP => LINEAR_MCP_WRITE_URL,
        _ => LINEAR_MCP_READ_ONLY_URL,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effective_skill_enabled;
    use crate::session::{CHAT_MODE_ASK, CHAT_MODE_PLAN, CHAT_MODE_REVIEW};

    fn server(id: &str) -> McpServer {
        McpServer {
            id: id.into(),
            ..Default::default()
        }
    }

    #[test]
    fn valid_ids() {
        for bad in [
            "recur",
            "playwright",
            "",
            "Linear",
            "1abc",
            "a_b",
            &"a".repeat(42),
        ] {
            assert!(!valid_mcp_id(bad), "{bad:?}");
        }
        for good in ["linear", "acme-jira", " acme ", &"a".repeat(41)] {
            assert!(valid_mcp_id(good), "{good:?}");
        }
    }

    #[test]
    fn catalog_linear_default_off() {
        let cat = mcp_catalog(&[]);
        assert_eq!(cat.len(), 1);
        assert_eq!(cat[0].id, MCP_ID_LINEAR);
        assert!(!cat[0].enabled);
        assert_eq!(mcp_global_disabled(&[]), [MCP_ID_LINEAR]);
        assert!(!effective_skill_enabled(
            MCP_ID_LINEAR,
            &mcp_global_disabled(&[]),
            &[],
            &[]
        ));
    }

    #[test]
    fn catalog_linear_enabled() {
        let stored = [McpServer {
            enabled: true,
            ..server("linear")
        }];
        let cat = mcp_catalog(&stored);
        assert_eq!(cat.len(), 1);
        assert!(cat[0].enabled);
        assert_eq!(cat[0].preset, MCP_PRESET_LINEAR);
        let off = mcp_global_disabled(&stored);
        assert!(effective_skill_enabled(MCP_ID_LINEAR, &off, &[], &[]));
        assert!(!effective_skill_enabled(
            MCP_ID_LINEAR,
            &off,
            &[MCP_ID_LINEAR.to_owned()],
            &[]
        ));
    }

    #[test]
    fn catalog_orders_custom_after_linear() {
        let stored = [
            McpServer {
                enabled: false,
                ..server("zed")
            },
            McpServer {
                preset: " LINEAR ".into(),
                name: "linear".into(),
                url: "https://x".into(),
                enabled: true,
                ..server("other")
            },
        ];
        let cat = mcp_catalog(&stored);
        let ids: Vec<_> = cat.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["linear", "zed"]);
        assert_eq!(cat[0].name, "Linear");
        assert!(cat[0].url.is_empty() && cat[0].enabled);
        assert_eq!(cat[1].transport, MCP_TRANSPORT_HTTP);
        assert_eq!(cat[1].name, "zed");
        assert_eq!(mcp_global_disabled(&stored), ["zed"]);
    }

    #[test]
    fn validate_rejects_reserved_and_duplicates() {
        assert_eq!(validate_mcp_servers(&[]), Ok(()));
        let acme = McpServer {
            url: "https://x".into(),
            ..server("acme")
        };
        assert_eq!(validate_mcp_servers(std::slice::from_ref(&acme)), Ok(()));
        assert_eq!(
            validate_mcp_servers(&[McpServer {
                enabled: true,
                ..server("linear")
            }]),
            Ok(())
        );
        assert_eq!(
            validate_mcp_servers(&[server("recur")]),
            Err(McpError::InvalidId("recur".into()))
        );
        assert_eq!(
            validate_mcp_servers(&[server("playwright")]),
            Err(McpError::InvalidId("playwright".into()))
        );
        assert_eq!(
            validate_mcp_servers(&[server("!!!")]),
            Err(McpError::InvalidId("!!!".into()))
        );
        assert_eq!(
            validate_mcp_servers(&[server("  ")]),
            Err(McpError::MissingId)
        );
        let upper = McpServer {
            url: "https://x".into(),
            ..server("Acme")
        };
        let lower = McpServer {
            url: "https://y".into(),
            ..server("acme")
        };
        assert_eq!(
            validate_mcp_servers(&[upper, lower]),
            Err(McpError::Duplicate("acme".into()))
        );
    }

    #[test]
    fn normalize_drops_reserved_and_dedupes() {
        let got = normalize_mcp_servers(&[
            McpServer {
                enabled: true,
                ..server("recur")
            },
            McpServer {
                name: "Acme".into(),
                url: "https://x/mcp".into(),
                enabled: true,
                ..server("acme")
            },
            McpServer {
                name: "Acme 2".into(),
                url: "https://y/mcp".into(),
                enabled: false,
                ..server("acme")
            },
        ]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "acme");
        assert!(!got[0].enabled);
        assert_eq!(got[0].url, "https://y/mcp");
        assert!(normalize_mcp_servers(&[server("recur")]).is_empty());
        let stdio = normalize_mcp_servers(&[McpServer {
            transport: " STDIO ".into(),
            ..server("x")
        }]);
        assert_eq!(stdio[0].transport, MCP_TRANSPORT_STDIO);
    }

    #[test]
    fn linear_url_by_mode() {
        assert_eq!(linear_mcp_url(CHAT_MODE_AGENT), LINEAR_MCP_WRITE_URL);
        assert_eq!(linear_mcp_url(CHAT_MODE_ASK), LINEAR_MCP_READ_ONLY_URL);
        assert_eq!(linear_mcp_url(CHAT_MODE_PLAN), LINEAR_MCP_READ_ONLY_URL);
        assert_eq!(linear_mcp_url(CHAT_MODE_REVIEW), LINEAR_MCP_READ_ONLY_URL);
        assert_eq!(linear_mcp_url(CHAT_MODE_LOOP), LINEAR_MCP_WRITE_URL);
        assert_eq!(
            linear_mcp_url(""),
            LINEAR_MCP_WRITE_URL,
            "empty mode normalizes to agent"
        );
    }
}
