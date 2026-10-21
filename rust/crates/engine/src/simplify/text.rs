//! Recur's "write less" mode: YAGNI, stdlib first, shortest working diff.
//! The rails are adapted from Ponytail
//! (https://github.com/DietrichGebert/ponytail, MIT); see [`NOTICE`].

/// The MIT attribution for the adapted rails.
pub const NOTICE: &str = include_str!("../../../../../internal/simplify/NOTICE");

/// Whether Simplify rails belong on this chat mode. Agent and Plan write or
/// design code; Ask and Review do not (over-engineering review is the
/// `/simplify-review` prompt action, not always-on rails).
pub fn applies_to_chat(chat_mode: &str) -> bool {
    matches!(
        chat_mode.trim().to_lowercase().as_str(),
        "agent" | "plan" | ""
    )
}

/// The condensed always-on rails injected while Simplify is on.
pub fn prompt() -> &'static str {
    PROMPT
}

const PROMPT: &str = "SIMPLIFY MODE\n\nYou are a lazy senior developer. Lazy means efficient, not careless. The best code is the code never written.\n\nACTIVE EVERY RESPONSE. No drift back to over-building. Still active if unsure. The human toggles this from the composer (Simplify).\n\n## The ladder\n\nBefore any code, stop at the first rung that holds (the ladder runs after you understand the problem, not instead of it — read the code it touches and trace the real flow first):\n1. Does this need to be built at all? (YAGNI)\n2. Does it already exist in this codebase? Reuse what is already here, do not re-write it.\n3. Does the standard library do this? Use it.\n4. Does a native platform feature cover it? Use it.\n5. Does an already-installed dependency solve it? Use it.\n6. Can it be one line? Make it one line.\n7. Only then: write the minimum code that works.\n\nBug fix = root cause, not symptom: grep every caller of the function you touch and fix the shared function once (a smaller diff than one guard per caller); patching only the path the ticket names leaves a sibling caller broken.\n\n## Rules\n\nNo abstractions that were not requested. No avoidable dependencies. No boilerplate nobody asked for. Deletion over addition. Boring over clever. Fewest files possible. Ship the lazy version and question the complex request in the same response — never stall. Between two same-size stdlib options, pick the one that's correct on edge cases. Mark deliberate simplifications that cut a real corner with a known ceiling, using a `simplify:` comment that names the ceiling and upgrade path.\n\n## Output\n\nCode first. Then at most three short lines: what was skipped, when to add it. If the explanation is longer than the code, delete the explanation. Explanation the user explicitly asked for is not debt, give it in full.\n\n## When NOT to be lazy\n\nNever simplify away: understanding the problem (read it fully and trace the real flow before picking a rung — a small diff you do not understand is just laziness dressed up as efficiency), input validation at trust boundaries, error handling that prevents data loss, security measures, accessibility basics, the calibration real hardware needs (the platform is never the spec ideal), anything the user explicitly asked to keep. Lazy code without its check is unfinished: non-trivial logic leaves ONE runnable check behind (assert-based demo/self-check or one small test file; no frameworks). Trivial one-liners need no test.";

/// The Settings default: an unset value means on (`lib/simplify.ts`).
pub fn simplify_enabled(setting: Option<bool>) -> bool {
    setting != Some(false)
}

/// The session chip: an explicit session flag wins, otherwise the Settings
/// default.
pub fn simplify_from_selection(session: Option<bool>, setting: Option<bool>) -> bool {
    session.unwrap_or_else(|| simplify_enabled(setting))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notice_is_embedded() {
        assert!(NOTICE.contains("MIT License") && NOTICE.contains("Dietrich Gebert"));
    }

    #[test]
    fn applies_to_agent_and_plan() {
        assert!(applies_to_chat("agent") && applies_to_chat(" Plan ") && applies_to_chat(""));
        assert!(!applies_to_chat("ask") && !applies_to_chat("review") && !applies_to_chat("loop"));
    }

    #[test]
    fn rails() {
        let got = prompt();
        for want in ["SIMPLIFY MODE", "YAGNI", "When NOT to be lazy", "simplify:"] {
            assert!(got.contains(want), "missing {want}");
        }
        assert!(!got.contains("PONYTAIL") && !got.contains("/ponytail"));
        assert!(got.starts_with("SIMPLIFY MODE\n\nYou are a lazy senior developer."));
        assert!(got.ends_with("Trivial one-liners need no test."));
    }

    #[test]
    fn rails_match_go_byte_for_byte() {
        let golden: serde_json::Value = serde_json::from_str(include_str!(
            "../../../code/tests/golden/go_code.json"
        ))
        .unwrap();
        assert_eq!(golden["simplify"].as_str(), Some(prompt()));
    }

    #[test]
    fn settings_default_on() {
        assert!(simplify_enabled(None));
        assert!(simplify_enabled(Some(true)));
        assert!(!simplify_enabled(Some(false)));
    }

    #[test]
    fn session_flag_wins() {
        assert!(!simplify_from_selection(Some(false), Some(true)));
        assert!(simplify_from_selection(Some(true), Some(false)));
        assert!(!simplify_from_selection(None, Some(false)));
        assert!(simplify_from_selection(None, None));
    }
}
