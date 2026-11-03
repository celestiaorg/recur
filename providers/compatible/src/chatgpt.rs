//! The ChatGPT Codex route's request contract: streaming and stateless, with
//! the same flat function tools OpenCode uses. Encrypted reasoning is requested
//! so it can be replayed between tool rounds.

use serde_json::{Map, Value, json};

const UNSUPPORTED: [&str; 16] = [
    "background",
    "conversation",
    "max_output_tokens",
    "max_tool_calls",
    "metadata",
    "moderation",
    "multi_agent",
    "prompt",
    "prompt_cache_retention",
    "safety_identifier",
    "temperature",
    "top_logprobs",
    "top_p",
    "truncation",
    "user",
    "previous_response_id",
];

pub(crate) fn chatgpt_body(raw: &str) -> Result<String, String> {
    let mut body: Map<String, Value> =
        serde_json::from_str(raw).map_err(|e| format!("ChatGPT request: {e}"))?;
    body.insert("stream".into(), true.into());
    body.insert("store".into(), false.into());
    body.insert("include".into(), json!(["reasoning.encrypted_content"]));
    for key in UNSUPPORTED {
        body.remove(key);
    }
    match body.get_mut("tools") {
        Some(Value::Array(tools)) if !tools.is_empty() => {
            for tool in tools {
                if let Value::Object(t) = tool
                    && t.get("type").and_then(Value::as_str) == Some("function")
                {
                    t.insert("strict".into(), false.into());
                }
            }
        }
        _ => {
            body.remove("tool_choice");
        }
    }
    Ok(Value::Object(body).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stateless_streaming_without_unsupported_fields() {
        let out: Value = serde_json::from_str(
            &chatgpt_body(
                r#"{"model":"m","temperature":0.2,"max_output_tokens":9,"tool_choice":"auto","tools":[{"type":"function","name":"Read"}]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(out["stream"], true);
        assert_eq!(out["store"], false);
        assert_eq!(out["include"], json!(["reasoning.encrypted_content"]));
        assert!(out.get("temperature").is_none() && out.get("max_output_tokens").is_none());
        assert_eq!(out["tools"][0]["strict"], false);
        assert_eq!(out["tool_choice"], "auto");
        let bare: Value =
            serde_json::from_str(&chatgpt_body(r#"{"tool_choice":"auto"}"#).unwrap()).unwrap();
        assert!(bare.get("tool_choice").is_none());
    }
}
