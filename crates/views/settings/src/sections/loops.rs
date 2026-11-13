//! Loops: the default loop and policy New Task and new runs start with.

use gpui_kit::{AnyElement, Context, IntoElement, ParentElement, Styled, Window, div};
use recur_engine::view::loops::{
    LoopInfo, PolicyInfo, autonomy_from_policy_id, enabled_loops, enabled_policies,
};
use recur_ui::text::span;
use recur_view_core::call;

use crate::style::{Row, RowsCtx, copy, rows, tokens};
use crate::view::SettingsView;

impl SettingsView {
    pub(crate) fn loops_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = self.settings(cx).cloned();
        let loops = enabled_loops(&self.catalog_loops, s.as_ref());
        let policies = enabled_policies(&self.catalog_policies, s.as_ref());
        let (l, p) = (self.default_loop.clone(), self.default_policy.clone());
        let backdrop = tokens(cx).bg;
        let props = format!("{:?}", (&l, &p, &loops, &policies, backdrop));
        if self.props_changed("loop-pickers", props) {
            self.loop_pickers.update(cx, |x, cx| {
                x.set(&l, &p, &loops, &policies, false, backdrop, cx)
            });
        }
        rows(
            RowsCtx {
                sides: false,
                in_group: false,
                more: false,
            },
            vec![
                Row::stack()
                    .child(copy(
                        "default-loop",
                        "Default workflow and policy",
                        Some(vec![span(
                            "Used on New Task and when a workspace has not picked its own. A run pins both after it starts.",
                        )]),
                        cx,
                    ))
                    .child(div().flex().child(self.loop_pickers.clone())),
            ],
            window,
            cx,
        )
        .into_any_element()
    }

    /// The Loops effect: the bundle catalogs (kept as the shipped fallbacks when empty).
    pub(crate) fn load_loops(&mut self, g: u64, cx: &mut Context<Self>) {
        let svc = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let loops = svc.bundle.list_loops().await.unwrap_or_default();
            let policies = svc.bundle.list_policies().await.unwrap_or_default();
            let _ = this.update(cx, |v, cx| {
                if !v.fx.live("section", g) {
                    return;
                }
                if !loops.is_empty() {
                    v.catalog_loops = loops
                        .into_iter()
                        .map(|l| LoopInfo {
                            id: l.id,
                            name: l.name,
                            hint: l.hint,
                        })
                        .collect();
                }
                if !policies.is_empty() {
                    v.catalog_policies = policies
                        .into_iter()
                        .map(|p| PolicyInfo {
                            id: p.id,
                            name: p.name,
                            hint: p.hint,
                        })
                        .collect();
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// `persistLoopPolicy`: save both defaults, and the legacy autonomy when the policy maps
    /// to a different one.
    pub(crate) fn persist_loop_policy(&mut self, l: String, p: String, cx: &mut Context<Self>) {
        self.default_loop = l.clone();
        self.default_policy = p.clone();
        let (dl, dp) = (l, p.clone());
        self.patch(
            false,
            move |s| {
                s.default_loop = dl;
                s.default_policy = dp;
            },
            cx,
        )
        .detach();
        let a = autonomy_from_policy_id(&p);
        let state = self.vcx.state.clone();
        let Some(mut policy) = state.read(cx).policy.clone() else {
            cx.notify();
            return;
        };
        if policy.autonomy.as_str() != a {
            policy.autonomy = a.into();
            let services = self.vcx.services.clone();
            cx.spawn(async move |_, cx| {
                if let Some(saved) = call(
                    cx,
                    services.policy.set_policy(&policy),
                    "Could not save policy",
                )
                .await
                {
                    cx.update(|cx| {
                        state.update(cx, |s, cx| {
                            s.policy = Some(saved);
                            cx.notify();
                        })
                    });
                }
            })
            .detach();
        }
        cx.notify();
    }
}
