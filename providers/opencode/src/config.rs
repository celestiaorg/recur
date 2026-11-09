//! OpenCode's JSON config for one turn: the model source as a custom
//! OpenAI-compatible provider, Recur's MCP host tools, permissions, and the
//! agent prompt.
//!
//! OpenCode 1.x reads `provider` (singular), `npm`, and `options`. Keep this
//! in one place: the next OpenCode config schema can move it without touching
//! the runner.

use std::collections::BTreeMap;

use recur_agent::turn::{TurnRequest, standing_in_user_prompt};
use recur_domain::CompatibleProvider;
use serde_json::{Map, Value, json};

/// Splits a Recur model id for a harness turn into its source slug and the
/// source's own model id. `openrouter/openai/gpt-5` -> (`openrouter`,
/// `openai/gpt-5`).
pub fn split_model(model: &str) -> Result<(String, String), String> {
    let model = model.trim();
    match model.split_once('/') {
        Some((source, id)) if !source.is_empty() && !id.is_empty() => {
            Ok((source.to_owned(), id.to_owned()))
        }
        _ => Err(format!(
            "OpenCode needs a model from a configured endpoint, got {model:?}"
        )),
    }
}

/// A per-source OpenCode provider id. Distinct from the source slug so a
/// custom endpoint named like an OpenCode built-in cannot shadow it.
pub fn provider_id(source: &str) -> String {
    let mut out = String::from("recur-");
    for c in source.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            out.push(c.to_ascii_lowercase());
        } else {
            out.push('-');
        }
    }
    out
}

/// The chat-completions base. Recur stores the root (`https://api.openai.com`)
/// because it appends `/v1/responses`; AI SDK providers expect the `/v1` root.
pub fn chat_base_url(base: &str) -> String {
    let base = base.trim().trim_end_matches('/');
    if base.ends_with("/v1") {
        base.to_owned()
    } else {
        format!("{base}/v1")
    }
}

fn permission_action(allowed: bool) -> Value {
    json!(if allowed { "allow" } else { "deny" })
}

fn allowed_tool(allowed: &[String], name: &str) -> bool {
    allowed.iter().any(|t| t == name)
}

/// OpenCode permissions from Recur's tool allowlist. No allowlist means the
/// caller already scoped the turn; only interactive questions are denied
/// because a non-interactive run cannot answer them.
pub fn permissions(req: &TurnRequest) -> Value {
    let known = [
        "read",
        "edit",
        "glob",
        "grep",
        "list",
        "bash",
        "task",
        "external_directory",
        "todowrite",
        "webfetch",
        "websearch",
        "skill",
    ];
    let mut out = Map::new();
    if req.tools.as_deref() == Some("") {
        for key in known {
            out.insert(key.to_owned(), permission_action(false));
        }
        out.insert("question".into(), permission_action(false));
        return Value::Object(out);
    }
    let Some(allowed) = req.allowed_tools.as_ref() else {
        out.insert("question".into(), permission_action(false));
        return Value::Object(out);
    };
    let edit = allowed_tool(allowed, "Edit")
        || allowed_tool(allowed, "Write")
        || allowed_tool(allowed, "NotebookEdit");
    let values = [
        ("read", allowed_tool(allowed, "Read")),
        ("edit", edit),
        ("glob", allowed_tool(allowed, "Glob")),
        ("grep", allowed_tool(allowed, "Grep")),
        ("list", true),
        ("bash", allowed_tool(allowed, "Bash")),
        ("task", allowed_tool(allowed, "Task")),
        ("external_directory", allowed_tool(allowed, "Bash")),
        ("todowrite", allowed_tool(allowed, "TodoWrite")),
        ("webfetch", allowed_tool(allowed, "WebFetch")),
        ("websearch", allowed_tool(allowed, "WebSearch")),
        ("skill", true),
    ];
    for (key, ok) in values {
        out.insert(key.to_owned(), permission_action(ok));
    }
    out.insert("question".into(), permission_action(false));
    Value::Object(out)
}

/// The prompt OpenCode receives. Role and standing notes ride in the user
/// message only when the turn is fresh, matching
/// [`standing_in_user_prompt`]. The durable system prompt lives on the agent.
pub fn user_prompt(req: &TurnRequest, resumed: bool) -> String {
    let mut parts = Vec::new();
    if standing_in_user_prompt(req, resumed) {
        if !req.append_system_prompt.trim().is_empty() {
            parts.push(req.append_system_prompt.trim());
        }
        if !req.turn_append_prompt.trim().is_empty() {
            parts.push(req.turn_append_prompt.trim());
        }
    }
    parts.push(req.prompt.trim());
    parts.retain(|p| !p.is_empty());
    parts.join("\n\n")
}

fn model_entry(provider: &CompatibleProvider, id: &str) -> Value {
    let label = provider
        .models
        .iter()
        .find(|m| m.id == id)
        .map(|m| m.label.clone())
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| id.to_owned());
    let mut model = Map::new();
    model.insert("name".into(), json!(label));
    if let Some(m) = provider.models.iter().find(|m| m.id == id)
        && m.context_window > 0
    {
        let output = (m.context_window / 4).clamp(1024, 32_768);
        model.insert(
            "limit".into(),
            json!({ "context": m.context_window, "output": output }),
        );
    }
    Value::Object(model)
}

/// The full `opencode.json`-shaped config for a turn.
pub fn opencode_config(
    req: &TurnRequest,
    provider: &CompatibleProvider,
    model_id: &str,
    system_prompt: &str,
    resumed: bool,
) -> Value {
    let pid = provider_id(&provider.id);
    let mut options = Map::new();
    options.insert("baseURL".into(), json!(chat_base_url(&provider.base_url)));
    if !provider.api_key.trim().is_empty() {
        options.insert("apiKey".into(), json!(provider.api_key.trim()));
    }
    if !provider.extra_headers.is_empty() {
        let headers: BTreeMap<&str, &str> = provider
            .extra_headers
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        options.insert("headers".into(), json!(headers));
    }

    let mut agent = Map::new();
    agent.insert("mode".into(), json!("primary"));
    if !system_prompt.trim().is_empty() {
        agent.insert("prompt".into(), json!(system_prompt.trim()));
    }
    agent.insert("permission".into(), permissions(req));

    let mut config = Map::new();
    config.insert("$schema".into(), json!("https://opencode.ai/config.json"));
    config.insert("autoupdate".into(), json!(false));
    config.insert(
        "provider".into(),
        json!({
            pid.clone(): {
                "npm": "@ai-sdk/openai-compatible",
                "name": if provider.name.trim().is_empty() { provider.id.clone() } else { provider.name.clone() },
                "options": Value::Object(options),
                "models": { model_id: model_entry(provider, model_id) },
            }
        }),
    );
    config.insert("agent".into(), json!({ "recur": Value::Object(agent) }));
    if !req.mcp_url.trim().is_empty() {
        config.insert(
            "mcp".into(),
            json!({
                "recur": {
                    "type": "remote",
                    "url": req.mcp_url.trim(),
                    "enabled": true,
                }
            }),
        );
    }
    // Keep OpenCode's own title/compaction calls on the selected model.
    config.insert("small_model".into(), json!(format!("{pid}/{model_id}")));
    let _ = resumed;
    Value::Object(config)
}

/// The `--model` value OpenCode sees: its provider id and the source model.
pub fn wire_model(source: &str, model_id: &str) -> String {
    format!("{}/{}", provider_id(source), model_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> TurnRequest {
        TurnRequest {
            prompt: "do the thing".into(),
            ..Default::default()
        }
    }

    fn provider() -> CompatibleProvider {
        CompatibleProvider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            base_url: "https://openrouter.ai/api".into(),
            api_key: "sk-test".into(),
            extra_headers: BTreeMap::from([("X-Tenant".into(), "eng".into())]),
            ..Default::default()
        }
    }

    #[test]
    fn splits_model_and_base() {
        assert_eq!(
            split_model("openrouter/openai/gpt-5").unwrap(),
            ("openrouter".into(), "openai/gpt-5".into())
        );
        assert!(split_model("opencode").is_err());
        assert_eq!(
            chat_base_url("https://api.openai.com/"),
            "https://api.openai.com/v1"
        );
        assert_eq!(
            chat_base_url("https://openrouter.ai/api/v1"),
            "https://openrouter.ai/api/v1"
        );
        assert_eq!(provider_id("my-endpoint"), "recur-my-endpoint");
        assert_eq!(
            wire_model("openrouter", "openai/gpt-5"),
            "recur-openrouter/openai/gpt-5"
        );
    }

    #[test]
    fn config_carries_source_and_mcp() {
        let mut req = request();
        req.mcp_url = "http://127.0.0.1:1/token/mcp".into();
        req.system_prompt = "You are Recur.".into();
        let cfg = opencode_config(&req, &provider(), "openai/gpt-5", &req.system_prompt, false);
        let p = &cfg["provider"]["recur-openrouter"];
        assert_eq!(p["options"]["baseURL"], "https://openrouter.ai/api/v1");
        assert_eq!(p["options"]["apiKey"], "sk-test");
        assert_eq!(p["options"]["headers"]["X-Tenant"], "eng");
        assert_eq!(p["models"]["openai/gpt-5"]["name"], "openai/gpt-5");
        assert_eq!(cfg["mcp"]["recur"]["url"], "http://127.0.0.1:1/token/mcp");
        assert_eq!(cfg["agent"]["recur"]["prompt"], "You are Recur.");
    }

    #[test]
    fn permissions_deny_disallowed_edit_and_bash() {
        let mut req = request();
        req.allowed_tools = Some(vec!["Read".into(), "Grep".into()]);
        let p = permissions(&req);
        assert_eq!(p["read"], "allow");
        assert_eq!(p["grep"], "allow");
        assert_eq!(p["edit"], "deny");
        assert_eq!(p["bash"], "deny");
        assert_eq!(p["question"], "deny");

        req.tools = Some(String::new());
        let p = permissions(&req);
        assert_eq!(p["read"], "deny");
    }

    #[test]
    fn standing_notes_only_on_fresh_turn() {
        let mut req = request();
        req.append_system_prompt = "mode note".into();
        req.turn_append_prompt = "surface note".into();
        assert_eq!(
            user_prompt(&req, false),
            "mode note\n\nsurface note\n\ndo the thing"
        );
        assert_eq!(user_prompt(&req, true), "do the thing");
        req.refresh_standing = true;
        assert!(user_prompt(&req, true).contains("mode note"));
    }
}
