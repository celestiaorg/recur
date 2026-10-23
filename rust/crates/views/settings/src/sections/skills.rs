//! Skills: turn Recur and personal skills on or off (projects override these).

use gpui_kit::{
    AnyElement, Context, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, px,
};
use recur_code::view::skills::{
    SkillKind, map_skill_rows, skill_kind, skill_location, toggle_disabled_name,
};
use recur_ui::text::span;
use recur_ui::widgets::{settings_group, settings_groups};

use crate::style::{ctrl_text, lead, skill_copy, skill_list, tog};
use crate::view::SettingsView;

impl SettingsView {
    pub(crate) fn skills_pane(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let mut out = settings_groups().child(lead(
            "skills-lead",
            vec![span(
                "Default for Recur and personal skills. Each project can override these (and manage repo skills) on its project page.",
            )],
            cx,
        ));
        if self.skills.loading {
            return out
                .child(ctrl_text("skills-msg", vec![span("Loading skills…")], window, cx))
                .into_any_element();
        }
        if !self.skills.err.is_empty() {
            let e = self.skills.err.clone();
            return out
                .child(ctrl_text("skills-msg", vec![span(e)], window, cx))
                .into_any_element();
        }
        let mut recur = Vec::new();
        let mut user = Vec::new();
        for s in &self.skills.rows {
            if skill_kind(s) == SkillKind::Recur {
                recur.push(s.clone());
            } else {
                user.push(s.clone());
            }
        }
        let by_name = |a: &recur_code::view::skills::SkillRow, b: &recur_code::view::skills::SkillRow| {
            recur_editor::text::locale_compare(&a.name, &b.name)
        };
        recur.sort_by(by_name);
        user.sort_by(by_name);
        let groups = [
            ("recur", "Recur", "Built into Recur.", recur),
            (
                "user",
                "Your skills",
                "Skills installed for Claude, Cursor, Codex, and similar tools.",
                user,
            ),
        ];
        if groups.iter().all(|g| g.3.is_empty()) {
            return out
                .child(ctrl_text("skills-msg", vec![span("No skills found yet.")], window, cx))
                .into_any_element();
        }
        let disabled = self
            .settings(cx)
            .map(|s| s.disabled_skills.clone())
            .unwrap_or_default();
        let has_settings = self.settings(cx).is_some();
        for (kind, label, blurb, list) in groups {
            if list.is_empty() {
                continue;
            }
            let mut rows = Vec::new();
            for sk in list {
                let on = !disabled.iter().any(|d| *d == sk.name);
                let loc = if kind == "user" {
                    skill_location(&sk.path, &sk.scope, "")
                } else {
                    String::new()
                };
                let id = format!("skill-{}{}", sk.name, sk.path);
                let name = sk.name.clone();
                let copy = skill_copy(
                    &id,
                    format!("/{}", sk.name),
                    (!sk.description.is_empty()).then(|| sk.description.clone()),
                    (!loc.is_empty()).then(|| (loc, sk.path.clone())),
                    cx,
                );
                let mut t = tog(
                    SharedString::from(format!("tog-{id}")),
                    format!("{} {}", if on { "Disable" } else { "Enable" }, sk.name),
                    on,
                    !has_settings,
                    cx,
                )
                .mt(px(2.));
                if has_settings {
                    t = t.on_click(cx.listener(move |this, _, _, cx| {
                        let n = name.clone();
                        this.patch(
                            false,
                            move |s| {
                                s.disabled_skills = toggle_disabled_name(&s.disabled_skills, &n, !on)
                            },
                            cx,
                        )
                        .detach()
                    }));
                }
                rows.push((copy, vec![t.into_any_element()]));
            }
            out = out.child(
                settings_group(SharedString::from(format!("grp-skills-{kind}")), label)
                    .blurb(blurb)
                    .child(skill_list(rows, cx)),
            );
        }
        out.into_any_element()
    }

    /// The Skills effect: home and Recur skills (no repo scan), workspace ones left out.
    pub(crate) fn load_skills(&mut self, g: u64, cx: &mut Context<Self>) {
        self.skills.loading = true;
        self.skills.err.clear();
        let services = self.vcx.services.clone();
        cx.spawn(async move |this, cx| {
            let res = services.code.list_all_skills("", "").await;
            let _ = this.update(cx, |this, cx| {
                if !this.fx.live("section", g) {
                    return;
                }
                match res {
                    Ok(list) => {
                        this.skills.rows = map_skill_rows(&list)
                            .into_iter()
                            .filter(|s| s.scope != "workspace")
                            .collect()
                    }
                    Err(e) => {
                        this.skills.rows.clear();
                        this.skills.err = e.message.clone();
                    }
                }
                this.skills.loading = false;
                cx.notify();
            });
        })
        .detach();
    }
}
