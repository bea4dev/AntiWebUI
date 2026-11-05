//! Makes shapes out of an SVG file. `usvg` works out most of it. What it has no idea about is HTML in
//! `foreignObject`, which is how Mermaid writes its labels, so that is taken out first and comes back
//! as text in its place. See `docs/design/25-svg.md`.

use crate::geometry::Affine;
use crate::svg::{Group, Svg, TextAnchor, TextBaseline, group, path, rect, svg, text};
use crate::view::{IntoView, View};
use anti_webui_style::{
    Color, FillRule, FontFamily, FontWeight, Gradient, GradientShape, GradientSpread,
    GradientUnits, Length, LineCap, LineHeight, LineJoin, Styled, SvgPaint, TextAlign,
};
use kurbo::BezPath;
use resvg::usvg::{self, Node as N, roxmltree};
use std::str::FromStr;

const LABEL_ID: &str = "__anti_webui_label_";

/// Makes shapes out of an SVG file, which then go like shapes made by hand: they follow the scale
/// and take no room in the picture cache. What this can't draw (patterns, masks, filters, pictures,
/// blending and clip paths other than rectangles) makes all of it one picture instead, as `img` shows it.
pub fn parse(data: &[u8]) -> Result<Svg, String> {
    let (without_labels, labels) = take_labels(data);
    let source = without_labels.as_deref().unwrap_or(data);
    #[allow(unused_mut)]
    let mut options = usvg::Options::default();
    // Reading the fonts of the system takes a while, and few pictures have text.
    #[cfg(feature = "svg-text")]
    if source.windows(5).any(|w| w == b"<text") {
        options.fontdb_mut().load_system_fonts();
    }
    let tree = usvg::Tree::from_data(source, &options).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    let root = svg()
        .view_box(0.0, 0.0, width, height)
        .w(Length::Px(width))
        .h(Length::Px(height));
    Ok(if drawable(tree.root()) {
        root.children(
            tree.root()
                .children()
                .iter()
                .filter_map(|n| parsed_node(n, &labels)),
        )
    } else {
        // The labels aren't in what is made of the file, so they go over it.
        let mut placed = Vec::new();
        placed_labels(tree.root(), &labels, &mut placed);
        root.child(
            crate::view::img(data.to_vec())
                .w(Length::Percent(100.0))
                .h(Length::Percent(100.0)),
        )
        .children(placed)
    })
}

fn placed_labels(g: &usvg::Group, labels: &[Label], out: &mut Vec<View>) {
    for node in g.children() {
        match node {
            N::Group(g) => placed_labels(g, labels, out),
            N::Path(p) => {
                if let Some(label) = label_of(p, labels) {
                    out.push(
                        group()
                            .transform(transform_of(p.abs_transform()))
                            .child(label_view(label, p.data().bounds()))
                            .into_view(),
                    );
                }
            }
            _ => {}
        }
    }
}

fn label_of<'a>(p: &usvg::Path, labels: &'a [Label]) -> Option<&'a Label> {
    p.id()
        .strip_prefix(LABEL_ID)
        .and_then(|i| i.parse::<usize>().ok())
        .and_then(|i| labels.get(i))
}

/// The rectangle a clip path is, if that is all it is. A `<marker>` always has one for its box.
fn clip_rectangle(clip: &usvg::ClipPath) -> Option<crate::scene::ClipQuad> {
    if clip.clip_path().is_some() {
        return None;
    }
    let [N::Path(shape)] = clip.root().children() else {
        return None;
    };
    let r = rectangle_of(shape.data())?;
    let t = transform_of(clip.transform()).then_apply(&transform_of(shape.abs_transform()));
    let (x0, y0, x1, y1) = (
        r.left() as f64,
        r.top() as f64,
        r.right() as f64,
        r.bottom() as f64,
    );
    Some([(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| t.apply(x, y)))
}

/// The rectangle a path is, if it is one along the axes.
fn rectangle_of(data: &usvg::tiny_skia_path::Path) -> Option<usvg::Rect> {
    use usvg::tiny_skia_path::PathSegment as S;
    let bounds = data.bounds();
    let mut points = 0;
    for segment in data.segments() {
        let on_corner = |p: usvg::tiny_skia_path::Point| {
            (p.x == bounds.left() || p.x == bounds.right())
                && (p.y == bounds.top() || p.y == bounds.bottom())
        };
        match segment {
            S::MoveTo(p) | S::LineTo(p) if on_corner(p) => points += 1,
            S::Close => {}
            _ => return None,
        }
    }
    (points == 4 || points == 5).then_some(bounds)
}

fn drawable(g: &usvg::Group) -> bool {
    let paint = |p: &usvg::Paint| !matches!(p, usvg::Paint::Pattern(_));
    g.clip_path()
        .is_none_or(|clip| clip_rectangle(clip).is_some())
        && g.mask().is_none()
        && g.filters().is_empty()
        && g.blend_mode() == usvg::BlendMode::Normal
        && g.children().iter().all(|n| match n {
            N::Group(g) => drawable(g),
            N::Path(p) => {
                p.fill().is_none_or(|f| paint(f.paint()))
                    && p.stroke().is_none_or(|s| paint(s.paint()))
            }
            N::Image(_) => false,
            N::Text(t) => drawable(t.flattened()),
        })
}

fn transform_of(t: usvg::Transform) -> Affine {
    Affine {
        a: t.sx as f64,
        b: t.ky as f64,
        c: t.kx as f64,
        d: t.sy as f64,
        e: t.tx as f64,
        f: t.ty as f64,
    }
}

fn color_of(c: usvg::Color, alpha: f32) -> Color {
    Color::new(
        c.red as f32 / 255.0,
        c.green as f32 / 255.0,
        c.blue as f32 / 255.0,
        alpha,
    )
}

fn paint_of(paint: &usvg::Paint) -> SvgPaint {
    let finish = |mut g: Gradient, base: &usvg::BaseGradient| {
        for stop in base.stops() {
            g = g.stop(
                stop.offset().get(),
                color_of(stop.color(), stop.opacity().get()),
            );
        }
        let t = transform_of(base.transform());
        SvgPaint::from(
            g.units(GradientUnits::UserSpace)
                .spread(match base.spread_method() {
                    usvg::SpreadMethod::Pad => GradientSpread::Pad,
                    usvg::SpreadMethod::Reflect => GradientSpread::Reflect,
                    usvg::SpreadMethod::Repeat => GradientSpread::Repeat,
                })
                .transform([t.a, t.b, t.c, t.d, t.e, t.f]),
        )
    };
    match paint {
        usvg::Paint::Color(c) => SvgPaint::Color(color_of(*c, 1.0)),
        usvg::Paint::LinearGradient(g) => {
            finish(Gradient::linear(g.x1(), g.y1(), g.x2(), g.y2()), g)
        }
        usvg::Paint::RadialGradient(g) => {
            let mut radial = Gradient::radial(g.cx(), g.cy(), g.r().get());
            radial.shape = GradientShape::Radial {
                cx: g.cx(),
                cy: g.cy(),
                r: g.r().get(),
                fx: g.fx(),
                fy: g.fy(),
            };
            finish(radial, g)
        }
        usvg::Paint::Pattern(_) => SvgPaint::None,
    }
}

fn parsed_node(node: &N, labels: &[Label]) -> Option<View> {
    Some(match node {
        N::Group(g) => parsed_group(g, labels).into_view(),
        N::Text(t) => match real_text(t) {
            Some(text) => text,
            None => parsed_group(t.flattened(), labels).into_view(),
        },
        N::Image(_) => return None,
        N::Path(p) if !p.is_visible() => return None,
        N::Path(p) => {
            if let Some(label) = label_of(p, labels) {
                return Some(label_view(label, p.data().bounds()));
            }
            parsed_path(p)
        }
    })
}

fn parsed_path(p: &usvg::Path) -> View {
    use usvg::tiny_skia_path::PathSegment as S;
    let mut curves = BezPath::new();
    for segment in p.data().segments() {
        match segment {
            S::MoveTo(a) => curves.move_to((a.x as f64, a.y as f64)),
            S::LineTo(a) => curves.line_to((a.x as f64, a.y as f64)),
            S::QuadTo(a, b) => curves.quad_to((a.x as f64, a.y as f64), (b.x as f64, b.y as f64)),
            S::CubicTo(a, b, c) => curves.curve_to(
                (a.x as f64, a.y as f64),
                (b.x as f64, b.y as f64),
                (c.x as f64, c.y as f64),
            ),
            S::Close => curves.close_path(),
        }
    }
    let mut shape = path(curves);
    shape = match p.fill() {
        Some(f) => shape
            .fill(paint_of(f.paint()))
            .fill_opacity(f.opacity().get())
            .fill_rule(match f.rule() {
                usvg::FillRule::NonZero => FillRule::NonZero,
                usvg::FillRule::EvenOdd => FillRule::EvenOdd,
            }),
        None => shape.fill_none(),
    };
    if let Some(s) = p.stroke() {
        shape = shape
            .stroke(paint_of(s.paint()))
            .stroke_opacity(s.opacity().get())
            .stroke_width(s.width().get())
            .miter_limit(s.miterlimit().get())
            .line_cap(match s.linecap() {
                usvg::LineCap::Butt => LineCap::Butt,
                usvg::LineCap::Round => LineCap::Round,
                usvg::LineCap::Square => LineCap::Square,
            })
            .line_join(match s.linejoin() {
                usvg::LineJoin::Miter | usvg::LineJoin::MiterClip => LineJoin::Miter,
                usvg::LineJoin::Round => LineJoin::Round,
                usvg::LineJoin::Bevel => LineJoin::Bevel,
            })
            .dash_offset(s.dashoffset());
        if let Some(dashes) = s.dasharray() {
            shape = shape.dash_array(dashes.iter().copied());
        }
    }
    shape.into_view()
}

fn parsed_group(g: &usvg::Group, labels: &[Label]) -> Group {
    let mut out = group()
        .transform(transform_of(g.transform()))
        .children(g.children().iter().filter_map(|n| parsed_node(n, labels)));
    if let Some(quad) = g.clip_path().and_then(clip_rectangle) {
        out = out.clip_quad(quad);
    }
    let opacity = g.opacity().get();
    if opacity < 1.0 {
        out = out.opacity(opacity);
    }
    out
}

fn font_list(font: &usvg::Font) -> FontFamily {
    let names: Vec<String> = font
        .families()
        .iter()
        .map(|f| match f {
            usvg::FontFamily::Serif => "serif".to_owned(),
            usvg::FontFamily::SansSerif => "sans-serif".to_owned(),
            usvg::FontFamily::Cursive => "cursive".to_owned(),
            usvg::FontFamily::Fantasy => "fantasy".to_owned(),
            usvg::FontFamily::Monospace => "monospace".to_owned(),
            usvg::FontFamily::Named(name) => format!("\"{name}\""),
        })
        .collect();
    FontFamily(names.join(", ").into())
}

/// `<text>` that lays out like text does here: the same fonts as the rest, and it can be selected.
/// What has more to it than that (letters on their own, along a path, in parts of different styles)
/// stays the outlines `usvg` made.
fn real_text(t: &usvg::Text) -> Option<View> {
    if t.writing_mode() != usvg::WritingMode::LeftToRight || t.rotate().iter().any(|r| *r != 0.0) {
        return None;
    }
    let mut parts = Vec::new();
    let mut at_char = 0;
    let mut y = 0.0;
    for chunk in t.chunks() {
        if !matches!(chunk.text_flow(), usvg::TextFlow::Linear) {
            return None;
        }
        let [span] = chunk.spans() else {
            return None;
        };
        let decoration = span.decoration();
        if span.stroke().is_some()
            || !span.baseline_shift().is_empty()
            || !span.is_visible()
            || decoration.underline().is_some()
            || decoration.overline().is_some()
            || decoration.line_through().is_some()
            || span.small_caps()
            || span.word_spacing() != 0.0
        {
            return None;
        }
        let count = chunk.text().chars().count();
        let moved = |list: &[f32]| {
            list.get(at_char + 1..at_char + count)
                .is_some_and(|rest| rest.iter().any(|v| *v != 0.0))
        };
        if moved(t.dx()) || moved(t.dy()) {
            return None;
        }
        let x = chunk.x()? + t.dx().get(at_char).copied().unwrap_or(0.0);
        y = chunk.y().unwrap_or(y) + t.dy().get(at_char).copied().unwrap_or(0.0);
        at_char += count;
        let Some(fill) = span.fill() else {
            continue;
        };
        let font = span.font();
        let mut part = text(chunk.text().to_owned())
            .at(x, y)
            .anchor(match chunk.anchor() {
                usvg::TextAnchor::Start => TextAnchor::Start,
                usvg::TextAnchor::Middle => TextAnchor::Middle,
                usvg::TextAnchor::End => TextAnchor::End,
            })
            .baseline(match span.dominant_baseline() {
                usvg::DominantBaseline::Central => TextBaseline::Central,
                usvg::DominantBaseline::Middle => TextBaseline::Middle,
                usvg::DominantBaseline::Hanging | usvg::DominantBaseline::TextBeforeEdge => {
                    TextBaseline::Hanging
                }
                _ => TextBaseline::Alphabetic,
            })
            .text_size(span.font_size().get())
            .font_family(font_list(font))
            .font_weight(FontWeight(font.weight()))
            .fill(paint_of(fill.paint()))
            .fill_opacity(fill.opacity().get())
            .whitespace_nowrap();
        if span.letter_spacing() != 0.0 {
            part = part.letter_spacing(span.letter_spacing());
        }
        if !matches!(font.style(), usvg::FontStyle::Normal) {
            part = part.italic();
        }
        parts.push(part.into_view());
    }
    Some(group().children(parts).into_view())
}

/// What a `foreignObject` holds as far as it can be shown: text in one style over its backgrounds.
struct Label {
    text: String,
    style: Html,
    backgrounds: Vec<Color>,
}

/// What is inherited in HTML and what labels use of it.
#[derive(Clone)]
struct Html {
    color: Color,
    family: Option<String>,
    size: f32,
    weight: u16,
    italic: bool,
    align: Option<TextAlign>,
    line_height: LineHeight,
    nowrap: bool,
    /// Not inherited.
    background: Option<Color>,
    /// Not inherited.
    hidden: bool,
}

impl Default for Html {
    fn default() -> Self {
        Self {
            color: Color::BLACK,
            family: None,
            size: 16.0,
            weight: 400,
            italic: false,
            align: None,
            line_height: LineHeight::Normal,
            nowrap: false,
            background: None,
            hidden: false,
        }
    }
}

fn length(value: &str, of: f32) -> Option<f32> {
    let value = value.trim();
    if let Some(v) = value.strip_suffix("px") {
        return v.trim().parse().ok();
    }
    if let Some(v) = value.strip_suffix("rem") {
        return v.trim().parse::<f32>().ok().map(|v| v * 16.0);
    }
    if let Some(v) = value.strip_suffix("em") {
        return v.trim().parse::<f32>().ok().map(|v| v * of);
    }
    if let Some(v) = value.strip_suffix('%') {
        return v.trim().parse::<f32>().ok().map(|v| v * of / 100.0);
    }
    value.parse().ok()
}

impl Html {
    fn set(&mut self, name: &str, value: &str) {
        let value = value.trim();
        match name {
            "color" => {
                if let Some(c) = css_color(value) {
                    self.color = c;
                }
            }
            "font-family" => self.family = Some(value.to_owned()),
            "font-size" => {
                if let Some(v) = length(value, self.size) {
                    self.size = v;
                }
            }
            "font-weight" => {
                self.weight = match value {
                    "normal" => 400,
                    "bold" => 700,
                    other => other.parse().unwrap_or(self.weight),
                }
            }
            "font-style" => self.italic = value == "italic" || value == "oblique",
            "text-align" => {
                self.align = match value {
                    "center" => Some(TextAlign::Center),
                    "right" | "end" => Some(TextAlign::Right),
                    "left" | "start" => Some(TextAlign::Left),
                    _ => self.align,
                }
            }
            "line-height" => {
                self.line_height = if value == "normal" {
                    LineHeight::Normal
                } else if let Ok(v) = value.parse::<f32>() {
                    LineHeight::Relative(v)
                } else if let Some(v) = length(value, self.size) {
                    LineHeight::Px(v)
                } else {
                    self.line_height
                }
            }
            "white-space" => self.nowrap = value == "nowrap" || value == "pre",
            "background-color" | "background" => self.background = css_color(value),
            "display" => self.hidden = value == "none",
            _ => {}
        }
    }
}

fn css_color(value: &str) -> Option<Color> {
    let c = svgtypes::Color::from_str(value).ok()?;
    Some(Color::new(
        c.red as f32 / 255.0,
        c.green as f32 / 255.0,
        c.blue as f32 / 255.0,
        c.alpha as f32 / 255.0,
    ))
}

#[derive(Clone, Copy)]
struct XmlElement<'a, 'input>(roxmltree::Node<'a, 'input>);

impl simplecss::Element for XmlElement<'_, '_> {
    fn parent_element(&self) -> Option<Self> {
        self.0.parent_element().map(XmlElement)
    }

    fn prev_sibling_element(&self) -> Option<Self> {
        self.0.prev_sibling_element().map(XmlElement)
    }

    fn has_local_name(&self, name: &str) -> bool {
        self.0.tag_name().name() == name
    }

    fn attribute_matches(&self, name: &str, operator: simplecss::AttributeOperator<'_>) -> bool {
        self.0.attribute(name).is_some_and(|v| operator.matches(v))
    }

    fn pseudo_class_matches(&self, class: simplecss::PseudoClass<'_>) -> bool {
        matches!(class, simplecss::PseudoClass::FirstChild)
            && self.0.prev_sibling_element().is_none()
    }
}

/// The style of an element, from what it inherits, the style sheets, and what it says itself.
fn style_of(
    node: roxmltree::Node<'_, '_>,
    inherited: &Html,
    sheets: &[simplecss::StyleSheet<'_>],
) -> Html {
    let mut style = inherited.clone();
    style.background = None;
    style.hidden = false;
    let inline: Vec<(&str, &str)> = node
        .attribute("style")
        .into_iter()
        .flat_map(|s| s.split(';'))
        .filter_map(|d| d.split_once(':'))
        .map(|(n, v)| (n.trim(), v.trim()))
        .collect();
    let element = XmlElement(node);
    for important in [false, true] {
        for sheet in sheets {
            for rule in &sheet.rules {
                if rule.selector.matches(&element) {
                    for d in &rule.declarations {
                        if d.important == important {
                            style.set(d.name, d.value);
                        }
                    }
                }
            }
        }
        if !important {
            for (name, value) in &inline {
                style.set(name, value.trim_end_matches("!important"));
            }
        }
    }
    style
}

/// Takes what `usvg` knows nothing about out of the file, leaving something in its place that says
/// where it was. That goes through all that `usvg` does, like what is around it being left out.
fn take_labels(data: &[u8]) -> (Option<Vec<u8>>, Vec<Label>) {
    if !data.windows(13).any(|w| w == b"foreignObject") {
        return (None, Vec::new());
    }
    let Ok(source) = std::str::from_utf8(data) else {
        return (None, Vec::new());
    };
    let Ok(doc) = roxmltree::Document::parse(source) else {
        return (None, Vec::new());
    };
    let sheets: Vec<simplecss::StyleSheet<'_>> = doc
        .descendants()
        .filter(|n| n.has_tag_name("style") || n.tag_name().name() == "style")
        .filter_map(|n| n.text())
        .map(simplecss::StyleSheet::parse)
        .collect();
    let mut labels = Vec::new();
    let mut out = String::with_capacity(source.len());
    let mut copied = 0;
    for node in doc.descendants() {
        if node.tag_name().name() != "foreignObject" {
            continue;
        }
        let range = node.range();
        if range.start < copied {
            continue;
        }
        out.push_str(&source[copied..range.start]);
        copied = range.end;
        let number = |name: &str| node.attribute(name).and_then(|v| length(v, 0.0));
        let (width, height) = (
            number("width").unwrap_or(0.0),
            number("height").unwrap_or(0.0),
        );
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let mut style = Html::default();
        let ancestors: Vec<_> = node.ancestors().filter(|n| n.is_element()).collect();
        for a in ancestors.iter().rev() {
            style = style_of(*a, &style, &sheets);
        }
        let mut label = Label {
            text: String::new(),
            style: style.clone(),
            backgrounds: Vec::new(),
        };
        let mut first = true;
        collect_html(node, &style, &sheets, &mut label, &mut first);
        let shown = label.text.trim_end().len();
        label.text.truncate(shown);
        if label.text.is_empty() && label.backgrounds.is_empty() {
            continue;
        }
        let (x, y) = (number("x").unwrap_or(0.0), number("y").unwrap_or(0.0));
        out.push_str(&format!(
            // Its own style, so that what a sheet says of `rect` has no say in it.
            "<rect id=\"{LABEL_ID}{}\" x=\"{x}\" y=\"{y}\" width=\"{width}\" height=\"{height}\" \
             style=\"opacity:1;fill:#000;fill-opacity:0;stroke:none;display:inline;visibility:visible\"/>",
            labels.len()
        ));
        labels.push(label);
    }
    out.push_str(&source[copied..]);
    (Some(out.into_bytes()), labels)
}

fn collect_html(
    node: roxmltree::Node<'_, '_>,
    inherited: &Html,
    sheets: &[simplecss::StyleSheet<'_>],
    label: &mut Label,
    first: &mut bool,
) {
    for child in node.children() {
        if child.is_text() {
            let mut piece = String::new();
            for word in child.text().unwrap_or("").split_whitespace() {
                if !piece.is_empty() {
                    piece.push(' ');
                }
                piece.push_str(word);
            }
            if piece.is_empty() {
                continue;
            }
            // Spaces between pieces on a line stay: they are in the text of a whole line.
            if child
                .text()
                .is_some_and(|t| t.starts_with(char::is_whitespace))
                && !label.text.is_empty()
                && !label.text.ends_with('\n')
            {
                label.text.push(' ');
            }
            label.text.push_str(&piece);
            if std::mem::take(first) {
                label.style = inherited.clone();
            }
            continue;
        }
        if !child.is_element() {
            continue;
        }
        let style = style_of(child, inherited, sheets);
        if style.hidden {
            continue;
        }
        let name = child.tag_name().name();
        if name == "br" {
            label.text.push('\n');
            continue;
        }
        if let Some(c) = style.background.filter(|c| !c.is_transparent()) {
            label.backgrounds.push(c);
        }
        let block = matches!(
            name,
            "div"
                | "p"
                | "ul"
                | "ol"
                | "li"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "pre"
                | "section"
                | "table"
                | "tr"
        );
        let break_line = |label: &mut Label| {
            if block && !label.text.is_empty() && !label.text.ends_with('\n') {
                label.text.push('\n');
            }
        };
        break_line(label);
        collect_html(child, &style, sheets, label, first);
        break_line(label);
    }
}

/// The backgrounds of a label and its text, in the rectangle it was left in.
fn label_view(label: &Label, bounds: usvg::Rect) -> View {
    let (x, y, width, height) = (bounds.x(), bounds.y(), bounds.width(), bounds.height());
    let style = &label.style;
    let (anchor, at) = match style.align {
        Some(TextAlign::Center) => (TextAnchor::Middle, x + width * 0.5),
        Some(TextAlign::Right | TextAlign::End) => (TextAnchor::End, x + width),
        _ => (TextAnchor::Start, x),
    };
    let mut words = text(label.text.clone())
        .at(at, y)
        .anchor(anchor)
        .baseline(TextBaseline::Hanging)
        .text_size(style.size)
        .font_weight(FontWeight(style.weight))
        .line_height(style.line_height)
        .text_align(style.align.unwrap_or(TextAlign::Left))
        .fill(SvgPaint::Color(style.color));
    if let Some(family) = &style.family {
        words = words.font_family(FontFamily(family.as_str().into()));
    }
    if style.italic {
        words = words.italic();
    }
    words = if style.nowrap {
        words.whitespace_nowrap()
    } else {
        words.max_width(width)
    };
    group()
        .children(label.backgrounds.iter().map(|c| {
            rect(x as f64, y as f64, width as f64, height as f64)
                .fill(SvgPaint::Color(*c))
                .into_view()
        }))
        .child(words)
        .into_view()
}
