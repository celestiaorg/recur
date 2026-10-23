//! Models: the model and flags per chat mode and for workspace titles, and which model a
//! child workspace starts on.

use gpui_kit::{
    AnyElement, Context, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use recur_domain::Settings;
use recur_engine::view::modes::CHAT_MODES;
use recur_ui::style::card;
use recur_ui::text::span;
use recur_view_core::models::{FlagMap, flags_equal, flags_for_model, normalize_flags};

use crate::style::{Row, RowsCtx, copy, ctrl, mm_radio, radio_rows, radios_width, rows, tokens};
use crate::view::{MODEL_SLOTS, SettingsView};

const FALLBACK: &str = "claude-code/claude-sonnet-5-5";

/// The title a slot's row shows (`settingsTitle ?? label`).
fn slot_title(slot: &str) -> &'static str {
    if slot == "title" {
        return "Workspace title";
    }
    CHAT_MODES
        .iter()
        .find(|m| m.value == slot)
        .map(|m| {
            if m.settings_title.is_empty() {
                m.label
            } else {
                m.settings_title
            }
        })
        .unwrap_or("")
}

/// The pickers' accessible names (`{title} model`, `{title} flags`).
pub fn picker_labels(slot: &str) -> (String, String) {
    let t = slot_title(slot);
    (format!("{t} model"), format!("{t} flags"))
}

/// `modeModelRef`.
pub fn mode_model_ref(s: Option<&Settings>, slot: &str) -> String {
    let or = |v: &str, d: &str| {
        if v.is_empty() {
            d.to_string()
        } else {
            v.to_string()
        }
    };
    let Some(s) = s else {
        return if slot == "title" {
            "claude-code/claude-haiku-4-5-20251001".into()
        } else {
            FALLBACK.into()
        };
    };
    match slot {
        "ask" => or(&s.model_ask, FALLBACK),
        "plan" => or(&s.model_plan, FALLBACK),
        "agent" => or(&s.model_agent, FALLBACK),
        "review" => or(&s.model_review, &or(&s.model_loop, FALLBACK)),
        "loop" => or(&s.model_loop, FALLBACK),
        _ => or(
            &s.model_workspace_title,
            "claude-code/claude-haiku-4-5-20251001",
        ),
    }
}

/// `modeFlagsCur`.
pub fn mode_flags(s: Option<&Settings>, slot: &str) -> FlagMap {
    let Some(s) = s else {
        return FlagMap::new();
    };
    match slot {
        "ask" => s.flags_ask.clone(),
        "plan" => s.flags_plan.clone(),
        "agent" => s.flags_agent.clone(),
        "review" => s.flags_review.clone(),
        "loop" => s.flags_loop.clone(),
        _ => s.flags_workspace_title.clone(),
    }
}

fn set_slot(s: &mut Settings, slot: &str, model: Option<String>, flags: FlagMap) {
    let (m, f) = match slot {
        "ask" => (&mut s.model_ask, &mut s.flags_ask),
        "plan" => (&mut s.model_plan, &mut s.flags_plan),
        "agent" => (&mut s.model_agent, &mut s.flags_agent),
        "review" => (&mut s.model_review, &mut s.flags_review),
        "loop" => (&mut s.model_loop, &mut s.flags_loop),
        _ => (&mut s.model_workspace_title, &mut s.flags_workspace_title),
    };
    if let Some(model) = model {
        *m = model;
    }
    *f = flags;
}

impl SettingsView {
    /// `setModel`.
    pub(crate) fn set_model(&mut self, slot: &'static str, r: String, f: FlagMap, cx: &mut Context<Self>) {
        self.patch(false, move |s| set_slot(s, slot, Some(r), f), cx)
            .detach();
    }

    /// `setFlags`: unchanged flags save nothing.
    pub(crate) fn set_flags(&mut self, slot: &'static str, f: FlagMap, cx: &mut Context<Self>) {
        if flags_equal(&mode_flags(self.settings(cx), slot), &f) {
            return;
        }
        self.patch(false, move |s| set_slot(s, slot, None, f), cx)
            .detach();
    }

    /// The Models effect: the catalog, re-fetched when provider readiness or the catalog
    /// revision changes.
    pub(crate) fn load_models(&mut self, g: u64, cx: &mut Context<Self>) {
        let load = self
            .vcx
            .state
            .update(cx, |s, cx| s.load_model_catalog(cx));
        cx.spawn(async move |this, cx| {
            let models = load.await;
            let _ = this.update(cx, |this, cx| {
                if this.fx.live("models", g) {
                    this.catalog_models = models;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn picker_row(&mut self, i: usize, cx: &mut Context<Self>) -> Row {
        let slot = MODEL_SLOTS[i];
        let s = self.settings(cx).cloned();
        let r = mode_model_ref(s.as_ref(), slot);
        let flags = normalize_flags(
            &flags_for_model(&self.catalog_models, &r),
            Some(&mode_flags(s.as_ref(), slot)),
        );
        let picker = self.model_pickers[i].clone();
        let props = format!("{:?}", (&self.catalog_models, &r, &flags));
        if self.props_changed(&format!("picker:{slot}"), props) {
            let models = self.catalog_models.clone();
            picker.update(cx, |p, cx| p.set(models, &r, &flags, None, cx));
        }
        let (title, desc) = if slot == "title" {
            (
                "Workspace titles",
                "Names new workspaces from the first chat message",
            )
        } else {
            let m = CHAT_MODES.iter().find(|m| m.value == slot);
            (
                slot_title(slot),
                m.map(|m| {
                    if m.settings_desc.is_empty() {
                        m.hint
                    } else {
                        m.settings_desc
                    }
                })
                .unwrap_or(""),
            )
        };
        let t = tokens(cx);
        Row::new()
            .child(copy(slot, title, Some(vec![span(desc)]), cx))
            .child(ctrl().child(
                card(t.pane, t.line, 9.)
                    .flex()
                    .items_center()
                    .min_w_0()
                    .p(px(3.))
                    .child(picker),
            ))
    }

    pub(crate) fn models_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(s) = self.settings(cx).cloned() else {
            return div().into_any_element();
        };
        let list: Vec<Row> = (0..MODEL_SLOTS.len())
            .map(|i| self.picker_row(i, cx))
            .collect();
        let child = if s.child_workspace_model.is_empty() {
            "parent".to_string()
        } else {
            s.child_workspace_model.clone()
        };
        let opts = [("parent", "Calling chat"), ("mode", "Mode default")];
        let w = radios_width(&opts.map(|o| o.1), window);
        let mut radios = div().flex().flex_col().gap(px(2.)).w_full().mt(px(-6.));
        for (val, label) in opts {
            let on = child == val;
            radios = radios.child(
                mm_radio(SharedString::from(format!("child-{val}")), label, on, cx)
                    .debug_selector(move || format!("radio:Child workspace model:{label}:{on}"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.patch(false, |s| s.child_workspace_model = val.to_string(), cx)
                            .detach()
                    })),
            );
        }
        div()
            .flex()
            .flex_col()
            .child(rows(
                RowsCtx {
                    sides: false,
                    in_group: false,
                    more: false,
                },
                list,
                window,
                cx,
            ))
            .child(radio_rows(
                vec![(
                    copy(
                        "child-model",
                        "Child workspace model",
                        Some(vec![span(
                            "When Agent opens a child workspace. Mode default uses the model for that child's mode above.",
                        )]),
                        cx,
                    )
                    .into_any_element(),
                    radios.into_any_element(),
                    true,
                )],
                w,
                window,
                cx,
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_refs_and_labels() {
        assert_eq!(mode_model_ref(None, "ask"), FALLBACK);
        let mut s = Settings::default();
        s.model_loop = "codex/gpt-5".into();
        assert_eq!(mode_model_ref(Some(&s), "review"), "codex/gpt-5");
        s.model_review = "claude-code/opus".into();
        assert_eq!(mode_model_ref(Some(&s), "review"), "claude-code/opus");
        assert_eq!(
            mode_model_ref(Some(&s), "title"),
            "claude-code/claude-haiku-4-5-20251001"
        );
        assert_eq!(picker_labels("agent").0, "Agent / Coder model");
        assert_eq!(picker_labels("review").1, "Review flags");
        assert_eq!(picker_labels("title").0, "Workspace title model");
    }
}
