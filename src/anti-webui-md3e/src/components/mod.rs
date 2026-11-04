//! The components. Each returns an ordinary builder that can be styled further.

mod button;
mod icon;
mod loading;
mod progress;

pub use button::{
    ButtonKind, ButtonSize, ButtonStyle, button, elevated_button, outlined_button, styled_button,
    text_button, tonal_button,
};
pub use icon::{icon, sized_icon};
pub use loading::loading_indicator;
pub use progress::wavy_progress;

/// Compiles the shaders of the components, so that the first press doesn't have to wait for that.
pub(crate) fn preload_shaders() {
    button::preload_shaders();
    loading::preload_shaders();
    progress::preload_shaders();
}

/// The shaders of the components, for checking them without a GPU.
#[doc(hidden)]
pub fn check_shaders() -> Result<(), String> {
    button::check_shader()?;
    loading::check_shader()?;
    progress::check_shader()
}
