//! A fast code editor for AntiWebUI: `text_editor` with the colors of tree-sitter grammars, line
//! numbers, the keys of a code editor, folding and a find bar. See
//! `docs/design/31-code-editor.md`.

mod editor;
mod edits;
mod highlight;
mod language;
mod search;
mod theme;

pub use editor::{CodeEditor, HIGHLIGHT_LIMIT, code_editor};
pub use language::Language;
pub use theme::{CodeTheme, MONO};
