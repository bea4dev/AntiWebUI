//! The color roles of Material 3, with the default schemes of Jetpack Compose.

use anti_webui::prelude::*;
use std::cell::Cell;

/// The color roles the components use. Make your own to change how they look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorScheme {
    pub primary: Color,
    pub on_primary: Color,
    pub primary_container: Color,
    pub on_primary_container: Color,
    pub secondary: Color,
    pub on_secondary: Color,
    pub secondary_container: Color,
    pub on_secondary_container: Color,
    pub tertiary: Color,
    pub on_tertiary: Color,
    pub tertiary_container: Color,
    pub on_tertiary_container: Color,
    pub error: Color,
    pub on_error: Color,
    pub error_container: Color,
    pub on_error_container: Color,
    pub surface: Color,
    pub surface_dim: Color,
    pub surface_bright: Color,
    pub surface_variant: Color,
    pub on_surface: Color,
    pub on_surface_variant: Color,
    pub inverse_surface: Color,
    pub inverse_on_surface: Color,
    pub inverse_primary: Color,
    pub scrim: Color,
    pub surface_container_lowest: Color,
    pub surface_container_low: Color,
    pub surface_container: Color,
    pub surface_container_high: Color,
    pub surface_container_highest: Color,
    pub outline: Color,
    pub outline_variant: Color,
}

impl ColorScheme {
    pub const fn light() -> Self {
        Self {
            primary: rgb(0x6750a4),
            on_primary: rgb(0xffffff),
            primary_container: rgb(0xeaddff),
            on_primary_container: rgb(0x21005d),
            secondary: rgb(0x625b71),
            on_secondary: rgb(0xffffff),
            secondary_container: rgb(0xe8def8),
            on_secondary_container: rgb(0x1d192b),
            tertiary: rgb(0x7d5260),
            on_tertiary: rgb(0xffffff),
            tertiary_container: rgb(0xffd8e4),
            on_tertiary_container: rgb(0x31111d),
            error: rgb(0xb3261e),
            on_error: rgb(0xffffff),
            error_container: rgb(0xf9dedc),
            on_error_container: rgb(0x410e0b),
            surface: rgb(0xfef7ff),
            surface_dim: rgb(0xded8e1),
            surface_bright: rgb(0xfef7ff),
            surface_variant: rgb(0xe7e0ec),
            on_surface: rgb(0x1d1b20),
            on_surface_variant: rgb(0x49454f),
            inverse_surface: rgb(0x322f35),
            inverse_on_surface: rgb(0xf5eff7),
            inverse_primary: rgb(0xd0bcff),
            scrim: rgb(0x000000),
            surface_container_lowest: rgb(0xffffff),
            surface_container_low: rgb(0xf7f2fa),
            surface_container: rgb(0xf3edf7),
            surface_container_high: rgb(0xece6f0),
            surface_container_highest: rgb(0xe6e0e9),
            outline: rgb(0x79747e),
            outline_variant: rgb(0xcac4d0),
        }
    }

    pub const fn dark() -> Self {
        Self {
            primary: rgb(0xd0bcff),
            on_primary: rgb(0x381e72),
            primary_container: rgb(0x4f378b),
            on_primary_container: rgb(0xeaddff),
            secondary: rgb(0xccc2dc),
            on_secondary: rgb(0x332d41),
            secondary_container: rgb(0x4a4458),
            on_secondary_container: rgb(0xe8def8),
            tertiary: rgb(0xefb8c8),
            on_tertiary: rgb(0x492532),
            tertiary_container: rgb(0x633b48),
            on_tertiary_container: rgb(0xffd8e4),
            error: rgb(0xf2b8b5),
            on_error: rgb(0x601410),
            error_container: rgb(0x8c1d18),
            on_error_container: rgb(0xf9dedc),
            surface: rgb(0x141218),
            surface_dim: rgb(0x141218),
            surface_bright: rgb(0x3b383e),
            surface_variant: rgb(0x49454f),
            on_surface: rgb(0xe6e0e9),
            on_surface_variant: rgb(0xcac4d0),
            inverse_surface: rgb(0xe6e0e9),
            inverse_on_surface: rgb(0x322f35),
            inverse_primary: rgb(0x6750a4),
            scrim: rgb(0x000000),
            surface_container_lowest: rgb(0x0f0d13),
            surface_container_low: rgb(0x1d1b20),
            surface_container: rgb(0x211f26),
            surface_container_high: rgb(0x2b2930),
            surface_container_highest: rgb(0x36343b),
            outline: rgb(0x938f99),
            outline_variant: rgb(0x49454f),
        }
    }
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::light()
    }
}

thread_local! {
    static SCHEME: Cell<Option<Signal<ColorScheme>>> = const { Cell::new(None) };
}

fn scheme() -> Signal<ColorScheme> {
    SCHEME.with(|s| {
        s.get().unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever asks for it first.
            let (signal, _) = create_root(|| signal(ColorScheme::default()));
            s.set(Some(signal));
            signal
        })
    })
}

/// The current scheme. Reading it tracks it, so what is built from it follows [`set_color_scheme`].
pub fn color_scheme() -> ColorScheme {
    scheme().get()
}

pub fn set_color_scheme(colors: ColorScheme) {
    scheme().set(colors);
}
