//! Text whose parts look different, with nodes among its words: `rich_text`. See "M1" in
//! `docs/design/28-markdown.md`.
//!
//! A paragraph stays one text node. Its spans are ranges of its text with a style of their own,
//! not nodes, because a node is heavy and a paragraph of a long document has many of them.

use super::{ShapeStyle, TextLayout};
use crate::event::PointerEvent;
use crate::geometry::{PointF, Scale, SizeI};
use crate::tree::{Node, NodeId, NodeKind, TextData, with_doc};
use crate::view::{IntoView, StyleFn, View, bind_dynamic_styles, create_node};
use anti_webui_style::{Color, ComputedStyle, CursorIcon, LineHeight, Style, Styled};
use std::ops::Range;
use std::rc::Rc;

/// Where a node among the words sits in its line, as CSS `vertical-align`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum VerticalAlign {
    /// The bottom of the node on the baseline.
    #[default]
    Baseline,
    /// The middle of the node half an x-height above the baseline.
    Middle,
    Top,
    Bottom,
    TextTop,
    TextBottom,
    Sub,
    Super,
    /// Raised by this many logical pixels, or lowered when it is negative.
    Px(f32),
}

impl VerticalAlign {
    pub(crate) fn parley(self, scale: Scale) -> parley::VerticalAlign {
        match self {
            Self::Baseline => parley::VerticalAlign::BASELINE,
            Self::Middle => parley::VerticalAlign::MIDDLE,
            Self::Top => parley::VerticalAlign::TOP,
            Self::Bottom => parley::VerticalAlign::BOTTOM,
            Self::TextTop => parley::VerticalAlign::TEXT_TOP,
            Self::TextBottom => parley::VerticalAlign::TEXT_BOTTOM,
            Self::Sub => parley::VerticalAlign::SUB,
            Self::Super => parley::VerticalAlign::SUPER,
            Self::Px(v) => parley::VerticalAlign::length(scale.pxf(v)),
        }
    }
}

/// A part of a `rich_text` with a style of its own. Only what is about letters applies: the font,
/// its size, weight and style, the spacing of letters, `vertical_align`, the color, the
/// background with its corners, the padding on the sides, the lines and the cursor.
pub struct Span {
    text: String,
    style: Rc<Style>,
    hover: Option<Rc<Style>>,
    align: VerticalAlign,
    on_click: Option<Rc<dyn Fn()>>,
}

/// A part of a `rich_text` with a style of its own.
pub fn span(text: impl Into<String>) -> Span {
    Span {
        text: text.into(),
        style: Rc::default(),
        hover: None,
        align: VerticalAlign::Baseline,
        on_click: None,
    }
}

impl Span {
    /// Takes its style from one that many spans share, as a theme does, so that it is held once.
    pub fn shared_style(mut self, style: Rc<Style>) -> Self {
        self.style = style;
        self
    }

    /// The style while the pointer is over it. Only what doesn't move letters applies: the color,
    /// the background and the lines.
    pub fn hover(mut self, f: impl FnOnce(Style) -> Style) -> Self {
        let old = self.hover.take().map(|s| (*s).clone()).unwrap_or_default();
        self.hover = Some(Rc::new(f(old)));
        self
    }

    /// Shares the style while the pointer is over it, like `shared_style`.
    pub fn shared_hover(mut self, style: Rc<Style>) -> Self {
        self.hover = Some(style);
        self
    }

    /// Raises or lowers it in its line, as CSS `vertical-align` does: `Super` for a footnote.
    pub fn vertical_align(mut self, align: VerticalAlign) -> Self {
        self.align = align;
        self
    }

    /// Called when it is pressed and let go of over it. The cursor is a hand over it unless its
    /// style says otherwise.
    pub fn on_click(mut self, f: impl Fn() + 'static) -> Self {
        self.on_click = Some(Rc::new(f));
        self
    }
}

impl Styled for Span {
    fn style_mut(&mut self) -> &mut Style {
        Rc::make_mut(&mut self.style)
    }
}

/// A node among the words of a `rich_text`, like an `img` in a paragraph.
pub struct Inline {
    view: View,
    alt: String,
    align: VerticalAlign,
}

/// Puts a node among the words of a `rich_text`. It takes the size it would like, as an
/// `inline-block` does.
pub fn inline(view: impl IntoView) -> Inline {
    Inline {
        view: view.into_view(),
        alt: String::new(),
        align: VerticalAlign::Baseline,
    }
}

impl Inline {
    /// What it is when the text around it is copied: `:party:` for an emoji.
    pub fn alt(mut self, alt: impl Into<String>) -> Self {
        self.alt = alt.into();
        self
    }

    pub fn vertical_align(mut self, align: VerticalAlign) -> Self {
        self.align = align;
        self
    }
}

/// A part of a `rich_text`.
pub enum Run {
    Text(String),
    Span(Span),
    Inline(Inline),
}

impl From<&str> for Run {
    fn from(s: &str) -> Self {
        Self::Text(s.to_owned())
    }
}

impl From<String> for Run {
    fn from(s: String) -> Self {
        Self::Text(s)
    }
}

impl From<Span> for Run {
    fn from(s: Span) -> Self {
        Self::Span(s)
    }
}

impl From<Inline> for Run {
    fn from(i: Inline) -> Self {
        Self::Inline(i)
    }
}

/// What a `rich_text` can be made of: a list of runs, or a tuple of things that are runs.
pub trait IntoRuns {
    fn into_runs(self) -> Vec<Run>;
}

impl<R: Into<Run>> IntoRuns for Vec<R> {
    fn into_runs(self) -> Vec<Run> {
        self.into_iter().map(Into::into).collect()
    }
}

macro_rules! runs_of_tuple {
    ($($name:ident),+) => {
        impl<$($name: Into<Run>),+> IntoRuns for ($($name,)+) {
            #[allow(non_snake_case)]
            fn into_runs(self) -> Vec<Run> {
                let ($($name,)+) = self;
                vec![$($name.into()),+]
            }
        }
    };
}

runs_of_tuple!(A);
runs_of_tuple!(A, B);
runs_of_tuple!(A, B, C);
runs_of_tuple!(A, B, C, D);
runs_of_tuple!(A, B, C, D, E);
runs_of_tuple!(A, B, C, D, E, F);
runs_of_tuple!(A, B, C, D, E, F, G);
runs_of_tuple!(A, B, C, D, E, F, G, H);
runs_of_tuple!(A, B, C, D, E, F, G, H, I);
runs_of_tuple!(A, B, C, D, E, F, G, H, I, J);
runs_of_tuple!(A, B, C, D, E, F, G, H, I, J, K);
runs_of_tuple!(A, B, C, D, E, F, G, H, I, J, K, L);

/// A paragraph whose parts look different: `rich_text(("a ", span("b").bold()))`. It is one text
/// node, which is selected and copied as one.
pub struct RichText {
    runs: Vec<Run>,
    style: Style,
    dynamic: Vec<StyleFn>,
}

pub fn rich_text(runs: impl IntoRuns) -> RichText {
    RichText {
        runs: runs.into_runs(),
        style: Style::default(),
        dynamic: Vec::new(),
    }
}

impl RichText {
    /// A reactive style layer, recomputed whenever the signals it reads change.
    pub fn dyn_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.dynamic.push(Box::new(f));
        self
    }
}

impl Styled for RichText {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl IntoView for RichText {
    fn into_view(self) -> View {
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut boxes = Vec::new();
        let mut children = Vec::new();
        for run in self.runs {
            match run {
                Run::Text(s) => text.push_str(&s),
                Run::Span(s) => {
                    let start = text.len();
                    text.push_str(&s.text);
                    spans.push(SpanData {
                        range: start..text.len(),
                        style: s.style,
                        hover: s.hover,
                        align: s.align,
                        on_click: s.on_click,
                    });
                }
                Run::Inline(i) => {
                    children.push(i.view.0);
                    boxes.push(BoxData {
                        at: text.len(),
                        node: i.view.0,
                        alt: i.alt,
                        align: i.align,
                        size: SizeI::default(),
                    });
                }
            }
        }
        let clickable = spans.iter().any(|s| s.on_click.is_some());
        let mut node = Node::new(NodeKind::Text(TextData {
            text,
            version: 0,
            layout: Default::default(),
            zoom: 1.0,
            in_svg: false,
            rich: Some(Box::new(RichData {
                spans,
                boxes,
                resolved: Vec::new(),
                hovered: None,
            })),
        }));
        node.style.base = self.style;
        let id = create_node(node);
        with_doc(|d| {
            d.set_children(id, children);
            if clickable {
                d.nodes[id].listeners.click = Some(Rc::new(move |e: &PointerEvent| {
                    let f = with_doc(|d| {
                        let NodeKind::Text(t) = &d.get(id)?.kind else {
                            return None;
                        };
                        let i = span_at(&t.layout, t.rich.as_deref()?, e.local)?;
                        t.rich.as_ref()?.spans[i].on_click.clone()
                    });
                    if let Some(f) = f {
                        f();
                    }
                }));
            }
        });
        bind_dynamic_styles(id, self.dynamic);
        View(id)
    }
}

/// A range of a text with a style of its own.
pub struct SpanData {
    pub range: Range<usize>,
    pub style: Rc<Style>,
    pub hover: Option<Rc<Style>>,
    pub align: VerticalAlign,
    pub on_click: Option<Rc<dyn Fn()>>,
}

/// A node among the words of a text.
pub struct BoxData {
    /// Where it is in the text, in bytes.
    pub at: usize,
    pub node: NodeId,
    pub alt: String,
    pub align: VerticalAlign,
    /// What it measured, in physical pixels.
    pub size: SizeI,
}

/// What a text has when its parts look different.
pub struct RichData {
    /// In the order of the text, and not overlapping.
    pub spans: Vec<SpanData>,
    /// In the order of the text.
    pub boxes: Vec<BoxData>,
    /// The spans with what they inherit, from the last time the style of the node was resolved.
    pub(crate) resolved: Vec<ResolvedSpan>,
    /// The span the pointer is over.
    pub(crate) hovered: Option<usize>,
}

/// What a span is painted with, in physical pixels.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SpanPaint {
    pub color: Color,
    pub background: Color,
    pub radii: [f32; 4],
    pub underline: bool,
    pub strikethrough: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedSpan {
    pub shape: ShapeStyle,
    pub align: parley::VerticalAlign,
    /// The padding on its sides.
    pub pad: (i32, i32),
    pub paint: SpanPaint,
    pub hover: Option<SpanPaint>,
    pub cursor: Option<CursorIcon>,
}

fn paint_of(style: &Style, parent: &ComputedStyle, scale: Scale) -> SpanPaint {
    let r = |v: Option<f32>| scale.pxf(v.unwrap_or(0.0));
    SpanPaint {
        color: style.color.unwrap_or(parent.color),
        background: style.background.unwrap_or(Color::TRANSPARENT),
        radii: [
            r(style.radius_top_left),
            r(style.radius_top_right),
            r(style.radius_bottom_right),
            r(style.radius_bottom_left),
        ],
        underline: style.underline.unwrap_or(parent.underline),
        strikethrough: style.strikethrough.unwrap_or(parent.strikethrough),
    }
}

impl SpanData {
    pub(crate) fn resolve(&self, parent: &ComputedStyle, scale: Scale) -> ResolvedSpan {
        let s = &*self.style;
        let mut shape = ShapeStyle::from_computed(parent, scale);
        if let Some(f) = &s.font_family {
            shape.family = f.clone();
        }
        if let Some(size) = s.font_size {
            shape.size = scale.pxf(size);
            shape.optical_size = size;
        }
        // A line height given as a number is one for the font of each part, as in CSS.
        let size = s.font_size.unwrap_or(parent.font_size);
        match s.line_height.unwrap_or(parent.line_height) {
            LineHeight::Normal => shape.line_height = None,
            LineHeight::Relative(r) => shape.line_height = Some(scale.px(r * size).max(1)),
            LineHeight::Px(v) => shape.line_height = Some(scale.px(v).max(1)),
        }
        if let Some(w) = s.font_weight {
            shape.weight = w;
        }
        if let Some(st) = s.font_style {
            shape.style = st;
        }
        if let Some(axes) = &s.font_axes {
            shape.axes.merge(axes);
        }
        if let Some(l) = s.letter_spacing {
            shape.letter_spacing = scale.pxf(l);
        }
        let paint = paint_of(s, parent, scale);
        let hover = self.hover.as_ref().map(|h| {
            let mut over = paint_of(h, parent, scale);
            // What the hover style doesn't say stays as it is.
            if h.color.is_none() {
                over.color = paint.color;
            }
            if h.background.is_none() {
                over.background = paint.background;
            }
            if h.radius_top_left.is_none() {
                over.radii = paint.radii;
            }
            if h.underline.is_none() {
                over.underline = paint.underline;
            }
            if h.strikethrough.is_none() {
                over.strikethrough = paint.strikethrough;
            }
            over
        });
        ResolvedSpan {
            shape,
            align: self.align.parley(scale),
            pad: (
                scale.px(s.padding_left.unwrap_or(0.0)),
                scale.px(s.padding_right.unwrap_or(0.0)),
            ),
            paint,
            hover,
            cursor: s
                .cursor
                .flatten()
                .or(self.on_click.as_ref().map(|_| CursorIcon::Pointer)),
        }
    }
}

impl RichData {
    /// Resolves the spans against the style of their node. Says whether the letters move.
    pub(crate) fn resolve(&mut self, parent: &ComputedStyle, scale: Scale) -> bool {
        let resolved: Vec<ResolvedSpan> = self
            .spans
            .iter()
            .map(|s| s.resolve(parent, scale))
            .collect();
        let moved = resolved.len() != self.resolved.len()
            || resolved
                .iter()
                .zip(&self.resolved)
                .any(|(a, b)| a.shape != b.shape || a.align != b.align || a.pad != b.pad);
        self.resolved = resolved;
        moved
    }

    /// What span `i` is painted with now.
    pub(crate) fn paint(&self, i: usize) -> Option<&SpanPaint> {
        let r = self.resolved.get(i)?;
        Some(match (&r.hover, self.hovered == Some(i)) {
            (Some(h), true) => h,
            _ => &r.paint,
        })
    }

    /// The span that holds the byte at `index`.
    pub fn span_at(&self, index: usize) -> Option<usize> {
        let i = self.spans.partition_point(|s| s.range.end <= index);
        self.spans
            .get(i)
            .filter(|s| s.range.start <= index && index < s.range.end)
            .map(|_| i)
    }

    /// What is copied of `a..b` of `text`: the text with the `alt` of the boxes in it.
    pub fn copied(&self, text: &str, a: usize, b: usize) -> String {
        let mut out = String::new();
        let mut at = a;
        for bx in self
            .boxes
            .iter()
            .filter(|bx| bx.at >= a && bx.at < b.max(a + 1))
        {
            if bx.at > b || (bx.at == b && b != text.len()) {
                continue;
            }
            out.push_str(&text[at..bx.at]);
            out.push_str(&bx.alt);
            at = bx.at;
        }
        out.push_str(&text[at..b]);
        out
    }
}

/// The span at `p`, relative to the text.
pub(crate) fn span_at(layout: &TextLayout, rich: &RichData, p: PointF) -> Option<usize> {
    let size = layout.size();
    if p.x < 0.0 || p.y < 0.0 || p.x >= size.width as f32 || p.y >= size.height as f32 {
        return None;
    }
    rich.span_at(layout.cluster_at(p.x, p.y)?)
}

/// Notes which span the pointer is over, at `p` in window coordinates over `target`, and gives the
/// cursor that span has.
pub(crate) fn hover_span(
    doc: &mut crate::tree::Document,
    target: Option<NodeId>,
    p: PointF,
) -> Option<CursorIcon> {
    let now = target.and_then(|id| {
        let n = doc.get(id)?;
        let NodeKind::Text(t) = &n.kind else {
            return None;
        };
        let q = doc.from_window(n, p)?;
        let local = PointF::new(q.x - n.abs.x as f32, q.y - n.abs.y as f32);
        span_at(&t.layout, t.rich.as_deref()?, local).map(|i| (id, i))
    });
    let was = doc.state.hovered_span;
    if now != was {
        doc.state.hovered_span = now;
        let mut repaint = false;
        for (id, i, on) in [
            was.map(|(id, i)| (id, i, false)),
            now.map(|(id, i)| (id, i, true)),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(n) = doc.nodes.get_mut(id)
                && let NodeKind::Text(t) = &mut n.kind
                && let Some(rich) = &mut t.rich
            {
                rich.hovered = on.then_some(i);
                repaint |= rich.resolved.get(i).is_some_and(|r| r.hover.is_some());
            }
        }
        if repaint {
            doc.dirty.content = true;
            doc.mark_paint();
        }
    }
    let (id, i) = now?;
    let NodeKind::Text(t) = &doc.get(id)?.kind else {
        return None;
    };
    t.rich.as_ref()?.resolved.get(i)?.cursor
}
