//! The panes, one module each, and the routing of field events to them.

pub mod appearance;
pub mod bundles;
pub mod data;
pub mod editor;
pub mod general;
pub mod integrations;
pub mod loops;
pub mod mcp;
pub mod models;
pub mod providers;
pub mod prs;
pub mod skills;

use gpui_kit::{AnyElement, Context, IntoElement, Window, div};

use crate::field::FieldEvent;
use crate::nav::Section;
use crate::view::SettingsView;

/// The active pane's body.
pub fn pane(
    v: &mut SettingsView,
    section: Option<Section>,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) -> AnyElement {
    match section {
        Some(Section::Agent) => v.providers_pane(window, cx),
        Some(Section::Models) => v.models_pane(window, cx),
        Some(Section::Loops) => v.loops_pane(window, cx),
        Some(Section::General) => v.general_pane(window, cx),
        Some(Section::Appearance) => v.appearance_pane(window, cx),
        Some(Section::Editor) => v.editor_pane(window, cx),
        Some(Section::Skills) => v.skills_pane(window, cx),
        Some(Section::BundleCommands) => v.commands_pane(window, cx),
        Some(Section::BundlePrompts) => v.prompts_pane(window, cx),
        Some(Section::BundleGrammars) => v.grammars_pane(window, cx),
        Some(Section::BundleThemes) => v.themes_pane(window, cx),
        Some(Section::BundleSnippets) => v.snippets_pane(window, cx),
        Some(Section::BundleLsp) => v.lsp_pane(window, cx),
        Some(Section::Integrations) => v.integrations_pane(window, cx),
        Some(Section::Mcp) => v.mcp_pane(window, cx),
        Some(Section::Prs) => v.prs_pane(window, cx),
        Some(Section::Data) => v.data_pane(window, cx),
        None => div().into_any_element(),
    }
}

/// A field event, by the field's key.
pub fn on_field(
    v: &mut SettingsView,
    key: &str,
    ev: FieldEvent,
    value: String,
    window: &mut Window,
    cx: &mut Context<SettingsView>,
) {
    match key {
        "nav-search" if ev == FieldEvent::Input => v.sync(window, cx),
        "codex-key" | "codex-bin" => v.codex_field(key, ev, value, cx),
        "max-loops" | "max-agents" | "turn-timeout" | "sched-keep" | "branch-prefix" => {
            v.general_field(key, ev, value, cx)
        }
        "max-open-files" => v.editor_field(key, ev, value, cx),
        k if k.starts_with("exclude:") => v.editor_field(key, ev, value, cx),
        k if k.starts_with("bundle:") => v.bundle_field(&k["bundle:".len()..], ev, value, cx),
        _ => {}
    }
}
