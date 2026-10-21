//! Rich content shared by the chat, PR, artifact and file views: sanitized markdown (marked +
//! DOMPurify, ported), streaming markdown, Mermaid diagrams, PDF pages, file previews with
//! find, and the image lightbox.

pub mod emoji;
#[cfg(feature = "gallery")]
pub mod gallery;
pub mod html;
pub mod image_menu;
pub mod images;
pub mod js;
pub mod lightbox;
pub mod links;
pub mod markdown;
pub mod marked;
pub mod mermaid;
pub mod pdf;
pub mod preview;
pub mod purify;
pub mod render;
pub mod sanitize_lsp;
pub mod streaming;
pub mod svg;
pub mod view;
pub mod webhost;

pub use html::{HtmlPreview, HtmlPreviewEvent};
pub use images::{ImageLoader, set_image_loader};
pub use lightbox::{AttachmentLightbox, LightboxEvent};
pub use mermaid::MermaidPreview;
pub use pdf::PdfPreview;
pub use preview::view::{FilePreview, FilePreviewEvent, SharedScroll};
pub use render::style::MarkdownClass;
pub use streaming::StreamingMarkdown;
pub use view::{MarkdownEvent, MarkdownView};
