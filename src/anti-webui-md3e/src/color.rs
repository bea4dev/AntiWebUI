//! The color roles of Material 3, with the default schemes of Jetpack Compose, and schemes made
//! from a seed color (see [`crate::dynamic`]).

use crate::dynamic::Hct;
use crate::dynamic::scheme::DynamicScheme;
use anti_webui::prelude::*;
use std::cell::{Cell, RefCell};

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

impl ColorScheme {
    /// The scheme Material You makes from `seed`: [`SchemeVariant::TonalSpot`], at the standard
    /// contrast.
    pub fn from_seed(seed: Color, dark: bool) -> Self {
        Self::from_seed_with(
            seed,
            SchemeOptions {
                dark,
                ..SchemeOptions::default()
            },
        )
    }

    /// ```
    /// use anti_webui::prelude::*;
    /// use anti_webui_md3e::{ColorScheme, SchemeOptions, SchemeVariant};
    ///
    /// let colors = ColorScheme::from_seed_with(
    ///     rgb(0x6750a4),
    ///     SchemeOptions {
    ///         variant: SchemeVariant::Vibrant,
    ///         dark: true,
    ///         ..SchemeOptions::default()
    ///     },
    /// );
    /// ```
    pub fn from_seed_with(seed: Color, options: SchemeOptions) -> Self {
        DynamicScheme::new(
            Hct::from_color(seed),
            options.variant,
            options.dark,
            options.contrast.clamp(-1.0, 1.0),
        )
        .color_scheme()
    }

    /// Each role `t` of the way to `other`'s, the way transitions mix colors. 0 gives `self` and 1
    /// gives `other`, exactly.
    pub fn lerp(&self, other: &ColorScheme, t: f32) -> ColorScheme {
        if t <= 0.0 {
            return *self;
        }
        if t >= 1.0 {
            return *other;
        }
        let m = |a: Color, b: Color| mix_premultiplied(a, b, t);
        ColorScheme {
            primary: m(self.primary, other.primary),
            on_primary: m(self.on_primary, other.on_primary),
            primary_container: m(self.primary_container, other.primary_container),
            on_primary_container: m(self.on_primary_container, other.on_primary_container),
            secondary: m(self.secondary, other.secondary),
            on_secondary: m(self.on_secondary, other.on_secondary),
            secondary_container: m(self.secondary_container, other.secondary_container),
            on_secondary_container: m(self.on_secondary_container, other.on_secondary_container),
            tertiary: m(self.tertiary, other.tertiary),
            on_tertiary: m(self.on_tertiary, other.on_tertiary),
            tertiary_container: m(self.tertiary_container, other.tertiary_container),
            on_tertiary_container: m(self.on_tertiary_container, other.on_tertiary_container),
            error: m(self.error, other.error),
            on_error: m(self.on_error, other.on_error),
            error_container: m(self.error_container, other.error_container),
            on_error_container: m(self.on_error_container, other.on_error_container),
            surface: m(self.surface, other.surface),
            surface_dim: m(self.surface_dim, other.surface_dim),
            surface_bright: m(self.surface_bright, other.surface_bright),
            surface_variant: m(self.surface_variant, other.surface_variant),
            on_surface: m(self.on_surface, other.on_surface),
            on_surface_variant: m(self.on_surface_variant, other.on_surface_variant),
            inverse_surface: m(self.inverse_surface, other.inverse_surface),
            inverse_on_surface: m(self.inverse_on_surface, other.inverse_on_surface),
            inverse_primary: m(self.inverse_primary, other.inverse_primary),
            scrim: m(self.scrim, other.scrim),
            surface_container_lowest: m(
                self.surface_container_lowest,
                other.surface_container_lowest,
            ),
            surface_container_low: m(self.surface_container_low, other.surface_container_low),
            surface_container: m(self.surface_container, other.surface_container),
            surface_container_high: m(self.surface_container_high, other.surface_container_high),
            surface_container_highest: m(
                self.surface_container_highest,
                other.surface_container_highest,
            ),
            outline: m(self.outline, other.outline),
            outline_variant: m(self.outline_variant, other.outline_variant),
        }
    }
}

impl Default for ColorScheme {
    fn default() -> Self {
        Self::light()
    }
}

/// How Material You turns a seed color into palettes. See `docs/design/15-md3e.md`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum SchemeVariant {
    /// Calm, with the seed's hue at a moderate chroma. What Android uses by default.
    #[default]
    TonalSpot,
    /// Close to gray, with a hint of the seed's hue.
    Neutral,
    /// The seed's hue as colorful as it gets.
    Vibrant,
    /// Hues away from the seed's, for a playful look.
    Expressive,
    /// The seed itself as the primary container, with a complementary tertiary.
    Fidelity,
    /// Like `Fidelity`, with an analogous tertiary: for colors taken from content, like an image.
    Content,
    /// Grays only.
    Monochrome,
    /// Colorful accents on gray surfaces.
    Rainbow,
    /// Accents with the hue turned from the seed's, on slightly tinted surfaces.
    FruitSalad,
}

/// What [`ColorScheme::from_seed_with`] makes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SchemeOptions {
    pub variant: SchemeVariant,
    pub dark: bool,
    /// From -1 for less contrast than standard, through 0 for standard, to 1 for the most.
    pub contrast: f64,
}

/// The scheme, and how it is on its way to another one.
#[derive(Clone, Copy)]
struct State {
    scheme: Signal<ColorScheme>,
    /// From 0 at the first of `ENDS` to 1 at the second.
    progress: Animated<f32>,
}

thread_local! {
    static STATE: Cell<Option<State>> = const { Cell::new(None) };
    /// Where a change of scheme goes from and to.
    static ENDS: RefCell<(ColorScheme, ColorScheme)> =
        const { RefCell::new((ColorScheme::light(), ColorScheme::light())) };
}

fn state() -> State {
    STATE.with(|s| {
        s.get().unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever asks for it first.
            let (state, _) = create_root(|| {
                let state = State {
                    scheme: signal(ColorScheme::default()),
                    progress: animated(1.0),
                };
                // Only runs while it moves: then each frame writes the scheme in between.
                effect(move || {
                    let t = state.progress.get().clamp(0.0, 1.0);
                    let (from, to) = ENDS.with(|e| *e.borrow());
                    state.scheme.set_if_changed(from.lerp(&to, t));
                });
                state
            });
            s.set(Some(state));
            state
        })
    })
}

/// The current scheme. Reading it tracks it, so what is built from it follows [`set_color_scheme`]
/// and [`set_color_scheme_animated`].
pub fn color_scheme() -> ColorScheme {
    state().scheme.get()
}

/// Changes the scheme at once, stopping a change that is going on.
pub fn set_color_scheme(colors: ColorScheme) {
    let state = state();
    state.progress.stop();
    ENDS.with(|e| *e.borrow_mut() = (colors, colors));
    state.scheme.set(colors);
}

/// Changes the scheme over a few frames like `spring`, from what it is now, even halfway through
/// another change. Every role is mixed as [`ColorScheme::lerp`] does, and what reads
/// [`color_scheme`] follows each frame. [`crate::motion::default_effects`] suits it; a spring that
/// bounces stops at the ends.
///
/// ```no_run
/// use anti_webui::prelude::*;
/// use anti_webui_md3e::{ColorScheme, motion, set_color_scheme_animated};
///
/// set_color_scheme_animated(ColorScheme::from_seed(rgb(0x386a20), true), motion::slow_effects());
/// ```
pub fn set_color_scheme_animated(colors: ColorScheme, spring: Spring) {
    let state = state();
    let now = state.scheme.get_untracked();
    if now == colors {
        set_color_scheme(colors);
        return;
    }
    ENDS.with(|e| *e.borrow_mut() = (now, colors));
    state.progress.set(0.0);
    state.progress.spring_to(1.0, spring);
}
