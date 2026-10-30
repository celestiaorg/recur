//! The settled-markdown cache and off-thread rendering for streaming prefixes.
//!
//! A worker thread runs comrak and highlighting; a coordinator thread
//! owns the queue, the 2 s watchdog and sanitizing, so nothing but layout touches the UI
//! thread. Settled renders stay synchronous ([`render_markdown_cached`]): tab-swap scroll
//! restore needs real heights on the first frame.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use futures::channel::oneshot;

use super::parse::{ParseOptions, parse_markdown};
use super::sanitize_markdown;
use crate::js::utf16_len;
use crate::render::doc::Doc;

/// Settled HTML cache budget. Byte-budgeted like editor undo history (20 MiB): generous for
/// chat, findings and a few file previews.
pub const CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;
/// Per-entry cap so one giant preview cannot own the cache.
pub const CACHE_ENTRY_MAX_BYTES: usize = 2 * 1024 * 1024;
/// Drop a wedged worker slot so later blocks can still render.
const JOB_MS: Duration = Duration::from_millis(2000);

/// Why an async render did not produce HTML.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RenderError {
    /// `AbortError`: a newer call with the same key superseded this one.
    Aborted,
    /// `TimeoutError`: the worker did not answer within 2 s.
    TimedOut,
    /// The render itself failed.
    Failed(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Aborted => f.write_str("markdown render superseded"),
            RenderError::TimedOut => f.write_str("markdown worker timed out"),
            RenderError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for RenderError {}

type Reply = oneshot::Sender<Result<Arc<str>, RenderError>>;

struct Job {
    id: u64,
    src: String,
    key: Option<String>,
    reply: Reply,
}

struct Cached<V> {
    value: V,
    bytes: usize,
    stamp: u64,
}

/// A byte-budgeted LRU: the oldest entries go once the total passes the budget (the newest
/// always stays).
pub(crate) struct Cache<V> {
    entries: HashMap<String, Cached<V>>,
    order: BTreeMap<u64, String>,
    stamp: u64,
    total: usize,
}

impl<V> Default for Cache<V> {
    fn default() -> Self {
        Cache {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            stamp: 0,
            total: 0,
        }
    }
}

impl<V: Clone> Cache<V> {
    fn drop_entry(&mut self, key: &str) {
        if let Some(c) = self.entries.remove(key) {
            self.order.remove(&c.stamp);
            self.total = self.total.saturating_sub(c.bytes);
        }
    }

    fn evict(&mut self, max: usize) {
        while self.total > max && self.entries.len() > 1 {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(c) = self.entries.remove(&oldest) {
                self.total = self.total.saturating_sub(c.bytes);
            }
        }
    }

    /// LRU-promoting read.
    pub(crate) fn get(&mut self, key: &str) -> Option<V> {
        let c = self.entries.get_mut(key)?;
        self.order.remove(&c.stamp);
        self.stamp += 1;
        c.stamp = self.stamp;
        self.order.insert(self.stamp, key.to_string());
        Some(c.value.clone())
    }

    pub(crate) fn contains(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub(crate) fn insert(&mut self, key: &str, value: V, bytes: usize, max: usize) {
        self.drop_entry(key);
        self.stamp += 1;
        self.order.insert(self.stamp, key.to_string());
        self.entries.insert(
            key.to_string(),
            Cached {
                value,
                bytes,
                stamp: self.stamp,
            },
        );
        self.total += bytes;
        self.evict(max);
    }

    /// `(entries, bytes)`.
    pub(crate) fn stats(&self) -> (usize, usize) {
        (self.entries.len(), self.total)
    }

    pub(crate) fn clear(&mut self) {
        *self = Cache::default();
    }
}

impl Cache<Arc<str>> {
    fn set(&mut self, key: &str, html: Arc<str>, max: usize) {
        // JS strings are UTF-16: this tracks heap, not UTF-8 on the wire.
        let bytes = (utf16_len(key) + utf16_len(&html)) * 2;
        if bytes > CACHE_ENTRY_MAX_BYTES {
            return;
        }
        self.insert(key, html, bytes, max);
    }
}

struct State {
    cache: Cache<Arc<str>>,
    max_bytes: usize,
    seq: u64,
    waiting: VecDeque<Job>,
    waiting_by_key: HashMap<String, u64>,
}

/// The markdown render host. [`MarkdownHost::global`] is the app's; tests make their own.
pub struct MarkdownHost {
    state: Arc<Mutex<State>>,
    wake: Mutex<Option<Sender<()>>>,
}

fn lock(m: &Mutex<State>) -> MutexGuard<'_, State> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn cache_key(src: &str, opts: ParseOptions) -> String {
    if opts.demote_images {
        format!("\u{1e}demoteimg\n{src}")
    } else {
        src.to_string()
    }
}

/// `renderMarkdown` with panics contained, as `runSync`'s try/catch does.
fn render_sync(src: &str) -> Result<String, RenderError> {
    match catch_unwind(AssertUnwindSafe(|| {
        parse_markdown(src, ParseOptions::default())
    })) {
        Ok(html) => Ok(sanitize_markdown(&html)),
        Err(_) => Err(RenderError::Failed("markdown render panicked".into())),
    }
}

impl Default for MarkdownHost {
    fn default() -> Self {
        MarkdownHost::new()
    }
}

impl MarkdownHost {
    pub fn new() -> MarkdownHost {
        MarkdownHost {
            state: Arc::new(Mutex::new(State {
                cache: Cache::default(),
                max_bytes: CACHE_MAX_BYTES,
                seq: 0,
                waiting: VecDeque::new(),
                waiting_by_key: HashMap::new(),
            })),
            wake: Mutex::new(None),
        }
    }

    pub fn global() -> &'static MarkdownHost {
        static HOST: OnceLock<MarkdownHost> = OnceLock::new();
        HOST.get_or_init(MarkdownHost::new)
    }

    /// `peekMarkdownCache`: LRU-promoting read; settled blocks hit this on every mount.
    pub fn peek(&self, src: &str) -> Option<Arc<str>> {
        lock(&self.state).cache.get(src)
    }

    /// `rememberMarkdown`: store a completed document (not a streaming prefix).
    pub fn remember(&self, src: &str, html: &str) {
        if src.is_empty() {
            return;
        }
        let mut s = lock(&self.state);
        let max = s.max_bytes;
        s.cache.set(src, html.into(), max);
    }

    /// `renderMarkdownCached`: cache hit, or parse, sanitize and store.
    pub fn render_cached(&self, src: &str, opts: ParseOptions) -> Arc<str> {
        if src.is_empty() {
            return "".into();
        }
        let key = cache_key(src, opts);
        if let Some(hit) = lock(&self.state).cache.get(&key) {
            return hit;
        }
        let html: Arc<str> = match catch_unwind(AssertUnwindSafe(|| parse_markdown(src, opts))) {
            Ok(raw) => sanitize_markdown(&raw).into(),
            Err(_) => super::escape_text(src).replace('\n', "<br>").into(),
        };
        let mut s = lock(&self.state);
        let max = s.max_bytes;
        s.cache.set(&key, html.clone(), max);
        html
    }

    pub fn clear(&self) {
        let mut s = lock(&self.state);
        s.cache = Cache::default();
        s.max_bytes = CACHE_MAX_BYTES;
    }

    /// Test helper: shrink the byte budget so eviction is cheap to assert.
    pub fn set_max_bytes_for_tests(&self, n: usize) {
        let mut s = lock(&self.state);
        s.max_bytes = n;
        s.cache.evict(n);
    }

    /// `(entries, bytes)`.
    pub fn stats_for_tests(&self) -> (usize, usize) {
        let s = lock(&self.state);
        (s.cache.entries.len(), s.cache.total)
    }

    /// `renderMarkdownAsync`: sanitized, highlighted HTML off the UI thread. `key` coalesces
    /// queued streaming prefixes: a superseded keyed call resolves to
    /// [`RenderError::Aborted`]. Keyed results are not cached (they would evict settled
    /// documents).
    pub fn render_async(
        &self,
        src: &str,
        key: Option<&str>,
    ) -> oneshot::Receiver<Result<Arc<str>, RenderError>> {
        let (tx, rx) = oneshot::channel();
        if src.is_empty() {
            let _ = tx.send(Ok("".into()));
            return rx;
        }
        {
            let mut s = lock(&self.state);
            if let Some(hit) = s.cache.get(src) {
                let _ = tx.send(Ok(hit));
                return rx;
            }
            s.seq += 1;
            let id = s.seq;
            if let Some(k) = key {
                if let Some(prev) = s.waiting_by_key.remove(k)
                    && let Some(i) = s.waiting.iter().position(|j| j.id == prev)
                    && let Some(old) = s.waiting.remove(i)
                {
                    let _ = old.reply.send(Err(RenderError::Aborted));
                }
                s.waiting_by_key.insert(k.to_string(), id);
            }
            s.waiting.push_back(Job {
                id,
                src: src.to_string(),
                key: key.map(str::to_string),
                reply: tx,
            });
        }
        self.wake();
        rx
    }

    fn wake(&self) {
        if RENDER_INLINE.load(std::sync::atomic::Ordering::Relaxed) {
            while let Some(job) = next_job(&self.state) {
                let res = render_sync(&job.src);
                finish(&self.state, job, res);
            }
            return;
        }
        let mut w = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(tx) = w.as_ref()
            && tx.send(()).is_ok()
        {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let state = self.state.clone();
        let spawned = std::thread::Builder::new()
            .name("markdown-host".into())
            .spawn(move || coordinate(state, rx));
        if spawned.is_ok() {
            let _ = tx.send(());
            *w = Some(tx);
        } else {
            // No threads: render on the caller, as the host does without Workers.
            drop(w);
            while let Some(job) = next_job(&self.state) {
                let res = render_sync(&job.src);
                finish(&self.state, job, res);
            }
        }
    }
}

static RENDER_INLINE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Render async jobs on the caller instead of the worker thread: GPUI's deterministic test
/// scheduler rejects wakes from other threads.
pub fn set_markdown_render_inline(on: bool) {
    RENDER_INLINE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// Whether [`set_markdown_render_inline`] is on.
pub fn markdown_render_inline() -> bool {
    RENDER_INLINE.load(std::sync::atomic::Ordering::Relaxed)
}

/// `pump`'s dequeue: the next job not already answered by the cache.
fn next_job(state: &Mutex<State>) -> Option<Job> {
    let mut s = lock(state);
    while let Some(job) = s.waiting.pop_front() {
        if let Some(k) = &job.key
            && s.waiting_by_key.get(k) == Some(&job.id)
        {
            s.waiting_by_key.remove(k);
        }
        if let Some(hit) = s.cache.get(&job.src) {
            let _ = job.reply.send(Ok(hit));
            continue;
        }
        return Some(job);
    }
    None
}

/// `finish`: keyed jobs are streaming prefixes and stay out of the cache.
fn finish(state: &Mutex<State>, job: Job, res: Result<String, RenderError>) {
    match res {
        Ok(html) => {
            let html: Arc<str> = html.into();
            if job.key.is_none() {
                let mut s = lock(state);
                let max = s.max_bytes;
                s.cache.set(&job.src, html.clone(), max);
            }
            let _ = job.reply.send(Ok(html));
        }
        Err(e) => {
            let _ = job.reply.send(Err(e));
        }
    }
}

type WorkerReply = (u64, Result<String, String>);
type Worker = (Sender<(u64, String)>, Receiver<WorkerReply>);

/// The worker: comrak and highlighting only.
fn spawn_worker() -> Option<Worker> {
    let (req_tx, req_rx) = mpsc::channel::<(u64, String)>();
    let (res_tx, res_rx) = mpsc::channel::<WorkerReply>();
    std::thread::Builder::new()
        .name("markdown-worker".into())
        .spawn(move || {
            while let Ok((id, src)) = req_rx.recv() {
                let out = catch_unwind(AssertUnwindSafe(|| {
                    parse_markdown(&src, ParseOptions::default())
                }));
                let msg = out.map_err(|_| "markdown worker panicked".to_string());
                if res_tx.send((id, msg)).is_err() {
                    break;
                }
            }
        })
        .ok()?;
    Some((req_tx, res_rx))
}

fn coordinate(state: Arc<Mutex<State>>, wake: Receiver<()>) {
    let mut worker: Option<Worker> = None;
    let mut worker_failed = false;
    let mut timeout_strikes = 0;
    loop {
        let Some(job) = next_job(&state) else {
            if wake.recv().is_err() {
                return;
            }
            continue;
        };
        if worker.is_none() && !worker_failed {
            worker = spawn_worker();
            worker_failed = worker.is_none();
        }
        let Some((req, res)) = worker.as_ref() else {
            let out = render_sync(&job.src);
            finish(&state, job, out);
            continue;
        };
        if req.send((job.id, job.src.clone())).is_err() {
            worker = None;
            worker_failed = true;
            let out = render_sync(&job.src);
            finish(&state, job, out);
            continue;
        }
        let deadline = Instant::now() + JOB_MS;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match res.recv_timeout(left) {
                // A late reply for a timed-out job: keep waiting for this one.
                Ok((id, _)) if id != job.id => continue,
                Ok((_, Ok(raw))) => {
                    timeout_strikes = 0;
                    finish(&state, job, Ok(sanitize_markdown(&raw)));
                    break;
                }
                Ok((_, Err(_))) => {
                    timeout_strikes = 0;
                    let out = render_sync(&job.src);
                    finish(&state, job, out);
                    break;
                }
                Err(RecvTimeoutError::Timeout) => {
                    // First timeout: reject this job, keep the worker (a cold start can burn
                    // the budget while it is healthy). A second in a row retires it.
                    timeout_strikes += 1;
                    if timeout_strikes >= 2 {
                        worker = None;
                        timeout_strikes = 0;
                    }
                    finish(&state, job, Err(RenderError::TimedOut));
                    break;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    worker = None;
                    worker_failed = true;
                    let out = render_sync(&job.src);
                    finish(&state, job, out);
                    break;
                }
            }
        }
    }
}

/// `peekMarkdownCache` on the app's host.
pub fn peek_markdown_cache(src: &str) -> Option<Arc<str>> {
    MarkdownHost::global().peek(src)
}

/// `rememberMarkdown` on the app's host.
pub fn remember_markdown(src: &str, html: &str) {
    MarkdownHost::global().remember(src, html)
}

/// `renderMarkdownCached` on the app's host.
pub fn render_markdown_cached(src: &str, opts: ParseOptions) -> Arc<str> {
    MarkdownHost::global().render_cached(src, opts)
}

/// `renderMarkdownAsync` on the app's host.
pub fn render_markdown_async(
    src: &str,
    key: Option<&str>,
) -> oneshot::Receiver<Result<Arc<str>, RenderError>> {
    MarkdownHost::global().render_async(src, key)
}

/// `clearMarkdownCache` on the app's host.
pub fn clear_markdown_cache() {
    MarkdownHost::global().clear();
    lock_docs().clear();
}

/// Parsed documents of settled markdown on the app's host, keyed like the HTML cache: a view
/// that mounts a source it has shown before (a transcript block scrolled back into view, a
/// tab switched back to) builds no DOM. A `Doc` holds about three times its HTML.
pub const DOC_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

fn lock_docs() -> MutexGuard<'static, Cache<Arc<Doc>>> {
    static DOCS: OnceLock<Mutex<Cache<Arc<Doc>>>> = OnceLock::new();
    DOCS.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// The parsed document of `src`: cache hit, or [`render_markdown_cached`] and parse. Safe off
/// the UI thread, which is how transcripts prewarm it.
pub fn markdown_doc_cached(src: &str, opts: ParseOptions) -> Arc<Doc> {
    if src.is_empty() {
        return Arc::default();
    }
    let key = cache_key(src, opts);
    if let Some(doc) = lock_docs().get(&key) {
        return doc;
    }
    let html = render_markdown_cached(src, opts);
    let doc = Arc::new(Doc::parse(&html));
    let bytes = key.len() + doc.heap_bytes();
    if bytes <= CACHE_ENTRY_MAX_BYTES * 3 {
        lock_docs().insert(&key, doc.clone(), bytes, DOC_CACHE_MAX_BYTES);
    }
    doc
}

/// Whether `src`'s document is cached.
pub fn markdown_doc_is_cached(src: &str, opts: ParseOptions) -> bool {
    src.is_empty() || lock_docs().contains(&cache_key(src, opts))
}

/// `(entries, bytes)` of the document cache.
pub fn markdown_doc_cache_stats() -> (usize, usize) {
    lock_docs().stats()
}

/// `setMarkdownCacheMaxBytesForTests` on the app's host.
pub fn set_markdown_cache_max_bytes_for_tests(n: usize) {
    MarkdownHost::global().set_max_bytes_for_tests(n)
}

/// `markdownCacheStatsForTests` on the app's host: `(entries, bytes)`.
pub fn markdown_cache_stats_for_tests() -> (usize, usize) {
    MarkdownHost::global().stats_for_tests()
}

#[cfg(test)]
mod tests {
    use super::super::render_markdown;
    use super::*;
    use futures::executor::block_on;

    fn rendered(src: &str) -> Arc<str> {
        render_markdown(src, ParseOptions::default()).into()
    }

    #[test]
    fn async_matches_sync_render() {
        let host = MarkdownHost::new();
        let src = "See [the PR](https://github.com/org/repo/pull/1).";
        assert_eq!(
            block_on(host.render_async(src, None)).unwrap(),
            Ok(rendered(src))
        );
    }

    #[test]
    fn does_not_cache_keyed_streaming_prefixes() {
        let host = MarkdownHost::new();
        let src = "growing prefix";
        block_on(host.render_async(src, Some("stream-test")))
            .unwrap()
            .unwrap();
        assert!(host.peek(src).is_none());
    }

    #[test]
    fn evicts_oldest_settled_entries_over_budget() {
        let host = MarkdownHost::new();
        let html = "h".repeat(50);
        host.set_max_bytes_for_tests(400);
        host.remember("keep", &html);
        host.remember("mid", &html);
        host.remember("old", &html);
        host.remember("newer", &html);
        assert!(host.peek("keep").is_none());
        host.peek("mid");
        host.remember("extra", &html);
        assert_eq!(host.peek("mid").as_deref(), Some(html.as_str()));
    }

    #[test]
    fn skips_a_document_over_the_entry_cap() {
        let host = MarkdownHost::new();
        host.remember("src", &"x".repeat(CACHE_ENTRY_MAX_BYTES));
        assert!(host.peek("src").is_none());
        assert_eq!(host.stats_for_tests().0, 0);
    }

    #[test]
    fn render_cached_stores_a_miss() {
        let host = MarkdownHost::new();
        let src = "settled card";
        assert!(host.peek(src).is_none());
        let html = host.render_cached(src, ParseOptions::default());
        assert_eq!(html, rendered(src));
        assert_eq!(host.peek(src), Some(html));
    }

    #[test]
    fn demote_cache_does_not_collide() {
        let host = MarkdownHost::new();
        let src = "![x](https://tracker.example/pixel.png)";
        let live = host.render_cached(src, ParseOptions::default());
        let demoted = host.render_cached(
            src,
            ParseOptions {
                demote_images: true,
            },
        );
        assert!(live.contains("<img"));
        assert!(!demoted.contains("<img"));
        assert_eq!(host.peek(src), Some(live));
    }

    #[test]
    fn remember_stores_a_completed_document() {
        let host = MarkdownHost::new();
        let src = "done streaming";
        let html = rendered(src);
        host.remember(src, &html);
        assert_eq!(host.peek(src), Some(html));
    }

    #[test]
    fn latest_same_key_call_resolves_to_its_own_src() {
        let host = MarkdownHost::new();
        let key = Some("stream-test");
        let a = host.render_async("one", key);
        let b = host.render_async("one *two*", key);
        let c = host.render_async("one **two**", key);
        assert_eq!(block_on(c).unwrap(), Ok(rendered("one **two**")));
        // Earlier keyed calls either finished first or were superseded.
        for r in [block_on(a).unwrap(), block_on(b).unwrap()] {
            assert!(matches!(r, Ok(_) | Err(RenderError::Aborted)));
        }
    }
}
