//! What fields and annotations look like, as the operators of a PDF. `hayro` draws them like any
//! other part of a document, so what shows and what is saved are the same thing. See "見た目の作り方"
//! in `docs/design/26-pdf.md`.

use crate::embed::Subset;
use crate::marks::PdfAnnotationKind;
use crate::writer::{Entries, Value, number};
use hayro::hayro_syntax::object::dict::keys::{AP, N};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, Object, Stream};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::sync::Arc;

/// How much room a line of text takes, for its size.
const LINE: f64 = 1.15;
/// How big a box is for the text in it, as PDF.js has it.
const BOX_PER_SIZE: f64 = 1.35;
const SMALLEST: f64 = 4.0;
const MULTILINE_SIZE: f64 = 12.0;
/// How far the baseline is above the bottom of a line, for its size.
const BASELINE: f64 = 0.23;
/// What the room of a line is taken from.
const PADDING: f64 = 2.0;
/// The room of a line that a picked option gets.
const PICKED: &str = "0.6 0.757 0.855 rg";

/// The widths of the characters of Helvetica from 32 to 126, for a thousand units.
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];
const HELVETICA_BOLD: [u16; 95] = [
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722, 722, 667,
    611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556,
    278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584,
];
const TIMES: [u16; 95] = [
    250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500, 500,
    500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667, 722, 611,
    556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722, 722, 944, 722,
    722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500, 500, 278, 278, 500,
    278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500, 444, 480, 200, 480, 541,
];

/// What a default appearance string says.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DefaultLook {
    pub font: Vec<u8>,
    /// 0 means as big as fits.
    pub size: f64,
    /// The operators that set the color of the text.
    pub color: String,
}

/// Reads something like `/Helv 12 Tf 0 0 1 rg`.
pub(crate) fn parse_default_look(da: &[u8]) -> DefaultLook {
    let text = String::from_utf8_lossy(da);
    let tokens: Vec<&str> = text.split_ascii_whitespace().collect();
    let mut look = DefaultLook {
        font: Vec::new(),
        size: 0.0,
        color: "0 g".to_owned(),
    };
    for (i, token) in tokens.iter().enumerate() {
        match *token {
            "Tf" if i >= 2 => {
                look.font = tokens[i - 2].trim_start_matches('/').as_bytes().to_vec();
                look.size = tokens[i - 1].parse().unwrap_or(0.0);
            }
            "g" | "rg" | "k" => {
                let count = match *token {
                    "g" => 1,
                    "rg" => 3,
                    _ => 4,
                };
                if i >= count {
                    look.color = tokens[i - count..=i].join(" ");
                }
            }
            _ => {}
        }
    }
    look
}

/// A font with one byte for each character.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SimpleFont {
    /// What goes in the resources to say what font it is.
    pub resource: Value,
    /// The first character it has a width for, and those widths.
    first: u32,
    widths: Arc<[u16]>,
    /// Characters from 160 on are in Latin-1, and not only ASCII means what it does.
    latin: bool,
}

impl SimpleFont {
    /// Helvetica, which everything has. It is what writes text that has nothing else saying how.
    pub fn helvetica() -> Self {
        let resource = Entries::new()
            .with(b"Type", Value::name("Font"))
            .with(b"Subtype", Value::name("Type1"))
            .with(b"BaseFont", Value::name("Helvetica"))
            .with(b"Encoding", Value::name("WinAnsiEncoding"));
        Self {
            resource: Value::Dict(resource),
            first: 32,
            widths: Arc::from(&HELVETICA[..]),
            latin: false,
        }
    }

    /// What a font of a document takes, if it has what a text needs to be laid out with. A subset
    /// that was embedded doesn't, as only what the document used to say is in it.
    pub fn read(font: &Dict<'_>, resource: Value) -> Option<Self> {
        let subtype = font.get::<Name<'_>>(b"Subtype" as &[u8])?;
        if !matches!(subtype.as_ref(), b"Type1" | b"TrueType" | b"MMType1") {
            return None;
        }
        let base = font
            .get::<Name<'_>>(b"BaseFont" as &[u8])
            .map(|n| n.as_ref().to_vec())
            .unwrap_or_default();
        if base.len() > 7 && base[6] == b'+' {
            return None;
        }
        let latin = matches!(
            font.get::<Object<'_>>(b"Encoding" as &[u8]),
            Some(Object::Name(n)) if n.as_ref() == b"WinAnsiEncoding"
        );
        if let (Some(first), Some(widths)) = (
            font.get::<u32>(b"FirstChar" as &[u8]),
            font.get::<Array<'_>>(b"Widths" as &[u8]),
        ) {
            let widths: Vec<u16> = widths.iter::<f64>().map(|w| w as u16).collect();
            return Some(Self {
                resource,
                first,
                widths: Arc::from(widths),
                latin,
            });
        }
        let table: &[u16] = match base.as_slice() {
            b"Helvetica" | b"Helvetica-Oblique" => &HELVETICA,
            b"Helvetica-Bold" | b"Helvetica-BoldOblique" => &HELVETICA_BOLD,
            b"Times-Roman" => &TIMES,
            b"Courier" | b"Courier-Oblique" | b"Courier-Bold" | b"Courier-BoldOblique" => {
                &[600; 95]
            }
            _ => return None,
        };
        Some(Self {
            resource,
            first: 32,
            widths: Arc::from(table),
            latin: false,
        })
    }

    fn width_of(&self, c: char) -> Option<f64> {
        let at = (c as u32).checked_sub(self.first)?;
        self.widths.get(at as usize).map(|w| *w as f64)
    }

    /// Whether it can write all of a text.
    pub fn can_write(&self, text: &str) -> bool {
        text.chars().all(|c| {
            matches!(c, '\n' | '\r')
                || ((c.is_ascii_graphic()
                    || c == ' '
                    || (self.latin && ('\u{a0}'..='\u{ff}').contains(&c)))
                    && self.width_of(c).is_some())
        })
    }
}

/// What writes the text of an appearance.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Face<'a> {
    Simple(&'a SimpleFont),
    /// The part of a font of the system that was embedded, under what it refers to.
    Embedded(&'a Subset, &'a Value),
}

impl Face<'_> {
    /// How wide a character is, for a size of 1.
    fn width(&self, c: char) -> f64 {
        match self {
            Face::Simple(font) => font.width_of(c).unwrap_or(0.0) / 1000.0,
            Face::Embedded(subset, _) => subset.width(c) / 1000.0,
        }
    }

    pub fn text_width(&self, text: &str, size: f64) -> f64 {
        text.chars().map(|c| self.width(c)).sum::<f64>() * size
    }

    fn resource(&self) -> Value {
        match self {
            Face::Simple(font) => font.resource.clone(),
            Face::Embedded(_, font) => (*font).clone(),
        }
    }

    /// A text as a string of a content stream.
    fn encoded(&self, text: &str) -> String {
        let mut out = String::with_capacity(text.len() * 2 + 2);
        out.push('<');
        for c in text.chars() {
            match self {
                Face::Simple(_) => {
                    write!(out, "{:02x}", c as u32).expect("a string takes anything")
                }
                Face::Embedded(subset, _) => {
                    write!(out, "{:04x}", subset.cid(c)).expect("a string takes anything")
                }
            }
        }
        out.push('>');
        out
    }

    pub fn resources(&self) -> Entries {
        Entries::new().with(
            b"Font",
            Value::Dict(Entries::new().with(b"F0", self.resource())),
        )
    }
}

/// Adds what a text takes to what the fonts of a document have to have in them.
pub(crate) fn needs_chars(text: &str, chars: &mut BTreeSet<char>) {
    chars.extend(text.chars().filter(|c| !matches!(c, '\n' | '\r')));
}

/// The lines a text takes in a width. Lines go after a space, and otherwise wherever they have to.
pub(crate) fn wrap(text: &str, face: Face<'_>, size: f64, width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let paragraph = paragraph.trim_end_matches('\r');
        let chars: Vec<char> = paragraph.chars().collect();
        let (mut start, mut used, mut space) = (0, 0.0, None);
        let mut i = 0;
        while i < chars.len() {
            let w = face.width(chars[i]) * size;
            if used + w > width && i > start {
                // Back to the last space, or else right here. A space that doesn't fit breaks by itself.
                let end = if chars[i] == ' ' {
                    i
                } else {
                    space.filter(|&s| s > start).unwrap_or(i)
                };
                let line: String = chars[start..end].iter().collect();
                lines.push(line.trim_end().to_owned());
                start = end;
                while start < chars.len() && chars[start] == ' ' {
                    start += 1;
                }
                i = start;
                used = 0.0;
                space = None;
                continue;
            }
            if chars[i] == ' ' {
                space = Some(i + 1);
            } else if !chars[i].is_ascii() {
                space = Some(i);
            }
            used += w;
            i += 1;
        }
        lines.push(chars[start..].iter().collect::<String>());
    }
    lines
}

/// The frame of a widget, as `MK` and `BS` say.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Frame {
    pub width: f64,
    pub height: f64,
    /// How far it is turned.
    pub rotation: i32,
    pub background: Option<String>,
    pub border: Option<String>,
    pub border_width: f64,
    pub dash: Option<Vec<f64>>,
    pub underline: bool,
    /// What shows when it is on: the character of ZapfDingbats for it.
    pub mark: Option<char>,
}

fn color_ops(colors: Option<Array<'_>>, stroke: bool) -> Option<String> {
    let parts: Vec<f64> = colors?.iter::<f64>().collect();
    let op = match (parts.len(), stroke) {
        (1, false) => "g",
        (1, true) => "G",
        (3, false) => "rg",
        (3, true) => "RG",
        (4, false) => "k",
        (4, true) => "K",
        _ => return None,
    };
    let numbers: Vec<String> = parts.iter().map(|p| number(*p)).collect();
    Some(format!("{} {op}", numbers.join(" ")))
}

impl Frame {
    pub fn read(widget: &Dict<'_>, rect: [f64; 4]) -> Self {
        let mk = widget.get::<Dict<'_>>(b"MK" as &[u8]).unwrap_or_default();
        let bs = widget.get::<Dict<'_>>(b"BS" as &[u8]).unwrap_or_default();
        let border_array: Vec<f64> = widget
            .get::<Array<'_>>(b"Border" as &[u8])
            .map(|b| b.iter::<f64>().collect())
            .unwrap_or_default();
        let border_width = bs
            .get::<f64>(b"W" as &[u8])
            .or(border_array.get(2).copied())
            .unwrap_or(1.0);
        let style = bs
            .get::<Name<'_>>(b"S" as &[u8])
            .map(|s| s.as_ref().to_vec());
        Self {
            width: (rect[2] - rect[0]).abs(),
            height: (rect[3] - rect[1]).abs(),
            rotation: mk.get::<i32>(b"R" as &[u8]).unwrap_or(0).rem_euclid(360),
            background: color_ops(mk.get::<Array<'_>>(b"BG" as &[u8]), false),
            border: color_ops(mk.get::<Array<'_>>(b"BC" as &[u8]), true),
            border_width,
            dash: (style.as_deref() == Some(b"D")).then(|| {
                let dash: Vec<f64> = bs
                    .get::<Array<'_>>(b"D" as &[u8])
                    .map(|d| d.iter::<f64>().collect())
                    .unwrap_or_default();
                if dash.is_empty() { vec![3.0] } else { dash }
            }),
            underline: style.as_deref() == Some(b"U"),
            mark: mk
                .get::<hayro::hayro_syntax::object::String<'_>>(b"CA" as &[u8])
                .and_then(|ca| ca.as_bytes().first().map(|b| *b as char)),
        }
    }

    /// The size of the box things are drawn in, which a rotation changes around.
    pub fn inner_size(&self) -> (f64, f64) {
        if matches!(self.rotation, 90 | 270) {
            (self.height, self.width)
        } else {
            (self.width, self.height)
        }
    }

    fn ops(&self, out: &mut String) {
        let (w, h) = self.inner_size();
        if let Some(background) = &self.background {
            let _ = writeln!(out, "q {background} 0 0 {} {} re f Q", number(w), number(h));
        }
        let Some(border) = self.border.as_ref().filter(|_| self.border_width > 0.0) else {
            return;
        };
        let b = self.border_width;
        out.push_str("q ");
        let _ = write!(out, "{border} {} w ", number(b));
        if let Some(dash) = &self.dash {
            let numbers: Vec<String> = dash.iter().map(|d| number(*d)).collect();
            let _ = write!(out, "[{}] 0 d ", numbers.join(" "));
        }
        if self.underline {
            let _ = writeln!(out, "0 {0} m {1} {0} l S Q", number(b / 2.0), number(w));
        } else {
            let _ = writeln!(
                out,
                "{0} {0} {1} {2} re S Q",
                number(b / 2.0),
                number(w - b),
                number(h - b)
            );
        }
    }

    /// The dictionary of the stream of a look of this frame.
    pub fn stream_dict(&self, resources: Entries) -> Entries {
        form_dict(self.inner_size(), self.rotation, resources)
    }
}

/// The dictionary of a stream that draws in a box of a size, which it maps turned onto the box of
/// what it is for.
pub(crate) fn form_dict((w, h): (f64, f64), rotation: i32, resources: Entries) -> Entries {
    let mut dict = Entries::new()
        .with(b"Type", Value::name("XObject"))
        .with(b"Subtype", Value::name("Form"))
        .with(b"BBox", Value::reals([0.0, 0.0, w, h]))
        .with(b"Resources", Value::Dict(resources));
    let matrix = match rotation {
        90 => Some([0.0, 1.0, -1.0, 0.0, h, 0.0]),
        180 => Some([-1.0, 0.0, 0.0, -1.0, w, h]),
        270 => Some([0.0, -1.0, 1.0, 0.0, 0.0, w]),
        _ => None,
    };
    if let Some(matrix) = matrix {
        dict.set(b"Matrix", Value::reals(matrix));
    }
    dict
}

/// How the text of a field looks, apart from what it says.
#[derive(Clone, Copy, Debug)]
pub(crate) struct TextStyle<'a> {
    pub face: Face<'a>,
    /// 0 means as big as fits.
    pub size: f64,
    pub color: &'a str,
    /// Where it goes: left, middle or right.
    pub quadding: u8,
}

fn text_line(out: &mut String, face: Face<'_>, x: f64, y: f64, line: &str) {
    let _ = writeln!(
        out,
        "1 0 0 1 {} {} Tm {} Tj",
        number(x),
        number(y),
        face.encoded(line)
    );
}

fn begin_text(out: &mut String, frame: &Frame, style: &TextStyle<'_>, size: f64) {
    let (w, h) = frame.inner_size();
    let inset = frame.border_width.max(0.0);
    let _ = writeln!(
        out,
        "/Tx BMC\nq\n{0} {0} {1} {2} re W n\nBT\n{3}\n/F0 {4} Tf",
        number(inset),
        number((w - 2.0 * inset).max(0.0)),
        number((h - 2.0 * inset).max(0.0)),
        style.color,
        number(size)
    );
}

fn end_text(out: &mut String) {
    out.push_str("ET\nQ\nEMC\n");
}

/// A line of text in a box, as big as fits if it doesn't say.
pub(crate) fn single_line(frame: &Frame, text: &str, style: TextStyle<'_>) -> Vec<u8> {
    let (w, h) = frame.inner_size();
    let room = (w - 2.0 * (frame.border_width + PADDING)).max(1.0);
    let size = if style.size > 0.0 {
        style.size
    } else {
        let tall = ((h - 2.0 * frame.border_width) / BOX_PER_SIZE).max(SMALLEST);
        let wide = style.face.text_width(text, tall);
        if wide > room {
            (tall * room / wide).max(SMALLEST)
        } else {
            tall
        }
    };
    let mut out = String::new();
    frame.ops(&mut out);
    begin_text(&mut out, frame, &style, size);
    let wide = style.face.text_width(text, size);
    let left = frame.border_width + PADDING;
    let x = match style.quadding {
        1 => (w - wide) / 2.0,
        2 => w - left - wide,
        _ => left,
    };
    let y = (h - size) / 2.0 + size * BASELINE;
    text_line(&mut out, style.face, x, y, text);
    end_text(&mut out);
    out.into_bytes()
}

/// Text that goes from the top down in as many lines as it takes.
pub(crate) fn multiple_lines(frame: &Frame, text: &str, style: TextStyle<'_>) -> Vec<u8> {
    let (w, h) = frame.inner_size();
    let inset = frame.border_width + PADDING;
    let room = (w - 2.0 * inset).max(1.0);
    let fits = |size: f64| {
        let lines = wrap(text, style.face, size, room);
        (lines.len() as f64 * size * LINE <= h - 2.0 * inset).then_some(lines)
    };
    let (size, lines) = if style.size > 0.0 {
        (style.size, wrap(text, style.face, style.size, room))
    } else {
        let mut size = MULTILINE_SIZE;
        loop {
            match fits(size) {
                Some(lines) => break (size, lines),
                None if size <= SMALLEST => {
                    break (size, wrap(text, style.face, size, room));
                }
                None => size -= 0.5,
            }
        }
    };
    let mut out = String::new();
    frame.ops(&mut out);
    begin_text(&mut out, frame, &style, size);
    let mut y = h - inset - size;
    for line in &lines {
        let wide = style.face.text_width(line, size);
        let x = match style.quadding {
            1 => (w - wide) / 2.0,
            2 => w - inset - wide,
            _ => inset,
        };
        text_line(&mut out, style.face, x, y + size * BASELINE, line);
        y -= size * LINE;
    }
    end_text(&mut out);
    out.into_bytes()
}

/// Characters that each take one of a number of equal cells.
pub(crate) fn comb(frame: &Frame, text: &str, cells: u32, style: TextStyle<'_>) -> Vec<u8> {
    let (w, h) = frame.inner_size();
    let cell = w / cells.max(1) as f64;
    let size = if style.size > 0.0 {
        style.size
    } else {
        ((h - 2.0 * frame.border_width) / BOX_PER_SIZE)
            .min(cell)
            .max(SMALLEST)
    };
    let mut out = String::new();
    frame.ops(&mut out);
    begin_text(&mut out, frame, &style, size);
    let y = (h - size) / 2.0 + size * BASELINE;
    for (i, c) in text.chars().take(cells as usize).enumerate() {
        let one = c.to_string();
        let x = cell * i as f64 + (cell - style.face.text_width(&one, size)) / 2.0;
        text_line(&mut out, style.face, x, y, &one);
    }
    end_text(&mut out);
    out.into_bytes()
}

/// The options of a list that shows, with a background on those that are picked.
pub(crate) fn list(
    frame: &Frame,
    options: &[(String, String)],
    picked: &[String],
    top: usize,
    style: TextStyle<'_>,
) -> Vec<u8> {
    let (w, h) = frame.inner_size();
    let size = if style.size > 0.0 {
        style.size
    } else {
        MULTILINE_SIZE
    };
    let inset = frame.border_width;
    let row = size * LINE;
    let mut out = String::new();
    frame.ops(&mut out);
    let _ = writeln!(
        out,
        "q {0} {0} {1} {2} re W n",
        number(inset),
        number((w - 2.0 * inset).max(0.0)),
        number((h - 2.0 * inset).max(0.0))
    );
    let mut y = h - inset - row;
    for (value, _) in options.iter().skip(top) {
        if y + row < inset {
            break;
        }
        if picked.contains(value) {
            let _ = writeln!(
                out,
                "{PICKED} {} {} {} {} re f",
                number(inset),
                number(y),
                number(w - 2.0 * inset),
                number(row)
            );
        }
        y -= row;
    }
    let _ = writeln!(out, "BT\n{}\n/F0 {} Tf", style.color, number(size));
    let mut y = h - inset - row;
    for (_, shown) in options.iter().skip(top) {
        if y + row < inset {
            break;
        }
        text_line(
            &mut out,
            style.face,
            inset + PADDING,
            y + row * BASELINE,
            shown,
        );
        y -= row;
    }
    out.push_str("ET\nQ\n");
    out.into_bytes()
}

/// A check box or a radio button when it is on or off, with the mark drawn as a shape so that it
/// doesn't take a font.
pub(crate) fn button(frame: &Frame, on: bool, radio: bool) -> Vec<u8> {
    let (w, h) = frame.inner_size();
    let mut out = String::new();
    frame.ops(&mut out);
    if !on {
        return out.into_bytes();
    }
    let mark = frame.mark.unwrap_or(if radio { 'l' } else { '4' });
    let (cx, cy, r) = (w / 2.0, h / 2.0, w.min(h) * 0.3);
    let at = |x: f64, y: f64| format!("{} {}", number(cx + x * r), number(cy + y * r));
    out.push_str("q 0 g 0 G\n");
    match mark {
        'l' => {
            // Four curves make a circle.
            let k = 0.5523;
            let _ = writeln!(
                out,
                "{} m {} {} {} c {} {} {} c {} {} {} c {} {} {} c f",
                at(1.0, 0.0),
                at(1.0, k),
                at(k, 1.0),
                at(0.0, 1.0),
                at(-k, 1.0),
                at(-1.0, k),
                at(-1.0, 0.0),
                at(-1.0, -k),
                at(-k, -1.0),
                at(0.0, -1.0),
                at(k, -1.0),
                at(1.0, -k),
                at(1.0, 0.0)
            );
        }
        '8' => {
            let _ = writeln!(
                out,
                "{} w 1 J {} m {} l {} m {} l S",
                number(r * 0.35),
                at(-1.0, -1.0),
                at(1.0, 1.0),
                at(-1.0, 1.0),
                at(1.0, -1.0)
            );
        }
        'n' => {
            let _ = writeln!(
                out,
                "{} m {} l {} l {} l f",
                at(-1.0, -1.0),
                at(1.0, -1.0),
                at(1.0, 1.0),
                at(-1.0, 1.0)
            );
        }
        'u' => {
            let _ = writeln!(
                out,
                "{} m {} l {} l {} l f",
                at(0.0, -1.0),
                at(1.0, 0.0),
                at(0.0, 1.0),
                at(-1.0, 0.0)
            );
        }
        'H' => {
            let _ = writeln!(
                out,
                "{} m {} l {} l {} l {} l f",
                at(0.0, 1.0),
                at(0.59, -0.81),
                at(-0.95, 0.31),
                at(0.95, 0.31),
                at(-0.59, -0.81)
            );
        }
        _ => {
            let _ = writeln!(
                out,
                "{} w 1 J 1 j {} m {} l {} l S",
                number(r * 0.35),
                at(-1.0, -0.05),
                at(-0.3, -0.8),
                at(1.0, 0.9)
            );
        }
    }
    out.push_str("Q\n");
    out.into_bytes()
}

fn color_string([r, g, b]: [f32; 3]) -> String {
    format!(
        "{} {} {}",
        number(r as f64),
        number(g as f64),
        number(b as f64)
    )
}

/// The look of something that marks text, in the space of the document.
pub(crate) fn markup(kind: PdfAnnotationKind, quads: &[[f64; 8]], color: [f32; 3]) -> Vec<u8> {
    let c = color_string(color);
    let mut out = String::new();
    if kind == PdfAnnotationKind::Highlight {
        let _ = writeln!(out, "/G gs {c} rg");
        for q in quads {
            let _ = writeln!(
                out,
                "{} {} m {} {} l {} {} l {} {} l h f",
                number(q[0]),
                number(q[1]),
                number(q[2]),
                number(q[3]),
                number(q[6]),
                number(q[7]),
                number(q[4]),
                number(q[5]),
            );
        }
        return out.into_bytes();
    }
    let _ = writeln!(out, "{c} RG");
    for q in quads {
        let tall = ((q[0] - q[4]).powi(2) + (q[1] - q[5]).powi(2)).sqrt();
        let along = match kind {
            // In the middle of the box, and otherwise near the bottom of it.
            PdfAnnotationKind::StrikeOut => 0.5,
            _ => 0.92,
        };
        let a = (q[0] + (q[4] - q[0]) * along, q[1] + (q[5] - q[1]) * along);
        let b = (q[2] + (q[6] - q[2]) * along, q[3] + (q[7] - q[3]) * along);
        let _ = writeln!(
            out,
            "{} w {} {} m {} {} l S",
            number((tall / 14.0).max(0.5)),
            number(a.0),
            number(a.1),
            number(b.0),
            number(b.1)
        );
    }
    out.into_bytes()
}

/// What makes a highlight multiply with what is below it, as a marker does.
pub(crate) fn markup_resources() -> Entries {
    let state = Entries::new()
        .with(b"Type", Value::name("ExtGState"))
        .with(b"BM", Value::name("Multiply"));
    Entries::new().with(
        b"ExtGState",
        Value::Dict(Entries::new().with(b"G", Value::Dict(state))),
    )
}

/// The look of lines drawn by hand, in the space of the document.
pub(crate) fn ink(strokes: &[Vec<(f64, f64)>], width: f64, color: [f32; 3]) -> Vec<u8> {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} RG {} w 1 J 1 j",
        color_string(color),
        number(width)
    );
    for stroke in strokes {
        for (i, (x, y)) in stroke.iter().enumerate() {
            let op = if i == 0 { "m" } else { "l" };
            let _ = writeln!(out, "{} {} {op}", number(*x), number(*y));
        }
        if stroke.len() == 1 {
            let (x, y) = stroke[0];
            let _ = writeln!(out, "{} {} l", number(x), number(y));
        }
        out.push_str("S\n");
    }
    out.into_bytes()
}

/// The look of a note: a piece of paper with lines on it, in a box of 20 by 20.
pub(crate) fn note(color: [f32; 3]) -> Vec<u8> {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{} rg 0.2 G 1 w 1 1 18 18 re B\n0.2 G 1.4 w 4 14.5 m 16 14.5 l 4 10 m 16 10 l 4 5.5 m 12 5.5 l S",
        color_string(color)
    );
    out.into_bytes()
}

/// How big a note is, in points.
pub(crate) const NOTE_SIZE: f64 = 20.0;

/// The look of text put on a page, in a box of that size.
pub(crate) fn free_text(
    (w, h): (f64, f64),
    text: &str,
    face: Face<'_>,
    size: f64,
    color: &str,
) -> Vec<u8> {
    let room = (w - 2.0 * PADDING).max(1.0);
    let mut out = String::new();
    let _ = writeln!(out, "BT\n{color}\n/F0 {} Tf", number(size));
    let mut y = h - PADDING - size;
    for line in wrap(text, face, size, room) {
        text_line(&mut out, face, PADDING, y + size * BASELINE, &line);
        y -= size * LINE;
    }
    out.push_str("ET\n");
    out.into_bytes()
}

/// About how big a box a text takes, without knowing what font it gets. Anything other than ASCII
/// takes a square, which is what most of it is.
pub(crate) fn rough_size(text: &str, size: f64, widest: f64) -> (f64, f64) {
    let room = widest - 2.0 * PADDING;
    let (mut lines, mut most, mut used) = (1usize, 0.0f64, 0.0f64);
    for c in text.chars() {
        if c == '\n' {
            lines += 1;
            most = most.max(used);
            used = 0.0;
            continue;
        }
        let wide = match c as u32 {
            code @ 32..=126 => HELVETICA[code as usize - 32] as f64 / 1000.0 * size,
            _ => size,
        };
        if used + wide > room {
            lines += 1;
            most = most.max(used);
            used = 0.0;
        }
        used += wide;
    }
    most = most.max(used);
    // A little more than it takes, so that another font doesn't break the lines differently.
    (
        (most + size + 2.0 * PADDING).min(widest),
        lines as f64 * size * LINE + 2.0 * PADDING,
    )
}

/// The font of a name in what an appearance of a widget has, or in the form.
pub(crate) fn font_named(
    widget: Option<&Dict<'_>>,
    form: Option<&Dict<'_>>,
    name: &[u8],
) -> Option<SimpleFont> {
    if name.is_empty() {
        return None;
    }
    let in_resources = |resources: &Dict<'_>| -> Option<SimpleFont> {
        let fonts = resources.get::<Dict<'_>>(b"Font" as &[u8])?;
        let raw = fonts.get_raw::<Object<'_>>(name)?;
        let font = fonts.get::<Dict<'_>>(name)?;
        let resource = match &raw {
            MaybeRef::Ref(r) => Value::Ref(*r),
            other @ MaybeRef::NotRef(_) => Value::of(other),
        };
        SimpleFont::read(&font, resource)
    };
    widget
        .and_then(|w| w.get::<Dict<'_>>(AP))
        .and_then(|ap| ap.get::<Stream<'_>>(N))
        .and_then(|look| look.dict().get::<Dict<'_>>(b"Resources" as &[u8]))
        .and_then(|r| in_resources(&r))
        .or_else(|| {
            form.and_then(|f| f.get::<Dict<'_>>(b"DR" as &[u8]))
                .and_then(|r| in_resources(&r))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_look_says_its_font_size_and_color() {
        let look = parse_default_look(b"0 0 1 rg /Helv 12 Tf");
        assert_eq!(look.font, b"Helv");
        assert_eq!(look.size, 12.0);
        assert_eq!(look.color, "0 0 1 rg");
        let look = parse_default_look(b"/F1 0 Tf 0.5 g");
        assert_eq!(look.size, 0.0);
        assert_eq!(look.color, "0.5 g");
        let look = parse_default_look(b"");
        assert_eq!(look.color, "0 g");
    }

    #[test]
    fn helvetica_writes_ascii_and_nothing_else() {
        let font = SimpleFont::helvetica();
        assert!(font.can_write("Hello, world\nand more"));
        assert!(!font.can_write("日本語"));
        assert!(!font.can_write("é"));
        assert_eq!(font.width_of('A'), Some(667.0));
    }

    #[test]
    fn lines_go_after_a_space_or_wherever_they_have_to() {
        let font = SimpleFont::helvetica();
        let face = Face::Simple(&font);
        // At 10 points these words are about 25 wide.
        let lines = wrap("aaaaa bbbbb ccccc", face, 10.0, 60.0);
        assert_eq!(lines, ["aaaaa bbbbb", "ccccc"]);
        let lines = wrap("aaaaaaaaaaaaaaaa", face, 10.0, 30.0);
        assert_eq!(lines, ["aaaaa", "aaaaa", "aaaaa", "a"]);
        assert_eq!(wrap("a\nb", face, 10.0, 100.0), ["a", "b"]);
        assert_eq!(wrap("", face, 10.0, 100.0), [""]);
    }

    #[test]
    fn a_turned_look_maps_its_box_on_what_it_is_for() {
        let dict = form_dict((10.0, 20.0), 90, Entries::new());
        assert_eq!(
            dict.get(b"Matrix"),
            Some(&Value::reals([0.0, 1.0, -1.0, 0.0, 20.0, 0.0]))
        );
        assert!(
            form_dict((10.0, 20.0), 0, Entries::new())
                .get(b"Matrix")
                .is_none()
        );
    }

    #[test]
    fn text_as_big_as_fits_stays_inside_of_its_box() {
        let font = SimpleFont::helvetica();
        let frame = Frame {
            width: 100.0,
            height: 20.0,
            rotation: 0,
            background: None,
            border: None,
            border_width: 1.0,
            dash: None,
            underline: false,
            mark: None,
        };
        let style = TextStyle {
            face: Face::Simple(&font),
            size: 0.0,
            color: "0 g",
            quadding: 0,
        };
        let long = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let ops = String::from_utf8(single_line(&frame, long, style)).unwrap();
        let size: f64 = ops
            .split("/F0 ")
            .nth(1)
            .unwrap()
            .split(' ')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(style.face.text_width(long, size) <= 94.0 + 1e-3 || size == SMALLEST);
    }
}
