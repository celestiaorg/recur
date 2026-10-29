//! The editor session's RPC client: debounced, change-gated session writes
//! through `EditorSessionService`, one ordered queue per workspace, the synchronous quit
//! journal, the one-time legacy import, and the explorer scope. The session shape, caps and
//! patch building are `recur_editor::session`.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Duration;

use futures::channel::oneshot;
use futures::future::LocalBoxFuture;
use gpui_kit::{App, AppContext as _, AsyncApp, Global, Task};
use recur_editor::session::{
    ScratchDoc, SentState, Session, StashedBuffer, build_patch, normalize_session,
    restorable_session, session_from_wire,
};
use recur_editor::view_state::parse_file_views;
use recur_services::Services;
use recur_services::api::EditorSessionPatch;
use recur_view_core::{AppState, LocalStorage};
use serde_json::{Map, Value, json};

/// Legacy single blob and per-workspace keys from the localStorage era.
const LEGACY_V1_KEY: &str = "recur.editorSession.v1";
const LEGACY_V2_PREFIX: &str = "recur.editorSession.v2:";
/// The synchronous last line of defence for quit: written in the same turn as a flush.
const QUIT_JOURNAL_PREFIX: &str = "recur.editorSession.quit:";
const SCOPE_KEY: &str = "recur.editorScope.v1";

/// A loaded session and whether its syntax maps are authoritative (`syntaxMapsKnown`:
/// `None` when the field was absent, as for a session read from the backend).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LoadedSession {
    pub session: Session,
    pub syntax_maps_known: Option<bool>,
}

type Job = Box<dyn FnOnce(AsyncApp) -> LocalBoxFuture<'static, ()>>;

#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    running: bool,
}

struct PendingSave {
    workspace_id: String,
    session: Session,
}

/// The session writer's state.
#[derive(Default)]
pub struct SessionHost {
    sent: HashMap<String, SentState>,
    queues: HashMap<String, Queue>,
    pending: Option<PendingSave>,
    save_timer: Option<Task<()>>,
    journal_gen: HashMap<String, u64>,
    /// `None` until the legacy import ran; then the imported sessions, one restore each.
    migrated: Option<HashMap<String, Session>>,
    /// Overrides the app's services (tests).
    services: Option<Services>,
}

impl Global for SessionHost {}

fn services(cx: &App) -> Option<Services> {
    if let Some(s) = cx
        .try_global::<SessionHost>()
        .and_then(|h| h.services.clone())
    {
        return Some(s);
    }
    AppState::try_global(cx).map(|s| s.read(cx).services().clone())
}

fn with_storage<R>(cx: &mut App, f: impl FnOnce(&mut LocalStorage) -> R) -> Option<R> {
    let state = AppState::try_global(cx)?;
    Some(state.update(cx, |s, _| f(s.storage_mut())))
}

fn host(cx: &mut App) -> &mut SessionHost {
    cx.default_global::<SessionHost>()
}

/// Serializes RPCs per workspace so the backend merge sees patches in order.
fn enqueue(workspace_id: &str, job: Job, cx: &mut App) {
    let q = host(cx).queues.entry(workspace_id.to_string()).or_default();
    q.jobs.push_back(job);
    if q.running {
        return;
    }
    q.running = true;
    let ws = workspace_id.to_string();
    cx.spawn(async move |cx| {
        loop {
            let next = cx.update(|cx| {
                let q = host(cx).queues.get_mut(&ws)?;
                let job = q.jobs.pop_front();
                if job.is_none() {
                    q.running = false;
                }
                job
            });
            let Some(job) = next else { break };
            job(cx.clone()).await;
        }
    })
    .detach();
}

fn to_patch(p: recur_editor::session::WirePatch) -> EditorSessionPatch {
    let mut v = serde_json::to_value(&p).unwrap_or_default();
    // Scroll offsets are whole CSS px on the wire (WebKit's `scrollTop`; the model is an int).
    if let Some(views) = v
        .pointer_mut("/meta/fileViews")
        .and_then(Value::as_object_mut)
    {
        for view in views.values_mut().filter_map(Value::as_object_mut) {
            for n in view.values_mut() {
                if let Some(f) = n.as_f64().filter(|_| n.is_f64()) {
                    *n = Value::from(f.trunc() as i64);
                }
            }
        }
    }
    serde_json::from_value(v).unwrap_or_else(|e| {
        tracing::warn!("editor session patch: {e}");
        EditorSessionPatch::default()
    })
}

fn string_map(v: Option<&Value>) -> BTreeMap<String, String> {
    v.and_then(Value::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    (!k.is_empty()).then_some((k.clone(), v.as_str()?.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty(m: BTreeMap<String, String>) -> Option<BTreeMap<String, String>> {
    (!m.is_empty()).then_some(m)
}

/// A session object as the TypeScript stored it (quit journal, legacy keys).
pub fn session_from_object(v: &Value) -> Option<LoadedSession> {
    let o = v.as_object()?;
    let open: Vec<String> = o
        .get("openFiles")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| p.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let mut stashed = BTreeMap::new();
    if let Some(bufs) = o.get("stashedBuffers").and_then(Value::as_object) {
        for (path, b) in bufs {
            let Some(draft) = b.get("draft").and_then(Value::as_str) else {
                continue;
            };
            stashed.insert(
                path.clone(),
                StashedBuffer {
                    draft: draft.to_string(),
                    saved: b.get("saved").and_then(Value::as_str).map(str::to_string),
                    base_hash: b
                        .get("baseHash")
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            );
        }
    }
    let mut scratch = BTreeMap::new();
    if let Some(docs) = o.get("scratchDocs").and_then(Value::as_object) {
        for (path, d) in docs {
            if let Ok(doc) = serde_json::from_value::<ScratchDoc>(d.clone()) {
                scratch.insert(path.clone(), doc);
            }
        }
    }
    let views = o.get("fileViews").map(parse_file_views).unwrap_or_default();
    let str_field = |k: &str| o.get(k).and_then(Value::as_str).map(str::to_string);
    Some(LoadedSession {
        session: Session {
            open_files: open,
            selected_path: str_field("selectedPath").unwrap_or_default(),
            split_path: str_field("splitPath").filter(|s| !s.is_empty()),
            scratch_docs: (!scratch.is_empty()).then_some(scratch),
            scratch_seq: o
                .get("scratchSeq")
                .and_then(Value::as_f64)
                .map(|n| n as u64),
            stashed_buffers: (!stashed.is_empty()).then_some(stashed),
            file_views: (!views.is_empty()).then_some(views),
            syntax_by_path: non_empty(string_map(o.get("syntaxByPath"))),
            syntax_by_file_type: non_empty(string_map(o.get("syntaxByFileType"))),
            main_surface_review: str_field("mainSurface").as_deref() == Some("review"),
            left_kind: str_field("leftKind"),
            right_kind: str_field("rightKind"),
            left_open: o.get("leftOpen").and_then(Value::as_bool),
            right_open: o.get("rightOpen").and_then(Value::as_bool),
            syntax_maps_known: o.get("syntaxMapsKnown").and_then(Value::as_bool) == Some(true),
        },
        syntax_maps_known: o.get("syntaxMapsKnown").and_then(Value::as_bool),
    })
}

/// The session as the TypeScript serialized it (`JSON.stringify(session)`).
pub fn session_to_object(s: &Session, syntax_maps_known: Option<bool>) -> Value {
    let mut o = Map::new();
    o.insert("openFiles".into(), json!(s.open_files));
    o.insert("selectedPath".into(), json!(s.selected_path));
    if let Some(sp) = &s.split_path {
        o.insert("splitPath".into(), json!(sp));
    }
    if let Some(docs) = &s.scratch_docs {
        o.insert("scratchDocs".into(), json!(docs));
    }
    if let Some(n) = s.scratch_seq {
        o.insert("scratchSeq".into(), json!(n));
    }
    if let Some(bufs) = &s.stashed_buffers {
        let m: Map<String, Value> = bufs
            .iter()
            .map(|(p, b)| {
                let mut e = Map::new();
                e.insert("draft".into(), json!(b.draft));
                if let Some(saved) = &b.saved {
                    e.insert("saved".into(), json!(saved));
                }
                if let Some(h) = &b.base_hash {
                    e.insert("baseHash".into(), json!(h));
                }
                (p.clone(), Value::Object(e))
            })
            .collect();
        o.insert("stashedBuffers".into(), Value::Object(m));
    }
    if let Some(v) = &s.file_views {
        o.insert("fileViews".into(), json!(v));
    }
    if let Some(m) = &s.syntax_by_path {
        o.insert("syntaxByPath".into(), json!(m));
    }
    if let Some(m) = &s.syntax_by_file_type {
        o.insert("syntaxByFileType".into(), json!(m));
    }
    if s.main_surface_review {
        o.insert("mainSurface".into(), json!("review"));
    }
    for (k, v) in [("leftKind", &s.left_kind), ("rightKind", &s.right_kind)] {
        if let Some(v) = v {
            o.insert(k.into(), json!(v));
        }
    }
    for (k, v) in [("leftOpen", s.left_open), ("rightOpen", s.right_open)] {
        if let Some(v) = v {
            o.insert(k.into(), json!(v));
        }
    }
    if let Some(k) = syntax_maps_known {
        o.insert("syntaxMapsKnown".into(), json!(k));
    }
    Value::Object(o)
}

fn workspace_id_from_storage_key(id: &str) -> &str {
    id.rfind("::").map_or(id, |i| &id[i + 2..])
}

fn journal_key(ws: &str) -> String {
    format!("{QUIT_JOURNAL_PREFIX}{ws}")
}

fn read_journal(ws: &str, cx: &mut App) -> Option<LoadedSession> {
    let raw = with_storage(cx, |s| s.get(&journal_key(ws)).map(str::to_string))??;
    let v: Value = serde_json::from_str(&raw).ok()?;
    session_from_object(&v)
}

fn write_journal(ws: &str, session: &Session, cx: &mut App) {
    *host(cx).journal_gen.entry(ws.to_string()).or_default() += 1;
    let known = session.syntax_maps_known.then_some(true);
    let raw = session_to_object(session, known).to_string();
    with_storage(cx, |s| s.set(&journal_key(ws), raw));
}

fn drop_journal(ws: &str, cx: &mut App) {
    with_storage(cx, |s| s.remove(&journal_key(ws)));
}

/// Pushes one legacy or journal session as a full patch (best effort). Runs inside a queue
/// job, so it does not enqueue.
async fn import_session(ws: String, sess: Session, cx: &mut AsyncApp) {
    let Some(svc) = cx.update(|cx| services(cx)) else {
        return;
    };
    let (patch, next) = build_patch(&normalize_session(&sess), None);
    let Some(patch) = patch else { return };
    if svc
        .editor_session
        .update_editor_session(&ws, &to_patch(patch))
        .await
        .is_ok()
    {
        cx.update(|cx| {
            host(cx).sent.insert(ws, next);
        });
    }
}

/// Sweeps every legacy localStorage session into the backend once, then removes the keys.
async fn migrate_legacy(cx: &mut AsyncApp) {
    let done = cx.update(|cx| host(cx).migrated.is_some());
    if done {
        return;
    }
    let found = cx.update(|cx| {
        with_storage(cx, |s| {
            let mut by_ws: Vec<(String, Session)> = Vec::new();
            let mut put = |ws: &str, sess: Session| {
                if let Some(slot) = by_ws.iter_mut().find(|(w, _)| w == ws) {
                    slot.1 = sess;
                } else {
                    by_ws.push((ws.to_string(), sess));
                }
            };
            if let Some(raw) = s.get(LEGACY_V1_KEY)
                && let Ok(Value::Object(all)) = serde_json::from_str::<Value>(raw)
            {
                for (id, v) in &all {
                    let ws = workspace_id_from_storage_key(id);
                    if ws.is_empty() || ws == "_" {
                        continue;
                    }
                    if let Some(l) = session_from_object(v) {
                        put(ws, l.session);
                    }
                }
            }
            let v2: Vec<String> = s
                .keys()
                .filter(|k| k.starts_with(LEGACY_V2_PREFIX))
                .map(str::to_string)
                .collect();
            for k in &v2 {
                let ws = workspace_id_from_storage_key(&k[LEGACY_V2_PREFIX.len()..]);
                if ws.is_empty() || ws == "_" {
                    continue;
                }
                if let Some(l) = s
                    .get(k)
                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                    .and_then(|v| session_from_object(&v))
                {
                    put(ws, l.session);
                }
            }
            (by_ws, v2)
        })
    });
    let Some((by_ws, v2)) = found else {
        cx.update(|cx| host(cx).migrated = Some(HashMap::new()));
        return;
    };
    for (ws, sess) in &by_ws {
        import_session(ws.clone(), sess.clone(), cx).await;
    }
    cx.update(|cx| {
        with_storage(cx, |s| {
            s.remove(LEGACY_V1_KEY);
            for k in &v2 {
                s.remove(k);
            }
        });
        host(cx).migrated = Some(by_ws.into_iter().collect());
    });
}

/// `loadEditorSession`: `Ok(None)` when there is genuinely nothing to restore, `Err` when the
/// backend read failed (the caller must not clear a session it never saw).
pub fn load_editor_session(
    workspace_id: &str,
    cx: &mut App,
) -> Task<Result<Option<LoadedSession>, String>> {
    if workspace_id.is_empty() {
        return Task::ready(Ok(None));
    }
    let (tx, rx) = oneshot::channel();
    let ws = workspace_id.to_string();
    enqueue(
        workspace_id,
        Box::new(move |mut cx| {
            Box::pin(async move {
                let res = load_inner(&ws, &mut cx).await;
                let _ = tx.send(res);
            })
        }),
        cx,
    );
    cx.background_spawn(async move { rx.await.unwrap_or(Err("cancelled".into())) })
}

async fn load_inner(ws: &str, cx: &mut AsyncApp) -> Result<Option<LoadedSession>, String> {
    migrate_legacy(cx).await;
    let svc = cx.update(|cx| services(cx)).ok_or("no services")?;
    let migrated = cx.update(|cx| host(cx).migrated.as_mut().and_then(|m| m.remove(ws)));
    let journal = cx.update(|cx| read_journal(ws, cx));
    let loaded = if let Some(journal) = journal {
        let mut recovered = journal;
        if recovered.syntax_maps_known != Some(true) {
            match svc.editor_session.get_editor_session(ws).await {
                Ok(wire) => {
                    let stored = wire
                        .and_then(|w| serde_json::to_value(w).ok())
                        .and_then(|v| session_from_wire(&v));
                    let merge =
                        |stored: Option<&BTreeMap<String, String>>,
                         mine: Option<&BTreeMap<String, String>>| {
                            let mut m = stored.cloned().unwrap_or_default();
                            m.extend(mine.cloned().unwrap_or_default());
                            non_empty(m)
                        };
                    let s = &mut recovered.session;
                    s.syntax_by_path = merge(
                        stored.as_ref().and_then(|x| x.syntax_by_path.as_ref()),
                        s.syntax_by_path.as_ref(),
                    );
                    s.syntax_by_file_type = merge(
                        stored.as_ref().and_then(|x| x.syntax_by_file_type.as_ref()),
                        s.syntax_by_file_type.as_ref(),
                    );
                    s.syntax_maps_known = true;
                    recovered.syntax_maps_known = Some(true);
                }
                Err(_) => {
                    recovered.session.syntax_maps_known = false;
                    recovered.syntax_maps_known = Some(false);
                }
            }
        }
        import_session(ws.to_string(), recovered.session.clone(), cx).await;
        cx.update(|cx| drop_journal(ws, cx));
        Some(recovered)
    } else if let Some(sess) = migrated {
        Some(LoadedSession {
            session: sess,
            syntax_maps_known: None,
        })
    } else {
        let wire = svc
            .editor_session
            .get_editor_session(ws)
            .await
            .map_err(|e| e.message)?;
        let sess = wire
            .and_then(|w| serde_json::to_value(w).ok())
            .and_then(|v| session_from_wire(&v));
        let seed = sess
            .as_ref()
            .map(SentState::for_session)
            .unwrap_or_default();
        cx.update(|cx| {
            host(cx).sent.entry(ws.to_string()).or_insert(seed);
        });
        sess.map(|session| LoadedSession {
            session,
            syntax_maps_known: None,
        })
    };
    Ok(loaded.and_then(|l| {
        let known = l.syntax_maps_known;
        restorable_session(l.session).map(|session| LoadedSession {
            session,
            syntax_maps_known: known,
        })
    }))
}

/// `saveEditorSession`: debounced; only the newest pending session is written.
pub fn save_editor_session(workspace_id: &str, session: Session, cx: &mut App) {
    if workspace_id.is_empty() {
        return;
    }
    let h = host(cx);
    h.pending = Some(PendingSave {
        workspace_id: workspace_id.to_string(),
        session,
    });
    if h.save_timer.is_some() {
        return;
    }
    let task = cx.spawn(async move |cx| {
        cx.background_executor()
            .timer(Duration::from_millis(
                recur_editor::session::SAVE_DEBOUNCE_MS,
            ))
            .await;
        cx.update(|cx| {
            let h = host(cx);
            h.save_timer = None;
            if let Some(p) = h.pending.take() {
                flush_session(p, cx);
            }
        });
    });
    host(cx).save_timer = Some(task);
}

/// `flushEditorSessionNow`: write the pending session now, journaling it synchronously.
pub fn flush_editor_session_now(cx: &mut App) {
    let h = host(cx);
    h.save_timer = None;
    let Some(p) = h.pending.take() else { return };
    write_journal(&p.workspace_id, &p.session, cx);
    flush_session(p, cx);
}

fn flush_session(p: PendingSave, cx: &mut App) {
    let ws = p.workspace_id.clone();
    let gen_at_enqueue = host(cx).journal_gen.get(&ws).copied().unwrap_or(0);
    enqueue(
        &ws.clone(),
        Box::new(move |mut cx| {
            Box::pin(async move {
                migrate_legacy(&mut cx).await;
                let session = normalize_session(&p.session);
                let prev = cx.update(|cx| host(cx).sent.get(&ws).cloned());
                let (patch, next) = build_patch(&session, prev.as_ref());
                let ack = |cx: &mut App| {
                    if host(cx).journal_gen.get(&ws).copied().unwrap_or(0) == gen_at_enqueue {
                        drop_journal(&ws, cx);
                    }
                };
                let Some(patch) = patch else {
                    cx.update(ack);
                    return;
                };
                let Some(svc) = cx.update(|cx| services(cx)) else {
                    return;
                };
                if svc
                    .editor_session
                    .update_editor_session(&ws, &to_patch(patch))
                    .await
                    .is_ok()
                {
                    cx.update(|cx| {
                        host(cx).sent.insert(ws.clone(), next);
                        ack(cx);
                    });
                }
            })
        }),
        cx,
    );
}

fn cancel_pending_save(ws: &str, cx: &mut App) {
    let h = host(cx);
    if h.pending.as_ref().is_some_and(|p| p.workspace_id == ws) {
        h.pending = None;
        h.save_timer = None;
    }
}

/// `clearEditorSession`: tabs closed or discarded.
pub fn clear_editor_session(workspace_id: &str, cx: &mut App) {
    if workspace_id.is_empty() {
        return;
    }
    cancel_pending_save(workspace_id, cx);
    drop_journal(workspace_id, cx);
    let h = host(cx);
    let sent = h.sent.get(workspace_id).cloned();
    if sent.as_ref().is_some_and(SentState::is_empty) {
        return;
    }
    h.sent
        .insert(workspace_id.to_string(), SentState::default());
    let ws = workspace_id.to_string();
    enqueue(
        workspace_id,
        Box::new(move |cx| {
            Box::pin(async move {
                let Some(svc) = cx.update(|cx| services(cx)) else {
                    return;
                };
                let ok = svc.editor_session.clear_editor_session(&ws).await.is_ok();
                cx.update(|cx| {
                    let h = host(cx);
                    if ok {
                        h.sent.insert(ws.clone(), SentState::default());
                    } else if let Some(s) = sent {
                        h.sent.insert(ws.clone(), s);
                    } else {
                        h.sent.remove(&ws);
                    }
                });
            })
        }),
        cx,
    );
}

/// `dropEditorSession`: a deleted workspace's local state (the backend sweeps its file).
pub fn drop_editor_session(workspace_id: &str, cx: &mut App) {
    if workspace_id.is_empty() {
        return;
    }
    cancel_pending_save(workspace_id, cx);
    drop_journal(workspace_id, cx);
    let h = host(cx);
    h.sent.remove(workspace_id);
    h.queues.remove(workspace_id);
    let ws = workspace_id.to_string();
    with_storage(cx, |s| drop_explorer_scope(s, &ws));
}

/// Routes the host's RPCs to `services` instead of the app's (tests).
pub fn set_services(services: Services, cx: &mut App) {
    host(cx).services = Some(services);
}

fn scope_key(project_id: &str, workspace_id: &str) -> String {
    let p = if project_id.is_empty() {
        "_"
    } else {
        project_id
    };
    let w = if workspace_id.is_empty() {
        "_"
    } else {
        workspace_id
    };
    format!("{p}::{w}")
}

/// `loadExplorerScope`: `None` when never chosen for this root.
pub fn load_explorer_scope(
    storage: &LocalStorage,
    project_id: &str,
    workspace_id: &str,
) -> Option<bool> {
    let raw = storage.get(SCOPE_KEY)?;
    let all: Value = serde_json::from_str(raw).ok()?;
    all.get(scope_key(project_id, workspace_id))?.as_bool()
}

/// `saveExplorerScope`.
pub fn save_explorer_scope(
    storage: &mut LocalStorage,
    project_id: &str,
    workspace_id: &str,
    browse_all: bool,
) {
    let mut all: Map<String, Value> = storage
        .get(SCOPE_KEY)
        .and_then(|raw| serde_json::from_str(raw).ok())
        .unwrap_or_default();
    all.insert(scope_key(project_id, workspace_id), Value::Bool(browse_all));
    storage.set(SCOPE_KEY, Value::Object(all).to_string());
}

fn drop_explorer_scope(storage: &mut LocalStorage, workspace_id: &str) {
    let Some(mut all) = storage
        .get(SCOPE_KEY)
        .and_then(|raw| serde_json::from_str::<Map<String, Value>>(raw).ok())
    else {
        return;
    };
    let before = all.len();
    all.retain(|k, _| workspace_id_from_storage_key(k) != workspace_id);
    if all.len() == before {
        return;
    }
    if all.is_empty() {
        storage.remove(SCOPE_KEY);
    } else {
        storage.set(SCOPE_KEY, Value::Object(all).to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explorer_scope_round_trips_and_drops_per_workspace() {
        let mut s = LocalStorage::in_memory();
        assert_eq!(load_explorer_scope(&s, "p", "w"), None);
        save_explorer_scope(&mut s, "p", "w", true);
        save_explorer_scope(&mut s, "", "", false);
        assert_eq!(load_explorer_scope(&s, "p", "w"), Some(true));
        assert_eq!(load_explorer_scope(&s, "", ""), Some(false));
        drop_explorer_scope(&mut s, "w");
        assert_eq!(load_explorer_scope(&s, "p", "w"), None);
        assert_eq!(load_explorer_scope(&s, "", ""), Some(false));
    }

    #[test]
    fn session_objects_round_trip() {
        let mut bufs = BTreeMap::new();
        bufs.insert(
            "a.go".to_string(),
            StashedBuffer {
                draft: "D".into(),
                saved: Some("S".into()),
                base_hash: None,
            },
        );
        let s = Session {
            open_files: vec!["a.go".into()],
            selected_path: "a.go".into(),
            stashed_buffers: Some(bufs),
            left_open: Some(false),
            ..Default::default()
        };
        let back = session_from_object(&session_to_object(&s, Some(false))).unwrap();
        assert_eq!(back.session, s);
        assert_eq!(back.syntax_maps_known, Some(false));
    }
}
