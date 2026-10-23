//! Pull requests: the default merge and update methods and the commit message formats.

use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::Settings;
use recur_ui::text::span;

use crate::style::{copy, mm_radio, radio_rows, radios_width};
use crate::view::SettingsView;

type Read = fn(&Settings) -> String;
type Write = fn(&mut Settings, &str);

/// One radio row: id, title, description, options, the default when unset, read and write.
struct Choice {
    id: &'static str,
    title: &'static str,
    desc: &'static str,
    opts: &'static [(&'static str, &'static str)],
    default: &'static str,
    read: Read,
    write: Write,
}

const CHOICES: [Choice; 4] = [
    Choice {
        id: "merge",
        title: "Default merge method",
        desc: "How PRs are merged by default. Projects can override this in their settings.",
        opts: &[
            ("merge", "Create a merge commit"),
            ("squash", "Squash and merge"),
            ("rebase", "Rebase and merge"),
        ],
        default: "merge",
        read: |s| s.default_merge_method.as_str().to_string(),
        write: |s, v| s.default_merge_method = v.into(),
    },
    Choice {
        id: "update",
        title: "Default update-with-base method",
        desc: "How a PR branch catches up when you click Update with base.",
        opts: &[
            ("merge", "Merge base into branch"),
            ("rebase", "Rebase onto base"),
        ],
        default: "merge",
        read: |s| s.default_update_base_method.as_str().to_string(),
        write: |s, v| s.default_update_base_method = v.into(),
    },
    Choice {
        id: "squash",
        title: "Squash commit message",
        desc: "Presented when squash-merging a pull request. Projects can override this.",
        opts: &[
            ("title", "Pull request title"),
            ("title-and-commits", "Pull request title and commit details"),
            ("title-and-description", "Pull request title and description"),
        ],
        default: "title-and-commits",
        read: |s| s.default_squash_commit_message.as_str().to_string(),
        write: |s, v| s.default_squash_commit_message = v.into(),
    },
    Choice {
        id: "merge-msg",
        title: "Merge commit message",
        desc: "Presented when merging a pull request with a merge commit. Projects can override this.",
        opts: &[
            ("title", "Pull request title"),
            ("title-and-description", "Pull request title and description"),
        ],
        default: "title",
        read: |s| s.default_merge_commit_message.as_str().to_string(),
        write: |s, v| s.default_merge_commit_message = v.into(),
    },
];

impl SettingsView {
    pub(crate) fn prs_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = self.settings(cx).cloned();
        let labels: Vec<&str> = CHOICES
            .iter()
            .flat_map(|c| c.opts.iter().map(|o| o.1))
            .collect();
        let w = radios_width(&labels, window);
        let mut list = Vec::new();
        for c in &CHOICES {
            let cur = s
                .as_ref()
                .map(|s| (c.read)(s))
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| c.default.to_string());
            let mut radios = div().flex().flex_col().gap(px(2.)).w_full().mt(px(-6.));
            for (val, label) in c.opts {
                let on = cur == *val;
                let (title, write) = (c.title, c.write);
                radios = radios.child(
                    mm_radio(
                        SharedString::from(format!("{}-{val}", c.id)),
                        *label,
                        on,
                        cx,
                    )
                    .debug_selector(move || format!("radio:{title}:{label}:{on}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.patch(false, |s| write(s, val), cx).detach()
                    })),
                );
            }
            list.push((
                copy(c.id, c.title, Some(vec![span(c.desc)]), cx).into_any_element(),
                radios.into_any_element(),
                true,
            ));
        }
        radio_rows(list, w, window, cx).into_any_element()
    }
}
