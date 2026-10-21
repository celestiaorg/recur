//! Rich content shared by the chat, PR, artifact and file views: sanitized markdown (marked +
//! DOMPurify, ported), streaming markdown, Mermaid diagrams, PDF pages, file previews with
//! find, and the image lightbox.

pub mod emoji;
pub mod js;
pub mod markdown;
pub mod marked;
pub mod purify;
pub mod sanitize_lsp;
