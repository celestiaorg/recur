//! Chat tab codes and the small rules every slice keys on (Go: `maestro.go`,
//! `workspace_chat.go`, `app_chat.go`, `chat_child.go`, `chat_errors.go`).

use recur_domain::{APP_AGENT_CODE, Workspace};

/// The per-loop conductor tab.
pub const MAESTRO_AGENT_CODE: &str = "MA";

/// The default workspace coding-agent tab. More tabs are PA2, PA3, ….
pub const PAIR_AGENT_CODE: &str = "PA";

/// The chat footer/retry display marker (word joiner). A retried turn's
/// stored display is exactly this.
pub const CHAT_RETRY_DISPLAY_SENTINEL: &str = "\u{2060}";

/// Maps an empty code to the default PA tab.
pub fn normalize_pair_chat_code(code: &str) -> String {
    let code = code.trim();
    if code.is_empty() {
        PAIR_AGENT_CODE.to_owned()
    } else {
        code.to_owned()
    }
}

/// A solo/pair Agent-panel chat (PA, PA2, …).
pub fn is_pair_chat_code(code: &str) -> bool {
    if code == PAIR_AGENT_CODE {
        return true;
    }
    let Some(rest) = code.strip_prefix("PA") else {
        return false;
    };
    !rest.is_empty() && go_atoi(rest).is_some_and(|n| n >= 2)
}

/// Go `strconv.Atoi`: optional sign, ASCII digits, no surrounding space.
fn go_atoi(s: &str) -> Option<i64> {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// An Ask Recur tab (APP, AP2, …).
pub fn is_app_chat_code(code: &str) -> bool {
    code == APP_AGENT_CODE || code.starts_with("AP")
}

/// A human-facing chat tab that takes part in unseen badges (PA* and MA).
pub fn is_chat_attention_code(code: &str) -> bool {
    code == MAESTRO_AGENT_CODE || is_pair_chat_code(code)
}

/// The registry key of one chat tab's in-flight turn.
pub fn chat_turn_key(workspace_id: &str, code: &str) -> String {
    format!("{workspace_id}\x00{code}")
}

/// The life-lock key: pair codes normalize so "" and "PA" share a lock.
pub fn chat_life_key(workspace_id: &str, code: &str) -> String {
    let code = code.trim();
    let code = if code == MAESTRO_AGENT_CODE {
        code.to_owned()
    } else {
        normalize_pair_chat_code(code)
    };
    chat_turn_key(workspace_id, &code)
}

/// The tab that owns the workspace's run: the stamped conductor, else MA
/// for a legacy run, else none.
pub fn loop_owner_code(ws: &Workspace) -> String {
    let c = ws.loop_chat_code.trim();
    if !c.is_empty() {
        return c.to_owned();
    }
    if ws.has_run() {
        return MAESTRO_AGENT_CODE.to_owned();
    }
    String::new()
}

/// A subagent created by an Agent chat (create_workspace), as opposed to a
/// fan-out step child.
pub fn is_chat_child(w: &Workspace) -> bool {
    !w.parent_id.trim().is_empty() && w.parent_step_ids.is_empty()
}

/// Whether a stored display is the retry marker.
pub fn is_chat_retry_display(s: &str) -> bool {
    s == CHAT_RETRY_DISPLAY_SENTINEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pair_codes() {
        assert!(is_pair_chat_code("PA"));
        assert!(is_pair_chat_code("PA2"));
        assert!(is_pair_chat_code("PA10"));
        assert!(is_pair_chat_code("PA+2"));
        assert!(!is_pair_chat_code("PA1"));
        assert!(!is_pair_chat_code("PA0"));
        assert!(!is_pair_chat_code("PAX"));
        assert!(!is_pair_chat_code("CO"));
        assert!(!is_pair_chat_code("MA"));
        assert!(!is_pair_chat_code(""));
    }

    #[test]
    fn attention_and_app_codes() {
        assert!(is_chat_attention_code("MA"));
        assert!(is_chat_attention_code("PA3"));
        assert!(!is_chat_attention_code("RV"));
        assert!(is_app_chat_code("APP"));
        assert!(is_app_chat_code("AP2"));
        assert!(!is_app_chat_code("PA"));
    }

    #[test]
    fn life_key_normalizes() {
        assert_eq!(chat_life_key("w", ""), chat_life_key("w", "PA"));
        assert_eq!(chat_life_key("w", " MA "), "w\x00MA");
    }
}
