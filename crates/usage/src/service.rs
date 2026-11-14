//! The Settings-facing usage API: poller lifecycle, the footer snapshot, and
//! Cursor usage Connect. `recur-services` wraps it with the store and the bus.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Weak};

use parking_lot::Mutex;
use recur_domain::{
    PROVIDER_CHATGPT, PROVIDER_CLAUDE_CODE, PROVIDER_CODEX, PROVIDER_CURSOR, Settings,
};

use crate::connect::{cursor_connect_tokens_quota, cursor_no_session_quota};
use crate::cursor::has_recur_cursor_auth_file;
use crate::fetcher::{ChatGptRefresh, Fetcher, Wants};
use crate::poller::{Poller, Publisher};
use crate::types::{CursorUsageConnect, ProviderQuota, Snapshot, empty_snapshot};

pub type SettingsSource = Arc<dyn Fn() -> Settings + Send + Sync>;
/// Saves settings through the full Settings save path, whose side effects
/// (provider remap, governor caps) are async.
pub type SettingsSink =
    Arc<dyn Fn(Settings) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send>> + Send + Sync>;
pub type DataDirSource = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

pub struct UsageService {
    settings: SettingsSource,
    save_settings: SettingsSink,
    data_dir: DataDirSource,
    publish: Option<Publisher>,
    poller: Mutex<Option<Arc<Poller>>>,
    chatgpt_refresh: Mutex<Option<ChatGptRefresh>>,
}

impl UsageService {
    pub fn new(
        settings: SettingsSource,
        save_settings: SettingsSink,
        data_dir: DataDirSource,
        publish: Option<Publisher>,
    ) -> Arc<Self> {
        Arc::new(Self {
            settings,
            save_settings,
            data_dir,
            publish,
            poller: Mutex::new(None),
            chatgpt_refresh: Mutex::new(None),
        })
    }

    /// Lets usage fetches refresh an expired ChatGPT token. Set before `start`.
    pub fn set_chatgpt_refresh(&self, refresh: ChatGptRefresh) {
        *self.chatgpt_refresh.lock() = Some(refresh);
    }

    /// Starts the background poller once. Call from the app entry point;
    /// services built for tests must not hit provider APIs.
    pub fn start(self: &Arc<Self>) {
        let mut slot = self.poller.lock();
        if slot.is_some() {
            return;
        }
        let me: Weak<Self> = Arc::downgrade(self);
        let (w1, w2, w3) = (me.clone(), me.clone(), me);
        let data_dir = Arc::clone(&self.data_dir);
        let fetch = Fetcher {
            chatgpt_refresh: self.chatgpt_refresh.lock().clone(),
            data_dir: Some(Arc::new(move || data_dir())),
            want: Some(Arc::new(move || {
                w1.upgrade().map_or_else(Wants::default, |s| s.wants())
            })),
            cursor_stub: Some(Arc::new(move || w2.upgrade().and_then(|s| s.cursor_stub()))),
            // The hook is sync inside a fetch; the save runs beside it.
            on_minted_api_key: Some(Arc::new(move |_prev: &str, next: &str| {
                if let Some(s) = w3.upgrade() {
                    let next = next.to_owned();
                    tokio::spawn(async move { s.apply_minted_cursor_key(&next).await });
                }
            })),
            ..Fetcher::default()
        };
        *slot = Some(Poller::new(Arc::new(fetch), self.publish.clone()));
    }

    /// Stops the ticker and any in-flight Cursor Connect.
    pub fn stop(&self) {
        if let Some(p) = self.poller.lock().take() {
            p.stop();
        }
    }

    fn poller(&self) -> Option<Arc<Poller>> {
        self.poller.lock().clone()
    }

    fn dir(&self) -> PathBuf {
        (self.data_dir)().unwrap_or_default()
    }

    /// Which providers to fetch. Cursor needs a Recur Connect session: the
    /// `crsr_` API key cannot read dashboard usage.
    pub fn wants(&self) -> Wants {
        let set = (self.settings)();
        let dir = self.dir();
        Wants {
            claude: set.provider_enabled(PROVIDER_CLAUDE_CODE),
            codex: set.provider_enabled(PROVIDER_CODEX) && set.codex_chat_gpt_auth,
            chatgpt: set.provider_enabled(PROVIDER_CHATGPT),
            cursor: set.provider_enabled(PROVIDER_CURSOR) && has_recur_cursor_auth_file(&dir),
        }
    }

    /// The local-only Cursor row when Cursor is not fetched.
    pub fn cursor_stub(&self) -> Option<ProviderQuota> {
        if has_recur_cursor_auth_file(&self.dir()) {
            return Some(cursor_connect_tokens_quota());
        }
        (self.settings)()
            .provider_enabled(PROVIDER_CURSOR)
            .then(cursor_no_session_quota)
    }

    /// The last snapshot, refreshed when stale.
    pub async fn get_provider_usage(&self) -> Snapshot {
        match self.poller() {
            Some(p) => p.snapshot(false).await,
            None => empty_snapshot(),
        }
    }

    /// A forced fetch that joins any in-flight collect.
    pub async fn refresh_provider_usage(&self) -> Snapshot {
        match self.poller() {
            Some(p) => p.snapshot(true).await,
            None => empty_snapshot(),
        }
    }

    /// Starts the Cursor browser login and returns its URL.
    pub fn connect_cursor_usage(&self) -> CursorUsageConnect {
        match self.poller() {
            Some(p) => p.connect_cursor(),
            None => CursorUsageConnect {
                error: "Usage poller is not running".into(),
                ..Default::default()
            },
        }
    }

    pub fn cancel_cursor_usage_connect(&self) {
        if let Some(p) = self.poller() {
            p.cancel_connect();
        }
    }

    /// Deletes Recur-owned Connect tokens and the minted SDK key. A leftover
    /// pasted key is cleared too: Connect is the only Cursor sign-in.
    pub async fn disconnect_cursor_usage(&self) -> Result<(), String> {
        let err = match self.poller() {
            Some(p) => p.disconnect_cursor().await.map_err(|e| e.to_string()),
            None => Ok(()),
        };
        self.clear_cursor_api_key().await;
        err
    }

    async fn clear_cursor_api_key(&self) {
        let mut set = (self.settings)();
        if set.cursor_api_key.trim().is_empty() {
            return;
        }
        set.cursor_api_key.clear();
        if let Err(e) = (self.save_settings)(set).await {
            tracing::warn!("recur: warning: clear cursor api key: {e}");
        }
    }

    /// Clears a Settings key when Recur has no Connect file, so a leftover
    /// paste cannot keep showing Connected (Cursor status calls this).
    pub async fn drop_orphan_cursor_key(&self, mut set: Settings, data_dir: &Path) -> Settings {
        if has_recur_cursor_auth_file(data_dir) || set.cursor_api_key.trim().is_empty() {
            return set;
        }
        self.clear_cursor_api_key().await;
        set.cursor_api_key.clear();
        set
    }

    /// Writes a Connect-minted user API key into Settings.
    async fn apply_minted_cursor_key(&self, next: &str) {
        let next = next.trim();
        if next.is_empty() {
            return;
        }
        let mut set = (self.settings)();
        if set.cursor_api_key.trim() == next {
            return;
        }
        set.cursor_api_key = next.to_owned();
        if let Err(e) = (self.save_settings)(set).await {
            tracing::warn!("recur: warning: save cursor api key: {e}");
        }
    }
}
