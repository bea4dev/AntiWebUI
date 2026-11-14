//! LaTeX formulas, laid out as KaTeX lays them out (by RaTeX) and drawn as AntiWebUI draws text,
//! with no JavaScript. See `docs/design/40-math.md`.
//!
//! ```ignore
//! math(r"\frac{a}{b}")                     // among words, as KaTeX's `displayMode: false`
//! math(r"\sum_{i=1}^n i").display()        // on a line of its own, in the middle
//! rich_text(math(r"a + b = c").runs())     // breaking after `+` and `=` when it must
//! ```

mod fonts;
mod layout;
mod source;

use anti_webui_core::drawing::{DrawingBox, drawing};
use anti_webui_core::prelude::*;
use anti_webui_core::style::Style;
use layout::{Drawn, Options, Piece};
use std::fmt;
use std::ops::Range;
use std::rc::Rc;

/// What is called with the label of a reference that is clicked.
type OnReference = Rc<dyn Fn(&str)>;

/// What KaTeX draws a formula at: 1.21 times the text around it.
pub const EM_SCALE: f32 = 1.21;

/// The color KaTeX shows a formula it can't read in.
pub const ERROR_COLOR: Color = Color::from_rgb(0xcc0000);

/// What `\ref` and `\eqref` show their numbers in, unless told otherwise.
pub const LINK_COLOR: Color = Color::from_rgb(0x0969da);

/// Why a formula can't be laid out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathError {
    pub message: String,
    /// Where in the source, in bytes, when that is known.
    pub at: Option<Range<usize>>,
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.at {
            Some(at) => write!(f, "{} (at {})", self.message, at.start),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for MathError {}

fn options(display: bool, first: u32) -> Options {
    Options {
        display,
        first,
        link: LINK_COLOR,
        number: None,
    }
}

/// Whether a formula can be laid out, and why not. What it lays out is kept, so showing it after
/// costs nothing more.
pub fn check(tex: &str, display: bool) -> Result<(), MathError> {
    layout::laid(tex, &options(display, 1)).map(|_| ())
}

/// What a formula on a line of its own numbers: how many of its lines it numbers on its own
/// (`equation`, `align` and the like, but not `\tag`), and what each `\label` in it refers to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Equations {
    pub numbered: u32,
    /// Each label, and what `\ref` shows for it: the number of its line.
    pub labels: Vec<(String, String)>,
}

/// What a formula on a line of its own numbers when its first automatic number is `first`: for
/// numbering the formulas of a document one after the other, and finding what their labels refer
/// to before showing any of them.
pub fn equations(tex: &str, first: u32) -> Equations {
    // Found once for each source, numbered from 1, and moved to where it starts: a document is
    // numbered again on each change to it, and its formulas start elsewhere when one before
    // them numbers more lines.
    type Found = Rc<(u32, Vec<(String, Result<u32, String>)>)>;
    thread_local! {
        static FOUND: std::cell::RefCell<std::collections::HashMap<Box<str>, Found>> =
            Default::default();
    }
    let found = FOUND.with(|f| f.borrow().get(tex).cloned());
    let found = found.unwrap_or_else(|| {
        let made: Found = Rc::new(match layout::laid(tex, &options(true, 1)) {
            Ok(l) => (
                l.numbered,
                l.labels
                    .iter()
                    .zip(&l.auto)
                    .map(|((label, shown), auto)| (label.clone(), auto.ok_or(shown.clone())))
                    .collect(),
            ),
            Err(_) => (0, Vec::new()),
        });
        FOUND.with(|f| {
            let mut f = f.borrow_mut();
            if f.len() >= 4096 {
                f.clear();
            }
            f.insert(tex.into(), made.clone());
        });
        made
    });
    Equations {
        numbered: found.0,
        labels: found
            .1
            .iter()
            .map(|(label, shown)| {
                let shown = match shown {
                    Ok(i) => (first + i).to_string(),
                    Err(text) => text.clone(),
                };
                (label.clone(), shown)
            })
            .collect(),
    }
}

/// A formula in LaTeX. Among words (in a `rich_text`, through `inline` or [`Math::runs`]), its
/// baseline is theirs. What it can't read shows as written, in [`ERROR_COLOR`].
pub fn math(tex: impl Into<String>) -> Math {
    Math {
        tex: tex.into(),
        options: options(false, 1),
        leqno: false,
        on_reference: None,
        style: Style::default(),
    }
}

pub struct Math {
    tex: String,
    options: Options,
    leqno: bool,
    on_reference: Option<OnReference>,
    style: Style,
}

impl Styled for Math {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl Math {
    /// On a line of its own, in the middle, scrolling sideways when it is wider than there is
    /// room, as KaTeX's `displayMode: true`: big operators and fractions at full size.
    pub fn display(mut self) -> Self {
        self.options.display = true;
        self
    }

    /// The numbers of its lines at the left end rather than the right, as KaTeX's `leqno`.
    pub fn leqno(mut self) -> Self {
        self.leqno = true;
        self
    }

    /// The number of its first automatically numbered line (`equation`, `align` and the like), for
    /// numbering the formulas of a document one after the other. 1 by default.
    pub fn first_number(mut self, n: u32) -> Self {
        self.options.first = n;
        self
    }

    /// What `\ref{label}` and `\eqref{label}` show: the number of the line called `label`, as
    /// [`equations`] finds it. Without it, or when it says `None`, they show `??`.
    pub fn references(mut self, number: impl Fn(&str) -> Option<String> + 'static) -> Self {
        self.options.number = Some(Rc::new(number));
        self
    }

    /// Called with the label of a reference that is clicked.
    pub fn on_reference(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_reference = Some(Rc::new(f));
        self
    }

    /// The color of what references show. [`LINK_COLOR`] by default.
    pub fn reference_color(mut self, color: Color) -> Self {
        self.options.link = color;
        self
    }

    /// A formula among words in parts a line can break between: after its relations and binary
    /// operators, as KaTeX breaks one, and where `\\` says. Each part is an `inline` box; the first
    /// is copied as `$source$`.
    pub fn runs(self) -> Vec<Run> {
        let tex = self.tex.clone();
        if self.options.display {
            return vec![Run::Inline(inline(self).alt(format!("$${tex}$$")))];
        }
        let mut first = true;
        self.pieces()
            .into_iter()
            .map(|p| match p {
                MathPiece::LineBreak => Run::Text("\n".into()),
                MathPiece::Part(part) if part.is_error() => {
                    Run::Span(span(tex.clone()).text_color(ERROR_COLOR).style(&part.style))
                }
                MathPiece::Part(part) => {
                    let alt = if std::mem::take(&mut first) {
                        format!("${tex}$")
                    } else {
                        String::new()
                    };
                    Run::Inline(inline(part).alt(alt))
                }
            })
            .collect()
    }

    /// What [`Math::runs`] makes inline boxes of, for putting them among words some other way.
    pub fn pieces(self) -> Vec<MathPiece> {
        let style = Rc::new(self.style);
        let part = |drawn: Option<Drawn>| {
            MathPiece::Part(MathPart {
                drawn,
                tex: self.tex.as_str().into(),
                style: style.clone(),
                on_reference: self.on_reference.clone(),
            })
        };
        let laid = match layout::laid(&self.tex, &self.options) {
            Ok(laid) => laid,
            Err(_) => return vec![part(None)],
        };
        if self.options.display {
            return vec![part(Some(laid.body.clone()))];
        }
        laid.pieces
            .iter()
            .map(|p| match p {
                Piece::Part(d) => part(Some(d.clone())),
                Piece::Break => MathPiece::LineBreak,
            })
            .collect()
    }
}

/// A part of a formula among words, or where its line ends.
#[derive(Clone)]
pub enum MathPiece {
    Part(MathPart),
    LineBreak,
}

/// A part of a formula among words, between places a line can break; or all of a formula that
/// can't be read, as written.
#[derive(Clone)]
pub struct MathPart {
    drawn: Option<Drawn>,
    tex: Rc<str>,
    style: Rc<Style>,
    on_reference: Option<OnReference>,
}

impl MathPart {
    pub fn is_error(&self) -> bool {
        self.drawn.is_none()
    }
}

impl IntoView for MathPart {
    fn into_view(self) -> View {
        match &self.drawn {
            Some(d) => shown(d, self.on_reference.clone(), &self.tex)
                .style(&self.style)
                .into_view(),
            None => error_view(&self.tex, &self.style),
        }
    }
}

fn error_view(tex: &str, style: &Style) -> View {
    text(tex.to_owned())
        .text_color(ERROR_COLOR)
        .style(style)
        .into_view()
}

/// A drawing that goes to what its references refer to when they are clicked.
fn shown(d: &Drawn, on_reference: Option<OnReference>, tex: &str) -> DrawingBox {
    let tex = tex.to_owned();
    let mut b = drawing(d.drawing.clone())
        .em_scale(EM_SCALE)
        .access(move |a| a.role(Role::Image).label(tex.clone()));
    if let Some(f) = on_reference
        && !d.links.is_empty()
    {
        let (links, width) = (d.links.clone(), d.drawing.width);
        b = b.cursor(CursorIcon::Pointer).on_click(move |e| {
            if width <= 0.0 || e.rect.width <= 0 {
                return;
            }
            let x = e.local.x * width / e.rect.width as f32;
            // The nearest, as long as it is within a quarter of an em.
            if let Some((l, _)) = links
                .iter()
                .map(|l| (l, (l.from - x).max(x - l.to).max(0.0)))
                .filter(|(_, d)| *d <= 0.25)
                .min_by(|a, b| a.1.total_cmp(&b.1))
            {
                f(&l.label);
            }
        });
    }
    b
}

impl IntoView for Math {
    fn into_view(self) -> View {
        let laid = match layout::laid(&self.tex, &self.options) {
            Ok(laid) => laid,
            Err(e) => {
                log::debug!("a formula that can't be laid out: {e}");
                return error_view(&self.tex, &self.style);
            }
        };
        let tex = self.tex;
        let body = shown(&laid.body, self.on_reference.clone(), &tex);
        if !self.options.display {
            return body.style(&self.style).into_view();
        }
        // On a line of the text's line height, in the middle, as `.katex-display` is a block with
        // the formula on a line in it: what is too wide starts at the left and scrolls.
        let line =
            |d: DrawingBox, alt: String| rich_text((inline(d).alt(alt),)).whitespace_nowrap();
        let body = line(body, format!("$${tex}$$"));
        let d = div().overflow_x_scroll().style(&self.style);
        let Some(tag) = &laid.tag else {
            return d.child(body.text_align(TextAlign::Center)).into_view();
        };
        // The numbers at an end, the formula in the middle of the whole line: the room on either
        // side is shared equally. The two drawings are equally high, so their baselines are at the
        // same height.
        let numbers = line(shown(tag, self.on_reference.clone(), &tex), String::new());
        let (left, right) = if self.leqno {
            (div().flex_1().child(numbers), div().flex_1())
        } else {
            (
                div().flex_1(),
                div().flex_1().flex().justify_end().child(numbers),
            )
        };
        d.child(
            div()
                .flex()
                .flex_row()
                .items_start()
                .child(left)
                .child(body.flex_none())
                .child(right),
        )
        .into_view()
    }
}
