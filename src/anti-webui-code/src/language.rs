//! The languages there are, each behind a feature, with their grammar, how they are colored, and
//! what the editor needs to know about them (comments, indentation).

use std::path::Path;

/// A language the editor knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Language {
    /// No grammar: text without colors.
    PlainText,
    Rust,
    C,
    Cpp,
    Python,
    JavaScript,
    TypeScript,
    Tsx,
    Json,
    Toml,
    Yaml,
    Markdown,
    /// What is inside the blocks of Markdown (emphasis, links, code spans). Only found inside of
    /// Markdown.
    #[doc(hidden)]
    MarkdownInline,
    Html,
    Css,
    Bash,
    Go,
}

/// What tree-sitter needs for a language.
pub(crate) struct Grammar {
    pub language: tree_sitter::Language,
    pub highlights: String,
    pub injections: &'static str,
}

impl Language {
    /// By the extension or the name of a file.
    pub fn from_path(path: impl AsRef<Path>) -> Language {
        let path = path.as_ref();
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        match name {
            "Cargo.lock" | "Pipfile" => return Language::Toml,
            "Makefile" | "makefile" | "GNUmakefile" => return Language::PlainText,
            ".bashrc" | ".zshrc" | ".profile" | ".bash_profile" | "PKGBUILD" => {
                return Language::Bash;
            }
            _ => {}
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default();
        Self::from_name(ext).unwrap_or(Language::PlainText)
    }

    /// By a name or an extension, as the info string of a Markdown code block gives it.
    pub fn from_name(name: &str) -> Option<Language> {
        let name = name.trim().to_ascii_lowercase();
        Some(match name.as_str() {
            "rust" | "rs" => Language::Rust,
            "c" | "h" => Language::C,
            "cpp" | "c++" | "cc" | "cxx" | "hpp" | "hh" | "hxx" | "h++" => Language::Cpp,
            "python" | "py" | "pyi" | "pyw" => Language::Python,
            "javascript" | "js" | "mjs" | "cjs" | "jsx" => Language::JavaScript,
            "typescript" | "ts" | "mts" | "cts" => Language::TypeScript,
            "tsx" => Language::Tsx,
            "json" | "jsonc" | "json5" => Language::Json,
            "toml" => Language::Toml,
            "yaml" | "yml" => Language::Yaml,
            "markdown" | "md" | "mkd" | "mdx" => Language::Markdown,
            "markdown_inline" => Language::MarkdownInline,
            "html" | "htm" | "xhtml" => Language::Html,
            "css" => Language::Css,
            "bash" | "sh" | "zsh" | "shell" | "console" => Language::Bash,
            "go" | "golang" => Language::Go,
            "text" | "txt" | "plain" | "plaintext" => Language::PlainText,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Language::PlainText => "Plain Text",
            Language::Rust => "Rust",
            Language::C => "C",
            Language::Cpp => "C++",
            Language::Python => "Python",
            Language::JavaScript => "JavaScript",
            Language::TypeScript => "TypeScript",
            Language::Tsx => "TSX",
            Language::Json => "JSON",
            Language::Toml => "TOML",
            Language::Yaml => "YAML",
            Language::Markdown | Language::MarkdownInline => "Markdown",
            Language::Html => "HTML",
            Language::Css => "CSS",
            Language::Bash => "Bash",
            Language::Go => "Go",
        }
    }

    /// What starts a comment to the end of the line, for Ctrl+/.
    pub fn line_comment(self) -> Option<&'static str> {
        match self {
            Language::Rust
            | Language::C
            | Language::Cpp
            | Language::JavaScript
            | Language::TypeScript
            | Language::Tsx
            | Language::Go => Some("//"),
            Language::Python | Language::Toml | Language::Yaml | Language::Bash => Some("#"),
            _ => None,
        }
    }

    /// What wraps a comment when there is no comment to the end of the line.
    pub fn block_comment(self) -> Option<(&'static str, &'static str)> {
        match self {
            Language::Html | Language::Markdown => Some(("<!--", "-->")),
            Language::Css => Some(("/*", "*/")),
            _ => None,
        }
    }

    /// One step of indentation.
    pub fn indent_unit(self) -> &'static str {
        match self {
            Language::JavaScript
            | Language::TypeScript
            | Language::Tsx
            | Language::Json
            | Language::Yaml
            | Language::Html
            | Language::Css
            | Language::Markdown => "  ",
            Language::Go => "\t",
            _ => "    ",
        }
    }

    /// Whether a line ending in `:` opens a block, as in Python.
    pub(crate) fn colon_opens(self) -> bool {
        matches!(self, Language::Python | Language::Yaml)
    }

    /// Every language that can be built in, whether or not it is.
    pub fn all() -> &'static [Language] {
        &[
            Language::Rust,
            Language::C,
            Language::Cpp,
            Language::Python,
            Language::JavaScript,
            Language::TypeScript,
            Language::Tsx,
            Language::Json,
            Language::Toml,
            Language::Yaml,
            Language::Markdown,
            Language::Html,
            Language::Css,
            Language::Bash,
            Language::Go,
        ]
    }

    /// The grammar, if its feature is on.
    pub(crate) fn grammar(self) -> Option<Grammar> {
        let g = |language: tree_sitter_language::LanguageFn, highlights: String, injections| {
            Some(Grammar {
                language: language.into(),
                highlights,
                injections,
            })
        };
        let _ = &g;
        match self {
            #[cfg(feature = "rust")]
            Language::Rust => g(
                tree_sitter_rust::LANGUAGE,
                tree_sitter_rust::HIGHLIGHTS_QUERY.into(),
                tree_sitter_rust::INJECTIONS_QUERY,
            ),
            #[cfg(feature = "c")]
            Language::C => g(
                tree_sitter_c::LANGUAGE,
                tree_sitter_c::HIGHLIGHT_QUERY.into(),
                "",
            ),
            // Each builds on another's highlighting, as their tree-sitter.json says.
            #[cfg(feature = "cpp")]
            Language::Cpp => g(
                tree_sitter_cpp::LANGUAGE,
                [
                    tree_sitter_c::HIGHLIGHT_QUERY,
                    tree_sitter_cpp::HIGHLIGHT_QUERY,
                ]
                .join("\n"),
                "",
            ),
            #[cfg(feature = "python")]
            Language::Python => g(
                tree_sitter_python::LANGUAGE,
                tree_sitter_python::HIGHLIGHTS_QUERY.into(),
                "",
            ),
            #[cfg(feature = "javascript")]
            Language::JavaScript => g(
                tree_sitter_javascript::LANGUAGE,
                [
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                ]
                .join("\n"),
                tree_sitter_javascript::INJECTIONS_QUERY,
            ),
            #[cfg(feature = "typescript")]
            Language::TypeScript => g(
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                [
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                ]
                .join("\n"),
                tree_sitter_javascript::INJECTIONS_QUERY,
            ),
            #[cfg(feature = "typescript")]
            Language::Tsx => g(
                tree_sitter_typescript::LANGUAGE_TSX,
                [
                    tree_sitter_typescript::HIGHLIGHTS_QUERY,
                    tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                    tree_sitter_javascript::HIGHLIGHT_QUERY,
                ]
                .join("\n"),
                tree_sitter_javascript::INJECTIONS_QUERY,
            ),
            #[cfg(feature = "json")]
            Language::Json => g(
                tree_sitter_json::LANGUAGE,
                tree_sitter_json::HIGHLIGHTS_QUERY.into(),
                "",
            ),
            #[cfg(feature = "toml")]
            Language::Toml => g(
                tree_sitter_toml_ng::LANGUAGE,
                tree_sitter_toml_ng::HIGHLIGHTS_QUERY.into(),
                "",
            ),
            #[cfg(feature = "yaml")]
            Language::Yaml => g(
                tree_sitter_yaml::LANGUAGE,
                tree_sitter_yaml::HIGHLIGHTS_QUERY.into(),
                "",
            ),
            #[cfg(feature = "markdown")]
            Language::Markdown => g(
                tree_sitter_md::LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK.into(),
                tree_sitter_md::INJECTION_QUERY_BLOCK,
            ),
            #[cfg(feature = "markdown")]
            Language::MarkdownInline => g(
                tree_sitter_md::INLINE_LANGUAGE,
                tree_sitter_md::HIGHLIGHT_QUERY_INLINE.into(),
                tree_sitter_md::INJECTION_QUERY_INLINE,
            ),
            #[cfg(feature = "html")]
            Language::Html => g(
                tree_sitter_html::LANGUAGE,
                tree_sitter_html::HIGHLIGHTS_QUERY.into(),
                tree_sitter_html::INJECTIONS_QUERY,
            ),
            #[cfg(feature = "css")]
            Language::Css => g(
                tree_sitter_css::LANGUAGE,
                tree_sitter_css::HIGHLIGHTS_QUERY.into(),
                "",
            ),
            #[cfg(feature = "bash")]
            Language::Bash => g(
                tree_sitter_bash::LANGUAGE,
                tree_sitter_bash::HIGHLIGHT_QUERY.into(),
                "",
            ),
            #[cfg(feature = "go")]
            Language::Go => g(
                tree_sitter_go::LANGUAGE,
                tree_sitter_go::HIGHLIGHTS_QUERY.into(),
                "",
            ),
            _ => None,
        }
    }
}
