//! The UI-facing service contract: one trait per former Wails-bound Go
//! service, with the Go method names in snake_case and the Go doc comments.
//!
//! Every method is async and returns [`crate::Result`]: a Go `error` (or any
//! failed call) is an [`crate::Error`], the rejected promise the Svelte UI saw.
//! Arguments are borrowed; results are owned. Lists and maps never come back
//! as `null` (Wails' `createFrom` turned a nil slice into `[]`), so they are
//! plain `Vec` / `BTreeMap`. Views hold a [`Services`] and never reach past it.

mod agents;
mod artifacts;
mod briefing;
mod bundle;
mod code;
mod editor_session;
mod escalation;
mod inbox;
mod lsp_service;
mod maestro;
mod observatory;
mod permissions;
mod policy;
mod projects;
mod prs;
mod questions;
mod serde_util;
mod settings;
mod shell;
mod terminal;
mod workspaces;

pub mod lsp;
pub mod models;
pub mod usage;

use std::sync::Arc;

use tokio::sync::mpsc::UnboundedReceiver;

pub use agents::AgentsService;
pub use artifacts::ArtifactsService;
pub use briefing::BriefingService;
pub use bundle::BundleService;
pub use code::CodeService;
pub use editor_session::EditorSessionService;
pub use escalation::EscalationService;
pub use inbox::InboxService;
pub use lsp_service::LspService;
pub use maestro::MaestroService;
pub use models::*;
pub use observatory::ObservatoryService;
pub use permissions::PermissionsService;
pub use policy::PolicyService;
pub use projects::ProjectsService;
pub use prs::PrsService;
pub use questions::QuestionsService;
pub use settings::SettingsService;
pub use shell::ShellService;
pub use terminal::TerminalService;
pub use workspaces::WorkspacesService;

use crate::events::{AppEvent, EventHub};

/// A backend that implements every service: the real services over the
/// engine (slice SV) or [`crate::mock::MockBridge`].
pub trait Backend:
    AgentsService
    + ArtifactsService
    + BriefingService
    + BundleService
    + CodeService
    + EditorSessionService
    + EscalationService
    + InboxService
    + LspService
    + MaestroService
    + ObservatoryService
    + PermissionsService
    + PolicyService
    + ProjectsService
    + PrsService
    + QuestionsService
    + SettingsService
    + ShellService
    + TerminalService
    + WorkspacesService
    + 'static
{
}

impl<T> Backend for T where
    T: AgentsService
        + ArtifactsService
        + BriefingService
        + BundleService
        + CodeService
        + EditorSessionService
        + EscalationService
        + InboxService
        + LspService
        + MaestroService
        + ObservatoryService
        + PermissionsService
        + PolicyService
        + ProjectsService
        + PrsService
        + QuestionsService
        + SettingsService
        + ShellService
        + TerminalService
        + WorkspacesService
        + 'static
{
}

/// What the app holds: every service plus the app event hub. Cheap to clone;
/// field names follow Go's `app.Services`.
#[derive(Clone)]
pub struct Services {
    pub agents: Arc<dyn AgentsService>,
    pub artifacts: Arc<dyn ArtifactsService>,
    pub briefing: Arc<dyn BriefingService>,
    pub bundle: Arc<dyn BundleService>,
    pub code: Arc<dyn CodeService>,
    pub editor_session: Arc<dyn EditorSessionService>,
    pub escalation: Arc<dyn EscalationService>,
    pub inbox: Arc<dyn InboxService>,
    pub lsp: Arc<dyn LspService>,
    pub maestro: Arc<dyn MaestroService>,
    pub observatory: Arc<dyn ObservatoryService>,
    pub permissions: Arc<dyn PermissionsService>,
    pub policy: Arc<dyn PolicyService>,
    pub projects: Arc<dyn ProjectsService>,
    pub prs: Arc<dyn PrsService>,
    pub questions: Arc<dyn QuestionsService>,
    pub settings: Arc<dyn SettingsService>,
    pub shell: Arc<dyn ShellService>,
    pub terminal: Arc<dyn TerminalService>,
    pub workspaces: Arc<dyn WorkspacesService>,
    events: EventHub,
}

impl Services {
    /// Every service served by one backend, publishing on `events`.
    pub fn new<B: Backend>(backend: Arc<B>, events: EventHub) -> Self {
        Self {
            agents: backend.clone(),
            artifacts: backend.clone(),
            briefing: backend.clone(),
            bundle: backend.clone(),
            code: backend.clone(),
            editor_session: backend.clone(),
            escalation: backend.clone(),
            inbox: backend.clone(),
            lsp: backend.clone(),
            maestro: backend.clone(),
            observatory: backend.clone(),
            permissions: backend.clone(),
            policy: backend.clone(),
            projects: backend.clone(),
            prs: backend.clone(),
            questions: backend.clone(),
            settings: backend.clone(),
            shell: backend.clone(),
            terminal: backend.clone(),
            workspaces: backend,
            events,
        }
    }

    /// A new subscription to every app event published from now on, in
    /// publish order. Nothing is dropped; drop the receiver to unsubscribe.
    pub fn subscribe(&self) -> UnboundedReceiver<AppEvent> {
        self.events.subscribe()
    }

    /// Publishes an event to every subscriber. Views use this for the events
    /// the Svelte UI emitted itself (`menu.state`, `app.ready`, …); the desktop
    /// shell subscribes to those.
    pub fn emit(&self, event: AppEvent) {
        self.events.publish(event);
    }

    pub fn events(&self) -> &EventHub {
        &self.events
    }
}
