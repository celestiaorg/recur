//! The pure New-workspace launch rules: when the
//! modal may create eagerly, the eager-create handle (abandon a row the
//! user walked away from, even one still in flight), the opening chat seed
//! after ExpandGoal, and which first replies are pre-turn failures that
//! keep the parked opening. The store-driven flows (⇧⌘N, revealing or
//! parking the opening) are `recur_app::new_workspace`'s.

use std::future::Future;
use std::sync::Arc;

use parking_lot::Mutex;
use recur_domain::Workspace;

use super::mentions::{compose_marked_text, strip_attachment_markers};

/// The project and base an eager create starts from: only once both are
/// known, the modal is open, it is not a loop, and no submit claimed the
/// row. Re-rendered project/branch lists must not restart it.
pub fn eager_create_input(
    open: bool,
    r#loop: bool,
    project_id: &str,
    branch: &str,
    claimed: bool,
) -> Option<(String, String)> {
    if !open || r#loop || claimed || project_id.is_empty() || branch.is_empty() {
        return None;
    }
    Some((project_id.to_owned(), branch.to_owned()))
}

type Abandon = Arc<dyn Fn(&str) + Send + Sync>;

#[derive(Default)]
struct EagerState {
    cancelled: bool,
    id: String,
}

/// Cancels an eager create: deletes the row now when it resolved, or as
/// soon as it does.
#[derive(Clone)]
pub struct EagerCancel {
    state: Arc<Mutex<EagerState>>,
    abandon: Abandon,
}

impl EagerCancel {
    pub fn cancel(&self) {
        let id = {
            let mut s = self.state.lock();
            s.cancelled = true;
            s.id.clone()
        };
        if !id.is_empty() {
            (self.abandon)(&id);
        }
    }
}

/// Starts an eager create. The future resolves to the created row (`None`
/// on failure or after cancel); `on_ready` paints it unless cancelled.
pub fn start_eager_create<F>(
    create: F,
    on_ready: impl FnOnce(&Workspace) + Send + 'static,
    abandon: impl Fn(&str) + Send + Sync + 'static,
) -> (EagerCancel, impl Future<Output = Option<Workspace>> + Send)
where
    F: Future<Output = Option<Workspace>> + Send,
{
    let handle = EagerCancel {
        state: Arc::default(),
        abandon: Arc::new(abandon),
    };
    let h = handle.clone();
    let fut = async move {
        let ws = create.await.filter(|w| !w.id.is_empty())?;
        let cancelled = {
            let mut s = h.state.lock();
            s.id = ws.id.clone();
            s.cancelled
        };
        if cancelled {
            (h.abandon)(&ws.id);
            return None;
        }
        on_ready(&ws);
        Some(ws)
    };
    (handle, fut)
}

/// Effect cleanup after an eager create: the pending id is always dropped
/// (a later open must not submit into it); the row is abandoned only when
/// no submit claimed it.
pub fn end_eager_create(claimed: bool, cancel: &EagerCancel) {
    if !claimed {
        cancel.cancel();
    }
}

/// The painted pending row, else whatever the in-flight create resolves to.
pub async fn take_eager_workspace<F>(
    pending_id: &str,
    pending: Option<F>,
    rows: &[Workspace],
) -> Option<Workspace>
where
    F: Future<Output = Option<Workspace>>,
{
    if !pending_id.is_empty()
        && let Some(found) = rows.iter().find(|w| w.id == pending_id)
    {
        return Some(found.clone());
    }
    match pending {
        Some(p) => p.await,
        None => None,
    }
}

/// The opening chat's wire prompt and bubble label after ExpandGoal: a ⌘K
/// draft keeps its label over the wire; otherwise the expansion's prompt
/// (else the typed goal) and display.
pub fn opening_chat_seed(
    typed: &str,
    draft_wire: &str,
    draft_display: &str,
    expanded: Option<(&str, &str)>,
) -> (String, String) {
    let t = typed.trim();
    if !draft_wire.is_empty() && t == draft_display {
        return (draft_wire.to_owned(), draft_display.to_owned());
    }
    let prompt = expanded.map(|(p, _)| p.trim()).unwrap_or_default();
    let display = expanded.map(|(_, d)| d.trim()).unwrap_or_default();
    (
        if prompt.is_empty() { t } else { prompt }.to_owned(),
        display.to_owned(),
    )
}

/// The seeded opening keeps inline attachment markers where the user put
/// them unless expansion rewrote the prompt (then drafts are appended).
pub fn opening_seed_text<'a>(
    marked: &str,
    prompt: &str,
    draft_ids: impl IntoIterator<Item = &'a str>,
) -> String {
    let stripped = strip_attachment_markers(marked);
    if prompt.trim().is_empty() || strip_attachment_markers(prompt).trim() == stripped.trim() {
        return marked.to_owned();
    }
    compose_marked_text(prompt, draft_ids)
}

/// Replies the send returns before the user row is recorded: the parked
/// opening seed is kept so the human can retry.
pub fn is_parked_opening_failure(text: &str) -> bool {
    [
        "I couldn't save that image attachment.",
        "I couldn't run that prompt action:",
        "I couldn't engage the loop:",
        "I can't find that workspace",
        "I can't run right now:",
        "I'm at capacity",
        "There's nothing to retry",
        "A loop is running in this workspace",
        "Create a workspace from this pull request",
    ]
    .iter()
    .any(|p| text.starts_with(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::mentions::attachment_marker;

    fn w(id: &str) -> Workspace {
        Workspace {
            id: id.into(),
            project_id: "p".into(),
            ..Workspace::default()
        }
    }

    #[test]
    fn eager_input_starts_once_repo_and_base_are_known() {
        assert_eq!(
            eager_create_input(true, false, "p1", "main", false),
            Some(("p1".into(), "main".into()))
        );
        assert_eq!(eager_create_input(false, false, "p1", "main", false), None);
        assert_eq!(eager_create_input(true, true, "p1", "main", false), None);
        assert_eq!(eager_create_input(true, false, "", "main", false), None);
        assert_eq!(eager_create_input(true, false, "p1", "", false), None);
        assert_eq!(eager_create_input(true, false, "p1", "main", true), None);
    }

    fn recorder() -> (
        Arc<Mutex<Vec<String>>>,
        impl Fn(&str) + Send + Sync + 'static,
    ) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = Arc::clone(&log);
        (log, move |id: &str| l.lock().push(id.to_owned()))
    }

    #[tokio::test]
    async fn abandons_a_workspace_that_resolves_after_cancel() {
        let (tx, rx) = tokio::sync::oneshot::channel::<Workspace>();
        let ready = Arc::new(Mutex::new(Vec::<String>::new()));
        let r = Arc::clone(&ready);
        let (abandoned, abandon) = recorder();
        let (handle, fut) = start_eager_create(
            async move { rx.await.ok() },
            move |ws| r.lock().push(ws.id.clone()),
            abandon,
        );
        let task = tokio::spawn(fut);
        handle.cancel();
        tx.send(w("WS-1")).unwrap();
        assert_eq!(task.await.unwrap(), None);
        assert!(ready.lock().is_empty());
        assert_eq!(*abandoned.lock(), ["WS-1"]);
    }

    #[tokio::test]
    async fn abandons_a_workspace_that_was_already_painted() {
        let ready = Arc::new(Mutex::new(Vec::<String>::new()));
        let r = Arc::clone(&ready);
        let (abandoned, abandon) = recorder();
        let (handle, fut) = start_eager_create(
            async { Some(w("WS-1")) },
            move |ws| r.lock().push(ws.id.clone()),
            abandon,
        );
        assert_eq!(fut.await.map(|w| w.id), Some("WS-1".into()));
        assert_eq!(*ready.lock(), ["WS-1"]);
        handle.cancel();
        assert_eq!(*abandoned.lock(), ["WS-1"]);
    }

    #[tokio::test]
    async fn after_a_claimed_create_submit_waits_for_the_next_one() {
        let rows = Arc::new(Mutex::new(Vec::<Workspace>::new()));
        let pending_id = Arc::new(Mutex::new(String::new()));
        let (abandoned, abandon) = recorder();
        let abandon = Arc::new(abandon);
        let (r1, p1) = (Arc::clone(&rows), Arc::clone(&pending_id));
        let a1 = Arc::clone(&abandon);
        let (first, fut) = start_eager_create(
            async { Some(w("WS-FIRST")) },
            move |ws| {
                *p1.lock() = ws.id.clone();
                r1.lock().push(ws.clone());
            },
            move |id| a1(id),
        );
        assert_eq!(fut.await.map(|w| w.id), Some("WS-FIRST".into()));
        end_eager_create(true, &first);
        pending_id.lock().clear();
        assert!(abandoned.lock().is_empty());

        let (tx, rx) = tokio::sync::oneshot::channel::<Workspace>();
        let (r2, p2) = (Arc::clone(&rows), Arc::clone(&pending_id));
        let a2 = Arc::clone(&abandon);
        let (_second, fut2) = start_eager_create(
            async move { rx.await.ok() },
            move |ws| {
                *p2.lock() = ws.id.clone();
                r2.lock().push(ws.clone());
            },
            move |id| a2(id),
        );
        let id = pending_id.lock().clone();
        let snapshot = rows.lock().clone();
        let taken =
            tokio::spawn(async move { take_eager_workspace(&id, Some(fut2), &snapshot).await });
        tx.send(w("WS-SECOND")).unwrap();
        assert_eq!(taken.await.unwrap().map(|w| w.id), Some("WS-SECOND".into()));
        assert!(abandoned.lock().is_empty());
    }

    #[tokio::test]
    async fn unclaimed_cleanup_abandons_the_pending_workspace() {
        let (abandoned, abandon) = recorder();
        let (handle, fut) = start_eager_create(async { Some(w("WS-1")) }, |_| {}, abandon);
        fut.await;
        end_eager_create(false, &handle);
        assert_eq!(*abandoned.lock(), ["WS-1"]);
    }

    #[test]
    fn opening_chat_seed_rules() {
        let wire = "Work on Linear issue ENG-1 in this workspace.\n\n## Description\nFull prose";
        let label = "Linear ENG-1 · Fix the thing";
        assert_eq!(
            opening_chat_seed(label, wire, label, None),
            (wire.into(), label.into())
        );
        assert_eq!(
            opening_chat_seed(
                "https://linear.app/acme/issue/ENG-1",
                "",
                "",
                Some((wire, label))
            ),
            (wire.into(), label.into())
        );
        assert_eq!(
            opening_chat_seed("add dark mode", "", "", Some(("add dark mode", ""))),
            ("add dark mode".into(), String::new())
        );
    }

    #[test]
    fn opening_seed_text_keeps_or_appends_markers() {
        let (a, b) = (attachment_marker("imgA"), attachment_marker("imgB"));
        let marked = format!("Before {a}, after {b}");
        assert_eq!(
            opening_seed_text(&marked, "Before , after", ["imgA", "imgB"]),
            marked
        );
        let marked = format!("https://linear.app/acme/issue/ENG-1 {a}");
        let wire = "Work on Linear issue ENG-1";
        assert_eq!(
            opening_seed_text(&marked, wire, ["imgA", "imgB"]),
            format!("{wire}\n{a}\n{b}")
        );
    }

    #[test]
    fn parked_opening_failures_are_pre_turn_replies() {
        assert!(is_parked_opening_failure(
            "I couldn't save that image attachment. Try a smaller PNG/JPEG/GIF/WebP."
        ));
        assert!(is_parked_opening_failure(
            "I'm at capacity — try again in a moment."
        ));
        assert!(!is_parked_opening_failure("I'll look at the flaky test."));
        assert!(!is_parked_opening_failure(""));
    }
}
