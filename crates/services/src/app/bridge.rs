//! The engine bus onto the app's events, and the services' own bus
//! subscriber: the file-index lifecycle and the per-workspace caches dropped with a workspace.

use std::sync::{Arc, Weak};

use recur_engine::EngineEvent;

use super::AppServices;
use crate::events::AppEvent;

/// The UI event for an engine event: same name, same payload JSON. Events
/// the UI never listened for (`workspace.watched`, `chat.renamed`,
/// `app.chat_tab`) map to `None`.
pub fn app_event(ev: &EngineEvent) -> Option<AppEvent> {
    let data = serde_json::to_value(ev).ok()?;
    match AppEvent::from_wire(ev.name(), data) {
        Ok(e) => Some(e),
        Err(crate::events::WireError::UnknownName(_)) => None,
        Err(e) => {
            tracing::warn!("event bridge: {e}");
            None
        }
    }
}

pub(super) fn attach(svc: &Arc<AppServices>) {
    // Synchronous on the publisher's task, so one publisher's events reach
    // every UI subscriber in order (engine plan §3); publishing only
    // enqueues.
    let hub = svc.hub.clone();
    let weak: Weak<AppServices> = Arc::downgrade(svc);
    svc.engine.bus.subscribe_fn(move |ev| {
        if let Some(app) = app_event(ev) {
            hub.publish(app);
        }
        let Some(s) = weak.upgrade() else { return };
        match ev {
            EngineEvent::WorkspaceDeleted(r) | EngineEvent::WorkspaceRemoved(r)
                if !r.workspace_id.is_empty() =>
            {
                s.editor.forget(&r.workspace_id);
                s.term_persist.forget(&r.workspace_id);
                s.index.drop_index(&r.workspace_id);
            }
            // Published under the engine's watch lock: pin only, no engine calls.
            EngineEvent::WorkspaceWatched(r) => s.index.set_pinned(&r.workspace_id),
            _ => {}
        }
    });

    // File-index work that stats the disk or reads the watched workspace runs
    // off the publisher.
    let mut rx = svc.engine.bus.subscribe();
    let weak = Arc::downgrade(svc);
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let Some(s) = weak.upgrade() else { return };
            match &*ev {
                EngineEvent::WorkspaceFsChanged(c) if !c.workspace_id.is_empty() => {
                    let Ok((root, _, _)) = s.resolve_scope("", &c.workspace_id) else {
                        continue;
                    };
                    s.index.apply_changes(
                        &c.workspace_id,
                        &root.to_string_lossy(),
                        &c.paths,
                        c.rescan.unwrap_or(false),
                    );
                }
                EngineEvent::AppFocused(_) => {
                    let ws = s.engine.watched_workspace();
                    if ws.is_empty() {
                        continue;
                    }
                    let Ok((root, _, _)) = s.resolve_scope("", &ws) else {
                        continue;
                    };
                    s.index.refresh_if_dirty(&ws, &root.to_string_lossy());
                }
                _ => {}
            }
        }
    });
}
