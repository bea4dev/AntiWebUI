//! Components that look like Material 3 Expressive, built on the public API of AntiWebUI. The
//! widgets of AntiWebUI itself stay as they are. See `docs/design/15-md3e.md`.
//!
//! ```no_run
//! use anti_webui::prelude::*;
//! use anti_webui_md3e::components::button;
//!
//! fn main() {
//!     App::new()
//!         .run(|| div().flex().p(16.0).child(button("Save").on_click(|_| println!("saved"))))
//!         .unwrap();
//! }
//! ```

pub mod color;
pub mod components;
pub mod motion;
mod shapes;
pub mod typography;

pub use anti_webui_symbols_outlined as icons;
pub use color::{ColorScheme, color_scheme, set_color_scheme};

use std::cell::Cell;

thread_local! {
    static READY: Cell<bool> = const { Cell::new(false) };
}

/// What components need before they are shown. Fonts belong to the thread that uses them.
fn ensure_ready() {
    if READY.replace(true) {
        return;
    }
    #[cfg(feature = "bundled-fonts")]
    {
        static ROBOTO_FLEX: &[u8] = include_bytes!("../fonts/RobotoFlex.ttf");
        anti_webui::prelude::register_font(ROBOTO_FLEX)
            .expect("the font that comes with the crate");
    }
    components::preload_shaders();
}
