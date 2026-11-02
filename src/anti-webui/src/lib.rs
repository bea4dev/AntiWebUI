//! # AntiWebUI
//!
//! A lightweight UI framework that renders with wgpu instead of a web view.
//!
//! ```no_run
//! use anti_webui::prelude::*;
//!
//! fn main() {
//!     App::new().title("Counter").run(|| {
//!         let count = signal(0);
//!         div()
//!             .flex().flex_col().gap(8.0).p(16.0)
//!             .child(text(move || format!("Count: {}", count.get())))
//!             .child(button("+1").on_click(move |_| count.update(|c| *c += 1)))
//!     }).unwrap();
//! }
//! ```
//!
//! Enable the `dynamic_linking` feature during development to link AntiWebUI as one shared
//! library (like Bevy), which makes incremental builds faster.

#[cfg(feature = "dynamic_linking")]
#[allow(unused_imports)]
use anti_webui_dylib;

pub use anti_webui_internal::*;
