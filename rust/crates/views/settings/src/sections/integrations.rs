//! Integrations: the bundles' `[[setting]]` values (the Linear API key among them), with
//! secrets masked and their links.

use std::collections::BTreeMap;

use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div,
};
use recur_editor::text::locale_compare;
use recur_services::api::BundleSettingInfo;
use recur_ui::IconName;
use recur_ui::text::span;
use recur_ui::widgets::{settings_group, settings_groups};

use crate::field::{FieldEvent, FieldOpts};
use crate::style::{
    InpStyle, Row, RowsCtx, copy, ctrl, ctrl_text, inp, rows, tbtn, tbtn_icon, tbtn_label,
};
use crate::view::SettingsView;

const GROUP_ROWS: RowsCtx = RowsCtx {
    sides: false,
    in_group: true,
    more: false,
};

/// `bundleSettingGroups`: settings by bundle name (`Bundle` when empty), groups A–Z.
pub fn setting_groups(schema: &[BundleSettingInfo]) -> Vec<(String, Vec<BundleSettingInfo>)> {
    let mut groups: Vec<(String, Vec<BundleSettingInfo>)> = Vec::new();
    for s in schema {
        let name = if s.bundle_name.is_empty() {
            "Bundle".to_string()
        } else {
            s.bundle_name.clone()
        };
        match groups.iter_mut().find(|(g, _)| *g == name) {
            Some((_, v)) => v.push(s.clone()),
            None => groups.push((name, vec![s.clone()])),
        }
    }
    groups.sort_by(|a, b| locale_compare(&a.0, &b.0));
    groups
}

/// `commitBundleSetting`'s next values: the non-empty live values with `id` set to `v` (or
/// dropped when empty); `None` when the stored value already equals `v`.
pub fn committed_values(
    live: &BTreeMap<String, String>,
    id: &str,
    v: &str,
) -> Option<BTreeMap<String, String>> {
    if live.get(id).map_or("", String::as_str) == v {
        return None;
    }
    let mut out: BTreeMap<String, String> = live
        .iter()
        .filter(|(_, val)| !val.is_empty())
        .map(|(k, val)| (k.clone(), val.clone()))
        .collect();
    if v.is_empty() {
        out.remove(id);
    } else {
        out.insert(id.to_string(), v.to_string());
    }
    Some(out)
}

impl SettingsView {
    pub(crate) fn integrations_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.bundle_settings.loading && self.bundle_settings.rows.is_empty() {
            return ctrl_text("int-msg", vec![span("Loading…")], window, cx).into_any_element();
        }
        if !self.bundle_settings.err.is_empty() {
            let e = self.bundle_settings.err.clone();
            return ctrl_text("int-msg", vec![span(e)], window, cx).into_any_element();
        }
        if self.bundle_settings.rows.is_empty() {
            // `.set-ctrl-text code` keeps the user agent's `<code>`: the generic monospace face.
            return ctrl_text(
                "int-msg",
                vec![
                    span("Bundles can declare "),
                    recur_ui::text::TextStyled::family(
                        span("[[setting]]"),
                        recur_ui::fonts::Family::Stack("Courier".into(), Vec::new()),
                    ),
                    span(" in bundle.toml. Values show up here and are available to that pack’s shebang scripts."),
                ],
                window,
                cx,
            )
            .into_any_element();
        }
        let mut out = settings_groups();
        for (bundle, items) in setting_groups(&self.bundle_settings.rows) {
            let mut list = Vec::new();
            for s in items {
                let label = if s.label.is_empty() { s.name.clone() } else { s.label.clone() };
                let key = format!("bundle:{}", s.id);
                let visible = self.bundle_visible.contains(&s.id);
                let draft = self.bundle_drafts.get(&s.id).cloned().unwrap_or_default();
                let input = self.bound_field(&key, &draft, FieldOpts::default(), window, cx);
                if let Some(f) = self.fields.get(&key) {
                    f.set_masked(s.secret && !visible, window, cx);
                }
                let aria = label.clone();
                let mut pick = div()
                    .flex()
                    .items_stretch()
                    .gap(gpui_kit::px(8.))
                    .w_full()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .debug_selector(move || format!("inp:{aria}"))
                            .child(inp(&input, &s.placeholder.clone().into(), InpStyle::BASE, window, cx)),
                    );
                if s.secret {
                    let id = s.id.clone();
                    let tip = if visible { "Hide" } else { "Show" };
                    let name = if visible { "Hide value" } else { "Show value" };
                    pick = pick.child(recur_ui::tooltip::titled(
                        SharedString::from(format!("bundle-eye-tip-{}", s.id)),
                        tip,
                        tbtn(SharedString::from(format!("bundle-eye-{}", s.id)), draft.is_empty(), cx)
                            .h_auto()
                            .debug_selector(move || format!("btn:{name}"))
                            .child(tbtn_icon(if visible { IconName::EyeOff } else { IconName::Eye }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if this.bundle_drafts.get(&id).is_none_or(String::is_empty) {
                                    return;
                                }
                                if !this.bundle_visible.remove(&id) {
                                    this.bundle_visible.insert(id.clone());
                                }
                                cx.notify();
                            })),
                    ));
                }
                list.push(
                    Row::stack()
                        .child(copy(
                            &s.id,
                            label,
                            (!s.description.is_empty()).then(|| vec![span(s.description.clone())]),
                            cx,
                        ))
                        .child(pick),
                );
                if !s.url.is_empty() {
                    let open = if s.url_label.is_empty() { "Open".to_string() } else { s.url_label.clone() };
                    let url = s.url.clone();
                    let o = open.clone();
                    list.push(
                        Row::new()
                            .child(copy(
                                &format!("{}-url", s.id),
                                open.clone(),
                                Some(vec![span("Opens in your browser.")]),
                                cx,
                            ))
                            .child(ctrl().child(
                                tbtn_label(SharedString::from(format!("bundle-url-{}", s.id)), open, false, cx)
                                    .debug_selector(move || format!("btn:{o}"))
                                    .on_click(move |_, _, cx| recur_content::links::open_external_url(&url, cx)),
                            )),
                    );
                }
            }
            out = out.child(
                settings_group(SharedString::from(format!("grp-int-{bundle}")), bundle.clone())
                    .child(rows(GROUP_ROWS, list, window, cx)),
            );
        }
        out.into_any_element()
    }

    /// The `bundleSettingDrafts` effect: every setting shows its saved value, except the
    /// focused one, which keeps its draft.
    pub(crate) fn sync_bundle_drafts(&mut self, values: &BTreeMap<String, String>, focus: &str) {
        let prev = std::mem::take(&mut self.bundle_drafts);
        self.bundle_drafts = self
            .bundle_settings
            .rows
            .iter()
            .map(|s| {
                let saved = || values.get(&s.id).cloned().unwrap_or_default();
                let v = if s.id == focus {
                    prev.get(&s.id).cloned().unwrap_or_else(saved)
                } else {
                    saved()
                };
                (s.id.clone(), v)
            })
            .collect();
    }

    /// Integrations field events (`id` is the setting id).
    pub(crate) fn bundle_field(&mut self, id: &str, ev: FieldEvent, value: String, cx: &mut Context<Self>) {
        match ev {
            FieldEvent::Focus => self.bundle_focus = id.to_string(),
            FieldEvent::Blur => self.bundle_focus.clear(),
            FieldEvent::Change => self.commit_bundle_setting(id.to_string(), value, cx),
            FieldEvent::Input => {
                self.bundle_drafts.insert(id.to_string(), value);
            }
        }
    }

    /// `commitBundleSetting`: saves run one after another, each from the then-current values.
    fn commit_bundle_setting(&mut self, id: String, v: String, cx: &mut Context<Self>) {
        let prev = self.bundle_save.take();
        self.bundle_save = Some(cx.spawn(async move |this, cx| {
            if let Some(p) = prev {
                p.await;
            }
            let save = this.update(cx, |this, cx| {
                let live = this
                    .settings(cx)
                    .map(|s| s.bundle_values.clone())
                    .unwrap_or_default();
                committed_values(&live, &id, &v).map(|next| this.patch(false, move |s| s.bundle_values = next, cx))
            });
            if let Ok(Some(task)) = save {
                task.await;
            }
        }));
    }

    /// The Integrations effect: the `[[setting]]` schema.
    pub(crate) fn load_bundle_settings(&mut self, g: u64, cx: &mut Context<Self>) {
        self.bundle_settings.loading = true;
        self.bundle_settings.err.clear();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.bundle.list_bundle_settings().await;
            let _ = this.update(cx, |this, cx| {
                if !this.fx.live("section", g) {
                    return;
                }
                match res {
                    Ok(l) => this.bundle_settings.rows = l,
                    Err(e) => {
                        this.bundle_settings.rows.clear();
                        this.bundle_settings.err = e.message.clone();
                    }
                }
                this.bundle_settings.loading = false;
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_and_commits() {
        let s = |id: &str, bundle: &str| BundleSettingInfo {
            id: id.into(),
            bundle_name: bundle.into(),
            ..Default::default()
        };
        let g = setting_groups(&[s("b.1", "Linear"), s("a.1", ""), s("b.2", "Linear")]);
        assert_eq!(g[0].0, "Bundle");
        assert_eq!(g[1].1.len(), 2);

        let mut live = BTreeMap::new();
        live.insert("x".to_string(), "1".to_string());
        live.insert("empty".to_string(), String::new());
        assert_eq!(committed_values(&live, "x", "1"), None);
        let next = committed_values(&live, "y", "2").expect("changed");
        assert_eq!(next.len(), 2);
        assert!(!next.contains_key("empty"));
        let cleared = committed_values(&live, "x", "").expect("changed");
        assert!(cleared.is_empty());
        assert_eq!(committed_values(&live, "missing", ""), None);
    }
}
