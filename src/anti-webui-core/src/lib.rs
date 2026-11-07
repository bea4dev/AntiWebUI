//! The platform- and GPU-independent core of AntiWebUI: reactivity, the node tree, style
//! resolution, layout, text, selection, editing and input handling.
//!
//! What is `#[doc(hidden)]` is for the other AntiWebUI crates and for tests, and is not part of
//! what semver promises. Programs use the `prelude`. See `docs/design/33-customization.md`.

pub mod access;
pub mod animation;
pub mod channel;
#[doc(hidden)]
pub mod damage;
pub mod drag;
pub mod event;
pub mod geometry;
pub mod images;
#[doc(hidden)]
pub mod layout;
#[doc(hidden)]
pub mod overlay;
#[doc(hidden)]
pub mod paint;
pub mod platform;
pub mod reactive;
#[doc(hidden)]
pub mod scene;
mod scroll;
pub mod settings;
pub mod shader;
pub mod svg;
#[cfg(feature = "svg")]
mod svg_file;
pub mod text;
#[doc(hidden)]
pub mod tree;
#[doc(hidden)]
pub mod ui;
pub mod video;
pub mod view;
mod visibility;

pub use anti_webui_style as style;
pub use ui::Ui;

pub mod prelude {
    pub use crate::access::{self as accessibility, Access, Checked, Live, Role};
    pub use crate::animation::{
        Animatable, Animated, Spring, Timeout, Timing, animated, clock, follow, request_frame,
        set_timeout,
    };
    pub use crate::channel::{UiSender, UiWork, ui_channel};
    pub use crate::drag::{
        DragData, DraggedText, DropEvent, ExternalDrag, GrabEvent, GrabPhase, dragged, start_drag,
    };
    pub use crate::event::{
        ClipboardKind, DragAction, DragActions, DragResult, Key, KeyboardEvent, Modifiers,
        NamedKey, PinchEvent, PinchPhase, PointerButton, PointerEvent, ScrollEvent, ScrollPhase,
        ScrollSource,
    };
    pub use crate::geometry::{PointF, RectF, RectI, Scale};
    pub use crate::platform::{ImePurpose, Placement, PopupKind};
    pub use crate::reactive::{
        Binding, Memo, Owner, Signal, batch, create_root, effect, memo, on_cleanup, signal, untrack,
    };
    pub use crate::settings::{FrameAdvance, ImageSettings, Settings, settings, update_settings};
    pub use crate::shader::{Image, IntoImage, IntoPaint, Shader, ShaderPaint, ShaderParams};
    pub use crate::svg::{self, Svg, svg};
    pub use crate::text::editor::{DecorateContext, Gutter, LineLook, Row, TextEdit, Widget};
    pub use crate::text::rich::{
        Inline, IntoRuns, RichText, Run, Span, VerticalAlign, inline, rich_text, span,
    };
    pub use crate::text::{NotAFont, register_font, register_font_as};
    pub use crate::video::{
        VideoError, VideoLibraries, VideoPlayer, VideoSettings, can_play_video, video_availability,
        video_player,
    };
    pub use crate::view::{
        BitmapView, Div, EditorRef, ForEach, Img, Interaction, IntoImageSource, IntoTextSource,
        IntoVideoSource, IntoView, NodeRef, Popup, Show, Text, TextEditor, TextInput, Video, View,
        bitmap_view, clear_text_selection, close_window, div, dyn_view, editor_ref,
        exit_fullscreen, for_each, fragment, img, node_ref, popup, portal, set_clipboard,
        set_window_fullscreen, shader, show, text, text_area, text_editor, text_input, video,
        window_fullscreen, window_scale,
    };
    pub use anti_webui_style::*;
}
