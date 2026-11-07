use crate::*;

/// Defines [`Style`] (a refinement where every property is optional) and [`ComputedStyle`]
/// (every property resolved) from a single property list.
///
/// `inherited` properties are copied from the parent's computed style before the node's own
/// refinements are applied, like inherited properties in CSS. `merged` properties are inherited
/// too, and refinements add to them instead of replacing them.
macro_rules! define_style {
    ( $( $(#[$m:meta])* $name:ident : $ty:ty = $default:expr , $inherit:ident ; )* ) => {
        /// A style refinement. Only the properties that are `Some` are applied.
        #[derive(Clone, Debug, Default, PartialEq)]
        pub struct Style {
            $( $(#[$m])* pub $name: Option<$ty>, )*
        }

        /// A style with every property resolved, still in logical pixels.
        #[derive(Clone, Debug, PartialEq)]
        #[doc(hidden)]
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
                $( define_style!(@refine $inherit, self, other, $name); )*
            }

            /// Writes the properties set in `self` into `computed`.
            pub fn apply_to(&self, computed: &mut ComputedStyle) {
                $( define_style!(@apply $inherit, self, computed, $name); )*
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
    (@inherit local, $s:ident, $p:ident, $name:ident) => {};
    (@inherit $any:ident, $s:ident, $p:ident, $name:ident) => { $s.$name = $p.$name.clone(); };
    (@refine merged, $s:ident, $o:ident, $name:ident) => {
        if let Some(v) = &$o.$name {
            $s.$name.get_or_insert_default().merge(v);
        }
    };
    (@refine $any:ident, $s:ident, $o:ident, $name:ident) => {
        if let Some(v) = &$o.$name { $s.$name = Some(v.clone()); }
    };
    (@apply merged, $s:ident, $c:ident, $name:ident) => {
        if let Some(v) = &$s.$name { $c.$name.merge(v); }
    };
    (@apply $any:ident, $s:ident, $c:ident, $name:ident) => {
        if let Some(v) = &$s.$name { $c.$name = v.clone(); }
    };
}

define_style! {
    display: Display = Display::Block, local;
    position: Position = Position::Relative, local;
    /// What the inside of a shape in an `svg` is painted with.
    fill_paint: SvgPaint = SvgPaint::Color(Color::BLACK), inherited;
    /// What the outline of a shape in an `svg` is painted with.
    stroke_paint: SvgPaint = SvgPaint::None, inherited;
    /// In the coordinates of the shape.
    stroke_width: f32 = 1.0, inherited;
    fill_opacity: f32 = 1.0, inherited;
    stroke_opacity: f32 = 1.0, inherited;
    fill_rule: FillRule = FillRule::NonZero, inherited;
    line_cap: LineCap = LineCap::Butt, inherited;
    line_join: LineJoin = LineJoin::Miter, inherited;
    miter_limit: f32 = 4.0, inherited;
    dash_array: Option<std::sync::Arc<[f32]>> = None, inherited;
    dash_offset: f32 = 0.0, inherited;
    /// The width of the outline is in pixels of the screen, whatever the shape is scaled by.
    non_scaling_stroke: bool = false, local;
    flex_direction: FlexDirection = FlexDirection::Row, local;
    flex_wrap: FlexWrap = FlexWrap::NoWrap, local;
    align_content: AlignContent = AlignContent::Stretch, local;
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
    /// Above the background color and below the border. See `docs/design/22-images.md`.
    background_image: Option<ImageSource> = None, local;
    background_size: BackgroundSize = BackgroundSize::Auto, local;
    /// Where the picture goes in the padding box. A percentage lines up that point of both.
    background_position: (Length, Length) = (Length::Percent(0.0), Length::Percent(0.0)), local;
    background_repeat: BackgroundRepeat = BackgroundRepeat::Repeat, local;
    /// How a picture fills the content box of `img`.
    object_fit: ObjectFit = ObjectFit::Fill, local;
    object_position: (Length, Length) = (Length::Percent(50.0), Length::Percent(50.0)), local;
    /// How opaque the picture of `img` is, on its own: what is behind it in the box stays. For
    /// fading a picture in over what shows while it loads.
    image_opacity: f32 = 1.0, local;
    image_rendering: ImageRendering = ImageRendering::Auto, inherited;
    /// Width over height. The side that is `auto` follows the other; other than for pictures, the
    /// box still grows to fit its content.
    aspect_ratio: Option<f32> = None, local;
    shadows: Vec<BoxShadow> = Vec::new(), local;
    opacity: f32 = 1.0, local;
    /// Logical pixels: the standard deviation, like CSS. Applies to what the node and its children
    /// paint. See `docs/design/16-filters.md`.
    blur: f32 = 0.0, local;
    /// The same for what is behind the node's box.
    backdrop_blur: f32 = 0.0, local;
    /// Logical pixels. Moves what is painted without affecting layout, like the other transforms.
    translate_x: f32 = 0.0, local;
    translate_y: f32 = 0.0, local;
    /// Physical pixels. How far a `for_each` still has the node to go to its place: on top of
    /// `translate`, whatever that is doing. Not for setting directly.
    move_x: f32 = 0.0, local;
    move_y: f32 = 0.0, local;
    scale_x: f32 = 1.0, local;
    scale_y: f32 = 1.0, local;
    /// Degrees, clockwise.
    rotate: f32 = 0.0, local;
    /// What the transforms keep in place, as a fraction of the border box.
    transform_origin_x: f32 = 0.5, local;
    transform_origin_y: f32 = 0.5, local;
    /// How changes go: for one property, or for all that aren't named later in the list.
    transitions: Vec<(Option<Prop>, Transition)> = Vec::new(), local;
    /// `None` means `auto`. Anything else creates a stacking context.
    z_index: Option<i32> = None, local;
    overflow_x: Overflow = Overflow::Visible, local;
    overflow_y: Overflow = Overflow::Visible, local;
    /// See `docs/design/29-content-visibility.md`.
    content_visibility: ContentVisibility = ContentVisibility::Visible, local;
    /// Logical pixels: the size of the content box while the contents are skipped and their size
    /// isn't remembered.
    contain_intrinsic_width: Option<f32> = None, local;
    contain_intrinsic_height: Option<f32> = None, local;
    /// While the contents are skipped and their size isn't remembered, they are measured with the
    /// text estimated instead of shaped. Not in CSS.
    estimate_contents: bool = false, local;
    /// `None` means `auto`: an I-beam over selectable text, the default arrow elsewhere.
    cursor: Option<CursorIcon> = None, inherited;
    /// Whether this node receives pointer events.
    pointer_events: bool = true, inherited;

    color: Color = Color::from_rgb(0x1f2328), inherited;
    font_family: FontFamily = FontFamily::default(), inherited;
    font_size: f32 = 14.0, inherited;
    font_weight: FontWeight = FontWeight::NORMAL, inherited;
    font_style: FontStyle = FontStyle::Normal, inherited;
    font_axes: FontAxes = FontAxes::default(), merged;
    line_height: LineHeight = LineHeight::default(), inherited;
    letter_spacing: f32 = 0.0, inherited;
    text_align: TextAlign = TextAlign::Start, inherited;
    white_space: WhiteSpace = WhiteSpace::PreWrap, inherited;
    underline: bool = false, inherited;
    strikethrough: bool = false, inherited;
    user_select: UserSelect = UserSelect::Text, inherited;
    selection_color: Color = Color::from_rgba(0x338fff59), inherited;
    /// The color of the text unless this says otherwise, as `caret-color: auto` is in CSS.
    caret_color: Option<Color> = None, inherited;
    scrollbar_color: Color = Color::from_rgba(0x00000055), inherited;
    /// What parts of controls that a sheet marks take instead of their color (CSS `accent-color`).
    accent_color: Option<Color> = None, inherited;
    /// The node or something around it is disabled. Set by the core and not by styles.
    disabled: bool = false, inherited;
}

impl ComputedStyle {
    /// Whether the node is painted through a transform. See `docs/design/12-transform.md`.
    pub fn has_transform(&self) -> bool {
        self.translate_x != 0.0
            || self.translate_y != 0.0
            || self.move_x != 0.0
            || self.move_y != 0.0
            || self.scale_x != 1.0
            || self.scale_y != 1.0
            || self.rotate != 0.0
    }

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
