//! `PRListTitlebar.svelte`: the dashboard's counts beside the crumb.

use gpui_kit::{
    Context, Entity, InteractiveElement, IntoElement, ParentElement, Render, Styled, Window, div,
};
use recur_ui::Theme;
use recur_ui::text::{TextStyled, text};
use recur_view_core::{AppState, ViewContext};

use crate::list_filter::{scoped_prs, title_counts};
use crate::ui::{PrUi, ui};

pub struct PrListTitlebar {
    state: Entity<AppState>,
    ui: Entity<PrUi>,
}

impl PrListTitlebar {
    pub fn new(vcx: &ViewContext, cx: &mut Context<Self>) -> Self {
        let ui = ui(cx);
        cx.observe(&vcx.state, |_, _, cx| cx.notify()).detach();
        cx.observe(&ui, |_, _, cx| cx.notify()).detach();
        PrListTitlebar {
            state: vcx.state.clone(),
            ui,
        }
    }
}

impl PrListTitlebar {
    fn render_view(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = Theme::of(cx).tokens;
        let s = self.state.read(cx);
        let ids = &self.ui.read(cx).project_ids;
        let scoped = scoped_prs(&s.prs, ids, |ws| {
            s.workspaces
                .iter()
                .find(|w| w.id == ws && !w.project_id.is_empty())
                .map(|w| w.project_id.as_str())
        });
        let c = title_counts(scoped);
        let label = format!("{} need you · {} open · {} merged", c.you, c.open, c.merged);
        let sel = format!("tb-sub={label}");
        div()
            .debug_selector(move || sel.clone())
            .flex_none()
            .child(text(label).mono().size(11.).color(t.faint).nowrap())
    }
}

impl Render for PrListTitlebar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        recur_ui::guard::view(self, window, cx, Self::render_view)
    }
}
