//! A formula to a [`Drawing`] with RaTeX, kept for when it is asked for again: its numbers, where
//! a line may break in it, and where its references are.

use crate::MathError;
use crate::fonts::font;
use crate::source::{Prepared, marked, prepare};
use anti_webui_core::drawing::{Drawing, DrawingItem, FontData, Glyph, PathGeometry};
use anti_webui_core::style::{Color, FillRule, Style, Styled};
use anti_webui_core::text::shape_glyphs;
use kurbo::BezPath;
use ratex_font::FontId;
use ratex_layout::layout_box::{BoxContent, LayoutBox};
use ratex_layout::spacing::{MathClass, atom_spacing};
use ratex_layout::{
    FallbackGlyphMetrics, LayoutOptions, layout, layout_with_fallback_metrics, to_display_list,
    validate_layout,
};
use ratex_parser::parse_node::{ArrayTag, AtomFamily, ParseNode, StyleStr};
use ratex_types::color::Color as TexColor;
use ratex_types::display_item::{DisplayItem, DisplayList};
use ratex_types::math_style::MathStyle;
use ratex_types::path_command::PathCommand;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// What decides how a formula is laid out, besides its source.
#[derive(Clone)]
pub(crate) struct Options {
    pub display: bool,
    /// The number of its first automatically numbered line.
    pub first: u32,
    /// The color of what `\ref` and `\eqref` show.
    pub link: Color,
    /// What the line a label is on is numbered.
    pub number: Option<Numbering>,
}

/// What a label's line is numbered.
pub(crate) type Numbering = Rc<dyn Fn(&str) -> Option<String>>;

/// A drawing, and where in it references are, to go to them when they are clicked.
#[derive(Clone, Debug)]
pub(crate) struct Drawn {
    pub drawing: Arc<Drawing>,
    pub links: Vec<Link>,
}

/// A reference: from where to where it is across, in em, and the label it refers to.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    pub from: f32,
    pub to: f32,
    pub label: Rc<str>,
}

/// Part of an in-line formula, between places a line may break.
#[derive(Clone, Debug)]
pub(crate) enum Piece {
    Part(Drawn),
    /// `\\` or `\newline`: the line ends here.
    Break,
}

/// What a formula is laid out as.
pub(crate) struct Laid {
    pub body: Drawn,
    /// The numbers of its lines (`\tag`, or `equation`, `align` and the like), in a column as high
    /// as `body`, to go at an end of the line.
    pub tag: Option<Drawn>,
    /// An in-line formula in parts, broken after its relations and binary operators as KaTeX
    /// breaks it.
    pub pieces: Vec<Piece>,
    /// How many lines it numbers automatically.
    pub numbered: u32,
    /// Each label, and what refers to it shows: the number of its line.
    pub labels: Vec<(String, String)>,
    /// For each label on an automatically numbered line, which of the formula's automatic
    /// numbers it is, counted from 0: what it shows when the formula starts elsewhere.
    pub auto: Vec<Option<u32>>,
}

/// What the layout is told the color is where nothing sets it: no color can be negative, so what
/// comes out with it is the text color.
const TEXT_COLOR: TexColor = TexColor {
    r: -1.0,
    g: -1.0,
    b: -1.0,
    a: 1.0,
};

type Kept = Result<Rc<Laid>, MathError>;

/// How many formulas are kept before all are dropped.
const KEPT: usize = 2048;

thread_local! {
    static KEPT_LAID: RefCell<HashMap<Box<str>, Kept>> = RefCell::new(HashMap::new());
    /// How characters the KaTeX fonts lack are drawn: shaped at 1 em.
    static FALLBACK: RefCell<HashMap<char, Option<Rc<FallbackGlyph>>>> = RefCell::new(HashMap::new());
}

/// The formula laid out, from what was kept if it was laid out before on this thread.
pub(crate) fn laid(tex: &str, o: &Options) -> Result<Rc<Laid>, MathError> {
    let prepared = prepare(tex, o.number.as_deref().unwrap_or(&|_| None));
    // The color of references counts only when there are some.
    let link = o.link;
    let link =
        (!prepared.refs.is_empty()).then(|| [link.r, link.g, link.b, link.a].map(f32::to_bits));
    let key: Box<str> = format!(
        "{}|{}|{:?}|{}",
        o.display as u8, o.first, link, prepared.tex
    )
    .into();
    if let Some(found) = KEPT_LAID.with(|k| k.borrow().get(&key).cloned()) {
        return found;
    }
    let made = lay(&prepared, o).map(Rc::new);
    KEPT_LAID.with(|k| {
        let mut kept = k.borrow_mut();
        if kept.len() >= KEPT {
            kept.clear();
        }
        kept.insert(key, made.clone());
    });
    made
}

fn error(e: ratex_parser::error::ParseError) -> MathError {
    MathError {
        message: e.message,
        at: e.loc.map(|l| l.start..l.end),
    }
}

fn lay(prepared: &Prepared, o: &Options) -> Result<Laid, MathError> {
    let mut nodes = ratex_parser::parser::parse(&prepared.tex).map_err(error)?;
    let style = if o.display {
        MathStyle::Display
    } else {
        MathStyle::Text
    };
    let options = LayoutOptions::default()
        .with_style(style)
        .with_color(TEXT_COLOR);
    // KaTeX puts the number of an equation at the end of its line, apart from the rest. The
    // parser leaves `\tag` where it was written, as a node of its own.
    let tag = match nodes
        .iter()
        .position(|n| matches!(n, ParseNode::Tag { .. }))
    {
        Some(_) if !o.display => {
            return Err(MathError {
                message: "\\tag works only in display equations".into(),
                at: None,
            });
        }
        Some(i) => {
            let ParseNode::Tag { body, tag, .. } = nodes.remove(i) else {
                unreachable!()
            };
            nodes.splice(i..i, body);
            Some(tag)
        }
        None => None,
    };
    // The numbers of the lines of an environment that numbers them, from the first one asked for.
    let (numbered, lines) = renumber(&mut nodes, o.first);
    let line_of = |line: usize| -> Option<(String, Option<u32>)> {
        match &tag {
            Some(t) => Some((tag_text(t), None)),
            None => lines.get(line).cloned().flatten(),
        }
    };
    let (labels, auto) = prepared
        .labels
        .iter()
        .map(|(l, line)| {
            let (shown, auto) = line_of(*line).unwrap_or_else(|| ("??".into(), None));
            ((l.clone(), shown), auto)
        })
        .unzip();

    let mut body_box = lay_box(&nodes, &options)?;
    let column = match &tag {
        Some(tag) => Some(vec![(
            0.0,
            lay_box(tag, &options.with_style(MathStyle::Text))?,
        )]),
        None if o.display => take_tags(&mut body_box),
        None => None,
    };
    let mut body = convert(&to_display_list(&body_box), o.link, &prepared.refs);
    let tag = column.map(|rows| {
        let width = rows.iter().map(|(_, b)| b.width).fold(0.0, f64::max) as f32;
        let mut d = Drawing::new(width, 0.0, 0.0);
        let mut links = Vec::new();
        for (y, b) in rows {
            let part = convert(&to_display_list(&b), o.link, &prepared.refs);
            let dx = width - b.width as f32;
            d.ascent = d.ascent.max(part.drawing.ascent - y as f32);
            d.descent = d.descent.max(part.drawing.descent + y as f32);
            d.items.extend(moved(&part.drawing.items, dx, y as f32));
            links.extend(part.links.into_iter().map(|l| Link {
                from: l.from + dx,
                to: l.to + dx,
                ..l
            }));
        }
        Drawn {
            drawing: Arc::new(d),
            links,
        }
    });
    // Equally high, so that on lines of their own their baselines are at the same height.
    let tag = tag.map(|mut tag| {
        let (b, t) = (&body.drawing, &tag.drawing);
        let (ascent, descent) = (b.ascent.max(t.ascent), b.descent.max(t.descent));
        for d in [&mut body.drawing, &mut tag.drawing] {
            let d = Arc::make_mut(d);
            (d.ascent, d.descent) = (ascent, descent);
        }
        tag
    });
    let pieces = if o.display {
        Vec::new()
    } else {
        pieces(&nodes, &body_box, &body, &options, o, &prepared.refs)?
    };
    Ok(Laid {
        body,
        tag,
        pieces,
        numbered,
        labels,
        auto,
    })
}

/// Lays out what the KaTeX fonts lack with the program's fonts. It is laid out again with how wide
/// those draw it, as a page does with what KaTeX's fonts lack.
fn lay_box(nodes: &[ParseNode], options: &LayoutOptions) -> Result<LayoutBox, MathError> {
    let checked = |b: LayoutBox| {
        validate_layout(&b).map_err(|message| MathError { message, at: None })?;
        Ok::<_, MathError>(b)
    };
    let b = checked(layout(nodes, options))?;
    let lacking = lacking(&to_display_list(&b));
    if lacking.is_empty() {
        return Ok(b);
    }
    let metrics: HashMap<char, FallbackGlyphMetrics> = lacking
        .into_iter()
        .filter_map(|ch| {
            let g = fallback(ch)?;
            Some((
                ch,
                FallbackGlyphMetrics {
                    width: g.advance as f64,
                    // KaTeX takes the height of an M for characters its metrics don't have.
                    height: 0.68333,
                    depth: 0.0,
                },
            ))
        })
        .collect();
    if metrics.is_empty() {
        return Ok(b);
    }
    checked(layout_with_fallback_metrics(
        nodes,
        options,
        Arc::new(metrics),
    ))
}

/// What a line of an environment is numbered, and which of the formula's automatic numbers that
/// is, if it is one.
type Line = Option<(String, Option<u32>)>;

/// Numbers the automatically numbered lines of the environments of a formula from `first`, as one
/// document numbers all of its equations (the parser numbers each formula from 1). Returns how many
/// it numbered, and what each line of the first numbering environment is numbered, with which of
/// the automatic numbers it is.
fn renumber(nodes: &mut [ParseNode], first: u32) -> (u32, Vec<Line>) {
    let mut next = first;
    let mut lines = None;
    renumber_in(nodes, first, &mut next, &mut lines);
    (next - first, lines.unwrap_or_default())
}

/// Numbers the environments in `nodes` and in the groups they are in, in the order they are
/// written.
fn renumber_in(nodes: &mut [ParseNode], first: u32, next: &mut u32, lines: &mut Option<Vec<Line>>) {
    for node in nodes.iter_mut() {
        match node {
            ParseNode::Array {
                tags: Some(tags), ..
            } => {
                let mut texts = Vec::with_capacity(tags.len());
                for tag in tags.iter_mut() {
                    texts.push(match tag {
                        ArrayTag::Explicit(parts) if is_automatic(parts) => {
                            if let ParseNode::MathOrd { text, .. } = &mut parts[1] {
                                *text = next.to_string();
                            }
                            *next += 1;
                            Some(((*next - 1).to_string(), Some(*next - 1 - first)))
                        }
                        ArrayTag::Explicit(parts) => Some((tag_text(parts), None)),
                        ArrayTag::Auto(_) => None,
                    });
                }
                lines.get_or_insert(texts);
            }
            ParseNode::Color { body, .. }
            | ParseNode::Styling { body, .. }
            | ParseNode::Sizing { body, .. }
            | ParseNode::OrdGroup { body, .. }
            | ParseNode::LeftRight { body, .. } => renumber_in(body, first, next, lines),
            _ => {}
        }
    }
}

/// Whether the number of a line is one the parser made: `(`, the number and `)`, none of them
/// written.
fn is_automatic(parts: &[ParseNode]) -> bool {
    matches!(
        parts,
        [
            ParseNode::MathOrd { text: a, loc: None, .. },
            ParseNode::MathOrd { loc: None, .. },
            ParseNode::MathOrd { text: b, loc: None, .. },
        ] if a == "(" && b == ")"
    )
}

/// What `\ref` shows for a written `\tag`: its text, without the parentheses `\tag` adds.
fn tag_text(parts: &[ParseNode]) -> String {
    fn text(n: &ParseNode, out: &mut String) {
        match n {
            ParseNode::MathOrd { text: t, .. }
            | ParseNode::TextOrd { text: t, .. }
            | ParseNode::Atom { text: t, .. } => out.push_str(t),
            ParseNode::OrdGroup { body, .. } | ParseNode::Text { body, .. } => {
                body.iter().for_each(|n| text(n, out))
            }
            ParseNode::SpacingNode { .. } => out.push(' '),
            _ => {}
        }
    }
    let mut out = String::new();
    parts.iter().for_each(|n| text(n, &mut out));
    match out.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        Some(inner) => inner.to_owned(),
        None => out,
    }
}

/// Takes the numbers of the lines of an environment out of it, to put them at an end of the line
/// as KaTeX does, rather than next to it as RaTeX does: the first environment that numbers its
/// lines, at the top or in a group that is on the baseline (`\color`, `\left…\right`, `\large`, a
/// box). Returns the baseline of each, below the formula's, with the number scaled as its
/// environment is.
fn take_tags(b: &mut LayoutBox) -> Option<Vec<(f64, LayoutBox)>> {
    let mut out = None;
    take_tags_in(b, 0.0, 1.0, &mut out);
    out.filter(|rows: &Vec<_>| !rows.is_empty())
}

/// Takes the numbers out of `b`, whose baseline is `y` below the formula's and which shows `scale`
/// times its size. Returns how much narrower `b` is.
fn take_tags_in(
    b: &mut LayoutBox,
    y: f64,
    scale: f64,
    out: &mut Option<Vec<(f64, LayoutBox)>>,
) -> f64 {
    if out.is_some() {
        return 0.0;
    }
    let taken = match &mut b.content {
        BoxContent::HBox(children) => children
            .iter_mut()
            .map(|c| take_tags_in(c, y, scale, out))
            .sum(),
        BoxContent::LeftRight { inner, .. } => take_tags_in(inner, y, scale, out),
        BoxContent::Framed { body, .. } => take_tags_in(body, y, scale, out),
        BoxContent::RaiseBox { body, shift } => take_tags_in(body, y - *shift * scale, scale, out),
        BoxContent::Scaled { body, child_scale } => {
            let k = *child_scale;
            take_tags_in(body, y, scale * k, out) * k
        }
        BoxContent::Array {
            row_heights,
            row_depths,
            offset,
            array_inner_width,
            tag_gap_em,
            tag_col_width,
            row_tags,
            ..
        } if *tag_col_width > 0.0 => {
            let mut at = -*offset;
            let mut rows = Vec::new();
            for (r, tag) in row_tags.iter_mut().enumerate() {
                at += row_heights[r];
                if let Some(t) = tag.take() {
                    let t = if (scale - 1.0).abs() < 1e-9 {
                        t
                    } else {
                        LayoutBox {
                            width: t.width * scale,
                            height: t.height * scale,
                            depth: t.depth * scale,
                            color: t.color,
                            content: BoxContent::Scaled {
                                body: Box::new(t),
                                child_scale: scale,
                            },
                        }
                    };
                    rows.push((y + at * scale, t));
                }
                at += row_depths[r];
            }
            let taken = *tag_gap_em + *tag_col_width;
            (*tag_gap_em, *tag_col_width) = (0.0, 0.0);
            debug_assert!((b.width - taken - *array_inner_width).abs() < 1e-6);
            *out = Some(rows);
            taken
        }
        _ => 0.0,
    };
    b.width -= taken;
    taken
}

/// Items moved by `dx` and `dy`.
fn moved(items: &[DrawingItem], dx: f32, dy: f32) -> impl Iterator<Item = DrawingItem> + '_ {
    items.iter().map(move |item| match item {
        DrawingItem::Glyphs {
            font,
            size,
            color,
            glyphs,
        } => DrawingItem::Glyphs {
            font: font.clone(),
            size: *size,
            color: *color,
            glyphs: glyphs
                .iter()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x + dx,
                    y: g.y + dy,
                })
                .collect(),
        },
        DrawingItem::Rect {
            x,
            y,
            width,
            height,
            color,
        } => DrawingItem::Rect {
            x: x + dx,
            y: y + dy,
            width: *width,
            height: *height,
            color: *color,
        },
        DrawingItem::Path {
            geometry,
            rule,
            stroke,
            color,
        } => {
            let mut path = geometry.path().clone();
            path.apply_affine(kurbo::Affine::translate((dx as f64, dy as f64)));
            DrawingItem::Path {
                geometry: PathGeometry::new(path),
                rule: *rule,
                stroke: *stroke,
                color: *color,
            }
        }
    })
}

/// The class of a node for spacing, as RaTeX finds it (its `node_math_class`).
fn class(node: &ParseNode) -> Option<MathClass> {
    let mut pending = vec![node];
    while let Some(current) = pending.pop() {
        return Some(match current {
            ParseNode::MathOrd { .. } | ParseNode::TextOrd { .. } => MathClass::Ord,
            ParseNode::Atom { family, .. } => match family {
                AtomFamily::Bin => MathClass::Bin,
                AtomFamily::Rel => MathClass::Rel,
                AtomFamily::Open => MathClass::Open,
                AtomFamily::Close => MathClass::Close,
                AtomFamily::Punct => MathClass::Punct,
                AtomFamily::Inner => MathClass::Inner,
            },
            ParseNode::OpToken { .. } | ParseNode::Op { .. } | ParseNode::OperatorName { .. } => {
                MathClass::Op
            }
            ParseNode::OrdGroup { .. } => MathClass::Ord,
            ParseNode::GenFrac {
                left_delim,
                right_delim,
                ..
            } => {
                let delim =
                    |d: &Option<String>| d.as_ref().is_some_and(|d| !d.is_empty() && d != ".");
                if delim(left_delim) || delim(right_delim) {
                    MathClass::Ord
                } else {
                    MathClass::Inner
                }
            }
            ParseNode::Sqrt { .. } => MathClass::Ord,
            ParseNode::SupSub { base, .. } => {
                if let Some(base) = base {
                    pending.push(base);
                }
                continue;
            }
            ParseNode::MClass { mclass, .. } | ParseNode::DelimSizing { mclass, .. } => {
                match mclass.as_str() {
                    "mop" => MathClass::Op,
                    "mbin" => MathClass::Bin,
                    "mrel" => MathClass::Rel,
                    "mopen" => MathClass::Open,
                    "mclose" => MathClass::Close,
                    "mpunct" => MathClass::Punct,
                    "minner" => MathClass::Inner,
                    _ => MathClass::Ord,
                }
            }
            ParseNode::SpacingNode { .. } | ParseNode::Kern { .. } | ParseNode::Lap { .. } => {
                continue;
            }
            ParseNode::HtmlMathMl { html, .. } => {
                pending.extend(html.iter().rev());
                continue;
            }
            ParseNode::Html { body, .. } => {
                pending.extend(body.iter().rev());
                continue;
            }
            ParseNode::LeftRight { .. } => MathClass::Inner,
            ParseNode::XArrow { .. } | ParseNode::CdArrow { .. } => MathClass::Rel,
            ParseNode::Enclose { label, .. } if label == "\\centernot" => MathClass::Rel,
            _ => MathClass::Ord,
        });
    }
    None
}

/// Binary operators that have nothing to work on are ordinary, as RaTeX makes them (its
/// `apply_bin_cancellation`).
fn cancelled(raw: &[Option<MathClass>]) -> Vec<Option<MathClass>> {
    let mut eff = raw.to_vec();
    for i in 0..raw.len() {
        if raw[i] != Some(MathClass::Bin) {
            continue;
        }
        let prev = if i == 0 { None } else { raw[i - 1] };
        let next = raw.get(i + 1).copied().flatten();
        let left = matches!(
            prev,
            None | Some(
                MathClass::Bin
                    | MathClass::Open
                    | MathClass::Rel
                    | MathClass::Op
                    | MathClass::Punct
            )
        );
        let right = matches!(
            next,
            None | Some(MathClass::Rel | MathClass::Close | MathClass::Punct)
        );
        if left || right {
            eff[i] = Some(MathClass::Ord);
        }
    }
    eff
}

/// Where a formula is cut, in em from its start, and how high and deep each part is.
type Cuts = (Vec<f32>, Vec<(f32, f32)>);

/// A node at the top of a formula, or in a group KaTeX opens up there (`\color`, `\textcolor`,
/// `\displaystyle`, `\large`, …), with where its box is, in em of the formula.
struct Atom<'a> {
    node: &'a ParseNode,
    x: f32,
    height: f32,
    depth: f32,
}

/// The atoms of `nodes`, laid out as `b`, which is at `x0` and `scale` times its size. RaTeX puts
/// a space before each node with a class; which those are follows from the classes as it finds
/// them. `None` when the boxes are not as the nodes say.
fn atoms<'a>(
    nodes: &'a [ParseNode],
    b: &LayoutBox,
    tight: bool,
    x0: f32,
    scale: f32,
    out: &mut Vec<Atom<'a>>,
) -> Option<()> {
    let BoxContent::HBox(children) = &b.content else {
        return None;
    };
    let eff = cancelled(&nodes.iter().map(class).collect::<Vec<_>>());
    let (mut x, mut at, mut prev) = (0.0, 0, None);
    let mut found = Vec::with_capacity(nodes.len());
    for (node, c) in nodes.iter().zip(&eff) {
        if let (Some(p), Some(c)) = (prev, *c)
            && atom_spacing(p, c, tight) > 0.0
        {
            x += children.get(at)?.width;
            at += 1;
        }
        if c.is_some() {
            prev = *c;
        }
        found.push((node, children.get(at)?, x));
        x += children[at].width;
        at += 1;
    }
    if at != children.len() {
        return None;
    }
    for (node, child, x) in found {
        let x = x0 + x as f32 * scale;
        // KaTeX builds these as fragments of the expression around them, so a line breaks in them
        // as it does outside.
        let opened = match (node, &child.content) {
            (ParseNode::Color { body, .. }, BoxContent::HBox(_)) => {
                let mut inner = Vec::new();
                atoms(body, child, tight, x, scale, &mut inner).map(|_| inner)
            }
            (ParseNode::Styling { style, body, .. }, content) => {
                let tight = matches!(style, StyleStr::Script | StyleStr::Scriptscript);
                let mut inner = Vec::new();
                match content {
                    BoxContent::Scaled {
                        body: b,
                        child_scale,
                    } => atoms(body, b, tight, x, scale * *child_scale as f32, &mut inner),
                    _ => atoms(body, child, tight, x, scale, &mut inner),
                }
                .map(|_| inner)
            }
            (
                ParseNode::Sizing { body, .. },
                BoxContent::Scaled {
                    body: b,
                    child_scale,
                },
            ) => {
                let mut inner = Vec::new();
                atoms(body, b, false, x, scale * *child_scale as f32, &mut inner).map(|_| inner)
            }
            (ParseNode::Sizing { body, .. }, BoxContent::HBox(_)) => {
                let mut inner = Vec::new();
                atoms(body, child, false, x, scale, &mut inner).map(|_| inner)
            }
            _ => None,
        };
        match opened {
            Some(inner) => out.extend(inner),
            None => out.push(Atom {
                node,
                x,
                height: child.height as f32 * scale,
                depth: child.depth as f32 * scale,
            }),
        }
    }
    Some(())
}

/// What a spacing node says of breaking a line: `\allowbreak` lets it break after it, and
/// `\nobreak` and `~` keep it from breaking at the space they are in.
fn glue(node: &ParseNode) -> Option<Glue> {
    match node {
        ParseNode::SpacingNode { text, .. } => Some(match text.as_str() {
            "\\allowbreak" => Glue::Allow,
            "\\nobreak" | "\\nobreakspace" | "~" => Glue::Keep,
            _ => Glue::Space,
        }),
        ParseNode::Kern { .. } => Some(Glue::Space),
        _ => None,
    }
}

#[derive(PartialEq)]
enum Glue {
    Space,
    Allow,
    Keep,
}

/// Where a line may break in a formula laid out as `b`: after each relation and binary operator
/// at its top (and in the groups KaTeX opens up there) and after `\allowbreak`, with the space
/// after it, as KaTeX's `buildHTML` puts them in boxes of their own, unless `\nobreak` or `~` is
/// in that space. Each part is as high as what is in it.
fn breaks(nodes: &[ParseNode], b: &LayoutBox, tight: bool) -> Option<Cuts> {
    let mut list = Vec::new();
    atoms(nodes, b, tight, 0.0, 1.0, &mut list)?;
    // The classes of what is at the top once the groups are opened, as KaTeX sees them there.
    let eff = cancelled(&list.iter().map(|a| class(a.node)).collect::<Vec<_>>());
    let mut cuts = Vec::new();
    let mut heights = vec![(0.0f32, 0.0f32)];
    let mut i = 0;
    while i < list.len() {
        let h = heights.last_mut().unwrap();
        (h.0, h.1) = (h.0.max(list[i].height), h.1.max(list[i].depth));
        let breaks_after = matches!(eff[i], Some(MathClass::Bin | MathClass::Rel))
            || glue(list[i].node) == Some(Glue::Allow);
        if !breaks_after {
            i += 1;
            continue;
        }
        // The space after it goes with it.
        let mut j = i + 1;
        let mut keep = false;
        while j < list.len()
            && let Some(g) = glue(list[j].node)
        {
            keep |= g == Glue::Keep;
            let h = heights.last_mut().unwrap();
            (h.0, h.1) = (h.0.max(list[j].height), h.1.max(list[j].depth));
            j += 1;
        }
        if j < list.len() && !keep {
            cuts.push(list[j].x);
            heights.push((0.0, 0.0));
        }
        i = j;
    }
    Some((cuts, heights))
}

/// An in-line formula in parts a line may break between, and breaks where it says.
fn pieces(
    nodes: &[ParseNode],
    whole: &LayoutBox,
    drawn: &Drawn,
    options: &LayoutOptions,
    o: &Options,
    refs: &[String],
) -> Result<Vec<Piece>, MathError> {
    let tight = options.style.is_tight();
    let lines: Vec<&[ParseNode]> = nodes.split(|n| matches!(n, ParseNode::Cr { .. })).collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push(Piece::Break);
        }
        let (b, d);
        let (b, d) = if lines.len() == 1 {
            (whole, drawn)
        } else {
            b = lay_box(line, options)?;
            d = convert(&to_display_list(&b), o.link, refs);
            (&b, &d)
        };
        match breaks(line, b, tight) {
            Some((cuts, heights)) if !cuts.is_empty() => {
                out.extend(split(d, &cuts, &heights).into_iter().map(Piece::Part))
            }
            _ => out.push(Piece::Part(d.clone())),
        }
    }
    Ok(out)
}

/// A drawing cut at `cuts` (em from its start), each part as high as `heights` says.
fn split(d: &Drawn, cuts: &[f32], heights: &[(f32, f32)]) -> Vec<Drawn> {
    let whole = &d.drawing;
    let part_of = |x: f32| cuts.partition_point(|&c| c <= x + 1e-4);
    let mut starts = vec![0.0];
    starts.extend_from_slice(cuts);
    let mut ends = cuts.to_vec();
    ends.push(whole.width);
    let mut parts: Vec<Drawing> = (0..starts.len())
        .map(|k| {
            Drawing::new(
                ends[k] - starts[k],
                heights[k].0.max(0.0),
                heights[k].1.max(0.0),
            )
        })
        .collect();
    let mut put = |k: usize, item: DrawingItem| {
        parts[k].items.extend(moved(&[item], -starts[k], 0.0));
    };
    for item in &whole.items {
        match item {
            DrawingItem::Glyphs {
                font,
                size,
                color,
                glyphs,
            } => {
                for g in glyphs {
                    put(
                        part_of(g.x),
                        DrawingItem::Glyphs {
                            font: font.clone(),
                            size: *size,
                            color: *color,
                            glyphs: vec![Glyph { ..*g }],
                        },
                    );
                }
            }
            DrawingItem::Rect { x, .. } => put(part_of(*x), item.clone()),
            DrawingItem::Path { geometry, .. } => {
                put(part_of(geometry.bounds().x0 as f32), item.clone())
            }
        }
    }
    // What was cut apart of one run is one run again.
    for p in &mut parts {
        let mut joined: Vec<DrawingItem> = Vec::with_capacity(p.items.len());
        for item in p.items.drain(..) {
            if let (
                Some(DrawingItem::Glyphs {
                    font: f,
                    size: s,
                    color: c,
                    glyphs: into,
                }),
                DrawingItem::Glyphs {
                    font,
                    size,
                    color,
                    glyphs,
                },
            ) = (joined.last_mut(), &item)
                && f.data.id() == font.data.id()
                && f.index == font.index
                && s == size
                && c == color
            {
                into.extend_from_slice(glyphs);
                continue;
            }
            joined.push(item);
        }
        p.items = joined;
    }
    parts
        .into_iter()
        .enumerate()
        .map(|(k, drawing)| Drawn {
            drawing: Arc::new(drawing),
            links: d
                .links
                .iter()
                .filter(|l| part_of(l.from) == k)
                .map(|l| Link {
                    from: l.from - starts[k],
                    to: l.to - starts[k],
                    label: l.label.clone(),
                })
                .collect(),
        })
        .collect()
}

/// A KaTeX font and its glyph for a character, if there is one.
fn katex_glyph(name: &str, ch: u32) -> Option<(&'static FontData, u32)> {
    let id = FontId::parse(name)?;
    let f = font(id)?;
    if let Some(g) = f.glyph(ch) {
        return Some((&f.data, g));
    }
    // As KaTeX's CSS falls back from any of its fonts to KaTeX_Main.
    let main = font(FontId::MainRegular)?;
    main.glyph(ch).map(|g| (&main.data, g))
}

fn lacking(dl: &DisplayList) -> Vec<char> {
    let mut out: Vec<char> = Vec::new();
    for item in &dl.items {
        if let DisplayItem::GlyphPath {
            font, char_code, ..
        } = item
            && katex_glyph(font, *char_code).is_none()
            && let Some(ch) = char::from_u32(*char_code)
            && !out.contains(&ch)
        {
            out.push(ch);
        }
    }
    out
}

/// A character drawn with the program's fonts, at 1 em.
struct FallbackGlyph {
    font: FontData,
    size: f32,
    glyphs: Vec<Glyph>,
    advance: f32,
}

fn fallback(ch: char) -> Option<Rc<FallbackGlyph>> {
    if let Some(found) = FALLBACK.with(|f| f.borrow().get(&ch).cloned()) {
        return found;
    }
    // KaTeX's text is `KaTeX_Main, "Times New Roman", serif`.
    const EM: f32 = 100.0;
    let shaped = shape_glyphs(
        ch.encode_utf8(&mut [0; 4]),
        &Style::default().font_family("serif").text_size(EM),
    );
    let made = shaped.runs.into_iter().next().map(|run| {
        Rc::new(FallbackGlyph {
            font: run.font,
            size: run.size / EM,
            glyphs: run
                .glyphs
                .iter()
                .map(|g| Glyph {
                    id: g.id,
                    x: g.x / EM,
                    y: g.y / EM,
                })
                .collect(),
            advance: shaped.width / EM,
        })
    });
    FALLBACK.with(|f| f.borrow_mut().insert(ch, made.clone()));
    made
}

fn color(c: TexColor) -> Option<Color> {
    (c != TEXT_COLOR).then(|| Color::new(c.r, c.g, c.b, c.a))
}

/// RaTeX's list, whose `y` goes down from the top, as glyph runs, rectangles and shapes whose `y`
/// goes down from the baseline. References, found by their colors, take `link`.
fn convert(dl: &DisplayList, link: Color, refs: &[String]) -> Drawn {
    let top = dl.height as f32;
    let mut d = Drawing::new(dl.width as f32, dl.height as f32, dl.depth as f32);
    let mut links: Vec<Link> = Vec::new();
    let mut paint = |c: TexColor, x: f32, scale: f32| match marked(c.r, c.g, c.b) {
        Some(k) if k < refs.len() => {
            // A glyph of a reference: about half an em wide, as the digits are.
            let (from, to) = (x, x + 0.5 * scale);
            match links
                .iter_mut()
                .find(|l| *l.label == *refs[k] && l.to >= from - 0.3)
            {
                Some(l) => l.to = l.to.max(to),
                None => links.push(Link {
                    from,
                    to,
                    label: refs[k].as_str().into(),
                }),
            }
            Some(link)
        }
        _ => color(c),
    };
    let items = &mut d.items;
    let mut glyph = |font: &FontData, size: f32, color: Option<Color>, g: Glyph| {
        // Runs of the same font, size and color are one run.
        if let Some(DrawingItem::Glyphs {
            font: f,
            size: s,
            color: c,
            glyphs,
        }) = items.last_mut()
            && f.data.id() == font.data.id()
            && f.index == font.index
            && *s == size
            && *c == color
        {
            glyphs.push(g);
            return;
        }
        items.push(DrawingItem::Glyphs {
            font: font.clone(),
            size,
            color,
            glyphs: vec![g],
        });
    };
    let mut shapes = Vec::new();
    for item in &dl.items {
        match item {
            DisplayItem::GlyphPath {
                x,
                y,
                scale,
                font,
                char_code,
                color: c,
            } => {
                let (x, y, scale) = (*x as f32, *y as f32 - top, *scale as f32);
                let c = &paint(*c, x, scale);
                if let Some((f, id)) = katex_glyph(font, *char_code) {
                    glyph(f, scale, *c, Glyph { id, x, y });
                } else if let Some(fb) = char::from_u32(*char_code).and_then(fallback) {
                    for g in &fb.glyphs {
                        let at = Glyph {
                            id: g.id,
                            x: x + g.x * scale,
                            y: y + g.y * scale,
                        };
                        glyph(&fb.font, fb.size * scale, *c, at);
                    }
                }
            }
            DisplayItem::Line {
                x,
                y,
                width,
                thickness,
                color: c,
                dashed,
            } => {
                // `y` is the middle of the line.
                let (x, y, w, t) = (*x as f32, *y as f32 - top, *width as f32, *thickness as f32);
                let rect = |x: f32, w: f32| DrawingItem::Rect {
                    x,
                    y: y - t / 2.0,
                    width: w,
                    height: t,
                    color: color(*c),
                };
                if *dashed {
                    // As RaTeX dashes them: four thicknesses on, four off.
                    let dash = 4.0 * t.max(f32::EPSILON);
                    let mut at = 0.0;
                    while at < w {
                        shapes.push(rect(x + at, dash.min(w - at)));
                        at += 2.0 * dash;
                    }
                } else {
                    shapes.push(rect(x, w));
                }
            }
            DisplayItem::Rect {
                x,
                y,
                width,
                height,
                color: c,
            } => shapes.push(DrawingItem::Rect {
                x: *x as f32,
                y: *y as f32 - top,
                width: *width as f32,
                height: *height as f32,
                color: color(*c),
            }),
            DisplayItem::Path {
                x,
                y,
                commands,
                fill,
                color: c,
            } => {
                let (x, y) = (*x, *y - dl.height);
                let path = |commands: &[PathCommand]| {
                    let mut p = BezPath::new();
                    for cmd in commands {
                        match *cmd {
                            PathCommand::MoveTo { x: cx, y: cy } => p.move_to((x + cx, y + cy)),
                            PathCommand::LineTo { x: cx, y: cy } => p.line_to((x + cx, y + cy)),
                            PathCommand::QuadTo {
                                x1,
                                y1,
                                x: cx,
                                y: cy,
                            } => p.quad_to((x + x1, y + y1), (x + cx, y + cy)),
                            PathCommand::CubicTo {
                                x1,
                                y1,
                                x2,
                                y2,
                                x: cx,
                                y: cy,
                            } => p.curve_to((x + x1, y + y1), (x + x2, y + y2), (x + cx, y + cy)),
                            PathCommand::Close => p.close_path(),
                        }
                    }
                    p
                };
                if *fill {
                    // Each part on its own, even-odd, as RaTeX fills them: the parts of a stretchy
                    // arrow wind opposite ways, and the stem of a tall delimiter is a second outline.
                    let starts: Vec<usize> = commands
                        .iter()
                        .enumerate()
                        .filter(|(i, c)| *i == 0 || matches!(c, PathCommand::MoveTo { .. }))
                        .map(|(i, _)| i)
                        .collect();
                    for (k, &start) in starts.iter().enumerate() {
                        let end = starts.get(k + 1).copied().unwrap_or(commands.len());
                        shapes.push(DrawingItem::Path {
                            geometry: PathGeometry::new(path(&commands[start..end])),
                            rule: FillRule::EvenOdd,
                            stroke: None,
                            color: color(*c),
                        });
                    }
                } else {
                    shapes.push(DrawingItem::Path {
                        geometry: PathGeometry::new(path(commands)),
                        rule: FillRule::NonZero,
                        // The width of a fraction's line, as KaTeX's thin SVG strokes are.
                        stroke: Some(0.04),
                        color: color(*c),
                    });
                }
            }
        }
    }
    // Rules and shapes, as `\colorbox` backgrounds, go under the glyphs.
    let glyphs = std::mem::take(&mut d.items);
    let (under, over): (Vec<_>, Vec<_>) = shapes
        .into_iter()
        .partition(|s| matches!(s, DrawingItem::Rect { .. }));
    d.items = under;
    d.items.extend(glyphs);
    d.items.extend(over);
    Drawn {
        drawing: Arc::new(d),
        links,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts::FONTS;

    fn o(display: bool) -> Options {
        Options {
            display,
            first: 1,
            link: Color::from_rgb(0x0969da),
            number: None,
        }
    }

    fn laid(tex: &str, display: bool) -> Result<Rc<Laid>, MathError> {
        super::laid(tex, &o(display))
    }

    fn glyph_runs(d: &Drawing) -> Vec<(&FontData, f32, Option<Color>, usize)> {
        d.items
            .iter()
            .filter_map(|i| match i {
                DrawingItem::Glyphs {
                    font,
                    size,
                    color,
                    glyphs,
                } => Some((font, *size, *color, glyphs.len())),
                _ => None,
            })
            .collect()
    }

    fn parts(l: &Laid) -> Vec<&Drawing> {
        l.pieces
            .iter()
            .filter_map(|p| match p {
                Piece::Part(d) => Some(&*d.drawing),
                Piece::Break => None,
            })
            .collect()
    }

    #[test]
    fn a_formula_is_glyphs_rules_and_shapes_from_the_baseline() {
        let laid = laid(r"\frac{a}{b} + \overrightarrow{AB}", true).unwrap();
        let d = &laid.body.drawing;
        assert!(d.width > 0.0 && d.ascent > 0.0 && d.descent > 0.0);
        // The fraction's line is a rule above the baseline (on the math axis), the arrow a shape,
        // and the letters glyphs of the KaTeX fonts in the text color.
        let rules: Vec<f32> = d
            .items
            .iter()
            .filter_map(|i| match i {
                DrawingItem::Rect { y, .. } => Some(*y),
                _ => None,
            })
            .collect();
        assert!(rules.iter().any(|y| *y < 0.0 && *y > -0.5), "{rules:?}");
        assert!(
            d.items
                .iter()
                .any(|i| matches!(i, DrawingItem::Path { .. }))
        );
        let runs = glyph_runs(d);
        assert!(!runs.is_empty());
        assert!(runs.iter().all(|r| r.2.is_none()));
        let italic = &font(FontId::MathItalic).unwrap().data;
        assert!(runs.iter().any(|r| r.0.data.id() == italic.data.id()));
    }

    #[test]
    fn colors_that_are_set_stay() {
        let laid = laid(r"\color{red} a + b", false).unwrap();
        let colors: Vec<_> = glyph_runs(&laid.body.drawing).iter().map(|r| r.2).collect();
        assert!(
            colors.contains(&Some(Color::new(1.0, 0.0, 0.0, 1.0))),
            "{colors:?}"
        );
    }

    #[test]
    fn it_is_kept_and_asked_for_again() {
        let a = laid("x^2 + y^2", false).unwrap();
        let b = laid("x^2 + y^2", false).unwrap();
        assert!(Rc::ptr_eq(&a, &b));
        // In display style it is another formula.
        let c = laid("x^2 + y^2", true).unwrap();
        assert!(!Rc::ptr_eq(&a, &c));
    }

    #[test]
    fn what_cant_be_read_says_where() {
        let e = laid(r"\frac{1}{", false).err().unwrap();
        assert!(e.message.contains("end of input"), "{e:?}");
        let e = laid(r"x + \nosuchthing", false).err().unwrap();
        assert!(e.message.contains("Undefined control sequence"), "{e:?}");
        assert_eq!(e.at, Some(4..16));
    }

    #[test]
    fn a_tag_is_apart_and_as_high() {
        let laid = laid(r"\tag{1} y = mx + b", true).unwrap();
        let (tag, body) = (&laid.tag.as_ref().unwrap().drawing, &laid.body.drawing);
        assert!(tag.width > 0.0 && tag.width < body.width);
        assert_eq!((tag.ascent, tag.descent), (body.ascent, body.descent));
        assert!(super::laid(r"\tag{1} y", &o(false)).is_err());
    }

    #[test]
    fn what_the_katex_fonts_lack_is_drawn_with_the_programs() {
        let laid = laid(r"\text{日本} x", false).unwrap();
        let katex: Vec<u64> = FONTS
            .iter()
            .filter_map(|(id, _)| font(*id))
            .map(|f| f.data.data.id())
            .collect();
        let runs = glyph_runs(&laid.body.drawing);
        let other: Vec<_> = runs
            .iter()
            .filter(|r| !katex.contains(&r.0.data.id()))
            .collect();
        assert_eq!(other.iter().map(|r| r.3).sum::<usize>(), 2, "{runs:?}");
        // Laid out as wide as the program's font draws them: about an em each.
        assert!(laid.body.drawing.width > 2.3, "{}", laid.body.drawing.width);
    }

    #[test]
    fn among_words_it_breaks_after_relations_and_binary_operators() {
        let laid = laid(r"a + b = -c \cdot \frac{1}{2}", false).unwrap();
        let p = parts(&laid);
        // `a +`, `b =`, `-c ·`, `½`: the minus has nothing before it, so it is no operator.
        assert_eq!(p.len(), 4, "{p:?}");
        let whole = laid.body.drawing.width;
        let sum: f32 = p.iter().map(|d| d.width).sum();
        assert!((sum - whole).abs() < 1e-4, "{sum} {whole}");
        // Each part starts at its own left and is as high as what is in it: the fraction's is
        // taller than the others.
        assert!(p.iter().all(|d| d.items.iter().all(|i| match i {
            DrawingItem::Glyphs { glyphs, .. } => glyphs.iter().all(|g| g.x >= -1e-3),
            _ => true,
        })));
        assert!(p[3].ascent > p[0].ascent && p[3].descent > p[0].descent);
        // Nothing to break at, and `\\`.
        assert_eq!(laid_parts(r"\frac{a}{b}"), vec![1]);
        assert_eq!(laid_parts(r"a = b \\ c = d"), vec![2, 2]);
        // In a group, nothing breaks; a display formula doesn't.
        assert_eq!(laid_parts(r"{a + b}"), vec![1]);
        assert!(super::laid("a + b", &o(true)).unwrap().pieces.is_empty());
    }

    #[test]
    fn it_breaks_in_the_groups_katex_opens_and_as_it_is_told() {
        assert_eq!(laid_parts(r"\color{red} a + b = c"), vec![3]);
        assert_eq!(laid_parts(r"\textcolor{blue}{a + b} + c"), vec![3]);
        assert_eq!(laid_parts(r"\large a + b"), vec![2]);
        assert_eq!(laid_parts(r"\displaystyle \sum_i a_i = b"), vec![2]);
        assert_eq!(laid_parts(r"a \allowbreak b"), vec![2]);
        assert_eq!(laid_parts(r"a + \nobreak b"), vec![1]);
        assert_eq!(laid_parts(r"a = ~ b"), vec![1]);
        // A part of a group starts where its letters do.
        let laid = laid(r"x + \color{red} a + b", false).unwrap();
        let p = parts(&laid);
        assert_eq!(p.len(), 3);
        let whole = laid.body.drawing.width;
        let sum: f32 = p.iter().map(|d| d.width).sum();
        assert!((sum - whole).abs() < 1e-4);
        assert!(
            glyph_runs(p[2])
                .iter()
                .all(|r| r.2 == Some(Color::new(1.0, 0.0, 0.0, 1.0)))
        );
    }

    /// How many parts each line of an in-line formula is in.
    fn laid_parts(tex: &str) -> Vec<usize> {
        let laid = laid(tex, false).unwrap();
        let mut lines = vec![0];
        for p in &laid.pieces {
            match p {
                Piece::Part(_) => *lines.last_mut().unwrap() += 1,
                Piece::Break => lines.push(0),
            }
        }
        lines
    }

    #[test]
    fn environments_number_from_where_the_document_is() {
        let tex = r"\begin{align} a &= b \label{first} \\ c &= d \nonumber \\ e &= f \tag{X} \label{x} \\ g &= h \label{last} \end{align}";
        let laid = super::laid(
            tex,
            &Options {
                first: 7,
                ..o(true)
            },
        )
        .unwrap();
        // Two lines numbered on their own: `\tag` and `\nonumber` take none.
        assert_eq!(laid.numbered, 2);
        assert_eq!(
            laid.labels,
            vec![
                ("first".into(), "7".into()),
                ("x".into(), "X".into()),
                ("last".into(), "8".into())
            ]
        );
        // The numbers are in a column of their own, as high as the formula, at their lines.
        let tag = &laid.tag.as_ref().unwrap().drawing;
        let body = &laid.body.drawing;
        assert_eq!((tag.ascent, tag.descent), (body.ascent, body.descent));
        let ys: Vec<f32> = tag
            .items
            .iter()
            .filter_map(|i| match i {
                DrawingItem::Glyphs { glyphs, .. } => Some(glyphs[0].y),
                _ => None,
            })
            .collect();
        assert!(ys.windows(2).all(|w| w[0] <= w[1]), "{ys:?}");
        let star = super::laid(r"\begin{align*} a &= b \end{align*}", &o(true)).unwrap();
        assert!(star.tag.is_none() && star.numbered == 0);
        let eq = super::laid(
            r"\begin{equation} E = mc^2 \label{e} \end{equation}",
            &o(true),
        )
        .unwrap();
        assert_eq!(eq.labels, vec![("e".into(), "1".into())]);
        assert!(eq.tag.is_some());
    }

    #[test]
    fn environments_in_groups_are_numbered_at_the_end_of_the_line_too() {
        for tex in [
            r"\color{blue}{\begin{align} a &= b \label{x} \\ c &= d \end{align}}",
            r"\left( \begin{gather} a \label{x} \\ b \end{gather} \right)",
            r"\large \begin{equation} a \label{x} \end{equation}",
        ] {
            let laid = super::laid(
                tex,
                &Options {
                    first: 5,
                    ..o(true)
                },
            )
            .unwrap_or_else(|e| panic!("{tex}: {e:?}"));
            assert_eq!(laid.labels, vec![("x".into(), "5".into())], "{tex}");
            let tag = laid.tag.as_ref().unwrap_or_else(|| panic!("{tex}"));
            assert!(tag.drawing.width > 0.0);
            // The formula is no wider than what is in it: nothing is left where the numbers were.
            let glyphs_end = laid
                .body
                .drawing
                .items
                .iter()
                .filter_map(|i| match i {
                    DrawingItem::Glyphs { glyphs, .. } => {
                        glyphs.iter().map(|g| g.x).reduce(f32::max)
                    }
                    _ => None,
                })
                .fold(0.0, f32::max);
            assert!(
                laid.body.drawing.width < glyphs_end + 1.5,
                "{tex}: {}",
                laid.body.drawing.width
            );
        }
    }

    #[test]
    fn references_show_numbers_and_say_where_they_are() {
        let number = |k: &str| (k == "e").then(|| "4".to_owned());
        let opts = Options {
            number: Some(Rc::new(number)),
            ..o(false)
        };
        let laid = super::laid(r"x = \eqref{e}", &opts).unwrap();
        let d = &laid.body;
        assert_eq!(d.links.len(), 1);
        let l = &d.links[0];
        assert_eq!(&*l.label, "e");
        assert!(l.from > 0.5 && l.to <= d.drawing.width + 0.5, "{l:?}");
        // In the link's color.
        assert!(
            glyph_runs(&d.drawing)
                .iter()
                .any(|r| r.2 == Some(Color::from_rgb(0x0969da)))
        );
        // The parts know where theirs are, from their own starts.
        let last = match laid.pieces.last().unwrap() {
            Piece::Part(p) => p,
            Piece::Break => unreachable!(),
        };
        assert_eq!(last.links.len(), 1);
        assert!(last.links[0].from < 0.5);
    }
}
