//! The text of a page: what each glyph says, where it is, and what copying and searching get. It
//! comes from a device that only keeps the glyphs, as drawing isn't needed for it. Everything here
//! is in points on the page as it shows, with y going down. See `docs/design/26-pdf.md`.

use hayro::hayro_interpret::font::{Glyph, GlyphRun};
use hayro::hayro_interpret::hayro_cmap::{self, BfString, CMap, CMapName, CidFamily, WritingMode};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Device, DrawMode, DrawProps, Image, ImageDrawProps, SoftMask,
};
use hayro::hayro_syntax::object::dict::keys::{
    BASE_FONT, CIDSYSTEMINFO, DESCENDANT_FONTS, ENCODING, ORDERING, REGISTRY, RESOURCES, SUBTYPE,
    TO_UNICODE,
};
use hayro::hayro_syntax::object::{Array, Dict, Name, Object, Stream, String as PdfString};
use hayro::hayro_syntax::page::{Page, Resources};
use hayro::kurbo::{Affine, BezPath, Point, Vec2};
use std::sync::{Arc, OnceLock};

/// The room a glyph takes above and below its baseline, in ems. Fonts say something else, or
/// nothing, so this makes all lines look the same. It adds up to an em, what vertical writing takes.
const ASCENT: f64 = 0.88;
const DESCENT: f64 = 0.12;
/// The units of a glyph's transform in an em.
const UNITS: f64 = 1000.0;
/// What a glyph says when the font doesn't say what it is.
const UNKNOWN: &str = "\u{fffd}";
/// What a glyph with no width says it takes, in ems.
const NO_WIDTH: f64 = 0.5;
/// How far apart, in ems, glyphs of a run can be from those of the one before to be the same ones
/// drawn again, as for bold or as with a fill and a stroke.
const SAME_GLYPH: f64 = 0.1;
/// How far off its baseline, in ems, a glyph can be to be on the same line, like an exponent.
const SAME_LINE: f64 = 0.6;
/// How far back, in ems, a glyph can start to be on the same line.
const BACK_ON_LINE: f64 = 1.0;
/// The gap, in ems, from which there is a space between words.
const SPACE: f64 = 0.15;
/// The same, between characters that don't use spaces. Justified Japanese has gaps of about a
/// quarter of an em that aren't spaces.
const SPACE_WITHOUT_WORDS: f64 = 0.8;
/// The gap, in ems, from which what comes next on a line is something else, like in a table.
const FAR_ON_LINE: f64 = 3.0;
/// What is drawn past the box of a glyph on any side at most, for nearly all fonts, in ems.
const INK_ROOM: f32 = 0.12;
/// How far, in points, a pointer can be from the text to be at it, as in Chromium's viewer.
pub(crate) const NEAR_TEXT: f32 = 20.0;
/// How much further away something counts as being across lines than along one.
const ACROSS_LINES: f32 = 8.0;
/// How many levels of forms to look in for the fonts of a page, and how many forms at most.
const FORM_DEPTH: usize = 4;
const MOST_FORMS: usize = 256;

/// A glyph as it was drawn.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawGlyph {
    /// None when the font doesn't say.
    pub text: Option<Box<str>>,
    /// Where it starts on the baseline.
    pub origin: Point,
    /// A point along the baseline per em, and a point up from it per em.
    pub along: Vec2,
    pub up: Vec2,
    /// How far it goes in ems.
    pub width: f64,
    /// Whether it is in a font written from top to bottom.
    pub vertical: bool,
}

impl RawGlyph {
    /// The next one is expected here.
    fn end(&self) -> Point {
        if self.vertical {
            self.origin - self.up
        } else {
            self.origin + self.along * self.width
        }
    }

    fn size(&self) -> f64 {
        self.up.hypot()
    }

    /// The box it takes, as (left, top, right, bottom).
    fn bounds(&self) -> [f32; 4] {
        let low = self.origin - self.up * DESCENT;
        let width = if self.vertical { 1.0 } else { self.width };
        let high = self.up * (ASCENT + DESCENT);
        let corners = [
            low,
            low + self.along * width,
            low + high,
            low + self.along * width + high,
        ];
        let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        for c in corners {
            b[0] = b[0].min(c.x);
            b[1] = b[1].min(c.y);
            b[2] = b[2].max(c.x);
            b[3] = b[3].max(c.y);
        }
        b.map(|v| v as f32)
    }

    fn same_as(&self, other: &Self) -> bool {
        self.text == other.text
            && (self.origin - other.origin).hypot() < SAME_GLYPH * self.size().max(other.size())
    }
}

/// What fonts of the CJK collections have for each of their glyphs, for those that don't say what
/// their glyphs are. A document keeps them, as they take a moment to read.
#[derive(Default)]
pub(crate) struct CollectionMaps {
    japan: OnceLock<Option<CMap>>,
    china: OnceLock<Option<CMap>>,
    taiwan: OnceLock<Option<CMap>>,
    korea: OnceLock<Option<CMap>>,
}

impl CollectionMaps {
    fn of(&self, family: &CidFamily) -> Option<&CMap> {
        let map = match family {
            CidFamily::AdobeJapan1 => &self.japan,
            CidFamily::AdobeGB1 => &self.china,
            CidFamily::AdobeCNS1 => &self.taiwan,
            CidFamily::AdobeKorea1 => &self.korea,
            _ => return None,
        };
        map.get_or_init(|| read_cmap(family.ucs2_cmap()?)).as_ref()
    }
}

fn read_cmap(name: CMapName<'_>) -> Option<CMap> {
    CMap::parse(hayro_cmap::load_embedded(name)?, hayro_cmap::load_embedded)
}

/// A font of a page that doesn't say what its glyphs are, which its character collection does.
struct SilentFont {
    name: Option<String>,
    /// None when the codes are the numbers of the glyphs in the collection.
    encoding: Option<CMap>,
    family: CidFamily,
}

impl SilentFont {
    fn read(font: &Dict<'_>) -> Option<Self> {
        if font.get::<Name<'_>>(SUBTYPE)?.as_ref() != b"Type0" || font.contains_key(TO_UNICODE) {
            return None;
        }
        let descendant = font
            .get::<Array<'_>>(DESCENDANT_FONTS)?
            .iter::<Dict<'_>>()
            .next()?;
        let info = descendant.get::<Dict<'_>>(CIDSYSTEMINFO)?;
        let family = CidFamily::from_registry_ordering(
            info.get::<PdfString<'_>>(REGISTRY)?.as_bytes(),
            info.get::<PdfString<'_>>(ORDERING)?.as_bytes(),
        );
        family.ucs2_cmap()?;
        let encoding = match font.get::<Object<'_>>(ENCODING)? {
            Object::Name(name) => match CMapName::from_bytes(name.as_ref()) {
                CMapName::IdentityH | CMapName::IdentityV => None,
                other => Some(read_cmap(other)?),
            },
            Object::Stream(stream) => {
                let data = stream.decoded_data().ok()?;
                Some(CMap::parse(&data, hayro_cmap::load_embedded)?)
            }
            _ => return None,
        };
        let name = font
            .get::<Name<'_>>(BASE_FONT)
            .map(|n| strip_subset(n.as_str()).to_owned());
        Some(Self {
            name,
            encoding,
            family,
        })
    }

    fn text(&self, code: u32, maps: &CollectionMaps) -> Option<BfString> {
        let glyph = match &self.encoding {
            None => code,
            Some(encoding) => [2, 1, 3, 4]
                .into_iter()
                .find_map(|len| encoding.lookup_cid_code(code, len))?,
        };
        maps.of(&self.family)?.lookup_bf_string(glyph)
    }
}

fn strip_subset(name: &str) -> &str {
    match name.split_once('+') {
        Some((tag, rest)) if tag.len() == 6 => rest,
        _ => name,
    }
}

/// The fonts of a page that don't say what their glyphs are, also in the forms it draws.
fn silent_fonts(page: &Page<'_>) -> Vec<SilentFont> {
    fn walk(resources: &Resources<'_>, depth: usize, forms: &mut usize, out: &mut Vec<SilentFont>) {
        for name in resources.fonts.keys() {
            if let Some(font) = resources.fonts.get::<Dict<'_>>(name.as_ref())
                && let Some(font) = SilentFont::read(&font)
            {
                out.push(font);
            }
        }
        if depth == 0 {
            return;
        }
        for name in resources.x_objects.keys() {
            if *forms == 0 {
                return;
            }
            let Some(form) = resources.x_objects.get::<Stream<'_>>(name.as_ref()) else {
                continue;
            };
            let dict = form.dict();
            if dict
                .get::<Name<'_>>(SUBTYPE)
                .is_some_and(|s| s.as_ref() == b"Form")
                && let Some(inner) = dict.get::<Dict<'_>>(RESOURCES)
            {
                *forms -= 1;
                walk(&Resources::new(inner), depth - 1, forms, out);
            }
        }
    }
    let mut out = Vec::new();
    let mut forms = MOST_FORMS;
    walk(page.resources(), FORM_DEPTH, &mut forms, &mut out);
    out
}

/// Keeps the glyphs of what is drawn, and nothing else.
pub(crate) struct TextDevice<'p, 'a> {
    page: &'p Page<'a>,
    maps: &'p CollectionMaps,
    silent: Option<Vec<SilentFont>>,
    glyphs: Vec<RawGlyph>,
    /// Where the last run starts, to see if the next one draws it again.
    last_run: usize,
}

impl<'p, 'a> TextDevice<'p, 'a> {
    pub fn new(page: &'p Page<'a>, maps: &'p CollectionMaps) -> Self {
        Self {
            page,
            maps,
            silent: None,
            glyphs: Vec::new(),
            last_run: 0,
        }
    }

    pub fn into_glyphs(self) -> Vec<RawGlyph> {
        self.glyphs
    }

    /// The font a glyph that doesn't say what it is comes from. A device isn't told, so it goes by
    /// the name, and otherwise by what the page has.
    fn silent_font(&mut self, glyph: &Glyph<'a>) -> Option<&SilentFont> {
        let page = self.page;
        let fonts = self.silent.get_or_insert_with(|| silent_fonts(page));
        let Glyph::Outline(outline) = glyph else {
            return None;
        };
        let name = outline.font_data().and_then(|d| d.postscript_name);
        fonts
            .iter()
            .find(|f| f.name.is_some() && f.name == name)
            .or(fonts.first())
    }

    fn text_of(&mut self, glyph: &Glyph<'a>) -> Option<Box<str>> {
        let text = glyph.as_unicode().or_else(|| {
            // What these have for names can be anything, and most of their codes are ASCII anyway.
            if let Glyph::Type3(_) = glyph {
                let code = char::from_u32(glyph.char_code()).filter(char::is_ascii_graphic);
                let space = (glyph.char_code() == 0x20).then_some(' ');
                return code.or(space).map(BfString::Char);
            }
            let maps = self.maps;
            self.silent_font(glyph)?.text(glyph.char_code(), maps)
        })?;
        Some(match text {
            BfString::Char(c) => c.to_string().into_boxed_str(),
            BfString::String(s) => s.into_boxed_str(),
        })
    }
}

impl<'a> Device<'a> for TextDevice<'_, 'a> {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        let vertical = run.writing_mode() == WritingMode::Vertical;
        let start = self.glyphs.len();
        for g in run.glyphs() {
            let text = self.text_of(g);
            self.glyphs.push(raw_glyph(
                g,
                text,
                props.transform * g.transform(),
                vertical,
            ));
        }
        let last = self.last_run..start;
        let again = last.len() == self.glyphs.len() - start
            && !last.is_empty()
            && (0..last.len())
                .all(|i| self.glyphs[last.start + i].same_as(&self.glyphs[start + i]));
        if again {
            self.glyphs.truncate(start);
        } else {
            self.last_run = start;
        }
    }
}

fn raw_glyph(glyph: &Glyph<'_>, text: Option<Box<str>>, full: Affine, vertical: bool) -> RawGlyph {
    let [a, b, c, d, e, f] = full.as_coeffs();
    let width = match glyph {
        Glyph::Outline(g) => g.advance_width(),
        Glyph::Type3(g) => g.advance_width(),
    };
    RawGlyph {
        text,
        origin: Point::new(e, f),
        along: Vec2::new(a, b) * UNITS,
        up: Vec2::new(c, d) * UNITS,
        width: width.map_or(NO_WIDTH, |w| w as f64 / UNITS),
        vertical,
    }
}

/// A glyph of a [`Run`], from where to where it goes along it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct TextGlyph {
    pub start: f32,
    pub end: f32,
    /// Where it starts in the text of the page.
    pub at: u32,
    /// Where it starts in what is searched.
    pub folded_at: u32,
}

/// Glyphs next to each other that take the same room across, as (left, top, right, bottom).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Run {
    pub first: u32,
    pub bounds: [f32; 4],
    /// Whether it goes down the page rather than across.
    pub down: bool,
}

/// The text of a page. See the top of this file.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct PageText {
    /// What copying gets: what the glyphs say with spaces and line breaks in between.
    pub text: String,
    pub glyphs: Vec<TextGlyph>,
    pub runs: Vec<Run>,
    /// What searching looks in: see [`fold`]. Searching keeps it after the rest is gone.
    pub folded: Arc<str>,
    pub links: Vec<crate::links::Link>,
}

impl PageText {
    /// Puts the glyphs in order on lines, as they were drawn, leaving out those off the page.
    pub fn new(raw: &[RawGlyph], page: (f32, f32)) -> Self {
        let mut out = Self::default();
        let mut folded = String::new();
        let mut last: Option<&RawGlyph> = None;
        for glyph in raw {
            let b = glyph.bounds();
            if b[2] < 0.0 || b[3] < 0.0 || b[0] > page.0 || b[1] > page.1 {
                continue;
            }
            out.push(glyph, last, &mut folded);
            last = Some(glyph);
        }
        out.text.truncate(out.text.trim_end().len());
        out.folded = folded.into();
        out
    }

    fn push(&mut self, glyph: &RawGlyph, last: Option<&RawGlyph>, folded: &mut String) {
        let text = glyph.text.as_deref().unwrap_or(UNKNOWN);
        let text = if text.chars().all(char::is_control) {
            ""
        } else if text.chars().any(|c| c as u32 & 0xfffe == 0xfffe) {
            // Not characters at all, which some fonts say when they don't know.
            UNKNOWN
        } else {
            // What is pasted should be letters, and not how a font happened to draw them.
            ligature_letters(text).unwrap_or(text)
        };
        let mut new_run = true;
        if let Some(last) = last {
            let size = last.size().max(glyph.size()).max(f64::EPSILON);
            let way = last.way();
            let gap = glyph.origin - last.end();
            let (ahead, off) = (gap.dot(way) / size, gap.cross(way).abs() / size);
            let turned = glyph.way().cross(way).abs() > 0.1;
            let same_line = !turned && off < SAME_LINE && ahead > -BACK_ON_LINE;
            if !same_line {
                self.break_line(text);
            } else if !self.text.ends_with(char::is_whitespace)
                && !text.starts_with(char::is_whitespace)
            {
                let wordless = self.text.chars().next_back().is_some_and(is_wordless)
                    || text.chars().next().is_some_and(is_wordless);
                if ahead > if wordless { SPACE_WITHOUT_WORDS } else { SPACE } {
                    self.text.push(' ');
                }
            }
            new_run = !same_line
                || off > 1e-3
                || ahead > FAR_ON_LINE
                || (glyph.size() - last.size()).abs() > 1e-3 * size
                || !glyph.is_straight();
        }
        let b = glyph.bounds();
        let way = glyph.way();
        let down = way.y.abs() > way.x.abs();
        if new_run {
            self.runs.push(Run {
                first: self.glyphs.len() as u32,
                bounds: b,
                down,
            });
        }
        let run = self.runs.last_mut().expect("there is one by now");
        run.bounds = [
            run.bounds[0].min(b[0]),
            run.bounds[1].min(b[1]),
            run.bounds[2].max(b[2]),
            run.bounds[3].max(b[3]),
        ];
        let (start, end) = if down { (b[1], b[3]) } else { (b[0], b[2]) };
        self.glyphs.push(TextGlyph {
            start,
            end,
            at: self.text.len() as u32,
            folded_at: folded.len() as u32,
        });
        self.text.push_str(text);
        fold(text, folded);
    }

    /// Ends a line before `next`. A word that went on from a hyphen is one word, as in Chromium's
    /// viewer.
    fn break_line(&mut self, next: &str) {
        // A space the document drew at the end of a line is of no use.
        self.text.truncate(self.text.trim_end_matches(' ').len());
        let end = self.text.len() as u32;
        for glyph in self.glyphs.iter_mut().rev().take_while(|g| g.at > end) {
            glyph.at = end;
        }
        let joined = self.text.strip_suffix('-').is_some_and(|before| {
            before.ends_with(char::is_alphabetic) && next.starts_with(char::is_lowercase)
        });
        if joined {
            self.text.pop();
        } else if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    /// Where a glyph starts in the text, or the end of it.
    fn at(&self, glyph: usize) -> usize {
        self.glyphs
            .get(glyph)
            .map_or(self.text.len(), |g| (g.at as usize).min(self.text.len()))
    }

    fn glyphs_of(&self, run: usize) -> std::ops::Range<usize> {
        let first = self.runs[run].first as usize;
        let end = self
            .runs
            .get(run + 1)
            .map_or(self.glyphs.len(), |r| r.first as usize);
        first..end
    }

    /// The place between glyphs nearest to a point, and how far the text is from there.
    pub fn caret_at(&self, x: f32, y: f32) -> Option<Caret> {
        let off = |b: &[f32; 4]| {
            let dx = (b[0] - x).max(x - b[2]).max(0.0);
            let dy = (b[1] - y).max(y - b[3]).max(0.0);
            (dx, dy)
        };
        // A point beside a short line is on that line, however long the one above it is.
        let rank = |r: &Run| {
            let (dx, dy) = off(&r.bounds);
            let (along, across) = if r.down { (dy, dx) } else { (dx, dy) };
            along * along + (ACROSS_LINES * across).powi(2)
        };
        // What overlaps goes to the smaller box, which is on top of the other one more often.
        let area = |r: &Run| (r.bounds[2] - r.bounds[0]) * (r.bounds[3] - r.bounds[1]);
        let run = (0..self.runs.len()).min_by(|&a, &b| {
            let (a, b) = (&self.runs[a], &self.runs[b]);
            rank(a)
                .total_cmp(&rank(b))
                .then_with(|| area(a).total_cmp(&area(b)))
        })?;
        let v = if self.runs[run].down { y } else { x };
        let glyphs = self.glyphs_of(run);
        let forward = self.glyphs[glyphs.start].start <= self.glyphs[glyphs.end - 1].start;
        let glyph = glyphs.clone().min_by(|&a, &b| {
            let off = |g: &TextGlyph| (g.start - v).max(v - g.end).max(0.0);
            off(&self.glyphs[a]).total_cmp(&off(&self.glyphs[b]))
        })?;
        let g = &self.glyphs[glyph];
        let after = (v > (g.start + g.end) / 2.0) == forward;
        let (dx, dy) = off(&self.runs[run].bounds);
        Some(Caret {
            at: glyph + after as usize,
            glyph,
            distance: dx.hypot(dy),
        })
    }

    /// The boxes of the glyphs in a range, with those next to each other on a line as one.
    pub fn boxes(&self, from: usize, to: usize) -> Vec<[f32; 4]> {
        self.boxes_with_room(from, to, 0.0)
    }

    /// The same with room around them for what sticks out of them, like the tail of a g.
    pub fn ink_boxes(&self, from: usize, to: usize) -> Vec<[f32; 4]> {
        self.boxes_with_room(from, to, INK_ROOM)
    }

    /// `room` is how much on all sides, in ems.
    fn boxes_with_room(&self, from: usize, to: usize, room: f32) -> Vec<[f32; 4]> {
        let mut out: Vec<[f32; 4]> = Vec::new();
        if from >= to {
            return out;
        }
        let first_run = self
            .runs
            .partition_point(|r| r.first as usize <= from)
            .saturating_sub(1);
        for run in first_run..self.runs.len() {
            let glyphs = self.glyphs_of(run);
            if glyphs.start >= to {
                break;
            }
            let start = glyphs.start.max(from);
            let end = glyphs.end.min(to);
            if start >= end {
                continue;
            }
            let (a, b) = (&self.glyphs[start], &self.glyphs[end - 1]);
            let (low, high) = (a.start.min(b.start), a.end.max(b.end));
            let r = &self.runs[run];
            let rect = if r.down {
                let room = room * (r.bounds[2] - r.bounds[0]);
                [
                    r.bounds[0] - room,
                    low - room,
                    r.bounds[2] + room,
                    high + room,
                ]
            } else {
                let room = room * (r.bounds[3] - r.bounds[1]);
                [
                    low - room,
                    r.bounds[1] - room,
                    high + room,
                    r.bounds[3] + room,
                ]
            };
            match out.last_mut() {
                Some(last) if on_same_line(last, &rect) => {
                    last[0] = last[0].min(rect[0]);
                    last[1] = last[1].min(rect[1]);
                    last[2] = last[2].max(rect[2]);
                    last[3] = last[3].max(rect[3]);
                }
                _ => out.push(rect),
            }
        }
        out
    }

    /// What copying a range gets.
    pub fn copied(&self, from: usize, to: usize) -> &str {
        if from >= to {
            return "";
        }
        self.text[self.at(from)..self.at(to)].trim_end()
    }

    /// Whether two glyphs next to each other are of one word.
    fn same_word(&self, glyph: usize) -> bool {
        let (a, b, c) = (self.at(glyph), self.at(glyph + 1), self.at(glyph + 2));
        if self.text[a..b].ends_with(char::is_whitespace) {
            return false;
        }
        let class = |range: &str| range.chars().next().map(CharClass::of);
        match (class(&self.text[a..b]), class(&self.text[b..c])) {
            // A hyphen that is gone says nothing.
            (None, _) | (_, None) => true,
            (Some(a), Some(b)) => a == b && a != CharClass::Other,
        }
    }

    /// The word a glyph is in, as a range of glyphs.
    pub fn word_at(&self, glyph: usize) -> (usize, usize) {
        let mut from = glyph;
        while from > 0 && self.same_word(from - 1) {
            from -= 1;
        }
        let mut to = glyph;
        while to + 1 < self.len() && self.same_word(to) {
            to += 1;
        }
        (from, to + 1)
    }

    /// The line a glyph is on, as a range of glyphs.
    pub fn line_at(&self, glyph: usize) -> (usize, usize) {
        let at = self.at(glyph);
        let start = self.text[..at].rfind('\n').map_or(0, |i| i + 1);
        let end = self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i);
        (
            self.glyphs.partition_point(|g| (g.at as usize) < start),
            self.glyphs.partition_point(|g| (g.at as usize) < end),
        )
    }

    /// The glyphs of something found in what is searched.
    pub fn found_glyphs(&self, found: (u32, u32)) -> (usize, usize) {
        let from = self.glyphs.partition_point(|g| g.folded_at <= found.0);
        let to = self.glyphs.partition_point(|g| g.folded_at < found.1);
        (from.saturating_sub(1), to)
    }
}

/// Whether two boxes are next to each other on a line.
fn on_same_line(a: &[f32; 4], b: &[f32; 4]) -> bool {
    let (ah, bh) = (a[3] - a[1], b[3] - b[1]);
    let overlap = a[3].min(b[3]) - a[1].max(b[1]);
    let gap = (b[0] - a[2]).max(a[0] - b[2]);
    overlap > 0.5 * ah.min(bh) && gap < ah.max(bh)
}

/// See [`PageText::caret_at`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Caret {
    /// The glyph after it.
    pub at: usize,
    /// The glyph nearest to the point.
    pub glyph: usize,
    pub distance: f32,
}

/// What a double click takes together.
#[derive(Clone, Copy, PartialEq)]
enum CharClass {
    Space,
    Word,
    Ideograph,
    Hiragana,
    Katakana,
    Other,
}

impl CharClass {
    fn of(c: char) -> Self {
        match c {
            c if c.is_whitespace() => Self::Space,
            '\u{3041}'..='\u{309f}' => Self::Hiragana,
            '\u{30a0}'..='\u{30ff}' | '\u{ff66}'..='\u{ff9f}' => Self::Katakana,
            '\u{3400}'..='\u{4dbf}' | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' => {
                Self::Ideograph
            }
            '\u{20000}'..='\u{3ffff}' => Self::Ideograph,
            '_' => Self::Word,
            c if c.is_alphanumeric() => Self::Word,
            _ => Self::Other,
        }
    }
}

/// All of a folded query in folded text, as ranges of it.
pub(crate) fn find_all(folded: &str, query: &str) -> Vec<(u32, u32)> {
    folded
        .match_indices(query)
        .map(|(at, found)| (at as u32, (at + found.len()) as u32))
        .collect()
}

impl RawGlyph {
    /// Whether it is level or upright, for which a box says where it is.
    fn is_straight(&self) -> bool {
        self.along.x.abs().min(self.along.y.abs()) < 0.05 * self.along.hypot()
    }

    /// The direction it goes in. Top to bottom it goes against what is up from it.
    fn way(&self) -> Vec2 {
        let along = if self.vertical { -self.up } else { self.along };
        along / along.hypot().max(f64::EPSILON)
    }
}

/// Characters that are written without spaces between words.
pub(crate) fn is_wordless(c: char) -> bool {
    matches!(c,
        '\u{2e80}'..='\u{2fdf}' | '\u{3000}'..='\u{30ff}' | '\u{3400}'..='\u{4dbf}'
        | '\u{4e00}'..='\u{9fff}' | '\u{f900}'..='\u{faff}' | '\u{ff00}'..='\u{ffef}'
        | '\u{20000}'..='\u{3ffff}')
}

/// The letters of a ligature.
fn ligature_letters(text: &str) -> Option<&'static str> {
    let mut chars = text.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c {
        '\u{fb00}' => "ff",
        '\u{fb01}' => "fi",
        '\u{fb02}' => "fl",
        '\u{fb03}' => "ffi",
        '\u{fb04}' => "ffl",
        '\u{fb05}' | '\u{fb06}' => "st",
        _ => return None,
    })
}

/// What searching compares, so that the same word is found however it is written and wherever a
/// line breaks it: no case, no spaces, no hyphens, and the wide forms of Latin letters and digits
/// as well as ligatures as what they stand for.
pub(crate) fn fold(text: &str, out: &mut String) {
    for c in text.chars() {
        if let Some(letters) = ligature_letters(c.encode_utf8(&mut [0; 4])) {
            out.push_str(letters);
            continue;
        }
        let c = match c {
            // Wide forms, from `!` to `~`.
            '\u{ff01}'..='\u{ff5e}' => char::from_u32(c as u32 - 0xfee0).unwrap_or(c),
            '\u{3000}' => ' ',
            '\u{2018}' | '\u{2019}' | '\u{201a}' | '\u{201b}' => '\'',
            '\u{201c}' | '\u{201d}' | '\u{201e}' | '\u{201f}' => '"',
            c => c,
        };
        if c.is_whitespace() || matches!(c, '-' | '\u{ad}' | '\u{2010}' | '\u{2011}' | '\u{fffd}') {
            continue;
        }
        out.extend(c.to_lowercase());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A glyph of a level line in a page with y going down, as they come out of a document.
    fn glyph(text: &str, x: f64, y: f64, size: f64, width: f64) -> RawGlyph {
        RawGlyph {
            text: Some(text.into()),
            origin: Point::new(x, y),
            along: Vec2::new(size, 0.0),
            up: Vec2::new(0.0, -size),
            width,
            vertical: false,
        }
    }

    /// A line of text with glyphs half an em wide.
    fn line(text: &str, x: f64, y: f64, size: f64) -> Vec<RawGlyph> {
        text.chars()
            .enumerate()
            .map(|(i, c)| glyph(&c.to_string(), x + i as f64 * size / 2.0, y, size, 0.5))
            .collect()
    }

    const PAGE: (f32, f32) = (600.0, 800.0);

    fn page_of(lines: &[Vec<RawGlyph>]) -> PageText {
        PageText::new(&lines.concat(), PAGE)
    }

    #[test]
    fn a_gap_is_a_space_and_another_line_is_a_line_break() {
        let mut first = line("foo", 10.0, 100.0, 10.0);
        first.extend(line("bar", 35.0, 100.0, 10.0));
        let text = page_of(&[first, line("baz", 10.0, 112.0, 10.0)]);
        assert_eq!(text.text, "foo bar\nbaz");
    }

    #[test]
    fn kerning_is_not_a_space() {
        let mut first = line("fo", 10.0, 100.0, 10.0);
        first.extend(line("o", 20.5, 100.0, 10.0));
        assert_eq!(page_of(&[first]).text, "foo");
    }

    #[test]
    fn a_space_that_is_drawn_does_not_become_two() {
        let text = page_of(&[line("a b", 10.0, 100.0, 10.0)]);
        assert_eq!(text.text, "a b");
    }

    #[test]
    fn spaces_at_the_end_of_a_line_go() {
        let text = page_of(&[
            line("a   ", 10.0, 100.0, 10.0),
            line("b", 10.0, 112.0, 10.0),
        ]);
        assert_eq!(text.text, "a\nb");
        // Those spaces have nothing left to say, and everything after them stays where it is.
        assert_eq!(text.copied(0, 5), "a\nb");
        assert_eq!(text.copied(4, 5), "b");
    }

    #[test]
    fn a_word_goes_on_after_a_hyphen_at_the_end_of_a_line() {
        let text = page_of(&[
            line("exam-", 10.0, 100.0, 10.0),
            line("ple", 10.0, 112.0, 10.0),
        ]);
        assert_eq!(text.text, "example");
        assert_eq!(&*text.folded, "example");
        // Not when what comes next can start a word of its own.
        let text = page_of(&[
            line("self-", 10.0, 100.0, 10.0),
            line("Made", 10.0, 112.0, 10.0),
        ]);
        assert_eq!(text.text, "self-\nMade");
    }

    #[test]
    fn japanese_has_no_spaces_for_justified_gaps() {
        let mut first = vec![glyph("日", 10.0, 100.0, 10.0, 1.0)];
        first.push(glyph("本", 22.5, 100.0, 10.0, 1.0));
        first.push(glyph("A", 35.0, 100.0, 10.0, 0.5));
        assert_eq!(page_of(&[first]).text, "日本A");
    }

    #[test]
    fn an_exponent_stays_on_the_line() {
        let mut first = line("x", 10.0, 100.0, 10.0);
        first.push(glyph("2", 15.0, 96.0, 7.0, 0.5));
        first.extend(line("y", 18.5, 100.0, 10.0));
        let text = page_of(&[first]);
        assert_eq!(text.text, "x2y");
        // It is one box when selected, whatever else it is.
        assert_eq!(text.boxes(0, 3).len(), 1);
    }

    #[test]
    fn what_is_off_the_page_is_not_there() {
        let text = page_of(&[
            line("in", 10.0, 100.0, 10.0),
            line("out", 10.0, 900.0, 10.0),
        ]);
        assert_eq!(text.text, "in");
        assert_eq!(text.len(), 2);
    }

    #[test]
    fn glyphs_that_are_not_known_show_as_such_and_are_not_searched() {
        let mut raw = line("a", 10.0, 100.0, 10.0);
        raw.push(RawGlyph {
            text: None,
            ..glyph("", 15.0, 100.0, 10.0, 0.5)
        });
        raw.extend(line("b", 20.0, 100.0, 10.0));
        let text = PageText::new(&raw, PAGE);
        assert_eq!(text.text, "a\u{fffd}b");
        assert_eq!(&*text.folded, "ab");
    }

    #[test]
    fn a_ligature_is_copied_as_its_letters() {
        let text = page_of(&[line("ef\u{fb01}cient", 10.0, 100.0, 10.0)]);
        assert_eq!(text.text, "efficient");
        // It is one glyph still, so selecting it takes all of it.
        assert_eq!(text.len(), 8);
        assert_eq!(text.copied(2, 3), "fi");
    }

    #[test]
    fn what_is_not_a_character_is_not_known() {
        let text = page_of(&[line("\u{ffff}", 10.0, 100.0, 10.0)]);
        assert_eq!(text.text, "\u{fffd}");
    }

    #[test]
    fn the_caret_goes_to_the_nearest_side_of_the_nearest_glyph() {
        let text = page_of(&[
            line("abc", 10.0, 100.0, 10.0),
            line("def", 10.0, 120.0, 10.0),
        ]);
        // Each is 5 points wide, from 10.
        assert_eq!(text.caret_at(11.0, 97.0).unwrap().at, 0);
        assert_eq!(text.caret_at(14.0, 97.0).unwrap().at, 1);
        assert_eq!(text.caret_at(21.0, 97.0).unwrap().at, 2);
        assert_eq!(text.caret_at(24.0, 97.0).unwrap().at, 3);
        // On the next line, and past its end.
        assert_eq!(text.caret_at(11.0, 117.0).unwrap().at, 3);
        assert_eq!(text.caret_at(100.0, 117.0).unwrap().at, 6);
        let far = text.caret_at(100.0, 117.0).unwrap();
        assert!((far.distance - 75.0).abs() < 0.01, "{far:?}");
        assert_eq!(text.caret_at(11.0, 97.0).unwrap().distance, 0.0);
        assert!(PageText::default().caret_at(0.0, 0.0).is_none());
    }

    #[test]
    fn beside_a_short_line_is_on_that_line() {
        let text = page_of(&[
            line("a long line", 10.0, 100.0, 10.0),
            line("b", 10.0, 112.0, 10.0),
        ]);
        // The first line is much closer, and the second is what the pointer is level with.
        let caret = text.caret_at(70.0, 110.0).unwrap();
        assert_eq!(caret.at, 12);
    }

    #[test]
    fn a_box_for_each_line_of_what_is_selected() {
        let text = page_of(&[
            line("abc", 10.0, 100.0, 10.0),
            line("def", 10.0, 120.0, 10.0),
        ]);
        let boxes = text.boxes(1, 5);
        assert_eq!(boxes.len(), 2);
        assert_eq!((boxes[0][0], boxes[0][2]), (15.0, 25.0));
        assert_eq!((boxes[1][0], boxes[1][2]), (10.0, 20.0));
        assert_eq!((boxes[0][1], boxes[0][3]), (91.2, 101.2));
        assert!(text.boxes(2, 2).is_empty());
    }

    #[test]
    fn far_on_a_line_is_another_box() {
        let mut first = line("a", 10.0, 100.0, 10.0);
        first.extend(line("b", 200.0, 100.0, 10.0));
        let text = page_of(&[first]);
        assert_eq!(text.text, "a b");
        assert_eq!(text.boxes(0, 2).len(), 2);
    }

    #[test]
    fn a_double_click_takes_what_is_alike() {
        let text = page_of(&[line("foo_1 bar, baz", 10.0, 100.0, 10.0)]);
        assert_eq!(text.word_at(1), (0, 5));
        assert_eq!(text.word_at(7), (6, 9));
        assert_eq!(text.word_at(9), (9, 10));
        let mut raw = Vec::new();
        for (i, c) in "漢字かなカナ".chars().enumerate() {
            raw.push(glyph(
                &c.to_string(),
                10.0 + i as f64 * 10.0,
                100.0,
                10.0,
                1.0,
            ));
        }
        let text = PageText::new(&raw, PAGE);
        assert_eq!(text.word_at(1), (0, 2));
        assert_eq!(text.word_at(2), (2, 4));
        assert_eq!(text.word_at(5), (4, 6));
    }

    #[test]
    fn a_word_goes_across_a_line_broken_by_a_hyphen() {
        let text = page_of(&[
            line("exam-", 10.0, 100.0, 10.0),
            line("ple it", 10.0, 112.0, 10.0),
        ]);
        assert_eq!(text.word_at(1), (0, 8));
    }

    #[test]
    fn a_triple_click_takes_the_line() {
        let text = page_of(&[
            line("one two", 10.0, 100.0, 10.0),
            line("three", 10.0, 112.0, 10.0),
            line("four", 10.0, 124.0, 10.0),
        ]);
        assert_eq!(text.line_at(1), (0, 7));
        assert_eq!(text.line_at(9), (7, 12));
        assert_eq!(text.line_at(13), (12, 16));
        assert_eq!(text.copied(7, 12), "three");
    }

    #[test]
    fn folding_finds_a_word_however_it_is_written() {
        let fold_of = |s: &str| {
            let mut out = String::new();
            fold(s, &mut out);
            out
        };
        assert_eq!(fold_of("Hello World"), "helloworld");
        assert_eq!(fold_of("ＰＤＦ１"), "pdf1");
        assert_eq!(fold_of("ef\u{fb01}cient"), "efficient");
        assert_eq!(fold_of("well-known"), "wellknown");
        assert_eq!(fold_of("it\u{2019}s"), "it's");
    }

    #[test]
    fn what_is_found_goes_back_to_glyphs() {
        let text = page_of(&[
            line("Foo bar", 10.0, 100.0, 10.0),
            line("baz", 10.0, 112.0, 10.0),
        ]);
        let found = find_all(&text.folded, "barbaz");
        assert_eq!(found.len(), 1);
        assert_eq!(text.found_glyphs(found[0]), (4, 10));
        assert_eq!(text.boxes(4, 10).len(), 2);
    }

    #[test]
    fn the_collections_know_their_glyphs() {
        let maps = CollectionMaps::default();
        let japan = maps.of(&CidFamily::AdobeJapan1).unwrap();
        assert_eq!(japan.lookup_bf_string(843), Some(BfString::Char('あ')));
        assert!(maps.of(&CidFamily::AdobeIdentity).is_none());
    }
}
