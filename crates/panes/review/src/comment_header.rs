//! `lib/comment-header.ts`.

use recur_domain::PrComment;
use recur_engine::view::js;

/// Approve / request-changes on a timeline or conversation row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReviewDecision {
    Approved,
    Changes,
    None,
}

pub fn review_decision(c: Option<&PrComment>) -> ReviewDecision {
    let Some(c) = c else {
        return ReviewDecision::None;
    };
    let verb = js::trim(&c.verb).to_lowercase();
    let body = js::trim(&c.body_md).to_lowercase();
    let s = if !verb.is_empty() {
        verb
    } else if c.kind == "action" {
        body
    } else {
        String::new()
    };
    if s == "approved" || s.starts_with("approved ") {
        ReviewDecision::Approved
    } else if s.contains("requested changes") {
        ReviewDecision::Changes
    } else {
        ReviewDecision::None
    }
}

/// The verb after the author on a conversation comment card.
pub fn comment_header_verb(c: Option<&PrComment>) -> String {
    let Some(c) = c else { return String::new() };
    if c.kind == "description" || c.kind == "action" {
        return String::new();
    }
    let v = js::trim(&c.verb);
    if v.is_empty() {
        return "commented".into();
    }
    if v.eq_ignore_ascii_case("approved") {
        return "approved these changes".into();
    }
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(kind: &str, verb: &str, body: &str) -> PrComment {
        PrComment {
            kind: kind.into(),
            verb: verb.into(),
            body_md: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn reads_approve_and_request_changes_from_verb_or_action_body() {
        assert_eq!(
            review_decision(Some(&c("", "approved", ""))),
            ReviewDecision::Approved
        );
        assert_eq!(
            review_decision(Some(&c("action", "", "approved"))),
            ReviewDecision::Approved
        );
        assert_eq!(
            review_decision(Some(&c("", "requested changes", ""))),
            ReviewDecision::Changes
        );
        assert_eq!(
            review_decision(Some(&c("action", "", "requested changes"))),
            ReviewDecision::Changes
        );
        assert_eq!(
            review_decision(Some(&c("action", "", "force-pushed the branch"))),
            ReviewDecision::None
        );
        assert_eq!(
            review_decision(Some(&c("comment", "", "looks approved to me"))),
            ReviewDecision::None
        );
    }

    #[test]
    fn header_verb_is_empty_on_description_and_action_rows() {
        assert_eq!(comment_header_verb(Some(&c("description", "", ""))), "");
        assert_eq!(comment_header_verb(Some(&c("action", "approved", ""))), "");
        assert_eq!(comment_header_verb(None), "");
    }

    #[test]
    fn header_verb_uses_the_stored_verb_or_commented() {
        assert_eq!(
            comment_header_verb(Some(&c("", "approved", ""))),
            "approved these changes"
        );
        assert_eq!(
            comment_header_verb(Some(&c("comment", "requested changes", ""))),
            "requested changes"
        );
        assert_eq!(comment_header_verb(Some(&c("", "", ""))), "commented");
        assert_eq!(
            comment_header_verb(Some(&c("comment", "  ", ""))),
            "commented"
        );
    }
}
