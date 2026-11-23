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

#[cfg(test)]
mod tests {
    use parking_lot::Mutex;
    use recur_domain::{Project, Workspace};

    use super::*;
    use crate::testing::TestEngine;

    #[tokio::test]
    async fn hook_gets_start_url_blocked_mcp_port_and_out_dir() {
        let te = TestEngine::new(None);
        let e = &te.engine;
        let err = e.browser_screenshot("W1", "", &[390]).await.unwrap_err();
        assert_eq!(err.0, BROWSER_UNAVAILABLE);

        let repo = tempfile::tempdir().unwrap();
        te.store
            .add_project(Project {
                id: "p1".into(),
                repo_path: repo.path().to_string_lossy().into_owned(),
                app_url: "http://localhost:$PORT/app".into(),
                write_project_file: true,
                ..Project::default()
            })
            .unwrap();
        te.store
            .add_workspace(Workspace {
                id: "W1".into(),
                project_id: "p1".into(),
                port: 4321,
                ..Workspace::default()
            })
            .unwrap();
        let seen: Arc<Mutex<Option<BrowserCapture>>> = Arc::default();
        let sink = Arc::clone(&seen);
        e.set_browser_capture(Arc::new(move |req| -> BrowserFuture {
            *sink.lock() = Some(req);
            Box::pin(async { Ok(BrowserShots::default()) })
        }));
        e.browser_screenshot("W1", "http://x.test", &[1440, 390])
            .await
            .unwrap();
        let req = seen.lock().take().unwrap();
        assert_eq!(
            req,
            BrowserCapture {
                workspace_id: "W1".into(),
                url: "http://x.test".into(),
                start_url: "http://localhost:4321/app".into(),
                widths: vec![1440, 390],
                blocked_ports: vec![e.mcp().port()],
                out_dir: recur_store::paths::screenshots_dir(&te.data_dir, "W1").unwrap(),
            }
        );
        assert_ne!(e.mcp().port(), 0);
        te.close().await;
    }
}
