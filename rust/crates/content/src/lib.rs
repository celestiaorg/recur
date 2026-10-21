//! Rich content shared by the chat, PR, artifact and file views: sanitized markdown (marked +
//! DOMPurify, ported), streaming markdown, Mermaid diagrams, PDF pages, file previews with
//! find, and the image lightbox.

pub mod emoji;
pub mod image_menu;
pub mod images;
pub mod js;
pub mod lightbox;
pub mod links;
pub mod markdown;
pub mod marked;
pub mod mermaid;
pub mod purify;
pub mod render;
pub mod sanitize_lsp;
pub mod view;

pub use images::{ImageLoader, set_image_loader};
pub use render::style::MarkdownClass;
pub use view::{MarkdownEvent, MarkdownView};
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod streaming;
pub mod svg;
pub mod html;
pub mod preview;
pub mod pdf;
pub use streaming::StreamingMarkdown;
