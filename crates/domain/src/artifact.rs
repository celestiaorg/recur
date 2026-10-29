//! Agent-created artifacts under workspaces/<id>/artifacts/.

use serde::{Deserialize, Serialize};

string_enum! {
    /// A supported agent-created artifact type.
    ArtifactKind {
        Html = "html",
        Svg = "svg",
        Mermaid = "mermaid",
        Png = "png",
        Jpeg = "jpeg",
        Csv = "csv",
        Json = "json",
        Md = "md",
    }
}

/// One agent-created file under workspaces/<id>/artifacts/.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Artifact {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    pub kind: ArtifactKind,
    /// Relative to the managed data root (workspaces/<id>/artifacts/…).
    pub path: String,
    /// RFC3339.
    pub created_at: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub created_by: String,
}
