//! The platform- and GPU-independent core of AntiWebUI: reactivity, the node tree, style
//! resolution, layout, text, selection, editing and input handling.

pub mod animation;
pub mod damage;
pub mod drag;
pub mod event;
pub mod geometry;
pub mod images;
pub mod layout;
pub mod paint;
pub mod platform;
pub mod reactive;
pub mod scene;
mod scroll;
pub mod settings;
pub mod shader;
pub mod text;
pub mod tree;
pub mod ui;
pub mod view;

pub use anti_webui_style as style;
pub use ui::Ui;

pub mod prelude {
    pub use crate::animation::{
        Animatable, Animated, Spring, Timeout, animated, request_frame, set_timeout,
    };
    pub use crate::drag::{DragData, DropEvent, ExternalDrag, GrabEvent, GrabPhase, dragged};
    pub use crate::event::{
        DragAction, DragActions, DragResult, Key, KeyboardEvent, Modifiers, NamedKey,
        PointerButton, PointerEvent, ScrollEvent, ScrollPhase, ScrollSource,
    };
    pub use crate::geometry::{PointF, RectI, Scale};
    pub use crate::platform::{ImePurpose, Placement, PopupKind};
    pub use crate::reactive::{
        Memo, Owner, Signal, batch, create_root, effect, memo, on_cleanup, signal, untrack,
    };
    pub use crate::settings::{FrameAdvance, ImageSettings, Settings, settings, update_settings};
    pub use crate::shader::{Image, IntoImage, IntoPaint, Shader, ShaderPaint, ShaderParams};
    pub use crate::text::{NotAFont, register_font, register_font_as};
    pub use crate::view::{
        Div, ForEach, Img, IntoImageSource, IntoTextSource, IntoView, NodeRef, Popup, Show, Text,
        TextInput, View, div, dyn_view, for_each, fragment, img, node_ref, popup, portal, shader,
        show, text, text_area, text_input, window_scale,
    };
    pub use anti_webui_style::*;
}
