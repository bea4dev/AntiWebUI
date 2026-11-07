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
#[cfg(feature = "svg-text")]
use std::sync::{Arc, OnceLock};

const LABEL_ID: &str = "__anti_webui_label_";

/// Makes shapes out of an SVG file, which then go like shapes made by hand: they follow the scale
/// and take no room in the picture cache. What this can't draw (patterns, masks, filters, pictures,
/// blending and clip paths other than rectangles) makes all of it one picture instead, as `img` shows it.
pub fn parse(data: &[u8]) -> Result<Svg, String> {
    let plain = plain_selectors(data);
    let data = plain.as_deref().unwrap_or(data);
    let (without_labels, labels) = take_labels(data);
    let source = without_labels.as_deref().unwrap_or(data);
    let tree = usvg::Tree::from_data(source, &options_for(source)).map_err(|e| e.to_string())?;
    let size = tree.size();
    let (width, height) = (size.width(), size.height());
    let mut root = svg().view_box(0.0, 0.0, width, height);
    root = match root_width(data) {
        // As wide as there is room for, up to what it says, and as tall as that makes it, as a
        // browser shows what Mermaid makes.
        Some((percent, max)) => {
            let r = root.w(Length::Percent(percent));
            match max {
                Some(max) => r.max_w(max),
                None => r,
            }
        }
        None => root.w(Length::Px(width)).h(Length::Px(height)),
    };
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
        // Without the text that was taken out, which is in `placed` instead.
        root.child(
            crate::view::img(source.to_vec())
                .w(Length::Percent(100.0))
                .h(Length::Percent(100.0)),
        )
        .children(placed)
    })
}

/// What `usvg` reads a file with: the fonts of the system when it has text, which few pictures do.
pub(crate) fn options_for(data: &[u8]) -> usvg::Options<'static> {
    #[allow(unused_mut)]
    let mut options = usvg::Options::default();
    #[cfg(feature = "svg-text")]
    if has_text(data) {
        options.fontdb = system_fonts();
    }
    options
}

#[cfg(feature = "svg-text")]
fn has_text(data: &[u8]) -> bool {
    data.windows(5).any(|w| w == b"<text")
}

/// The fonts of the system, read once for every thread: that takes hundreds of milliseconds on
/// macOS, which on the UI thread stops scrolling for as long.
#[cfg(feature = "svg-text")]
fn system_fonts() -> Arc<usvg::fontdb::Database> {
    static FONTS: OnceLock<Arc<usvg::fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = usvg::fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Reads the fonts of the system now if the file needs them for its text, so that [`parse`] on the
/// UI thread later doesn't wait for them. For threads that make SVG files.
#[doc(hidden)]
pub fn prepare_fonts_for(data: &[u8]) {
    #[cfg(feature = "svg-text")]
    if has_text(data) {
        system_fonts();
    }
    #[cfg(not(feature = "svg-text"))]
    let _ = data;
}

/// A width of the file's `<svg>` that is a share of the room around it, with the `max-width` of its
/// style. The height then follows from the `viewBox`.
fn root_width(data: &[u8]) -> Option<(f32, Option<f32>)> {
    let head = &data[..data.len().min(4096)];
    let head = std::str::from_utf8(head)
        .ok()
        .or_else(|| (1..4).find_map(|cut| std::str::from_utf8(&head[..head.len() - cut]).ok()))?;
    let start = head.find("<svg")?;
    let tag = &head[start..start + head[start..].find('>')?];
    let attribute = |name: &str| {
        let mut from = 0;
        while let Some(i) = tag[from..].find(name) {
            let at = from + i;
            from = at + name.len();
            if !tag[..at].ends_with(char::is_whitespace) {
                continue;
            }
            let rest = tag[from..].trim_start().strip_prefix('=')?.trim_start();
            let quote = rest.chars().next()?;
            return rest[1..].split(quote).next();
        }
        None
    };
    let percent: f32 = attribute("width")?
        .trim()
        .strip_suffix('%')?
        .trim()
        .parse()
        .ok()?;
    let max = attribute("style").and_then(|style| {
        style
            .split(';')
            .filter_map(|d| d.split_once(':'))
            .find(|(n, _)| n.trim() == "max-width")
            .and_then(|(_, v)| length(v, 0.0))
    });
    Some((percent, max))
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
        && (g.filters().is_empty() || hard_shadow(g).is_some())
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
    parsed_path_shape(p).into_view()
}

fn parsed_path_shape(p: &usvg::Path) -> crate::svg::Shape {
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
    shape
}

fn parsed_group(g: &usvg::Group, labels: &[Label]) -> Group {
    let mut out = group().transform(transform_of(g.transform()));
    if let Some(shadow) = hard_shadow(g) {
        out = out.child(shadow_of(g, &shadow));
    }
    out = out.children(g.children().iter().filter_map(|n| parsed_node(n, labels)));
    if let Some(quad) = g.clip_path().and_then(clip_rectangle) {
        out = out.clip_quad(quad);
    }
    let opacity = g.opacity().get();
    if opacity < 1.0 {
        out = out.opacity(opacity);
    }
    out
}

/// A drop shadow that isn't blurred, as Mermaid puts under its boxes: the same shapes, moved and
/// in one color.
struct HardShadow {
    dx: f32,
    dy: f32,
    color: Color,
    opacity: f32,
}

fn hard_shadow(g: &usvg::Group) -> Option<HardShadow> {
    let [filter] = g.filters() else {
        return None;
    };
    let [primitive] = filter.primitives() else {
        return None;
    };
    let usvg::filter::Kind::DropShadow(d) = primitive.kind() else {
        return None;
    };
    if d.std_dev_x().get() != 0.0
        || d.std_dev_y().get() != 0.0
        || !matches!(d.input(), usvg::filter::Input::SourceGraphic)
    {
        return None;
    }
    let c = d.color();
    Some(HardShadow {
        dx: d.dx(),
        dy: d.dy(),
        color: Color::new(
            c.red as f32 / 255.0,
            c.green as f32 / 255.0,
            c.blue as f32 / 255.0,
            1.0,
        ),
        opacity: d.opacity().get(),
    })
}

/// The shadow of what is in a group, in its coordinates. It fades as one, so that where its shapes
/// overlap is no darker, as the shadow of what they paint together.
fn shadow_of(g: &usvg::Group, shadow: &HardShadow) -> View {
    fn shapes(g: &usvg::Group, color: Color, out: &mut Vec<View>) {
        for n in g.children() {
            match n {
                N::Group(inner) => {
                    let mut kids = Vec::new();
                    shapes(inner, color, &mut kids);
                    if !kids.is_empty() {
                        out.push(
                            group()
                                .transform(transform_of(inner.transform()))
                                .children(kids)
                                .into_view(),
                        );
                    }
                }
                N::Path(p) if p.is_visible() && !p.id().starts_with(LABEL_ID) => {
                    let mut shape = parsed_path_shape(p);
                    if p.fill().is_some() {
                        shape = shape.fill(SvgPaint::Color(color));
                    }
                    if p.stroke().is_some() {
                        shape = shape.stroke(SvgPaint::Color(color));
                    }
                    out.push(shape.into_view());
                }
                _ => {}
            }
        }
    }
    let mut kids = Vec::new();
    shapes(g, shadow.color, &mut kids);
    group()
        .transform(Affine::translation(shadow.dx as f64, shadow.dy as f64))
        .opacity(shadow.opacity)
        .children(kids)
        .into_view()
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
    /// A line of a plain `<text>`, which is put at a point instead of in a box.
    plain: Option<Plain>,
}

/// How a line of a plain `<text>` is put at its point.
#[derive(Clone)]
struct Plain {
    anchor: TextAnchor,
    baseline: TextBaseline,
    fill: Color,
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
                // `bolder` and `lighter` go from what is inherited, as CSS's table has it.
                self.weight = match value {
                    "normal" => 400,
                    "bold" => 700,
                    "bolder" => match self.weight {
                        0..350 => 400,
                        350..550 => 700,
                        _ => 900,
                    },
                    "lighter" => match self.weight {
                        0..550 => 100,
                        550..750 => 400,
                        _ => 700,
                    },
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

/// The lines of a `<text>` that is only words at points, each with where it is and how it looks,
/// so that it is laid out like the rest of the text here without `usvg` loading every font of the
/// system to lay it out first. What has more to it (letters moved one by one, a path, parts of
/// different styles on a line) gives `None` and is left to `usvg`.
fn plain_text(
    node: roxmltree::Node<'_, '_>,
    sheets: &[simplecss::StyleSheet<'_>],
) -> Option<Vec<(f32, f32, Label)>> {
    let one = |n: roxmltree::Node<'_, '_>, name: &str, of: f32| -> Option<Option<f32>> {
        match n.attribute(name) {
            None => Some(None),
            // A list moves letters one by one.
            Some(v) if v.trim().contains([' ', ',']) => None,
            Some(v) => length(v, of).map(Some),
        }
    };
    for name in ["rotate", "textLength", "lengthAdjust", "writing-mode"] {
        if node.attribute(name).is_some() {
            return None;
        }
    }
    let mut style = Html::default();
    let mut plain = Plain {
        anchor: TextAnchor::Start,
        baseline: TextBaseline::Alphabetic,
        fill: Color::BLACK,
    };
    let ancestors: Vec<_> = node.ancestors().filter(|n| n.is_element()).collect();
    for a in ancestors.iter().rev() {
        if !svg_style_of(*a, &mut style, &mut plain, sheets) {
            return None;
        }
    }
    let size = style.size;
    let x = one(node, "x", size)?.unwrap_or(0.0) + one(node, "dx", size)?.unwrap_or(0.0);
    let y = one(node, "y", size)?.unwrap_or(0.0) + one(node, "dy", size)?.unwrap_or(0.0);
    let mut lines: Vec<(f32, f32, String)> = Vec::new();
    let put = |lines: &mut Vec<(f32, f32, String)>, new_line: bool, x: f32, y: f32, t: &str| {
        let words: Vec<&str> = t.split_whitespace().collect();
        if words.is_empty() {
            return;
        }
        let joined = words.join(" ");
        match lines.last_mut() {
            Some((_, _, line)) if !new_line => {
                if t.starts_with(char::is_whitespace) && !line.is_empty() {
                    line.push(' ');
                }
                line.push_str(&joined);
            }
            _ => lines.push((x, y, joined)),
        }
    };
    let mut at = Cursor {
        x,
        y,
        new_line: true,
    };
    walk_tspans(node, size, &mut at, &mut lines, &put)?;
    Some(
        lines
            .into_iter()
            .map(|(x, y, text)| {
                (
                    x,
                    y,
                    Label {
                        text,
                        style: style.clone(),
                        backgrounds: Vec::new(),
                        plain: Some(plain.clone()),
                    },
                )
            })
            .collect(),
    )
}

/// Where the next words of a plain `<text>` go.
struct Cursor {
    x: f32,
    y: f32,
    /// What comes next starts a line of its own.
    new_line: bool,
}

type Put = dyn Fn(&mut Vec<(f32, f32, String)>, bool, f32, f32, &str);

/// Goes through the words and `tspan` of a `<text>`, at any depth. A `tspan` that is put somewhere
/// starts a line; one with a style of its own makes it more than plain.
fn walk_tspans(
    node: roxmltree::Node<'_, '_>,
    size: f32,
    at: &mut Cursor,
    lines: &mut Vec<(f32, f32, String)>,
    put: &Put,
) -> Option<()> {
    let one = |n: roxmltree::Node<'_, '_>, name: &str| -> Option<Option<f32>> {
        match n.attribute(name) {
            None => Some(None),
            Some(v) if v.trim().contains([' ', ',']) => None,
            Some(v) => length(v, size).map(Some),
        }
    };
    for child in node.children() {
        if child.is_text() {
            put(lines, at.new_line, at.x, at.y, child.text().unwrap_or(""));
            if child.text().is_some_and(|t| !t.trim().is_empty()) {
                at.new_line = false;
            }
            continue;
        }
        if !child.is_element() {
            continue;
        }
        if child.tag_name().name() != "tspan" {
            return None;
        }
        let own = [
            "style",
            "class",
            "font-size",
            "font-weight",
            "font-family",
            "fill",
        ]
        .iter()
        .any(|a| child.attribute(*a).is_some());
        let (tx, ty, dx, dy) = (
            one(child, "x")?,
            one(child, "y")?,
            one(child, "dx")?,
            one(child, "dy")?,
        );
        let starts = tx.is_some() || ty.is_some() || dy.is_some_and(|d| d != 0.0);
        if own || (dx.is_some_and(|d| d != 0.0) && !starts) {
            return None;
        }
        if starts {
            at.x = tx.unwrap_or(at.x) + dx.unwrap_or(0.0);
            at.y = ty.unwrap_or(at.y) + dy.unwrap_or(0.0);
            at.new_line = true;
        }
        walk_tspans(child, size, at, lines, put)?;
    }
    Some(())
}

/// What an element of an `svg` says of the text in it, in its attributes, the style sheets and its
/// own style. Says whether the text can still be plain.
fn svg_style_of(
    node: roxmltree::Node<'_, '_>,
    style: &mut Html,
    plain: &mut Plain,
    sheets: &[simplecss::StyleSheet<'_>],
) -> bool {
    let mut ok = true;
    let mut set = |name: &str, value: &str| {
        let value = value.trim().trim_end_matches("!important").trim();
        match name {
            "fill" => match value {
                "none" | "transparent" => ok = false,
                "currentColor" | "currentcolor" => plain.fill = style.color,
                v if v.starts_with("url(") => ok = false,
                v => {
                    if let Some(c) = css_color(v) {
                        plain.fill = c;
                    }
                }
            },
            "text-anchor" => {
                plain.anchor = match value {
                    "middle" => TextAnchor::Middle,
                    "end" => TextAnchor::End,
                    _ => TextAnchor::Start,
                }
            }
            "dominant-baseline" | "alignment-baseline" => {
                plain.baseline = match value {
                    "central" => TextBaseline::Central,
                    "middle" => TextBaseline::Middle,
                    "hanging" | "text-before-edge" => TextBaseline::Hanging,
                    "auto" | "alphabetic" | "baseline" => TextBaseline::Alphabetic,
                    _ => plain.baseline,
                }
            }
            "stroke" if value != "none" => ok = false,
            "letter-spacing" | "word-spacing" | "text-decoration"
                if value != "normal" && value != "none" && value != "0" =>
            {
                ok = false
            }
            other => style.set(other, value),
        }
    };
    for name in [
        "fill",
        "text-anchor",
        "dominant-baseline",
        "alignment-baseline",
        "font-size",
        "font-family",
        "font-weight",
        "font-style",
        "stroke",
        "color",
    ] {
        if let Some(v) = node.attribute(name) {
            set(name, v);
        }
    }
    let element = XmlElement(node);
    for important in [false, true] {
        for sheet in sheets {
            for rule in &sheet.rules {
                if rule.selector.matches(&element) {
                    for d in &rule.declarations {
                        if d.important == important {
                            set(d.name, d.value);
                        }
                    }
                }
            }
        }
        if !important && let Some(inline) = node.attribute("style") {
            for (n, v) in inline.split(';').filter_map(|d| d.split_once(':')) {
                set(n.trim(), v);
            }
        }
    }
    ok && !style.hidden
}

/// The style sheets with selectors that test how an attribute starts, ends or what it has in it
/// (`^=`, `$=`, `*=`), as Mermaid's have, written as the values in the file that they match. The
/// style sheets of `usvg` don't know them and leave out the whole rule, with the selectors beside
/// them.
fn plain_selectors(data: &[u8]) -> Option<Vec<u8>> {
    if !data.windows(6).any(|w| w == b"<style")
        || !data.windows(2).any(|w| matches!(w, b"^=" | b"$=" | b"*="))
    {
        return None;
    }
    let source = std::str::from_utf8(data).ok()?;
    let doc = roxmltree::Document::parse(source).ok()?;
    let mut out = String::with_capacity(source.len());
    let mut copied = 0;
    for node in doc.descendants().filter(|n| n.tag_name().name() == "style") {
        let range = node.range();
        let Some(start) = source[range.clone()].find('>').map(|i| range.start + i + 1) else {
            continue;
        };
        let Some(end) = source[start..range.end].rfind("</").map(|i| start + i) else {
            continue;
        };
        let css = &source[start..end];
        if !has_attribute_test(css) {
            continue;
        }
        out.push_str(&source[copied..start]);
        match css
            .trim()
            .strip_prefix("<![CDATA[")
            .and_then(|c| c.strip_suffix("]]>"))
        {
            Some(inner) => {
                out.push_str("<![CDATA[");
                out.push_str(&plain_sheet(inner, &doc));
                out.push_str("]]>");
            }
            None => out.push_str(&plain_sheet(css, &doc)),
        }
        copied = end;
    }
    if copied == 0 {
        return None;
    }
    out.push_str(&source[copied..]);
    Some(out.into_bytes())
}

fn has_attribute_test(css: &str) -> bool {
    ["^=", "$=", "*="].iter().any(|op| css.contains(op))
}

fn plain_sheet(css: &str, doc: &roxmltree::Document<'_>) -> String {
    let mut css = css.to_owned();
    while let Some(open) = css.find("/*") {
        let close = css[open..].find("*/").map_or(css.len(), |i| open + i + 2);
        css.replace_range(open..close, "");
    }
    let mut out = String::with_capacity(css.len());
    let mut rest = css.as_str();
    while let Some(open) = rest.find('{') {
        let mut depth = 0;
        let mut close = None;
        for (i, c) in rest[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        close = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(close) = close else {
            break;
        };
        let prelude = &rest[..open];
        if prelude.trim_start().starts_with('@') || !has_attribute_test(prelude) {
            out.push_str(&rest[..=close]);
        } else {
            let selectors: Vec<String> = prelude
                .split(',')
                .flat_map(|s| plain_selector(s.trim(), doc))
                .collect();
            // A rule none of whose selectors match anything goes.
            if !selectors.is_empty() {
                out.push_str(&selectors.join(","));
                out.push_str(&rest[open..=close]);
            }
        }
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    out
}

/// A selector as one selector for each value of the file its first test of how an attribute
/// starts, ends or what it has in it matches, and so on for the tests after it.
fn plain_selector(selector: &str, doc: &roxmltree::Document<'_>) -> Vec<String> {
    let mut from = 0;
    while let Some(open) = selector[from..].find('[').map(|i| from + i) {
        let Some(close) = selector[open..].find(']').map(|i| open + i) else {
            break;
        };
        from = close + 1;
        let inner = &selector[open + 1..close];
        let Some(at) = ["^=", "$=", "*="]
            .iter()
            .filter_map(|op| inner.find(op))
            .min()
        else {
            continue;
        };
        let name = inner[..at].trim();
        let raw = inner[at + 2..].trim();
        let value = match raw.chars().next() {
            Some(q @ ('"' | '\'')) => raw[1..].split(q).next().unwrap_or(""),
            _ => raw.split_whitespace().next().unwrap_or(""),
        };
        if name.contains('|') || value.is_empty() {
            return Vec::new();
        }
        let op = inner.as_bytes()[at];
        let mut values: Vec<&str> = doc
            .descendants()
            .filter_map(|n| n.attribute(name))
            .filter(|v| match op {
                b'^' => v.starts_with(value),
                b'$' => v.ends_with(value),
                _ => v.contains(value),
            })
            .filter(|v| !v.contains(['"', '\\']))
            .collect();
        values.sort_unstable();
        values.dedup();
        return values
            .into_iter()
            .flat_map(|v| {
                let one = format!(
                    "{}[{name}=\"{v}\"]{}",
                    &selector[..open],
                    &selector[close + 1..]
                );
                plain_selector(&one, doc)
            })
            .collect();
    }
    vec![selector.to_owned()]
}

/// Takes what `usvg` knows nothing about out of the file, leaving something in its place that says
/// where it was. That goes through all that `usvg` does, like what is around it being left out.
fn take_labels(data: &[u8]) -> (Option<Vec<u8>>, Vec<Label>) {
    if !data.windows(13).any(|w| w == b"foreignObject") && !data.windows(5).any(|w| w == b"<text") {
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
        if node.tag_name().name() == "text" {
            let range = node.range();
            if range.start < copied {
                continue;
            }
            if let Some(lines) = plain_text(node, &sheets) {
                out.push_str(&source[copied..range.start]);
                copied = range.end;
                let transform = node
                    .attribute("transform")
                    .map(|t| format!(" transform=\"{t}\""))
                    .unwrap_or_default();
                for (x, y, label) in lines {
                    out.push_str(&format!(
                        "<rect id=\"{LABEL_ID}{}\" x=\"{x}\" y=\"{y}\" width=\"1\" height=\"1\"{transform} \
                         style=\"opacity:1;fill:#000;fill-opacity:0;stroke:none;display:inline;visibility:visible\"/>",
                        labels.len()
                    ));
                    labels.push(label);
                }
            }
            continue;
        }
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
            plain: None,
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
    if let Some(plain) = &label.plain {
        let mut words = text(label.text.clone())
            .at(x, y)
            .anchor(plain.anchor)
            .baseline(plain.baseline)
            .text_size(style.size)
            .font_weight(FontWeight(style.weight))
            .whitespace_nowrap()
            .fill(SvgPaint::Color(plain.fill));
        if let Some(family) = &style.family {
            words = words.font_family(FontFamily(family.as_str().into()));
        }
        if style.italic {
            words = words.italic();
        }
        return words.into_view();
    }
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
