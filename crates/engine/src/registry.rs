//! The extension-point registries: agent providers, forges and issue
//! trackers, keyed by their persisted ids (`claude-code`, `codex`, `cursor`,
//! each compatible provider's own id; forge `github` with PR rows keeping
//! `source = "gh"`; trackers `github`, `linear`). `recur-services` is the
//! composition root that fills them; the engine only looks things up here
//! and never compares provider, forge or tracker ids itself.

use std::sync::Arc;

use parking_lot::RwLock;
use recur_agent::Provider;
use recur_domain::{PrSource, parse_model_ref};
use recur_forge::Forge;
use recur_issues::IssueTracker;

/// Registered agent providers. A pinned provider (tests) receives every turn
/// regardless of the model ref; pinning `None` fails every turn closed rather
/// than reaching a real provider.
#[derive(Default)]
pub struct ProviderRegistry {
    providers: RwLock<Vec<Arc<dyn Provider>>>,
    pinned: RwLock<Option<Option<Arc<dyn Provider>>>>,
}

impl ProviderRegistry {
    pub fn new(providers: Vec<Arc<dyn Provider>>) -> Self {
        Self {
            providers: RwLock::new(providers),
            pinned: RwLock::new(None),
        }
    }

    /// Every turn goes to `provider` (`None`: every turn fails).
    pub fn pinned(provider: Option<Arc<dyn Provider>>) -> Self {
        let r = Self::default();
        r.pin(provider);
        r
    }

    /// Pins `provider` for every turn.
    pub fn pin(&self, provider: Option<Arc<dyn Provider>>) {
        *self.pinned.write() = Some(provider);
    }

    /// Whether a test pin overrides routing.
    pub fn is_pinned(&self) -> bool {
        self.pinned.read().is_some()
    }

    /// Replaces the registered set (settings changed compatible providers).
    pub fn replace(&self, providers: Vec<Arc<dyn Provider>>) {
        *self.providers.write() = providers;
    }

    /// The provider registered under `id`.
    pub fn get(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.providers.read().iter().find(|p| p.id() == id).cloned()
    }

    /// The provider a model ref routes to (`provider/id`, unqualified =
    /// claude-code). A pin wins.
    pub fn for_model(&self, model_ref: &str) -> Option<Arc<dyn Provider>> {
        if let Some(p) = self.pinned.read().as_ref() {
            return p.clone();
        }
        self.get(&parse_model_ref(model_ref).provider)
    }

    /// The provider persisted under `id` (transcript display, context
    /// windows): the registered one, else a pin that carries that id.
    pub fn for_id(&self, id: &str) -> Option<Arc<dyn Provider>> {
        self.get(id).or_else(|| match self.pinned.read().as_ref() {
            Some(Some(p)) if p.id() == id => Some(Arc::clone(p)),
            _ => None,
        })
    }

    /// Every registered provider (not the pin).
    pub fn all(&self) -> Vec<Arc<dyn Provider>> {
        self.providers.read().clone()
    }

    /// Every provider a lifecycle hook (release, idle, shutdown) must reach:
    /// the registered ones plus a pin that is not among them.
    pub fn lifecycle(&self) -> Vec<Arc<dyn Provider>> {
        let mut out = self.all();
        if let Some(Some(p)) = self.pinned.read().as_ref()
            && !out.iter().any(|q| Arc::ptr_eq(q, p))
        {
            out.push(Arc::clone(p));
        }
        out
    }
}

/// Registered forges (remote git hosting).
#[derive(Default)]
pub struct ForgeRegistry {
    forges: RwLock<Vec<Arc<dyn Forge>>>,
}

impl ForgeRegistry {
    pub fn new(forges: Vec<Arc<dyn Forge>>) -> Self {
        Self {
            forges: RwLock::new(forges),
        }
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Forge>> {
        self.forges.read().iter().find(|f| f.id() == id).cloned()
    }

    /// The forge that owns PR rows with this `source` (local PRs have none).
    pub fn for_pr_source(&self, source: &PrSource) -> Option<Arc<dyn Forge>> {
        self.forges
            .read()
            .iter()
            .find(|f| f.pr_source() == *source)
            .cloned()
    }

    /// The forge that recognizes a remote URL, with its `owner/repo`.
    pub fn detect(&self, remote_url: &str) -> Option<(Arc<dyn Forge>, String)> {
        self.forges
            .read()
            .iter()
            .find_map(|f| f.detect(remote_url).map(|r| (Arc::clone(f), r)))
    }

    pub fn all(&self) -> Vec<Arc<dyn Forge>> {
        self.forges.read().clone()
    }
}

/// Registered issue trackers.
#[derive(Default)]
pub struct TrackerRegistry {
    trackers: RwLock<Vec<Arc<dyn IssueTracker>>>,
}

impl TrackerRegistry {
    pub fn new(trackers: Vec<Arc<dyn IssueTracker>>) -> Self {
        Self {
            trackers: RwLock::new(trackers),
        }
    }

    /// The tracker for an `IssueRef.source`.
    pub fn get(&self, id: &str) -> Option<Arc<dyn IssueTracker>> {
        self.trackers.read().iter().find(|t| t.id() == id).cloned()
    }

    pub fn all(&self) -> Vec<Arc<dyn IssueTracker>> {
        self.trackers.read().clone()
    }

    /// Env vars no spawned agent may inherit (every tracker's secrets).
    pub fn secret_env(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in self.trackers.read().iter() {
            for k in t.secret_env() {
                if !out.iter().any(|x| x == k) {
                    out.push((*k).to_owned());
                }
            }
        }
        out
    }
}
