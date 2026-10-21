//! Rich content shared by the chat, PR, artifact and file views: sanitized markdown (marked +
//! DOMPurify, ported), streaming markdown, Mermaid diagrams, PDF pages, file previews with
//! find, and the image lightbox.

pub mod js;
pub mod marked;
