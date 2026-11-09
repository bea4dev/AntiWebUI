//! Text fields, filled and outlined, with a label that floats. Sizes, colors and timing are those of
//! Jetpack Compose. See `docs/design/20-md3e-components.md`.

use super::icon::sized_icon;
use super::menu::{MenuItem, MenuStyle, styled_menu};
use super::surface::{When, when};
use crate::color::{ColorScheme, color_scheme};
use crate::icons;
use crate::motion::{fast_effects, fast_spatial, slow_effects};
use crate::typography::{FONT_FAMILY, TextStyle};
use anti_webui::prelude::*;
use std::rc::Rc;

const HEIGHT: f32 = 56.0;
const WIDTH: f32 = 280.0;
const PAD: f32 = 16.0;
const ICON_PAD: f32 = 4.0;
const ICON_BOX: f32 = 48.0;

#[derive(ShaderParams, Clone, Copy)]
struct OutlineField {
    color: Color,
    width: f32,
    radius: f32,
    notch_start: f32,
    notch_end: f32,
}

static OUTLINE: Shader<OutlineField> = Shader::new(
    "outline_field",
    include_str!("../../shaders/outline_field.wgsl"),
);

pub(super) fn preload_shaders() {
    OUTLINE.preload();
}

pub(super) fn check_shader() -> Result<(), String> {
    anti_webui::wgpu::check_shader(&OUTLINE)
}

/// What a text field says and has around the text.
#[derive(Default)]
pub struct TextFieldStyle {
    label: Option<String>,
    placeholder: Option<String>,
    supporting: Option<Rc<dyn Fn() -> String>>,
    leading: Option<View>,
    trailing: Option<View>,
    prefix: Option<String>,
    suffix: Option<String>,
    rows: Option<u32>,
    read_only: bool,
    password: bool,
    on_submit: Option<Rc<dyn Fn(&str)>>,
    error: Option<When>,
    disabled: Option<When>,
}

impl TextFieldStyle {
    pub fn new() -> Self {
        Self::default()
    }

    /// What it is for: in the field while it is empty, above the text otherwise.
    pub fn label(mut self, text: impl Into<String>) -> Self {
        self.label = Some(text.into());
        self
    }

    /// An example, while it is empty and has the focus.
    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = Some(text.into());
        self
    }

    /// A line below the field, which may change.
    pub fn supporting(mut self, f: impl Fn() -> String + 'static) -> Self {
        self.supporting = Some(Rc::new(f));
        self
    }

    /// Usually an icon.
    pub fn leading(mut self, view: impl IntoView) -> Self {
        self.leading = Some(view.into_view());
        self
    }

    /// Usually an icon button.
    pub fn trailing(mut self, view: impl IntoView) -> Self {
        self.trailing = Some(view.into_view());
        self
    }

    /// A short text in front of what is typed.
    pub fn prefix(mut self, text: impl Into<String>) -> Self {
        self.prefix = Some(text.into());
        self
    }

    /// A short text after what is typed.
    pub fn suffix(mut self, text: impl Into<String>) -> Self {
        self.suffix = Some(text.into());
        self
    }

    /// Several lines, at least this many.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = Some(rows);
        self
    }

    pub fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    /// For a password: what is typed shows as dots and can't be copied out.
    pub fn password(mut self) -> Self {
        self.password = true;
        self
    }

    /// Called with the text on Enter (Ctrl+Enter with several rows).
    pub fn on_submit(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_submit = Some(Rc::new(f));
        self
    }

    /// In the error colors while `f` returns true.
    pub fn error_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.error = Some(Rc::new(f));
        self
    }

    /// Doesn't react and looks that way while `f` returns true.
    pub fn disabled_when(mut self, f: impl Fn() -> bool + 'static) -> Self {
        self.disabled = Some(Rc::new(f));
        self
    }
}

/// The colors a text field has now.
#[derive(Clone, Copy, PartialEq)]
struct FieldColors {
    line: Color,
    label: Color,
    input: Color,
    side: Color,
    supporting: Color,
    caret: Color,
}

fn field_colors(
    c: &ColorScheme,
    outlined: bool,
    focused: bool,
    error: bool,
    off: bool,
) -> FieldColors {
    let faded = c.on_surface.with_alpha(0.38);
    if off {
        return FieldColors {
            line: if outlined {
                c.on_surface.with_alpha(0.12)
            } else {
                faded
            },
            label: faded,
            input: faded,
            side: faded,
            supporting: faded,
            caret: c.primary,
        };
    }
    let (line, label) = if error {
        (c.error, c.error)
    } else if focused {
        (c.primary, c.primary)
    } else {
        (
            if outlined {
                c.outline
            } else {
                c.on_surface_variant
            },
            c.on_surface_variant,
        )
    };
    FieldColors {
        line,
        label,
        input: c.on_surface,
        side: c.on_surface_variant,
        supporting: if error { c.error } else { c.on_surface_variant },
        caret: if error { c.error } else { c.primary },
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// A text field on a filled background, bound to `value`.
pub fn text_field(value: Signal<String>, style: TextFieldStyle) -> Div {
    build(value, style, false, None)
}

/// A text field with an outline, bound to `value`.
pub fn outlined_text_field(value: Signal<String>, style: TextFieldStyle) -> Div {
    build(value, style, true, None)
}

fn build(
    value: Signal<String>,
    style: TextFieldStyle,
    outlined: bool,
    field_ref: Option<NodeRef>,
) -> Div {
    crate::ensure_ready();
    let TextFieldStyle {
        label,
        placeholder,
        supporting,
        leading,
        trailing,
        prefix,
        suffix,
        rows,
        read_only,
        password,
        on_submit,
        error,
        disabled,
    } = style;
    let has_leading = leading.is_some();
    let has_trailing = trailing.is_some();
    let has_label = label.is_some();
    let multiline = rows.is_some();
    let is_error = when(error);
    let is_off = when(disabled);
    let focused = signal(false);
    let empty = move || value.with(|v| v.is_empty());

    let floated = follow(
        move || {
            if focused.get() || !empty() {
                1.0f32
            } else {
                0.0
            }
        },
        fast_spatial(),
    );
    let (color_error, color_off) = (is_error.clone(), is_off.clone());
    let colors = move || {
        field_colors(
            &color_scheme(),
            outlined,
            focused.get(),
            color_error(),
            color_off(),
        )
    };
    let line_color = {
        let colors = colors.clone();
        follow(move || colors().line, fast_effects())
    };
    let label_color = {
        let colors = colors.clone();
        follow(move || colors().label, fast_effects())
    };
    let thick_off = is_off.clone();
    let thickness = follow(
        move || {
            if focused.get() && !thick_off() {
                2.0f32
            } else {
                1.0
            }
        },
        fast_spatial(),
    );
    // In the field it stays out of the way until the label has made room.
    let hint = follow(
        move || {
            if !empty() {
                0.0f32
            } else if focused.get() || !has_label {
                1.0
            } else {
                0.0
            }
        },
        if has_label {
            fast_effects()
        } else {
            slow_effects()
        },
    );
    let affix = follow(
        move || {
            if focused.get() || !empty() || !has_label {
                1.0f32
            } else {
                0.0
            }
        },
        fast_effects(),
    );

    let start_space = if has_leading {
        ICON_BOX + ICON_PAD
    } else {
        PAD
    };
    let end_space = if has_trailing {
        ICON_BOX + ICON_PAD
    } else {
        PAD
    };
    let (input_top, input_bottom) = if outlined || !has_label {
        (PAD, PAD)
    } else {
        (24.0, 8.0)
    };

    let mut input = if let Some(rows) = rows {
        text_area().rows(rows)
    } else {
        text_input()
    };
    input = TextStyle::BODY_LARGE.apply(input);
    let input_off = is_off.clone();
    let input_colors = colors.clone();
    let told = label.clone();
    if password {
        input = input.purpose(ImePurpose::Password);
    }
    if let Some(f) = on_submit {
        input = input.on_submit(move |s| f(s));
    }
    let mut input = input
        .value(value)
        .read_only(read_only)
        .access(move |a| match &told {
            Some(label) => a.label(label.clone()),
            None => a,
        })
        .flex_grow(1.0)
        .flex_basis(0.0)
        .min_w(0.0)
        .pt(input_top)
        .pb(input_bottom)
        .bg(Color::TRANSPARENT)
        .dyn_style(move |s| {
            let c = input_colors();
            let s = s
                .text_color(c.input)
                .caret_color(c.caret)
                .selection_color(color_scheme().primary.with_alpha(0.4));
            if input_off() {
                s.pointer_events_none()
            } else {
                s
            }
        })
        .on_focus_change(move |on| focused.set(on));
    if let Some(field_ref) = field_ref {
        input = input.node_ref(field_ref);
    }

    let row_text = {
        let side = colors.clone();
        move |line: String| {
            let side = side.clone();
            TextStyle::BODY_LARGE
                .apply(
                    div()
                        .flex_none()
                        .pt(input_top)
                        .pb(input_bottom)
                        .child(text(line)),
                )
                .dyn_style(move |s| {
                    s.text_color(side().side)
                        .opacity(affix.get().clamp(0.0, 1.0))
                })
        }
    };
    let middle = div()
        .relative()
        .flex()
        .flex_row()
        .flex_grow(1.0)
        .flex_basis(0.0)
        .min_w(0.0)
        .ml(start_space)
        .mr(end_space)
        .children(prefix.map(|p| row_text.clone()(p).mr(2.0)))
        .child(input)
        .children(suffix.map(|p| row_text(p).ml(2.0)))
        .children(placeholder.map(|line| {
            let side = colors.clone();
            TextStyle::BODY_LARGE
                .apply(div().absolute().left(0.0).top(input_top).child(text(line)))
                .pointer_events_none()
                .dyn_style(move |s| {
                    s.text_color(side().side)
                        .opacity(hint.get().clamp(0.0, 1.0))
                })
        }));

    let label_width = signal(0.0f32);
    let floating = label.map(|line| {
        div()
            .absolute()
            .font_family(FONT_FAMILY)
            .font_weight(FontWeight(400))
            .whitespace_nowrap()
            .pointer_events_none()
            .on_resized(move |w, _| label_width.set(w))
            .dyn_style(move |s| {
                let p = floated.get();
                let size = lerp(16.0, 12.0, p.clamp(0.0, 1.2));
                let height = lerp(24.0, 16.0, p);
                let start_y = (HEIGHT - 24.0) / 2.0;
                let end_y = if outlined { -8.0 } else { 8.0 };
                let start_x = start_space;
                let end_x = if outlined { PAD } else { start_space };
                let y = if multiline {
                    lerp(if outlined { PAD } else { 16.0 }, end_y, p)
                } else {
                    lerp(start_y, end_y, p)
                };
                s.text_size(size)
                    .line_height(LineHeight::Px(height))
                    .letter_spacing(lerp(0.5, 0.4, p))
                    .left(lerp(start_x, end_x, p))
                    .top(y)
                    .text_color(label_color.get())
            })
            .child(text(line))
    });

    let field_off = is_off.clone();
    let field = div()
        .relative()
        .flex()
        .flex_row()
        .flex_none()
        .items_stretch()
        .w_full()
        .min_h(HEIGHT)
        .cursor(CursorIcon::Text)
        .dyn_style(move |s| {
            let s = if outlined {
                s.rounded(4.0)
            } else {
                s.rounded_t(4.0)
                    .bg(color_scheme().surface_container_highest)
            };
            if field_off() {
                s.cursor(CursorIcon::Default)
            } else {
                s
            }
        });
    let side_box = |view: View, colors: Rc<dyn Fn() -> FieldColors>, left: bool| {
        let mut slot = div()
            .absolute()
            .top(0.0)
            .bottom(0.0)
            .w(ICON_BOX)
            .flex()
            .items_center()
            .justify_center()
            .cursor(CursorIcon::Default)
            .dyn_style(move |s| s.text_color(colors().side))
            .child(view);
        let style = slot.style_mut();
        if left {
            style.inset_left = Some(Length::Px(ICON_PAD));
        } else {
            style.inset_right = Some(Length::Px(ICON_PAD));
        }
        slot
    };
    let rc_colors: Rc<dyn Fn() -> FieldColors> = Rc::new(colors.clone());
    let field = field
        .child(middle)
        .children(floating)
        .children(leading.map(|v| side_box(v, rc_colors.clone(), true)))
        .children(trailing.map(|v| side_box(v, rc_colors.clone(), false)));
    let field = if outlined {
        field.overlay(move || {
            let p = floated.get().max(0.0);
            let w = label_width.get() * p;
            let (notch_start, notch_end) = if has_label && p > 0.0 {
                (PAD - 4.0, PAD + w + 4.0)
            } else {
                (0.0, 0.0)
            };
            OUTLINE.params(OutlineField {
                color: line_color.get(),
                width: thickness.get(),
                radius: 4.0,
                notch_start,
                notch_end,
            })
        })
    } else {
        field.child(
            div()
                .absolute()
                .left(0.0)
                .right(0.0)
                .bottom(0.0)
                .pointer_events_none()
                .dyn_style(move |s| s.h(thickness.get()).bg(line_color.get())),
        )
    };
    let below = supporting.map(|line| {
        let side = colors.clone();
        TextStyle::BODY_SMALL
            .apply(div().px(PAD).pt(4.0).child(text(move || line())))
            .dyn_style(move |s| s.text_color(side().supporting))
    });
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(WIDTH)
        .child(field)
        .children(below)
}

/// A text field that opens a menu to choose from: the exposed dropdown menu of Compose.
pub fn dropdown_field(
    value: Signal<String>,
    options: Vec<String>,
    style: TextFieldStyle,
    outlined: bool,
) -> Div {
    let open = signal(false);
    let turn = follow(
        move || if open.get() { 180.0f32 } else { 0.0 },
        fast_spatial(),
    );
    let anchor = node_ref();
    let items: Vec<MenuItem> = options
        .into_iter()
        .map(|option| {
            let shown = option.clone();
            let chosen = option.clone();
            super::menu::menu_item(option)
                .selected_when(move || value.with(|v| *v == shown))
                .on_click(move || {
                    value.set(chosen.clone());
                    open.set(false);
                })
        })
        .collect();
    let style = TextFieldStyle {
        read_only: true,
        trailing: Some(
            sized_icon(icons::ARROW_DROP_DOWN, 24.0)
                .dyn_style(move |s| s.rotate(turn.get()))
                .into_view(),
        ),
        ..style
    };
    build(value, style, outlined, None)
        .node_ref(anchor)
        .cursor(CursorIcon::Pointer)
        .on_pointer_down(move |_| open.update(|o| *o = !*o))
        .child(styled_menu(
            move || open.get(),
            move || open.set(false),
            vec![items],
            MenuStyle::new().anchor(anchor).match_anchor_width(),
        ))
}
