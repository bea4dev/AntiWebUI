//! Basic widgets built on the public core API. See `docs/design/21-widgets.md`.
//!
//! Every widget returns a regular builder (`Div`, `TextInput` or `Show`), so it can be styled and
//! extended further with the usual methods. What is said about an element itself goes above what
//! the theme says about it, in every state, as the styles of a page go above those of a browser.

mod button;
mod choice;
mod dialog;
mod display;
mod field;
mod image;
mod layout;
mod long;
mod menu;
mod select;
mod slider;
mod sortable;
mod structure;
mod theme;
mod toast;
mod tooltip;

pub use anti_webui_core::view::IntoTextSource;
pub use button::*;
pub use choice::*;
pub use dialog::*;
pub use display::*;
pub use field::*;
pub use image::*;
pub use layout::*;
pub use long::*;
pub use menu::{
    MenuChoice, MenuEntry, MenuItem, MenuOptions, MenuParts, MenuState, context_menu,
    dropdown_button, menu, menu_item, menu_separator, menu_with, styled_menu, submenu,
};
pub use select::*;
pub use slider::*;
pub use sortable::*;
pub use structure::*;
pub use theme::{Theme, focus_ring, page, set_theme, theme, themed};
pub use toast::*;
pub use tooltip::*;
