//! The platform- and GPU-independent core of AntiWebUI: reactivity, the node tree, style
//! resolution, layout, text, selection, editing and input handling.

pub mod damage;
pub mod event;
pub mod geometry;
pub mod layout;
pub mod paint;
pub mod platform;
pub mod reactive;
pub mod scene;
mod scroll;
pub mod settings;
pub mod text;
pub mod tree;
pub mod ui;
pub mod view;

pub use anti_webui_style as style;
pub use ui::Ui;

pub mod prelude {
    pub use crate::event::{
        Key, KeyboardEvent, Modifiers, NamedKey, PointerButton, PointerEvent, ScrollEvent,
        ScrollPhase, ScrollSource,
    };
    pub use crate::geometry::RectI;
    pub use crate::platform::{ImePurpose, Placement, PopupKind};
    pub use crate::reactive::{
        Memo, Owner, Signal, batch, create_root, effect, memo, on_cleanup, signal, untrack,
    };
    pub use crate::settings::{Settings, settings, update_settings};
    pub use crate::view::{
        Div, IntoTextSource, IntoView, NodeRef, Popup, Text, TextInput, View, div, dyn_view,
        for_each, fragment, node_ref, popup, show, text, text_area, text_input,
    };
    pub use anti_webui_style::*;
}
