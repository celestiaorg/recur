//! The PR chrome rules of `lib/colors.ts` the PR views use: tints, status labels and the
//! dashboard's group headings.

use recur_domain::Pr;
use recur_ui::{Color, Tokens};
use recur_view_core::pr::{PrLifecycle, pr_lifecycle};

/// `prGroupColor`.
pub fn group_color(group: &str, t: &Tokens) -> Option<Color> {
    Some(match group {
        "you" => t.coral,
        "loop" => t.amber,
        "open" => t.emerald,
        "merged" => t.indigo,
        _ => return None,
    })
}

/// `prGroupLabel`.
pub fn group_label(group: &str) -> &str {
    match group {
        "you" => "Needs your attention",
        "loop" => "Workspace is responding",
        "open" => "Open",
        "merged" => "Merged",
        other => other,
    }
}

/// `prStatusLabel`.
pub fn status_label(pr: &Pr) -> String {
    match pr_lifecycle(Some(pr)) {
        Some(PrLifecycle::Draft) => "Draft".into(),
        Some(PrLifecycle::Queued) => "Queued".into(),
        _ => pr.state.trim().to_string(),
    }
}

/// `prChromeColor`: merged indigo, queued warn, closed and changes-requested coral, draft
/// cyan, else emerald.
pub fn chrome_color(pr: Option<&Pr>, t: &Tokens) -> Color {
    match pr_lifecycle(pr) {
        Some(PrLifecycle::Merged) => t.indigo,
        Some(PrLifecycle::Queued) => t.warn,
        Some(PrLifecycle::Closed) => t.coral,
        Some(PrLifecycle::Draft) => t.cyan,
        _ => {
            let changes = pr.is_some_and(|p| p.state.to_lowercase().contains("changes requested"));
            if changes { t.coral } else { t.emerald }
        }
    }
}

#[cfg(test)]
mod tests {
    use recur_ui::ThemeId;

    use super::*;

    #[test]
    fn chrome_and_labels() {
        let t = ThemeId::default().tokens();
        let pr = |state: &str, f: fn(&mut Pr)| {
            let mut p = Pr {
                state: state.into(),
                ..Default::default()
            };
            f(&mut p);
            p
        };
        assert_eq!(chrome_color(Some(&pr("Merged", |_| {})), &t), t.indigo);
        assert_eq!(
            chrome_color(Some(&pr("In review", |p| p.merge_queued = true)), &t),
            t.warn
        );
        assert_eq!(chrome_color(Some(&pr("Closed", |_| {})), &t), t.coral);
        assert_eq!(
            chrome_color(Some(&pr("In review", |p| p.draft = true)), &t),
            t.cyan
        );
        assert_eq!(
            chrome_color(Some(&pr("Changes requested", |_| {})), &t),
            t.coral
        );
        assert_eq!(chrome_color(Some(&pr("In review", |_| {})), &t), t.emerald);
        assert_eq!(chrome_color(None, &t), t.emerald);
        assert_eq!(status_label(&pr("In review", |p| p.draft = true)), "Draft");
        assert_eq!(
            status_label(&pr("In review", |p| p.merge_queued = true)),
            "Queued"
        );
        assert_eq!(status_label(&pr(" In review ", |_| {})), "In review");
        assert_eq!(group_label("loop"), "Workspace is responding");
        assert_eq!(group_color("open", &t), Some(t.emerald));
        assert_eq!(group_color("x", &t), None);
    }
}
