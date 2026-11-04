use crate::*;

/// What a kind of element looks like in each of its states, like a stylesheet for it: it goes
/// below everything the element is given itself in every state, so `button("Save").bg(red)` stays
/// red while it is hovered, like a browser's stylesheet below those of a page. Elements of a kind
/// can share one. See `docs/design/21-widgets.md`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Sheet {
    pub base: Style,
    pub hover: Option<Style>,
    pub focus: Option<Style>,
    pub focus_visible: Option<Style>,
    pub active: Option<Style>,
    pub disabled: Option<Style>,
    pub accent: AccentParts,
}

/// What takes the [`accent_color`](Styled::accent_color), if there is one, unless it is disabled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AccentParts {
    pub background: bool,
    pub border: bool,
    pub text: bool,
}

impl Sheet {
    pub fn new(base: impl FnOnce(Style) -> Style) -> Self {
        Self {
            base: base(Style::default()),
            ..Self::default()
        }
    }

    /// While the pointer is over it and it isn't disabled.
    pub fn hover(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.hover = Some(f(Style::default()));
        self
    }

    /// While it has the keyboard focus.
    pub fn focus(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.focus = Some(f(Style::default()));
        self
    }

    /// While it has a focus that should be shown: what was reached with the keyboard and not what
    /// was clicked (CSS `:focus-visible`).
    pub fn focus_visible(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.focus_visible = Some(f(Style::default()));
        self
    }

    /// While it is pressed and it isn't disabled.
    pub fn active(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.active = Some(f(Style::default()));
        self
    }

    /// While it or something around it is disabled.
    pub fn disabled(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        self.disabled = Some(f(Style::default()));
        self
    }

    /// The background takes the accent color.
    pub fn accent_background(mut self) -> Self {
        self.accent.background = true;
        self
    }

    /// The border takes the accent color.
    pub fn accent_border(mut self) -> Self {
        self.accent.border = true;
        self
    }

    /// The text takes the accent color.
    pub fn accent_text(mut self) -> Self {
        self.accent.text = true;
        self
    }
}
