//! Licenses: Recur's own licenses and the third-party software it includes.

use gpui_kit::{AnyElement, AppContext as _, Context, IntoElement, ParentElement, Window};
use recur_content::MarkdownClass;
use recur_content::view::MarkdownView;
use recur_ui::text::span;
use recur_ui::widgets::settings_groups;

use crate::style::lead;
use crate::view::SettingsView;

/// `build.rs`'s copy of `mise run generate:licenses`'s output.
const THIRD_PARTY: &str = include_str!(concat!(env!("OUT_DIR"), "/licenses.md"));

impl SettingsView {
    pub(crate) fn licenses_pane(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let scroll = self.main_scroll.clone();
        let doc = self
            .licenses
            .get_or_insert_with(|| {
                cx.new(|cx| {
                    let mut v = MarkdownView::new(THIRD_PARTY, MarkdownClass::MdDoc, false, cx);
                    v.set_scroller(scroll);
                    v
                })
            })
            .clone();
        settings_groups()
            .child(lead(
                "licenses-lead",
                vec![span(
                    "Recur is source-available under the Elastic License 2.0. Its bundles, skills and some libraries are licensed under Apache 2.0.",
                )],
                cx,
            ))
            .child(doc)
            .into_any_element()
    }
}
