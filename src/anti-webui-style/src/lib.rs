//! Style types for AntiWebUI.
//!
//! Every length in this crate is a *logical* pixel value (`f32`). Conversion to physical
//! integer pixels happens later in `anti-webui-core`, once the buffer size and scale are known
//! (see `docs/design/01-pixel-model.md`).

mod color;
mod style;
mod styled;
mod transition;
mod types;

pub use color::*;
pub use style::*;
pub use styled::*;
pub use transition::*;
pub use types::*;
