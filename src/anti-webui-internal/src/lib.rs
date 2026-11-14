//! Re-exports every AntiWebUI crate. Use the `anti-webui` crate instead of depending on this
//! directly; it exists so `anti-webui-dylib` can link everything into one shared library.

pub use anti_webui_app as app;
#[cfg(feature = "code")]
pub use anti_webui_code as code;
pub use anti_webui_core as core;
#[cfg(feature = "markdown")]
pub use anti_webui_markdown as markdown;
#[cfg(feature = "math")]
pub use anti_webui_math as math;
#[cfg(feature = "pdf")]
pub use anti_webui_pdf as pdf;
pub use anti_webui_style as style;
#[cfg(feature = "wayland-shell")]
pub use anti_webui_wayland_shell as shell;
pub use anti_webui_wgpu as wgpu;
pub use anti_webui_widgets as widgets;

pub use anti_webui_app::App;

/// Drawing with wgpu in a box of the UI: [`gpu_canvas`](gpu::gpu_canvas). See
/// `docs/design/41-gpu-canvas.md`.
pub mod gpu {
    pub use anti_webui_wgpu::canvas::{GpuCanvas, GpuFrame, GpuInit, GpuPaint, gpu_canvas};
    pub use anti_webui_wgpu::wgpu;
}

pub mod prelude {
    pub use anti_webui_app::App;
    #[cfg(feature = "code")]
    pub use anti_webui_code::{
        CodeEditor, CodeHighlighter, CodeTheme, Language as CodeLanguage, code_editor,
    };
    pub use anti_webui_core::prelude::*;
    pub use anti_webui_macros::ShaderParams;
    #[cfg(feature = "markdown")]
    pub use anti_webui_markdown::{
        Emoji, MarkdownEditor, MarkdownSplitEditor, MarkdownTheme, MarkdownView, markdown_editor,
        markdown_split_editor, markdown_view,
    };
    #[cfg(feature = "math")]
    pub use anti_webui_math::{Math, MathError, math};
    #[cfg(feature = "pdf")]
    pub use anti_webui_pdf::{
        PdfAnnotationId, PdfAnnotationKind, PdfEditing, PdfError, PdfField, PdfFieldChange,
        PdfFieldKind, PdfFieldValue, PdfOutlineItem, PdfState, PdfTarget, PdfThumbnails, PdfTool,
        PdfView, PdfViewer, PdfZoom, pdf_thumbnails, pdf_view, pdf_viewer,
    };
    pub use anti_webui_wgpu::canvas::{GpuCanvas, GpuPaint, gpu_canvas};
    pub use anti_webui_widgets::*;
}
