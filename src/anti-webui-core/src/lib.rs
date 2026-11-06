//! The platform- and GPU-independent core of AntiWebUI: reactivity, the node tree, style
//! resolution, layout, text, selection, editing and input handling.

pub mod animation;
pub mod channel;
pub mod damage;
pub mod drag;
pub mod event;
pub mod geometry;
pub mod images;
pub mod layout;
pub mod overlay;
pub mod paint;
pub mod platform;
pub mod reactive;
pub mod scene;
mod scroll;
pub mod settings;
pub mod shader;
pub mod svg;
#[cfg(feature = "svg")]
mod svg_file;
pub mod text;
pub mod tree;
pub mod ui;
pub mod video;
pub mod view;
mod visibility;

pub use anti_webui_style as style;
pub use ui::Ui;

pub mod prelude {
    pub use crate::animation::{
        Animatable, Animated, Spring, Timeout, animated, request_frame, set_timeout,
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
    pub use crate::geometry::{PointF, RectI, Scale};
    pub use crate::platform::{ImePurpose, Placement, PopupKind};
    pub use crate::reactive::{
        Memo, Owner, Signal, batch, create_root, effect, memo, on_cleanup, signal, untrack,
    };
    pub use crate::settings::{FrameAdvance, ImageSettings, Settings, settings, update_settings};
    pub use crate::shader::{Image, IntoImage, IntoPaint, Shader, ShaderPaint, ShaderParams};
    pub use crate::svg::{self, Svg, svg};
    pub use crate::text::editor::{DecorateContext, LineLook, Row, Widget};
    pub use crate::text::rich::{
        Inline, IntoRuns, RichText, Run, Span, VerticalAlign, inline, rich_text, span,
    };
    pub use crate::text::{NotAFont, register_font, register_font_as};
    pub use crate::video::{
        VideoError, VideoLibraries, VideoPlayer, VideoSettings, can_play_video, video_availability,
        video_player,
    };
    pub use crate::view::{
        BitmapView, Div, EditorRef, ForEach, Img, IntoImageSource, IntoTextSource, IntoVideoSource,
        IntoView, NodeRef, Popup, Show, Text, TextEditor, TextInput, Video, View, bitmap_view,
        clear_text_selection, div, dyn_view, editor_ref, exit_fullscreen, for_each, fragment, img,
        node_ref, popup, portal, set_clipboard, set_window_fullscreen, shader, show, text,
        text_area, text_editor, text_input, video, window_fullscreen, window_scale,
    };
    pub use anti_webui_style::*;
}
