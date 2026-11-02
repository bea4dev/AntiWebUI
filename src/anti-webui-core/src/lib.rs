//! The platform- and GPU-independent core of AntiWebUI: reactivity, the node tree, style
//! resolution, layout, text, selection, editing and input handling.

pub mod event;
pub mod geometry;
pub mod layout;
pub mod paint;
pub mod platform;
pub mod reactive;
pub mod scene;
pub mod text;
pub mod tree;
pub mod ui;
pub mod view;

pub use anti_webui_style as style;
pub use ui::Ui;

pub mod prelude {
    pub use crate::event::{
        Key, KeyboardEvent, Modifiers, NamedKey, PointerButton, PointerEvent, ScrollEvent,
    };
    pub use crate::platform::ImePurpose;
    pub use crate::reactive::{
        Memo, Owner, Signal, batch, create_root, effect, memo, on_cleanup, signal, untrack,
    };
    pub use crate::view::{
        Div, IntoTextSource, IntoView, NodeRef, Text, TextInput, View, div, dyn_view, for_each,
        fragment, node_ref, show, text, text_area, text_input,
    };
    pub use anti_webui_style::*;
}
