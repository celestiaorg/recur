//! The resident Quick Open / @-mention path index per workspace, under one
//! byte budget, built with ripgrep (or a walk) and spliced from file-system
//! events.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use recur_domain::FileSearchResult;
use tokio::runtime::Handle;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::filesearch::{
    ExcludeSet, compile_excludes, exclude_to_rg_globs, excluded, normalize_exclude_patterns,
    rank_hits,
};
use crate::gopath;
use crate::root::skip_file;
use crate::{files, rg};

/// The default resident budget for all indexes together: a few medium trees
/// or one large monorepo. Cold workspaces are evicted least recently used
/// first.
pub const DEFAULT_MAX_INDEX_BYTES: i64 = 64 << 20;

/// Approximate per-path cost beyond the strings themselves (headers, map
/// entry, slots). Tuned for budgeting, not accounting.
const ENTRY_OVERHEAD: i64 = 64;

/// Coalesces bus publishes after file-system splices.
const PUBLISH_DEBOUNCE: Duration = Duration::from_millis(300);

/// A consistent view of one index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot {
    pub root: String,
    pub paths: Vec<String>,
    pub basenames: Vec<String>,
    pub generation: u64,
    pub ready: bool,
    pub building: bool,
    pub dirty: bool,
    pub bytes: i64,
}

/// Resolves the ripgrep binary inside a build (it may download).
pub type ResolveRg = Arc<dyn Fn(CancellationToken) -> BoxFuture<String> + Send + Sync>;
/// Receives build progress and the final ready snapshot.
pub type OnUpdate = Arc<dyn Fn(Snapshot) + Send + Sync>;
/// Publishes a workspace's index status (the `workspace.file_index` event).
pub type Publish = Arc<dyn Fn(&str, Snapshot) + Send + Sync>;
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

struct IndexState {
    /// Parallel to `basenames`.
    paths: Vec<String>,
    basenames: Vec<String>,
    by_path: HashMap<String, usize>,
    generation: u64,
    building: bool,
    /// At least one full rebuild finished.
    ready: bool,
    /// Needs a full rebuild (missed file-system events).
    dirty: bool,
    /// Bumped by `mark_dirty` / `apply_fs_change`, so a rebuild that raced
    /// them keeps `dirty` instead of trusting a stale walk.
    dirty_seq: u64,
    bytes: i64,
    last_used: Instant,
    cancel: Option<CancellationToken>,
}

/// One workspace's path list. Readers take snapshots; mutations lock.
pub struct FileIndex {
    root: String,
    st: RwLock<IndexState>,
}

impl FileIndex {
    /// An empty index for `root`.
    pub fn new(root: &str) -> Self {
        Self {
            root: root.to_string(),
            st: RwLock::new(IndexState {
                paths: Vec::new(),
                basenames: Vec::new(),
                by_path: HashMap::new(),
                generation: 0,
                building: false,
                ready: false,
                dirty: false,
                dirty_seq: 0,
                bytes: 0,
                last_used: Instant::now(),
                cancel: None,
            }),
        }
    }

    /// The index with copies of the path lists.
    pub fn snapshot(&self) -> Snapshot {
        let st = self.st.read();
        Snapshot {
            paths: st.paths.clone(),
            basenames: st.basenames.clone(),
            ..self.status_locked(&st)
        }
    }

    /// Readiness without copying the path lists (for bus publishes).
    pub fn status_snapshot(&self) -> Snapshot {
        self.status_locked(&self.st.read())
    }

    fn status_locked(&self, st: &IndexState) -> Snapshot {
        Snapshot {
            root: self.root.clone(),
            generation: st.generation,
            ready: st.ready,
            building: st.building,
            dirty: st.dirty,
            bytes: st.bytes,
            ..Default::default()
        }
    }

    /// Swaps in a complete path list and bumps the generation. Clears
    /// `dirty` unless a file-system mutation races the swap.
    pub fn replace_all(&self, paths: Vec<String>) {
        let start_seq = {
            let mut st = self.st.write();
            st.dirty = false;
            st.dirty_seq
        };
        self.finish_rebuild(paths, start_seq);
    }

    /// Installs a walk result. `dirty_seq` bumps after `start_seq` mean the
    /// walk may have missed those updates, so `dirty` stays set; a dirty bit
    /// set before the walk (why it ran) is cleared.
    fn finish_rebuild(&self, mut paths: Vec<String>, start_seq: u64) {
        paths.sort();
        let basenames: Vec<String> = paths.iter().map(|p| gopath::base(p)).collect();
        let by_path = paths
            .iter()
            .enumerate()
            .map(|(i, p)| (p.clone(), i))
            .collect();
        let mut st = self.st.write();
        let keep_dirty = st.dirty_seq != start_seq;
        st.bytes = estimate_index_bytes(&paths, &basenames);
        st.paths = paths;
        st.basenames = basenames;
        st.by_path = by_path;
        st.generation += 1;
        st.ready = true;
        st.building = false;
        st.dirty = keep_dirty;
        st.last_used = Instant::now();
    }

    /// Notes that the index may be incomplete (dropped file-system events).
    pub fn mark_dirty(&self) {
        let mut st = self.st.write();
        st.dirty = true;
        st.dirty_seq += 1;
    }

    /// Inserts or refreshes a repo-relative path.
    pub fn upsert(&self, rel: &str) {
        let rel = normalize_rel(rel);
        if rel.is_empty() || should_skip_indexed(&rel) {
            return;
        }
        let base = gopath::base(&rel);
        let mut st = self.st.write();
        if let Some(&i) = st.by_path.get(&rel) {
            let delta = base.len() as i64 - st.basenames[i].len() as i64;
            st.basenames[i] = base;
            st.bytes += delta;
        } else {
            st.bytes += (rel.len() + base.len()) as i64 + ENTRY_OVERHEAD;
            let i = st.paths.len();
            st.paths.push(rel.clone());
            st.basenames.push(base);
            st.by_path.insert(rel, i);
        }
        st.generation += 1;
        st.last_used = Instant::now();
    }

    /// Deletes a repo-relative path if present.
    pub fn remove(&self, rel: &str) {
        let rel = normalize_rel(rel);
        if rel.is_empty() {
            return;
        }
        let mut st = self.st.write();
        let Some(i) = st.by_path.remove(&rel) else {
            return;
        };
        st.bytes = (st.bytes
            - (st.paths[i].len() + st.basenames[i].len()) as i64
            - ENTRY_OVERHEAD)
            .max(0);
        st.paths.swap_remove(i);
        st.basenames.swap_remove(i);
        if let Some(moved) = st.paths.get(i).cloned() {
            st.by_path.insert(moved, i);
        }
        st.generation += 1;
        st.last_used = Instant::now();
    }

    /// Splices a create or delete of `rel` by stating it under `root`.
    pub fn apply_fs_change(&self, root: &str, rel: &str) {
        let rel = normalize_rel(rel);
        if rel.is_empty() || should_skip_indexed(&rel) {
            return;
        }
        match std::fs::symlink_metadata(gopath::join(&[root, &rel])) {
            Err(_) => self.remove(&rel),
            Ok(m) if m.is_dir() => return,
            Ok(_) if skip_file(&gopath::base(&rel)) => self.remove(&rel),
            Ok(_) => self.upsert(&rel),
        }
        self.st.write().dirty_seq += 1;
    }

    /// Lists files under the root with ripgrep (walk fallback) on a task.
    /// `resolve_rg` runs inside the task (it may download) so callers are not
    /// blocked. `on_update` gets partial batches when `stream_partials`.
    /// `force` cancels an in-flight build; otherwise a build in flight makes
    /// this a no-op. `extra_globs` / `user_skip` apply `files.exclude`.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn rebuild(
        self: &Arc<Self>,
        rt: &Handle,
        resolve_rg: Option<ResolveRg>,
        on_update: Option<OnUpdate>,
        stream_partials: bool,
        force: bool,
        extra_globs: Vec<String>,
        user_skip: ExcludeSet,
    ) {
        let (token, start_seq) = {
            let mut st = self.st.write();
            if st.building && !force {
                return;
            }
            if let Some(c) = st.cancel.take() {
                c.cancel();
            }
            let token = CancellationToken::new();
            st.cancel = Some(token.clone());
            st.building = true;
            st.last_used = Instant::now();
            (token, st.dirty_seq)
        };
        let idx = Arc::clone(self);
        rt.spawn(async move {
            let _done = token.clone().drop_guard();
            let rg_path = match &resolve_rg {
                Some(resolve) => resolve(token.clone()).await,
                None => String::new(),
            };
            let partial_idx = Arc::clone(&idx);
            let partial_update = on_update.clone();
            let partial_skip = user_skip.clone();
            let mut batch = |partial: Vec<String>| {
                let Some(update) = partial_update.as_ref().filter(|_| stream_partials) else {
                    return;
                };
                // No path copies here: every 500-file batch would allocate a
                // multi-MB snapshot. The hub snapshots once at flush.
                let snap = {
                    let mut st = partial_idx.st.write();
                    merge_partial(&mut st, partial, &partial_skip);
                    Snapshot {
                        building: true,
                        ..partial_idx.status_locked(&st)
                    }
                };
                update(snap);
            };
            let (mut paths, err) = list_all_files(
                &token,
                &rg_path,
                &idx.root,
                &extra_globs,
                &user_skip,
                Some(&mut batch),
            )
            .await;
            if token.is_cancelled() {
                idx.st.write().building = false;
                return;
            }
            if err && paths.is_empty() {
                paths = walk_files(&token, &idx.root, &user_skip).await.0;
            }
            idx.finish_rebuild(paths, start_seq);
            if let Some(update) = &on_update {
                update(idx.status_snapshot());
            }
        });
    }
}

/// Appends unseen paths from a streaming batch.
fn merge_partial(st: &mut IndexState, partial: Vec<String>, user_skip: &ExcludeSet) {
    for p in partial {
        let p = normalize_rel(&p);
        if p.is_empty() || !indexable_path(&p, user_skip) || st.by_path.contains_key(&p) {
            continue;
        }
        let base = gopath::base(&p);
        st.bytes += (p.len() + base.len()) as i64 + ENTRY_OVERHEAD;
        let i = st.paths.len();
        st.paths.push(p.clone());
        st.basenames.push(base);
        st.by_path.insert(p, i);
    }
    st.generation += 1;
    st.last_used = Instant::now();
}

fn estimate_index_bytes(paths: &[String], basenames: &[String]) -> i64 {
    paths
        .iter()
        .enumerate()
        .map(|(i, p)| {
            p.len() as i64 + basenames.get(i).map_or(0, |b| b.len() as i64) + ENTRY_OVERHEAD
        })
        .sum()
}

/// `rg --files` when `rg_path` is set, else (or when rg yields nothing and
/// fails) the walk. `batch` gets progressive 500-path chunks. The flag is
/// whether the listing failed.
async fn list_all_files(
    cancel: &CancellationToken,
    rg_path: &str,
    root: &str,
    extra_globs: &[String],
    user_skip: &ExcludeSet,
    mut batch: Option<&mut (dyn FnMut(Vec<String>) + Send)>,
) -> (Vec<String>, bool) {
    if !rg_path.is_empty() {
        let mut out = Vec::new();
        let mut chunk = Vec::new();
        let res = rg::files(
            cancel,
            Path::new(rg_path),
            Path::new(root),
            extra_globs,
            |rel| {
                if !indexable_path(&rel, user_skip) {
                    return Ok(());
                }
                if let Some(b) = batch.as_mut() {
                    chunk.push(rel.clone());
                    if chunk.len() >= 500 {
                        b(std::mem::take(&mut chunk));
                    }
                }
                out.push(rel);
                Ok(())
            },
        )
        .await;
        if let Some(b) = batch.as_mut()
            && !chunk.is_empty()
        {
            b(chunk);
        }
        if res.is_ok() || !out.is_empty() {
            return (out, res.is_err());
        }
    }
    walk_files(cancel, root, user_skip).await
}

/// The walk fallback (tests, rg missing), off the async workers.
async fn walk_files(cancel: &CancellationToken, root: &str, user_skip: &ExcludeSet) -> (Vec<String>, bool) {
    let (cancel, root) = (cancel.clone(), root.to_string());
    let listed = tokio::task::spawn_blocking(move || files::list_files(&cancel, Path::new(&root)))
        .await;
    match listed {
        Ok(Ok(paths)) => (filter_for_index(paths, user_skip), false),
        _ => (Vec::new(), true),
    }
}

pub(crate) fn filter_for_index(paths: Vec<String>, user_skip: &ExcludeSet) -> Vec<String> {
    paths
        .into_iter()
        .filter(|p| indexable_path(p, user_skip))
        .collect()
}

fn indexable_path(rel: &str, user_skip: &ExcludeSet) -> bool {
    !should_skip_indexed(rel) && !excluded(rel, user_skip)
}

fn normalize_rel(rel: &str) -> String {
    let rel = rel.trim();
    let rel = rel.strip_prefix("./").unwrap_or(rel);
    if rel.is_empty() || rel == "." || rel.starts_with("../") {
        return String::new();
    }
    rel.to_string()
}

/// Paths rg's default globs drop, plus binary/noise extensions. Hidden dirs
/// like `.github` stay (`rg --files -uu`).
fn should_skip_indexed(rel: &str) -> bool {
    let wrapped = format!("/{rel}/");
    if wrapped.contains("/.git/") || wrapped.contains("/node_modules/") {
        return true;
    }
    let base = gopath::base(wrapped.trim_end_matches('/'));
    base.is_empty() || base == "." || skip_file(&base)
}

struct HubState {
    /// Workspace id → index.
    indexes: HashMap<String, Arc<FileIndex>>,
    rg_path: String,
    data_dir: String,
    max_bytes: i64,
    /// The watched workspace; never evicted.
    pinned_id: String,
    exclude: Vec<String>,
    exclude_set: ExcludeSet,
    /// Negated exclude patterns for `rg --glob`.
    rg_globs: Vec<String>,
    publish: Option<Publish>,
    pending_pub: HashSet<String>,
    publish_timer: Option<JoinHandle<()>>,
}

struct HubInner {
    st: Mutex<HubState>,
    rt: Handle,
}

/// Per-workspace file indexes under one resident byte budget. Cold
/// workspaces build on demand; the watched one is pinned against eviction.
/// Full rebuilds run on first [`IndexHub::ensure`] and when dirty, not on
/// every file-system tick. `files.exclude` ([`IndexHub::set_exclude`]) is
/// baked into every listing, so Quick Open and @-mentions search the
/// explorer-visible corpus with no per-keystroke filter.
#[derive(Clone)]
pub struct IndexHub(Arc<HubInner>);

impl IndexHub {
    /// A hub with [`DEFAULT_MAX_INDEX_BYTES`]. Builds and the publish timer run
    /// on the current tokio runtime, so call this inside one.
    pub fn new(data_dir: &str, publish: Option<Publish>) -> Self {
        Self(Arc::new(HubInner {
            st: Mutex::new(HubState {
                indexes: HashMap::new(),
                rg_path: String::new(),
                data_dir: data_dir.to_string(),
                max_bytes: DEFAULT_MAX_INDEX_BYTES,
                pinned_id: String::new(),
                exclude: Vec::new(),
                exclude_set: ExcludeSet::default(),
                rg_globs: Vec::new(),
                publish,
                pending_pub: HashSet::new(),
                publish_timer: None,
            }),
            rt: Handle::current(),
        }))
    }

    /// Installs VS Code-style `files.exclude` patterns for every index
    /// (`.git` / `node_modules` are always excluded). A changed set rebuilds
    /// the warm indexes so search matches the explorer.
    pub fn set_exclude(&self, patterns: &[String]) {
        let next = normalize_exclude_patterns(patterns);
        let warm: Vec<(String, Arc<FileIndex>)> = {
            let mut st = self.0.st.lock();
            if st.exclude == next {
                return;
            }
            st.exclude_set = compile_excludes(&next);
            st.rg_globs = exclude_to_rg_globs(&next);
            st.exclude = next;
            st.indexes
                .iter()
                .map(|(id, idx)| (id.clone(), Arc::clone(idx)))
                .collect()
        };
        for (id, idx) in warm {
            idx.mark_dirty();
            self.rebuild(&id, &idx, true);
        }
    }

    /// Overrides the resident budget; `<= 0` restores the default.
    pub fn set_max_bytes(&self, n: i64) {
        let mut st = self.0.st.lock();
        st.max_bytes = if n <= 0 { DEFAULT_MAX_INDEX_BYTES } else { n };
        evict_locked(&mut st);
    }

    /// Overrides the resolved ripgrep binary (tests).
    pub fn set_rg_path(&self, p: &str) {
        self.0.st.lock().rg_path = p.to_string();
    }

    /// Marks the watched workspace (`""` clears); pinned indexes are never
    /// evicted to make room for colder trees.
    pub fn set_pinned(&self, workspace_id: &str) {
        let mut st = self.0.st.lock();
        st.pinned_id = workspace_id.to_string();
        if !workspace_id.is_empty()
            && let Some(idx) = st.indexes.get(workspace_id)
        {
            idx.st.write().last_used = Instant::now();
        }
        evict_locked(&mut st);
    }

    /// The cached ripgrep, resolving it on first use. With a data dir,
    /// [`rg::ensure`] runs so a stale managed sidecar is upgraded (a no-network
    /// hit when current); it may download, so it runs inside builds.
    async fn ensure_rg(&self, cancel: CancellationToken) -> String {
        let data_dir = {
            let st = self.0.st.lock();
            if !st.rg_path.is_empty() {
                return st.rg_path.clone();
            }
            st.data_dir.clone()
        };
        let p = if data_dir.is_empty() {
            rg::locate(Path::new(&data_dir)).ok()
        } else {
            tokio::select! {
                r = rg::ensure(Path::new(&data_dir)) => r.ok(),
                () = cancel.cancelled() => None,
            }
        };
        let p = p.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let mut st = self.0.st.lock();
        if st.rg_path.is_empty() && !p.is_empty() {
            st.rg_path = p.clone();
        }
        if st.rg_path.is_empty() { p } else { st.rg_path.clone() }
    }

    /// Starts (or returns) the index for a workspace at `root`, rebuilding
    /// when missing, incomplete or dirty. An in-flight build is coalesced,
    /// not restarted. Cold workspaces are untouched until this is called.
    pub fn ensure(&self, workspace_id: &str, root: &str) -> Snapshot {
        let idx = self.get_or_create(workspace_id, root);
        self.touch(workspace_id);
        let snap = idx.snapshot();
        if snap.root == root && (snap.building || (snap.ready && !snap.dirty)) {
            return snap;
        }
        self.rebuild(workspace_id, &idx, false);
        idx.snapshot()
    }

    /// Forces a full rebuild (explicit refresh), cancelling one in flight.
    pub fn refresh(&self, workspace_id: &str, root: &str) -> Snapshot {
        let idx = self.get_or_create(workspace_id, root);
        self.touch(workspace_id);
        self.rebuild(workspace_id, &idx, true);
        idx.snapshot()
    }

    /// Rebuilds only a warm index that is dirty or incomplete. Cold
    /// workspaces stay unloaded.
    pub fn refresh_if_dirty(&self, workspace_id: &str, root: &str) -> Snapshot {
        let Some(mut idx) = self.0.st.lock().indexes.get(workspace_id).cloned() else {
            return Snapshot {
                root: root.to_string(),
                ..Default::default()
            };
        };
        if idx.root != root {
            idx = self.get_or_create(workspace_id, root);
        }
        let snap = idx.snapshot();
        self.touch(workspace_id);
        if snap.building || (snap.ready && !snap.dirty) {
            return snap;
        }
        self.rebuild(workspace_id, &idx, false);
        idx.snapshot()
    }

    /// The current paths (starts a build when empty or dirty).
    pub fn paths(&self, workspace_id: &str, root: &str) -> Vec<String> {
        self.ensure(workspace_id, root).paths
    }

    /// Ensures the index, then ranks its paths for `query` under the read
    /// lock (no corpus copy on the keystroke path).
    pub fn search(
        &self,
        cancel: &CancellationToken,
        workspace_id: &str,
        root: &str,
        query: &str,
        limit: usize,
        files_only: bool,
    ) -> FileSearchResult {
        let idx = self.get_or_create(workspace_id, root);
        self.touch(workspace_id);
        let (building, ready, dirty) = {
            let st = idx.st.read();
            (st.building, st.ready, st.dirty)
        };
        let root_ok = idx.root == root;
        if !(root_ok && building) && !(ready && !dirty && root_ok) {
            self.rebuild(workspace_id, &idx, false);
        }
        let st = idx.st.read();
        FileSearchResult {
            hits: Some(rank_hits(
                Some(cancel),
                &st.paths,
                &st.basenames,
                query,
                limit,
                files_only,
            )),
            building: st.building,
            ready: st.ready,
        }
    }

    /// Splices path-level file-system updates. A rescan or an empty path
    /// list marks the index dirty instead of walking (focus or the next
    /// ensure rebuilds). A no-op for cold workspaces. Publishes are debounced
    /// so bursts do not flood the bridge.
    pub fn apply_changes(&self, workspace_id: &str, root: &str, rels: &[String], rescan: bool) {
        let (idx, skip) = {
            let st = self.0.st.lock();
            let Some(idx) = st.indexes.get(workspace_id).cloned() else {
                return;
            };
            (idx, st.exclude_set.clone())
        };
        let idx = if idx.root != root {
            self.get_or_create(workspace_id, root)
        } else {
            idx
        };
        if rescan || rels.is_empty() {
            idx.mark_dirty();
            self.touch(workspace_id);
            self.schedule_publish(workspace_id);
            return;
        }
        for rel in rels {
            let rel = normalize_rel(rel);
            if rel.is_empty() {
                continue;
            }
            if !indexable_path(&rel, &skip) {
                idx.remove(&rel);
                continue;
            }
            idx.apply_fs_change(root, &rel);
        }
        self.touch(workspace_id);
        evict_locked(&mut self.0.st.lock());
        self.schedule_publish(workspace_id);
    }

    /// Marks a warm index dirty without rebuilding. A no-op when cold.
    pub fn mark_dirty(&self, workspace_id: &str) {
        let idx = self.0.st.lock().indexes.get(workspace_id).cloned();
        if let Some(idx) = idx {
            idx.mark_dirty();
        }
    }

    /// Drops a workspace's index (closed, or its root changed).
    pub fn drop_index(&self, workspace_id: &str) {
        drop_locked(&mut self.0.st.lock(), workspace_id);
    }

    /// The approximate resident size of all warm indexes.
    pub fn total_bytes(&self) -> i64 {
        total_bytes_locked(&self.0.st.lock())
    }

    /// How many workspace indexes are warm.
    pub fn len(&self) -> usize {
        self.0.st.lock().indexes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn touch(&self, workspace_id: &str) {
        let mut st = self.0.st.lock();
        if let Some(idx) = st.indexes.get(workspace_id) {
            idx.st.write().last_used = Instant::now();
        }
        evict_locked(&mut st);
    }

    fn get_or_create(&self, workspace_id: &str, root: &str) -> Arc<FileIndex> {
        let mut st = self.0.st.lock();
        if let Some(idx) = st.indexes.get(workspace_id)
            && idx.root == root
        {
            return Arc::clone(idx);
        }
        drop_locked(&mut st, workspace_id);
        let idx = Arc::new(FileIndex::new(root));
        st.indexes
            .insert(workspace_id.to_string(), Arc::clone(&idx));
        evict_locked(&mut st);
        idx
    }

    fn rebuild(&self, workspace_id: &str, idx: &Arc<FileIndex>, force: bool) {
        let (globs, skip) = {
            let st = self.0.st.lock();
            (st.rg_globs.clone(), st.exclude_set.clone())
        };
        let hub = self.clone();
        let resolve: ResolveRg = Arc::new(move |cancel| {
            let hub = hub.clone();
            Box::pin(async move { hub.ensure_rg(cancel).await })
        });
        let hub = self.clone();
        let ws = workspace_id.to_string();
        let on_update: OnUpdate = Arc::new(move |snap: Snapshot| {
            evict_locked(&mut hub.0.st.lock());
            // Partials are debounced; the final ready snapshot goes out now.
            if snap.ready && !snap.building {
                hub.publish_now(&ws, snap);
            } else {
                hub.schedule_publish(&ws);
            }
        });
        idx.rebuild(
            &self.0.rt,
            Some(resolve),
            Some(on_update),
            true,
            force,
            globs,
            skip,
        );
    }

    /// Publishes `snap` now and flushes the other pending debounced publishes
    /// (stopping the shared timer must not orphan them).
    fn publish_now(&self, workspace_id: &str, snap: Snapshot) {
        let (publish, others) = {
            let mut st = self.0.st.lock();
            if let Some(t) = st.publish_timer.take() {
                t.abort();
            }
            st.pending_pub.remove(workspace_id);
            let pending = std::mem::take(&mut st.pending_pub);
            let others: Vec<(String, Arc<FileIndex>)> = pending
                .into_iter()
                .filter_map(|id| st.indexes.get(&id).map(|idx| (id, Arc::clone(idx))))
                .collect();
            (st.publish.clone(), others)
        };
        let Some(publish) = publish else {
            return;
        };
        publish(workspace_id, snap);
        for (id, idx) in others {
            publish(&id, idx.status_snapshot());
        }
    }

    fn schedule_publish(&self, workspace_id: &str) {
        let mut st = self.0.st.lock();
        if st.publish.is_none() {
            return;
        }
        st.pending_pub.insert(workspace_id.to_string());
        if let Some(t) = st.publish_timer.take() {
            t.abort();
        }
        let hub = self.clone();
        st.publish_timer = Some(self.0.rt.spawn(async move {
            tokio::time::sleep(PUBLISH_DEBOUNCE).await;
            hub.flush_publish();
        }));
    }

    fn flush_publish(&self) {
        let (publish, out) = {
            let mut st = self.0.st.lock();
            st.publish_timer = None;
            let pending = std::mem::take(&mut st.pending_pub);
            let out: Vec<(String, Arc<FileIndex>)> = pending
                .into_iter()
                .filter_map(|id| st.indexes.get(&id).map(|idx| (id, Arc::clone(idx))))
                .collect();
            (st.publish.clone(), out)
        };
        let Some(publish) = publish else {
            return;
        };
        for (id, idx) in out {
            publish(&id, idx.status_snapshot());
        }
    }

    #[cfg(test)]
    fn set_publish(&self, publish: Option<Publish>) {
        self.0.st.lock().publish = publish;
    }

    #[cfg(test)]
    fn index(&self, workspace_id: &str) -> Option<Arc<FileIndex>> {
        self.0.st.lock().indexes.get(workspace_id).cloned()
    }

    #[cfg(test)]
    fn insert_index(&self, workspace_id: &str, idx: Arc<FileIndex>) {
        self.0.st.lock().indexes.insert(workspace_id.into(), idx);
    }
}

fn drop_locked(st: &mut HubState, workspace_id: &str) {
    if let Some(idx) = st.indexes.remove(workspace_id)
        && let Some(c) = idx.st.read().cancel.as_ref()
    {
        c.cancel();
    }
}

fn total_bytes_locked(st: &HubState) -> i64 {
    st.indexes.values().map(|idx| idx.st.read().bytes).sum()
}

/// Drops least recently used unpinned indexes until under the budget. A
/// single oversized pinned or active index is kept.
fn evict_locked(st: &mut HubState) {
    if st.max_bytes <= 0 {
        return;
    }
    while total_bytes_locked(st) > st.max_bytes && st.indexes.len() > 1 {
        let victim = st
            .indexes
            .iter()
            .filter(|(id, _)| **id != st.pinned_id)
            .min_by_key(|(_, idx)| idx.st.read().last_used)
            .map(|(id, _)| id.clone());
        let Some(victim) = victim else {
            return;
        };
        drop_locked(st, &victim);
    }
}

#[cfg(test)]
#[path = "fileindex_tests.rs"]
mod tests;
