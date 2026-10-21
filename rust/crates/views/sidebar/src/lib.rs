//! Sidebar.
use gpui_kit::{Context, IntoElement, Render, Window, div};
use recur_view_core::ViewContext;
pub struct Sidebar;
impl Sidebar {
    pub fn new(_: ViewContext, _: &mut Window, _: &mut Context<Self>) -> Self { Sidebar }
}
impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement { div() }
}
