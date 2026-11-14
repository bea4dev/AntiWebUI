//! Markdown for AntiWebUI: a viewer made of ordinary nodes, with Mermaid diagrams. See
//! `docs/design/28-markdown.md`.
//!
//! ```ignore
//! markdown_view(move || source.get())
//!     .theme(MarkdownTheme::default())
//!     .on_link(|url| open(url))
//! ```

mod editor;
#[cfg(feature = "mermaid")]
mod mermaid;
mod numbers;
mod parse;
#[cfg(feature = "code")]
mod split;
mod theme;
mod view;

pub use editor::{MarkdownEditor, markdown_editor};
#[cfg(feature = "mermaid")]
pub use mermaid::mermaid_svg;
#[cfg(feature = "code")]
pub use split::{MarkdownSplitEditor, markdown_split_editor};
pub use theme::MarkdownTheme;
pub use view::{Emoji, IntoMarkdown, MarkdownView, markdown_view};
