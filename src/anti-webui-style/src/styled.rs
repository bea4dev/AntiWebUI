use crate::*;

macro_rules! setter {
    ($( $(#[$m:meta])* $method:ident => $($field:ident),+ : $ty:ty ;)*) => {
        $(
            $(#[$m])*
            fn $method(mut self, v: impl Into<$ty>) -> Self {
                let v: $ty = v.into();
                let style = self.style_mut();
                $( style.$field = Some(v.clone()); )+
                self
            }
        )*
    };
}

macro_rules! flag {
    ($( $(#[$m:meta])* $method:ident => $field:ident = $value:expr ;)*) => {
        $(
            $(#[$m])*
            fn $method(mut self) -> Self {
                self.style_mut().$field = Some($value);
                self
            }
        )*
    };
}

/// Builder methods for anything that carries a [`Style`].
///
/// Implemented by [`Style`] itself and by element builders, so state refinements can be written
/// the same way: `div().bg(a).hover(|s| s.bg(b))`.
pub trait Styled: Sized {
    fn style_mut(&mut self) -> &mut Style;

    /// Applies every property set in `style`.
    fn style(mut self, style: &Style) -> Self {
        self.style_mut().refine(style);
        self
    }

    /// Sets an axis of a variable font, in the units of the font: `.font_axis(b"GRAD", 50.0)`. A
    /// font without it ignores it. The other axes stay as they are.
    fn font_axis(mut self, axis: impl Into<FontAxis>, value: f32) -> Self {
        self.style_mut()
            .font_axes
            .get_or_insert_default()
            .set(axis.into(), value);
        self
    }

    setter! {
        w => width: Length;
        h => height: Length;
        size => width, height: Length;
        min_w => min_width: Length;
        min_h => min_height: Length;
        max_w => max_width: Length;
        max_h => max_height: Length;

        p => padding_top, padding_right, padding_bottom, padding_left: f32;
        px => padding_left, padding_right: f32;
        py => padding_top, padding_bottom: f32;
        pt => padding_top: f32;
        pr => padding_right: f32;
        pb => padding_bottom: f32;
        pl => padding_left: f32;

        m => margin_top, margin_right, margin_bottom, margin_left: Length;
        mx => margin_left, margin_right: Length;
        my => margin_top, margin_bottom: Length;
        mt => margin_top: Length;
        mr => margin_right: Length;
        mb => margin_bottom: Length;
        ml => margin_left: Length;

        inset => inset_top, inset_right, inset_bottom, inset_left: Length;
        top => inset_top: Length;
        right => inset_right: Length;
        bottom => inset_bottom: Length;
        left => inset_left: Length;

        gap => row_gap, column_gap: f32;
        row_gap => row_gap: f32;
        column_gap => column_gap: f32;
        flex_grow => flex_grow: f32;
        flex_shrink => flex_shrink: f32;
        flex_basis => flex_basis: Length;

        border => border_top_width, border_right_width, border_bottom_width, border_left_width: f32;
        border_t => border_top_width: f32;
        border_r => border_right_width: f32;
        border_b => border_bottom_width: f32;
        border_l => border_left_width: f32;
        border_color => border_color: Color;
        rounded => radius_top_left, radius_top_right, radius_bottom_right, radius_bottom_left: f32;
        rounded_t => radius_top_left, radius_top_right: f32;
        rounded_b => radius_bottom_left, radius_bottom_right: f32;
        outline => outline_width: f32;
        outline_offset => outline_offset: f32;
        outline_color => outline_color: Color;

        bg => background: Color;
        opacity => opacity: f32;
        blur => blur: f32;
        backdrop_blur => backdrop_blur: f32;

        translate_x => translate_x: f32;
        translate_y => translate_y: f32;
        scale => scale_x, scale_y: f32;
        scale_x => scale_x: f32;
        scale_y => scale_y: f32;
        /// Degrees, clockwise.
        rotate => rotate: f32;

        text_color => color: Color;
        text_size => font_size: f32;
        font_family => font_family: FontFamily;
        font_weight => font_weight: FontWeight;
        line_height => line_height: LineHeight;
        letter_spacing => letter_spacing: f32;
        text_align => text_align: TextAlign;
        selection_color => selection_color: Color;
        caret_color => caret_color: Color;
    }

    flag! {
        block => display = Display::Block;
        flex => display = Display::Flex;
        hidden => display = Display::None;
        flex_row => flex_direction = FlexDirection::Row;
        flex_col => flex_direction = FlexDirection::Column;
        flex_row_reverse => flex_direction = FlexDirection::RowReverse;
        flex_col_reverse => flex_direction = FlexDirection::ColumnReverse;
        /// `flex: 1 1 0`
        flex_1 => flex_grow = 1.0;
        flex_none => flex_shrink = 0.0;
        items_start => align_items = AlignItems::Start;
        items_end => align_items = AlignItems::End;
        items_center => align_items = AlignItems::Center;
        items_stretch => align_items = AlignItems::Stretch;
        self_start => align_self = Some(AlignItems::Start);
        self_end => align_self = Some(AlignItems::End);
        self_center => align_self = Some(AlignItems::Center);
        self_stretch => align_self = Some(AlignItems::Stretch);
        justify_start => justify_content = JustifyContent::Start;
        justify_end => justify_content = JustifyContent::End;
        justify_center => justify_content = JustifyContent::Center;
        justify_between => justify_content = JustifyContent::SpaceBetween;
        justify_around => justify_content = JustifyContent::SpaceAround;
        justify_evenly => justify_content = JustifyContent::SpaceEvenly;
        relative => position = Position::Relative;
        absolute => position = Position::Absolute;
        w_full => width = Length::Percent(100.0);
        h_full => height = Length::Percent(100.0);
        overflow_hidden => overflow_x = Overflow::Hidden;
        italic => font_style = FontStyle::Italic;
        bold => font_weight = FontWeight::BOLD;
        underline => underline = true;
        strikethrough => strikethrough = true;
        whitespace_nowrap => white_space = WhiteSpace::Pre;
        select_none => user_select = UserSelect::None;
        select_text => user_select = UserSelect::Text;
        pointer_events_none => pointer_events = false;
        pointer_events_auto => pointer_events = true;
    }

    fn cursor(mut self, icon: CursorIcon) -> Self {
        self.style_mut().cursor = Some(Some(icon));
        self
    }

    fn size_full(mut self) -> Self {
        let style = self.style_mut();
        style.width = Some(Length::Percent(100.0));
        style.height = Some(Length::Percent(100.0));
        self
    }

    fn shadow(mut self, shadow: BoxShadow) -> Self {
        self.style_mut().shadows = Some(vec![shadow]);
        self
    }

    fn shadows(mut self, shadows: Vec<BoxShadow>) -> Self {
        self.style_mut().shadows = Some(shadows);
        self
    }

    /// A soft, small elevation shadow.
    fn shadow_sm(self) -> Self {
        self.shadow(BoxShadow {
            offset_y: 1.0,
            blur: 3.0,
            color: Color::from_rgba(0x0000002a),
            ..Default::default()
        })
    }

    /// A medium elevation shadow.
    fn shadow_md(self) -> Self {
        self.shadow(BoxShadow {
            offset_y: 4.0,
            blur: 12.0,
            color: Color::from_rgba(0x00000030),
            ..Default::default()
        })
    }

    /// A large elevation shadow, for what floats above the page.
    fn shadow_lg(self) -> Self {
        self.shadows(vec![
            BoxShadow {
                offset_y: 8.0,
                blur: 24.0,
                color: Color::from_rgba(0x00000033),
                ..Default::default()
            },
            BoxShadow {
                offset_y: 2.0,
                blur: 6.0,
                color: Color::from_rgba(0x0000001f),
                ..Default::default()
            },
        ])
    }

    /// Paints above or below what comes in tree order, and makes the node a stacking context.
    fn z_index(mut self, z: i32) -> Self {
        self.style_mut().z_index = Some(Some(z));
        self
    }

    /// Moves what is painted by logical pixels, without affecting layout.
    fn translate(mut self, x: f32, y: f32) -> Self {
        let style = self.style_mut();
        style.translate_x = Some(x);
        style.translate_y = Some(y);
        self
    }

    /// What `scale` and `rotate` keep in place, as a fraction of the border box. The center by
    /// default.
    fn transform_origin(mut self, x: f32, y: f32) -> Self {
        let style = self.style_mut();
        style.transform_origin_x = Some(x);
        style.transform_origin_y = Some(y);
        self
    }

    /// Makes changes of every animatable property go smoothly.
    fn transition(mut self, transition: Transition) -> Self {
        self.style_mut().transitions = Some(vec![(None, transition)]);
        self
    }

    /// Makes changes of these properties go smoothly, on top of what is already said here.
    fn transition_of(
        mut self,
        props: impl IntoIterator<Item = Prop>,
        transition: Transition,
    ) -> Self {
        let list = self.style_mut().transitions.get_or_insert_default();
        list.extend(props.into_iter().map(|p| (Some(p), transition)));
        self
    }

    fn overflow_scroll(mut self) -> Self {
        let style = self.style_mut();
        style.overflow_x = Some(Overflow::Scroll);
        style.overflow_y = Some(Overflow::Scroll);
        self
    }

    fn overflow_y_scroll(mut self) -> Self {
        let style = self.style_mut();
        style.overflow_x = Some(Overflow::Hidden);
        style.overflow_y = Some(Overflow::Scroll);
        self
    }

    fn overflow_clip(mut self) -> Self {
        let style = self.style_mut();
        style.overflow_x = Some(Overflow::Hidden);
        style.overflow_y = Some(Overflow::Hidden);
        self
    }
}

impl Styled for Style {
    fn style_mut(&mut self) -> &mut Style {
        self
    }
}
