//! Buttons. Sizes, colors and timing are those of Jetpack Compose. See `docs/design/15-md3e.md`.

use crate::color::{ColorScheme, color_scheme};
use crate::motion::default_effects;
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonKind {
    Filled,
    Tonal,
    Elevated,
    Outlined,
    Text,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ButtonSize {
    ExtraSmall,
    #[default]
    Small,
    Medium,
    Large,
    ExtraLarge,
}

struct Metrics {
    height: f32,
    padding: f32,
    square_radius: f32,
    pressed_radius: f32,
    outline: f32,
    text: TextStyle,
}

impl ButtonSize {
    fn metrics(self) -> Metrics {
        let (height, padding, square_radius, pressed_radius, outline, text) = match self {
            ButtonSize::ExtraSmall => (32.0, 16.0, 12.0, 8.0, 1.0, TextStyle::LABEL_LARGE),
            ButtonSize::Small => (40.0, 16.0, 12.0, 8.0, 1.0, TextStyle::LABEL_LARGE),
            ButtonSize::Medium => (56.0, 24.0, 16.0, 12.0, 1.0, TextStyle::TITLE_MEDIUM),
            ButtonSize::Large => (96.0, 48.0, 28.0, 16.0, 2.0, TextStyle::HEADLINE_SMALL),
            ButtonSize::ExtraLarge => (136.0, 64.0, 28.0, 16.0, 3.0, TextStyle::HEADLINE_LARGE),
        };
        Metrics {
            height,
            padding,
            square_radius,
            pressed_radius,
            outline,
            text,
        }
    }
}

/// How a button looks.
#[derive(Clone)]
pub struct ButtonStyle {
    kind: ButtonKind,
    size: ButtonSize,
    square: bool,
    disabled: Option<Rc<dyn Fn() -> bool>>,
}

impl ButtonStyle {
    pub fn new(kind: ButtonKind) -> Self {
        Self {
            kind,
            size: ButtonSize::default(),
            square: false,
            disabled: None,
        }
    }

    pub fn filled() -> Self {
        Self::new(ButtonKind::Filled)
    }

    pub fn tonal() -> Self {
        Self::new(ButtonKind::Tonal)
    }

    pub fn elevated() -> Self {
        Self::new(ButtonKind::Elevated)
    }

    pub fn outlined() -> Self {
        Self::new(ButtonKind::Outlined)
    }

    pub fn text() -> Self {
        Self::new(ButtonKind::Text)
    }

    pub fn size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }

    /// Corners that are only rounded, rather than a pill.
    pub fn square(mut self) -> Self {
        self.square = true;
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// The colors of a kind of button: background, label and border.
fn colors(kind: ButtonKind, c: &ColorScheme) -> (Color, Color, Color) {
    let none = Color::TRANSPARENT;
    match kind {
        ButtonKind::Filled => (c.primary, c.on_primary, none),
        ButtonKind::Tonal => (c.secondary_container, c.on_secondary_container, none),
        ButtonKind::Elevated => (c.surface_container_low, c.primary, none),
        ButtonKind::Outlined => (none, c.on_surface_variant, c.outline_variant),
        ButtonKind::Text => (none, c.primary, none),
    }
}

/// How high a kind of button is at rest and under the pointer.
fn elevations(kind: ButtonKind) -> (f32, f32) {
    match kind {
        ButtonKind::Filled | ButtonKind::Tonal => (0.0, 1.0),
        ButtonKind::Elevated => (1.0, 2.0),
        ButtonKind::Outlined | ButtonKind::Text => (0.0, 0.0),
    }
}

/// The shadows of a height between levels 0 and 2.
pub(crate) fn elevation_shadows(level: f32) -> Vec<BoxShadow> {
    if level <= 0.0 {
        return Vec::new();
    }
    let up = (level - 1.0).clamp(0.0, 1.0);
    let strength = level.min(1.0);
    vec![
        BoxShadow {
            offset_y: 1.0,
            blur: 2.0,
            color: Color::BLACK.with_alpha(0.3 * strength),
            ..Default::default()
        },
        BoxShadow {
            offset_y: 1.0 + up,
            blur: 3.0 + 3.0 * up,
            spread: 1.0 + up,
            color: Color::BLACK.with_alpha(0.15 * strength),
            ..Default::default()
        },
    ]
}

#[derive(ShaderParams, Clone, Copy)]
struct StateLayer {
    color: Color,
    state: f32,
    ripples: [[f32; 4]; 3],
}

static STATE_LAYER: Shader<StateLayer> = Shader::new(
    "state_layer",
    include_str!("../../shaders/state_layer.wgsl"),
);

pub(super) fn preload_shaders() {
    STATE_LAYER.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&STATE_LAYER)
}

const HOVER: f32 = 0.08;
const FOCUS: f32 = 0.10;
const PRESS: f32 = 0.10;
const FADE_IN: Duration = Duration::from_millis(75);
const GROW: Duration = Duration::from_millis(225);
const FADE_OUT: Duration = Duration::from_millis(150);

#[derive(Clone, Copy)]
struct Ripple {
    /// Where it starts and where it goes, unless it stays in the center of the box.
    path: Option<([f32; 2], [f32; 2])>,
    started: Instant,
    released: Option<Instant>,
}

impl Ripple {
    /// `(x, y, how far it has grown, how strong it is)`, or `None` when it is over.
    fn at(&self, now: Instant) -> Option<[f32; 4]> {
        let elapsed = now.saturating_duration_since(self.started);
        let grown = (elapsed.as_secs_f32() / GROW.as_secs_f32()).min(1.0);
        let mut strength = (elapsed.as_secs_f32() / FADE_IN.as_secs_f32()).min(1.0);
        if let Some(released) = self.released {
            // It goes once it has grown, however short the press was.
            let fading = released.max(self.started + GROW);
            let faded = now.saturating_duration_since(fading).as_secs_f32();
            strength *= 1.0 - faded / FADE_OUT.as_secs_f32();
            if strength <= 0.0 {
                return None;
            }
        }
        let (x, y) = self.path.map_or((-1.0, -1.0), |(from, to)| {
            (
                from[0] + (to[0] - from[0]) * grown,
                from[1] + (to[1] - from[1]) * grown,
            )
        });
        Some([
            x,
            y,
            Easing::CubicBezier(0.4, 0.0, 0.2, 1.0).apply(grown),
            strength * PRESS,
        ])
    }
}

/// The ripples of a button, which move on by themselves until they are over.
#[derive(Clone)]
struct Ripples {
    list: Rc<RefCell<[Option<Ripple>; 3]>>,
    shown: Signal<[[f32; 4]; 3]>,
    ticking: Rc<Cell<bool>>,
}

impl Ripples {
    fn new() -> Self {
        Self {
            list: Rc::default(),
            shown: signal([[0.0; 4]; 3]),
            ticking: Rc::default(),
        }
    }

    /// Starts a ripple that goes from a point to the center of the box, or that is there all along.
    fn press(&self, path: Option<([f32; 2], [f32; 2])>) {
        let now = Instant::now();
        let ripple = Ripple {
            path,
            started: now,
            released: None,
        };
        {
            let mut list = self.list.borrow_mut();
            let slot = list.iter().position(Option::is_none).unwrap_or_else(|| {
                // The oldest makes room.
                (0..list.len())
                    .min_by_key(|i| list[*i].map(|r| r.started))
                    .unwrap_or(0)
            });
            list[slot] = Some(ripple);
        }
        self.tick(now);
    }

    fn release(&self) {
        let now = Instant::now();
        for r in self.list.borrow_mut().iter_mut().flatten() {
            r.released.get_or_insert(now);
        }
    }

    /// Shows how things are at `now` and, if something is still going on, comes back.
    fn tick(&self, now: Instant) {
        if !self.shown.is_alive() {
            return;
        }
        let mut shown = [[0.0; 4]; 3];
        let mut going_on = false;
        for (slot, out) in self.list.borrow_mut().iter_mut().zip(&mut shown) {
            match slot.as_ref().and_then(|r| r.at(now)) {
                Some(v) => {
                    *out = v;
                    going_on = true;
                }
                None => *slot = None,
            }
        }
        self.shown.set(shown);
        if going_on && !self.ticking.replace(true) {
            let this = self.clone();
            request_frame(move |now| {
                this.ticking.set(false);
                this.tick(now);
            });
        }
    }
}

/// A filled button in the default size: the one for the main action.
pub fn button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::filled())
}

pub fn tonal_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::tonal())
}

pub fn elevated_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::elevated())
}

pub fn outlined_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::outlined())
}

pub fn text_button(label: impl IntoTextSource) -> Div {
    styled_button(label, ButtonStyle::text())
}

/// A button of any kind and size.
pub fn styled_button(label: impl IntoTextSource, style: ButtonStyle) -> Div {
    crate::ensure_ready();
    let ButtonStyle {
        kind,
        size,
        square,
        disabled,
    } = style;
    let metrics = size.metrics();
    let is_disabled = move || disabled.as_ref().is_some_and(|f| f());
    let is_disabled = Rc::new(is_disabled);
    let rest_radius = if square {
        metrics.square_radius
    } else {
        metrics.height / 2.0
    };
    let pressed_radius = metrics.pressed_radius;
    let outline = metrics.outline;
    let (rest_level, hover_level) = elevations(kind);

    let radius = animated(rest_radius);
    let level = animated(rest_level);
    let state = animated(0.0f32);
    let hovered = signal(false);
    let focused = signal(false);
    let ripples = Ripples::new();

    // How strong the state layer and the shadow should be follow from what is going on. What stays
    // as it is starts no animation: a page of buttons at rest asks for no frames.
    let disabled_now = is_disabled.clone();
    let wanted = Cell::new((0.0, rest_level));
    effect(move || {
        let off = disabled_now();
        let target_state = if off {
            0.0
        } else if focused.get() {
            FOCUS
        } else if hovered.get() {
            HOVER
        } else {
            0.0
        };
        let target_level = if off {
            0.0
        } else if hovered.get() {
            hover_level
        } else {
            rest_level
        };
        let (last_state, last_level) = wanted.replace((target_state, target_level));
        untrack(|| {
            if target_state != last_state {
                state.spring_to(target_state, default_effects());
            }
            if target_level != last_level {
                level.spring_to(target_level, default_effects());
            }
        });
    });

    let looks = is_disabled.clone();
    let layer_off = is_disabled.clone();
    let press_off = is_disabled.clone();
    let key_off = is_disabled.clone();
    let press_ripples = ripples.clone();
    let release_ripples = ripples.clone();
    let key_ripples = ripples.clone();
    let key_up_ripples = ripples.clone();
    let focus_ripples = ripples.clone();
    let shown_ripples = ripples.shown;
    let root = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(8.0)
        .h(metrics.height)
        .px(metrics.padding)
        .select_none()
        .cursor(CursorIcon::Pointer)
        .focusable()
        .dyn_style(move |s| {
            let c = color_scheme();
            let (background, label, border) = colors(kind, &c);
            let off = looks();
            let s = s
                .rounded(radius.get())
                .shadows(elevation_shadows(level.get()));
            let s = if off {
                let label = c.on_surface_variant;
                let s = s
                    .text_color(label.with_alpha(0.38))
                    .cursor(CursorIcon::Default);
                if background.is_transparent() {
                    s
                } else {
                    s.bg(c.on_surface.with_alpha(0.1))
                }
            } else {
                s.bg(background).text_color(label)
            };
            let s = if border.is_transparent() {
                s
            } else {
                s.border(outline).border_color(border)
            };
            if focused.get() && !off {
                s.outline(3.0)
                    .outline_offset(2.0)
                    .outline_color(c.secondary)
            } else {
                s
            }
        })
        .overlay(move || {
            let shown = shown_ripples.get();
            let level = state.get();
            if layer_off() || (level <= 0.001 && shown.iter().all(|r| r[3] <= 0.0)) {
                return None;
            }
            let (_, label, _) = colors(kind, &color_scheme());
            Some(STATE_LAYER.params(StateLayer {
                color: label,
                state: level,
                ripples: shown,
            }))
        })
        .on_hover(move |on| hovered.set(on))
        .on_focus_visible(move |on| focused.set(on))
        // A key is released somewhere else if the focus moved on.
        .on_focus_change(move |on| {
            if !on {
                focus_ripples.release();
            }
        })
        .on_pointer_down(move |e| {
            if !press_off() {
                let center = [e.rect.width as f32 / 2.0, e.rect.height as f32 / 2.0];
                press_ripples.press(Some(([e.local.x, e.local.y], center)));
            }
        })
        .on_active(move |pressed| {
            let target = if pressed { pressed_radius } else { rest_radius };
            radius.spring_to(target, default_effects());
            if !pressed {
                release_ripples.release();
            }
        })
        .on_key_down(move |e| {
            if !e.repeat
                && matches!(e.key, Key::Named(NamedKey::Enter | NamedKey::Space))
                && !key_off()
            {
                key_ripples.press(None);
            }
        })
        .on_key_up(move |e| {
            if matches!(e.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
                key_up_ripples.release();
            }
        })
        .child(metrics.text.apply(text(label)));
    let off = is_disabled;
    root.disabled(move || off())
}
