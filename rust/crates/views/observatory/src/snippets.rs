//! The bundle `[[snippet]]` catalog the editors expand (`lib/snippets.ts`): a 2s per-file
//! cache (16 entries) for synchronous Tab expansion, scope / glob matching and the
//! disabled-snippet setting, plus the focused editor's snippet handler for the command palette.

use std::cell::RefCell;
use std::collections::{BTreeSet, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use futures::future::LocalBoxFuture;
use gpui_kit::{App, Global, Window};
use recur_editor::snippet::{RegisteredSnippet, SnippetInfo, SnippetProvider, sort_snippets};
use recur_services::Services;
use recur_view_core::commands::command_matches;

const CACHE_MS: u64 = 2000;
const CACHE_MAX: usize = 16;

fn from_service(s: &recur_services::api::SnippetInfo) -> RegisteredSnippet {
    RegisteredSnippet::from_info(&SnippetInfo {
        id: s.id.clone(),
        name: s.name.clone(),
        tab_trigger: s.tab_trigger.clone(),
        scope: s.scope.clone(),
        file_glob: s.file_glob.clone(),
        body: s.body.clone(),
        order: s.order,
        bundle_name: s.bundle_name.clone(),
        key_equivalent: s.key_equivalent.clone(),
    })
}

fn matches(s: &RegisteredSnippet, scope: &str, path: &str) -> bool {
    command_matches(
        s.scope.as_deref().unwrap_or(""),
        s.file_glob.as_deref().unwrap_or(""),
        scope,
        path,
    )
}

struct Cache {
    entries: VecDeque<(String, Vec<RegisteredSnippet>, Instant)>,
}

/// `loadBundleSnippets` and friends over `BundleService::list_snippets`.
pub struct BundleSnippets {
    services: Services,
    cache: Rc<RefCell<Cache>>,
    disabled: Rc<RefCell<BTreeSet<String>>>,
}

impl BundleSnippets {
    pub fn new(services: Services) -> Self {
        BundleSnippets {
            services,
            cache: Rc::new(RefCell::new(Cache {
                entries: VecDeque::new(),
            })),
            disabled: Rc::default(),
        }
    }

    /// `disabledBundleSnippets` from Settings.
    pub fn set_disabled(&self, ids: impl IntoIterator<Item = String>) {
        *self.disabled.borrow_mut() = ids.into_iter().filter(|s| !s.is_empty()).collect();
    }

    /// `invalidateSnippetCache`.
    pub fn invalidate(&self) {
        self.cache.borrow_mut().entries.clear();
    }

    fn filter_enabled(&self, list: Vec<RegisteredSnippet>) -> Vec<RegisteredSnippet> {
        let d = self.disabled.borrow();
        if d.is_empty() {
            return list;
        }
        list.into_iter().filter(|s| !d.contains(&s.id)).collect()
    }

    fn raw(&self, scope: &str, path: &str) -> LocalBoxFuture<'static, Vec<RegisteredSnippet>> {
        let key = format!("{scope}\0{path}");
        let hit = self
            .cache
            .borrow()
            .entries
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, v, at)| (v.clone(), *at));
        if let Some((items, at)) = &hit
            && at.elapsed() < Duration::from_millis(CACHE_MS)
        {
            let items = items.clone();
            return Box::pin(async move { items });
        }
        let services = self.services.clone();
        let cache = self.cache.clone();
        let (scope, path) = (scope.to_string(), path.to_string());
        Box::pin(async move {
            match services.bundle.list_snippets(&scope, &path).await {
                Ok(list) => {
                    let items: Vec<RegisteredSnippet> = list
                        .iter()
                        .map(from_service)
                        .filter(|s| matches(s, &scope, &path))
                        .collect();
                    let mut c = cache.borrow_mut();
                    c.entries.retain(|(k, _, _)| *k != key);
                    c.entries.push_back((key, items.clone(), Instant::now()));
                    if c.entries.len() > CACHE_MAX {
                        c.entries.pop_front();
                    }
                    items
                }
                Err(_) => hit.map(|(v, _)| v).unwrap_or_default(),
            }
        })
    }

    /// `listSnippets`: enabled, sorted.
    pub fn list(&self, scope: &str, path: &str) -> LocalBoxFuture<'static, Vec<RegisteredSnippet>> {
        let raw = self.raw(scope, path);
        let disabled = self.disabled.clone();
        Box::pin(async move {
            let d = disabled.borrow().clone();
            let mut v: Vec<RegisteredSnippet> = raw
                .await
                .into_iter()
                .filter(|s| !d.contains(&s.id))
                .collect();
            sort_snippets(&mut v);
            v
        })
    }
}

impl SnippetProvider for BundleSnippets {
    fn load(&self, scope: &str, path: &str) -> LocalBoxFuture<'static, Vec<RegisteredSnippet>> {
        let raw = self.raw(scope, path);
        let disabled = self.disabled.clone();
        Box::pin(async move {
            let d = disabled.borrow().clone();
            raw.await
                .into_iter()
                .filter(|s| !d.contains(&s.id))
                .collect()
        })
    }

    fn cached(&self, scope: &str, path: &str) -> Vec<RegisteredSnippet> {
        let key = format!("{scope}\0{path}");
        let items = self
            .cache
            .borrow()
            .entries
            .iter()
            .find(|(k, _, _)| *k == key)
            .map(|(_, v, _)| v.clone())
            .unwrap_or_default();
        self.filter_enabled(items)
    }
}

pub type SnippetHandler = Rc<dyn Fn(RegisteredSnippet, &mut Window, &mut App)>;

/// The focused editor's snippet insertion (`setEditorSnippetHandler`).
#[derive(Default)]
pub struct EditorSnippetHandler(pub Option<SnippetHandler>);

impl Global for EditorSnippetHandler {}

pub fn set_editor_snippet_handler(h: Option<SnippetHandler>, cx: &mut App) {
    cx.default_global::<EditorSnippetHandler>().0 = h;
}

/// `runEditorSnippet`: insert at the focused editor's caret or selection.
pub fn run_editor_snippet(
    sn: RegisteredSnippet,
    window: &mut Window,
    cx: &mut App,
) -> Result<(), String> {
    let h = cx
        .try_global::<EditorSnippetHandler>()
        .and_then(|h| h.0.clone())
        .ok_or_else(|| "No editor is focused — open a file in the Observatory first".to_string())?;
    h(sn, window, cx);
    Ok(())
}
