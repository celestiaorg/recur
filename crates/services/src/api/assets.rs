use async_trait::async_trait;

/// One served file: what the webview's asset server answered a `GET` with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asset {
    pub content_type: String,
    pub cache_control: String,
    pub body: Vec<u8>,
}

/// A refused request: the HTTP status and the error body the handler wrote.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{status}: {message}")]
pub struct AssetError {
    pub status: u16,
    pub message: String,
}

impl AssetError {
    pub fn new(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    /// A 404 with the plain `404 page not found` body.
    pub fn not_found() -> Self {
        Self::new(404, "404 page not found")
    }
}

/// The app's asset routes, for a native UI that has no webview to fetch them: per-workspace
/// managed files (`/workspaces/<id>/…`), worktree files
/// (`/workspace-file?project=&workspace=&path=[&at=base]`), dropped chat images
/// (`/chat-image?path=`), dropped external files (`/external-file?path=`) and GitHub attachment
/// images (`/gh-image?url=`). `url` is the requested path plus query; traversal, the external-file
/// allowlist and the GitHub host rules are enforced.
#[async_trait]
pub trait AssetsService: Send + Sync {
    async fn fetch_asset(&self, url: &str) -> Result<Asset, AssetError>;
}
