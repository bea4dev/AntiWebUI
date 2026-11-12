//! What the buttons have in common: the state layer, ripples, focus, shadow and changes of shape.
//! See `docs/design/19-buttons.md`.

use crate::color::{ColorScheme, color_scheme};
use crate::motion::default_effects;
use anti_webui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// The shadows of a height between levels 0 and 5: offset and blur of the key shadow, then offset,
/// blur and spread of the ambient one.
const LEVELS: [[f32; 5]; 5] = [
    [1.0, 2.0, 1.0, 3.0, 1.0],
    [1.0, 2.0, 2.0, 6.0, 2.0],
    [1.0, 3.0, 4.0, 8.0, 3.0],
    [2.0, 3.0, 6.0, 10.0, 4.0],
    [4.0, 4.0, 8.0, 12.0, 6.0],
];

pub(super) fn elevation_shadows(level: f32) -> Vec<BoxShadow> {
    if level <= 0.0 {
        return Vec::new();
    }
    let strength = level.min(1.0);
    let at = (level - 1.0).clamp(0.0, 4.0);
    let low = at.floor() as usize;
    let high = (low + 1).min(4);
    let t = at - low as f32;
    let v = |i: usize| LEVELS[low][i] + (LEVELS[high][i] - LEVELS[low][i]) * t;
    vec![
        BoxShadow {
            offset_y: v(0),
            blur: v(1),
            color: Color::BLACK.with_alpha(0.3 * strength),
            ..Default::default()
        },
        BoxShadow {
            offset_y: v(2),
            blur: v(3),
            spread: v(4),
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
    /// Whether it still changes from `now` on by itself: not once it has grown and come in, and
    /// waits to be released.
    fn moving(&self, now: Instant) -> bool {
        self.released.is_some() || now.saturating_duration_since(self.started) < GROW.max(FADE_IN)
    }

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
        let mut held = false;
        for r in self.list.borrow_mut().iter_mut().flatten() {
            held |= r.released.is_none();
            r.released.get_or_insert(now);
        }
        // One that waited to be released asked for no frames meanwhile.
        if held {
            self.tick(now);
        }
    }

    /// Shows how things are at `now` and, if something is still going on, comes back. A ripple
    /// that waits to be released is not going on: it is drawn as it is until then.
    fn tick(&self, now: Instant) {
        if !self.shown.is_alive() {
            return;
        }
        let mut shown = [[0.0; 4]; 3];
        let mut going_on = false;
        for (slot, out) in self.list.borrow_mut().iter_mut().zip(&mut shown) {
            match slot
                .as_ref()
                .and_then(|r| Some((r.at(now)?, r.moving(now))))
            {
                Some((v, moving)) => {
                    *out = v;
                    going_on |= moving;
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

/// The radii of the corners: top left, top right, bottom right, bottom left.
pub(super) type Corners = [f32; 4];

/// Background, content and border.
pub(super) type Colors = (Color, Color, Color);

/// Whether something is so, which can change.
pub(super) type When = Rc<dyn Fn() -> bool>;

/// The colors of a button, given whether it is checked.
pub(super) type ColorsOf = Rc<dyn Fn(&ColorScheme, bool) -> Colors>;

/// The state layer and the ripples of Material 3: how a component shows that it is under the
/// pointer, has the focus from the keyboard or is pressed. They are caused by one box and drawn on
/// another, which may be the same, in that box's shape (its rounded corners).
///
/// The state layer is 8% of `color` under the pointer and 10% with the focus from the keyboard,
/// and a press makes a ripple of 10% that grows from where it was pressed. Enter and Space make one
/// that grows from the middle. Nothing is drawn, and no frames are asked for, while nothing goes
/// on.
///
/// ```no_run
/// use anti_webui::prelude::*;
/// use anti_webui_md3e::color_scheme;
/// use anti_webui_md3e::components::Ink;
///
/// // A box of its own that reacts like the components do.
/// let ink = Ink::new(|c| c.on_secondary_container, || false);
/// let tile = div()
///     .w(120.0)
///     .h(56.0)
///     .rounded(16.0)
///     .focusable()
///     .dyn_style(|s| s.bg(color_scheme().secondary_container))
///     .on_click(|_| println!("clicked"));
/// let tile = ink.paint(ink.listen(tile));
/// ```
///
/// [`paint`](Self::paint) uses the `overlay` of the box it is given, which goes over the background
/// and under the children; that box can't have another overlay.
#[derive(Clone)]
pub struct Ink {
    /// Whether the pointer is over the box that `listen` was given.
    pub hovered: Signal<bool>,
    /// Whether it has the focus from the keyboard (`:focus-visible`), which may also be set from
    /// outside, for a list whose items the keys move between.
    pub focused: Signal<bool>,
    /// Whether it is pressed by the pointer.
    pub pressed: Signal<bool>,
    state: Animated<f32>,
    ripples: Ripples,
    color: Rc<dyn Fn(&ColorScheme) -> Color>,
    disabled: When,
}

impl Ink {
    /// `color` is usually that of the content. Nothing is drawn while `disabled` returns true.
    pub fn new(
        color: impl Fn(&ColorScheme) -> Color + 'static,
        disabled: impl Fn() -> bool + 'static,
    ) -> Self {
        Self::new_when(color, Rc::new(disabled))
    }

    pub(super) fn new_when(
        color: impl Fn(&ColorScheme) -> Color + 'static,
        disabled: When,
    ) -> Self {
        crate::ensure_ready();
        let state = animated(0.0f32);
        let hovered = signal(false);
        let focused = signal(false);
        let pressed = signal(false);
        let off = disabled.clone();
        let wanted = Cell::new(0.0);
        // What stays as it is starts no animation: a page at rest asks for no frames.
        effect(move || {
            let target = if off() {
                0.0
            } else if focused.get() {
                FOCUS
            } else if hovered.get() {
                HOVER
            } else {
                0.0
            };
            if wanted.replace(target) != target {
                untrack(|| state.spring_to(target, default_effects()));
            }
        });
        Self {
            hovered,
            focused,
            pressed,
            state,
            ripples: Ripples::new(),
            color: Rc::new(color),
            disabled,
        }
    }

    /// Makes the pointer and the keyboard on `node` show: hovering, the focus, and ripples that
    /// start where it is pressed. `node` should be `focusable` for the keyboard to reach it. For a
    /// state layer on another box, use `listen_centered`.
    pub fn listen(&self, node: Div) -> Div {
        let this = self.clone();
        self.listeners(node).on_pointer_down(move |e| {
            if !(this.disabled)() {
                let center = [e.rect.width as f32 / 2.0, e.rect.height as f32 / 2.0];
                this.ripples.press(Some(([e.local.x, e.local.y], center)));
                this.tap_unless_primary(e.button);
            }
        })
    }

    /// Like `listen`, with ripples that grow from the middle of what they are drawn on, for when
    /// that is another box, like the circle around the handle of a switch.
    pub fn listen_centered(&self, node: Div) -> Div {
        let this = self.clone();
        self.listeners(node).on_pointer_down(move |e| {
            if !(this.disabled)() {
                this.ripples.press(None);
                this.tap_unless_primary(e.button);
            }
        })
    }

    /// Only the primary button makes a box active, and the end of that is what releases a ripple.
    /// One that another button starts is released at once, like a tap; it would stay otherwise.
    fn tap_unless_primary(&self, button: Option<PointerButton>) {
        if button != Some(PointerButton::Primary) {
            self.ripples.release();
        }
    }

    fn listeners(&self, node: Div) -> Div {
        let Self {
            hovered,
            focused,
            pressed,
            ..
        } = *self;
        let release = self.ripples.clone();
        let focus_release = self.ripples.clone();
        let key_press = self.ripples.clone();
        let key_release = self.ripples.clone();
        let key_off = self.disabled.clone();
        node.on_hover(move |on| hovered.set(on))
            .on_focus_visible(move |on| focused.set(on))
            // A key is released somewhere else if the focus moved on.
            .on_focus_change(move |on| {
                if !on {
                    focus_release.release();
                }
            })
            .on_active(move |on| {
                pressed.set(on);
                if !on {
                    release.release();
                }
            })
            .on_key_down(move |e| {
                if !e.repeat
                    && matches!(e.key, Key::Named(NamedKey::Enter | NamedKey::Space))
                    && !key_off()
                {
                    key_press.press(None);
                }
            })
            .on_key_up(move |e| {
                if matches!(e.key, Key::Named(NamedKey::Enter | NamedKey::Space)) {
                    key_release.release();
                }
            })
    }

    /// Draws the state layer and the ripples on `node`, in its shape, over its background and
    /// under its children.
    pub fn paint(&self, node: Div) -> Div {
        let shown = self.ripples.shown;
        let state = self.state;
        let off = self.disabled.clone();
        let color = self.color.clone();
        node.overlay(move || {
            let shown = shown.get();
            let level = state.get();
            if off() || (level <= 0.001 && shown.iter().all(|r| r[3] <= 0.0)) {
                return None;
            }
            Some(STATE_LAYER.params(StateLayer {
                color: color(&color_scheme()),
                state: level,
                ripples: shown,
            }))
        })
    }
}

/// 0 or 1, following whether something is so.
pub(super) fn progress_of(on: When, spring: Spring) -> Animated<f32> {
    follow(move || if on() { 1.0 } else { 0.0 }, spring)
}

/// A box that is only there for its state layer: a circle of this diameter around the center of
/// its parent, which has to be `relative`.
pub(super) fn halo(ink: &Ink, diameter: f32) -> Div {
    ink.paint(
        div()
            .absolute()
            .w(diameter)
            .h(diameter)
            .rounded(diameter / 2.0)
            .pointer_events_none(),
    )
}

/// What is around a component that isn't disabled and has the focus from the keyboard: to be used
/// from its style.
pub(super) fn focus_ring(s: Style, focused: bool, c: &ColorScheme) -> Style {
    if focused {
        s.outline(3.0)
            .outline_offset(2.0)
            .outline_color(c.secondary)
    } else {
        s
    }
}

/// Whether something that may not be there is so.
pub(super) fn when(f: Option<When>) -> When {
    Rc::new(move || f.as_ref().is_some_and(|f| f()))
}

/// What a kind of button looks like and how it reacts.
pub(super) struct Surface {
    pub colors: ColorsOf,
    /// The color of the content while disabled.
    pub disabled_content: fn(&ColorScheme) -> Color,
    pub rest: Corners,
    pub pressed: Corners,
    pub checked_shape: Corners,
    /// How the shape changes.
    pub spring: Spring,
    /// How high it is at rest and under the pointer.
    pub levels: (f32, f32),
    /// The width of the border it has, whatever the color of that is at the moment.
    pub outline: f32,
    pub disabled: Option<When>,
    pub checked: Option<When>,
}

/// The box of a button, for the caller to size and fill.
pub(super) fn surface(surface: Surface) -> Div {
    crate::ensure_ready();
    let Surface {
        colors,
        disabled_content,
        rest,
        pressed: pressed_shape,
        checked_shape,
        spring,
        levels: (rest_level, hover_level),
        outline,
        disabled,
        checked,
    } = surface;
    let is_disabled = when(disabled);
    let is_checked = when(checked);

    let at_start = if untrack(|| is_checked()) {
        checked_shape
    } else {
        rest
    };
    let corners = at_start.map(animated);
    let level = animated(rest_level);

    let layer_colors = colors.clone();
    let layer_checked = is_checked.clone();
    let ink = Ink::new_when(
        move |c| layer_colors(c, layer_checked()).1,
        is_disabled.clone(),
    );
    let Ink {
        hovered,
        focused,
        pressed,
        ..
    } = ink;

    // How high it is follows from what is going on.
    let disabled_now = is_disabled.clone();
    let wanted_level = Cell::new(rest_level);
    effect(move || {
        let target = if disabled_now() {
            0.0
        } else if hovered.get() && !pressed.get() {
            hover_level
        } else {
            rest_level
        };
        if wanted_level.replace(target) != target {
            untrack(|| level.spring_to(target, default_effects()));
        }
    });

    let checked_now = is_checked.clone();
    let wanted_shape = Cell::new(at_start);
    effect(move || {
        let target = if pressed.get() {
            pressed_shape
        } else if checked_now() {
            checked_shape
        } else {
            rest
        };
        if wanted_shape.replace(target) != target {
            untrack(|| {
                for (corner, radius) in corners.iter().zip(target) {
                    corner.spring_to(radius, spring);
                }
            });
        }
    });

    let looks = is_disabled.clone();
    let looks_checked = is_checked;
    let root = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_center()
        .select_none()
        .cursor(CursorIcon::Pointer)
        .focusable()
        .access(|a| a.role(Role::Button))
        .dyn_style(move |s| {
            let c = color_scheme();
            let (background, content, border) = colors(&c, looks_checked());
            let off = looks();
            let [top_left, top_right, bottom_right, bottom_left] = corners.map(|r| r.get());
            let mut s = s.shadows(elevation_shadows(level.get()));
            s.radius_top_left = Some(top_left);
            s.radius_top_right = Some(top_right);
            s.radius_bottom_right = Some(bottom_right);
            s.radius_bottom_left = Some(bottom_left);
            let s = if off {
                let s = s
                    .text_color(disabled_content(&c))
                    .cursor(CursorIcon::Default);
                if background.is_transparent() {
                    s
                } else {
                    s.bg(c.on_surface.with_alpha(0.1))
                }
            } else {
                s.bg(background).text_color(content)
            };
            let s = if outline > 0.0 {
                s.border(outline).border_color(border)
            } else {
                s
            };
            focus_ring(s, focused.get() && !off, &c)
        });
    let root = ink.paint(ink.listen(root));
    root.disabled(move || is_disabled())
}

/// All corners the same.
pub(super) fn round(radius: f32) -> Corners {
    [radius; 4]
}

/// An icon that fills in while checked.
pub(super) fn filling_icon(
    symbol: &'static crate::icons::Symbol,
    size: f32,
    checked: Option<When>,
) -> Div {
    let icon = super::sized_icon(symbol, size);
    match checked {
        Some(checked) => icon
            .transition_of([Prop::FontAxes], Transition::ms(200).ease_out())
            .dyn_style(move |s| s.font_axis(b"FILL", if checked() { 1.0 } else { 0.0 })),
        None => icon,
    }
}
