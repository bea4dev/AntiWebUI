use crate::*;

/// Defines [`Style`] (a refinement where every property is optional) and [`ComputedStyle`]
/// (every property resolved) from a single property list.
///
/// `inherited` properties are copied from the parent's computed style before the node's own
/// refinements are applied, like inherited properties in CSS.
macro_rules! define_style {
    ( $( $(#[$m:meta])* $name:ident : $ty:ty = $default:expr , $inherit:ident ; )* ) => {
        /// A style refinement. Only the properties that are `Some` are applied.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct Style {
            $( $(#[$m])* pub $name: Option<$ty>, )*
        }

        /// A style with every property resolved, still in logical pixels.
        #[derive(Clone, Debug, PartialEq)]
        pub struct ComputedStyle {
            $( $(#[$m])* pub $name: $ty, )*
        }

        impl Default for ComputedStyle {
            fn default() -> Self {
                Self { $( $name: $default, )* }
            }
        }

        impl Style {
            /// Overrides properties of `self` with the properties set in `other`.
            pub fn refine(&mut self, other: &Style) {
                $( if let Some(v) = &other.$name { self.$name = Some(v.clone()); } )*
            }

            /// Writes the properties set in `self` into `computed`.
            pub fn apply_to(&self, computed: &mut ComputedStyle) {
                $( if let Some(v) = &self.$name { computed.$name = v.clone(); } )*
            }

            pub fn is_empty(&self) -> bool {
                true $( && self.$name.is_none() )*
            }
        }

        impl ComputedStyle {
            /// A default style that inherits the inherited properties of `parent`.
            pub fn inherit_from(parent: &ComputedStyle) -> Self {
                #[allow(unused_mut)]
                let mut style = Self::default();
                $( define_style!(@inherit $inherit, style, parent, $name); )*
                style
            }
        }
    };
    (@inherit inherited, $s:ident, $p:ident, $name:ident) => { $s.$name = $p.$name.clone(); };
    (@inherit local, $s:ident, $p:ident, $name:ident) => {};
}

define_style! {
    display: Display = Display::Block, local;
    position: Position = Position::Relative, local;
    flex_direction: FlexDirection = FlexDirection::Row, local;
    justify_content: JustifyContent = JustifyContent::Start, local;
    align_items: AlignItems = AlignItems::Stretch, local;
    /// `None` means `auto` (use the parent's `align_items`).
    align_self: Option<AlignItems> = None, local;
    flex_grow: f32 = 0.0, local;
    flex_shrink: f32 = 1.0, local;
    flex_basis: Length = Length::Auto, local;
    row_gap: f32 = 0.0, local;
    column_gap: f32 = 0.0, local;

    width: Length = Length::Auto, local;
    height: Length = Length::Auto, local;
    min_width: Length = Length::Auto, local;
    min_height: Length = Length::Auto, local;
    /// `Auto` means no maximum.
    max_width: Length = Length::Auto, local;
    max_height: Length = Length::Auto, local;

    margin_top: Length = Length::Px(0.0), local;
    margin_right: Length = Length::Px(0.0), local;
    margin_bottom: Length = Length::Px(0.0), local;
    margin_left: Length = Length::Px(0.0), local;
    padding_top: f32 = 0.0, local;
    padding_right: f32 = 0.0, local;
    padding_bottom: f32 = 0.0, local;
    padding_left: f32 = 0.0, local;
    inset_top: Length = Length::Auto, local;
    inset_right: Length = Length::Auto, local;
    inset_bottom: Length = Length::Auto, local;
    inset_left: Length = Length::Auto, local;

    border_top_width: f32 = 0.0, local;
    border_right_width: f32 = 0.0, local;
    border_bottom_width: f32 = 0.0, local;
    border_left_width: f32 = 0.0, local;
    border_color: Color = Color::from_rgb(0x808080), local;
    radius_top_left: f32 = 0.0, local;
    radius_top_right: f32 = 0.0, local;
    radius_bottom_right: f32 = 0.0, local;
    radius_bottom_left: f32 = 0.0, local;
    outline_width: f32 = 0.0, local;
    outline_offset: f32 = 0.0, local;
    outline_color: Color = Color::from_rgb(0x2f81f7), local;

    background: Color = Color::TRANSPARENT, local;
    shadows: Vec<BoxShadow> = Vec::new(), local;
    opacity: f32 = 1.0, local;
    /// `None` means `auto`. Anything else creates a stacking context.
    z_index: Option<i32> = None, local;
    overflow_x: Overflow = Overflow::Visible, local;
    overflow_y: Overflow = Overflow::Visible, local;
    /// `None` means `auto`: an I-beam over selectable text, the default arrow elsewhere.
    cursor: Option<CursorIcon> = None, inherited;
    /// Whether this node receives pointer events.
    pointer_events: bool = true, inherited;

    color: Color = Color::from_rgb(0x1f2328), inherited;
    font_family: FontFamily = FontFamily::default(), inherited;
    font_size: f32 = 14.0, inherited;
    font_weight: FontWeight = FontWeight::NORMAL, inherited;
    font_style: FontStyle = FontStyle::Normal, inherited;
    line_height: LineHeight = LineHeight::default(), inherited;
    letter_spacing: f32 = 0.0, inherited;
    text_align: TextAlign = TextAlign::Start, inherited;
    white_space: WhiteSpace = WhiteSpace::PreWrap, inherited;
    underline: bool = false, inherited;
    strikethrough: bool = false, inherited;
    user_select: UserSelect = UserSelect::Text, inherited;
    selection_color: Color = Color::from_rgba(0x338fff59), inherited;
    caret_color: Color = Color::from_rgb(0x1f2328), inherited;
}

impl ComputedStyle {
    pub fn margin(&self) -> Edges<Length> {
        Edges {
            top: self.margin_top,
            right: self.margin_right,
            bottom: self.margin_bottom,
            left: self.margin_left,
        }
    }

    pub fn padding(&self) -> Edges<f32> {
        Edges {
            top: self.padding_top,
            right: self.padding_right,
            bottom: self.padding_bottom,
            left: self.padding_left,
        }
    }

    pub fn inset(&self) -> Edges<Length> {
        Edges {
            top: self.inset_top,
            right: self.inset_right,
            bottom: self.inset_bottom,
            left: self.inset_left,
        }
    }

    pub fn border_widths(&self) -> Edges<f32> {
        Edges {
            top: self.border_top_width,
            right: self.border_right_width,
            bottom: self.border_bottom_width,
            left: self.border_left_width,
        }
    }

    pub fn radii(&self) -> Corners<f32> {
        Corners {
            top_left: self.radius_top_left,
            top_right: self.radius_top_right,
            bottom_right: self.radius_bottom_right,
            bottom_left: self.radius_bottom_left,
        }
    }
}
