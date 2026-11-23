//! browser_screenshot: the engine resolves what the UI cannot know (the start URL, Recur's
//! ports, where PNGs go) and hands the render to the hook the desktop installs, which runs it
//! on the UI thread.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use recur_agent_tools::HostError;
pub use recur_agent_tools::{BROWSER_UNAVAILABLE, BrowserShot, BrowserShots};

use crate::Engine;

/// One browser_screenshot render.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BrowserCapture {
    pub workspace_id: String,
    /// Empty: the workspace browser's page, else `start_url`.
    pub url: String,
    /// The project's app URL at the workspace's port.
    pub start_url: String,
    pub widths: Vec<u32>,
    /// Recur's own servers, which the page must not reach.
    pub blocked_ports: Vec<u16>,
    pub out_dir: PathBuf,
}

pub type BrowserFuture = Pin<Box<dyn Future<Output = Result<BrowserShots, String>> + Send>>;
pub type BrowserHook = Arc<dyn Fn(BrowserCapture) -> BrowserFuture + Send + Sync>;

impl Engine {
    /// The UI's renderer for browser_screenshot; without one the tool reports the browser
    /// unavailable.
    pub fn set_browser_capture(&self, hook: BrowserHook) {
        *self.browser_capture.lock() = Some(hook);
    }

    pub async fn browser_screenshot(
        &self,
        workspace_id: &str,
        url: &str,
        widths: &[u32],
    ) -> Result<BrowserShots, HostError> {
        let hook = self
            .browser_capture
            .lock()
            .clone()
            .ok_or_else(|| HostError::new(BROWSER_UNAVAILABLE))?;
        let ws = self
            .store
            .workspace(workspace_id)
            .ok_or_else(|| HostError::new("workspace not found"))?;
        let app_url = self
            .store
            .project(&ws.project_id)
            .map(|p| p.app_url)
            .unwrap_or_default();
        self.ensure_mcp()
            .await
            .map_err(|e| HostError(e.to_string()))?;
        let req = BrowserCapture {
            workspace_id: workspace_id.to_owned(),
            url: url.to_owned(),
            start_url: recur_domain::browser_url(&app_url, ws.port),
            widths: widths.to_vec(),
            blocked_ports: vec![self.mcp().port()],
            out_dir: recur_store::paths::screenshots_dir(&self.data_dir, workspace_id)
                .map_err(|e| HostError(e.to_string()))?,
        };
        hook(req).await.map_err(HostError)
    }
}
