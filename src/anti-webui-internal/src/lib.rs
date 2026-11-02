//! Re-exports every AntiWebUI crate. Use the `anti-webui` crate instead of depending on this
//! directly; it exists so `anti-webui-dylib` can link everything into one shared library.

pub use anti_webui_app as app;
pub use anti_webui_core as core;
pub use anti_webui_style as style;
pub use anti_webui_wgpu as wgpu;
pub use anti_webui_widgets as widgets;

pub use anti_webui_app::App;

pub mod prelude {
    pub use anti_webui_app::App;
    pub use anti_webui_core::prelude::*;
    pub use anti_webui_widgets::*;
}
