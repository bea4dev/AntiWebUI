//! What a document has for filling in and marking up: the fields of its forms and its annotations.
//! `hayro` draws what they look like and nothing else, so this reads the dictionaries. It is read
//! from the document as it was opened, and the edits go on top. See "フォームと注釈" in
//! `docs/design/26-pdf.md`.

use crate::links::text_string;
use crate::writer::Tail;
use hayro::hayro_interpret::RectExt;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::String as PdfString;
use hayro::hayro_syntax::object::dict::keys::{ANNOTS, AP, AS, N, PARENT, RECT, SUBTYPE};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, Name, ObjRef, Object, Rect, Stream};
use hayro::kurbo::Affine;
use std::collections::HashMap;
use std::sync::Arc;

/// How many parents a field can have, as a broken document can make this go on forever.
const FIELD_DEPTH: usize = 32;
/// How many actions can follow each other with `/Next`, and how deep a tree of names goes.
const ACTION_DEPTH: usize = 16;

pub(crate) const READ_ONLY: u32 = 1;
const REQUIRED: u32 = 1 << 1;
const MULTILINE: u32 = 1 << 12;
const PASSWORD: u32 = 1 << 13;
const NO_TOGGLE_TO_OFF: u32 = 1 << 14;
const RADIO: u32 = 1 << 15;
const PUSH_BUTTON: u32 = 1 << 16;
const COMBO: u32 = 1 << 17;
const EDIT: u32 = 1 << 18;
const FILE_SELECT: u32 = 1 << 20;
const MULTI_SELECT: u32 = 1 << 21;
const COMB: u32 = 1 << 24;
pub(crate) const HIDDEN: u32 = 1 << 1;
pub(crate) const PRINT: u32 = 1 << 2;
pub(crate) const NO_VIEW: u32 = 1 << 5;

/// Whether a document can be edited, and otherwise why not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PdfEditing {
    /// It can.
    Yes,
    /// It is still being opened.
    #[default]
    NotYet,
    /// What gets added to an encrypted document has to be encrypted too, which isn't done yet.
    Encrypted,
    /// It only opened after being repaired, so there is nothing to add to.
    Broken,
}

/// What kind of field something is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfFieldKind {
    Text,
    Check,
    Radio,
    /// A list that opens, which can take other text if `editable` says so.
    Combo {
        editable: bool,
    },
    /// A list that shows, of which more than one can be picked if `multiple` says so.
    List {
        multiple: bool,
    },
    Push,
    Signature,
}

/// What is in a field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfFieldValue {
    Text(String),
    /// What a check box or a set of radio buttons is on, by the name its widget has for that, or
    /// `None` when it is off.
    State(Option<String>),
    /// What is picked in a list, by the values of its options.
    Choice(Vec<String>),
}

/// A field of a form.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfField {
    /// Its whole name, with the parts separated by dots as in a PDF.
    pub name: String,
    pub kind: PdfFieldKind,
    pub read_only: bool,
    pub required: bool,
    /// What can be picked: its value and what shows for it.
    pub options: Vec<(String, String)>,
    /// What can be turned on, for check boxes and radio buttons.
    pub states: Vec<String>,
    /// The page of its first widget.
    pub page: usize,
}

/// A field and what only drawing and writing it takes.
#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub info: PdfField,
    pub value: PdfFieldValue,
    pub default: PdfFieldValue,
    pub flags: u32,
    pub max_len: Option<u32>,
    /// What says what its text looks like.
    pub da: Vec<u8>,
    /// Where its text goes: left, middle or right.
    pub quadding: u8,
    /// The objects that hold its value.
    pub holders: Vec<ObjRef>,
    /// Its widgets: the page and which of the page's widgets.
    pub widgets: Vec<(usize, usize)>,
    /// What shows for it has to be made when opening.
    pub needs_look: bool,
    pub scripts: FieldScripts,
}

impl Field {
    pub fn multiline(&self) -> bool {
        self.flags & MULTILINE != 0
    }

    pub fn password(&self) -> bool {
        self.flags & PASSWORD != 0
    }

    pub fn comb(&self) -> Option<u32> {
        self.max_len.filter(|_| {
            self.flags & COMB != 0 && self.flags & (MULTILINE | PASSWORD | FILE_SELECT) == 0
        })
    }

    pub fn can_turn_off(&self) -> bool {
        self.flags & NO_TOGGLE_TO_OFF == 0
    }
}

/// The scripts of a field, from the `/AA` of the field: what is typed, how the value shows, whether
/// it may be, and how it is worked out from the others.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct FieldScripts {
    pub keystroke: Option<String>,
    pub format: Option<String>,
    pub validate: Option<String>,
    pub calculate: Option<String>,
}

impl FieldScripts {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The scripts of a widget, for what the pointer and the focus do to it. `up` has what `/A` says
/// too, which is what a click does.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct WidgetScripts {
    pub down: Option<String>,
    pub up: Option<String>,
    pub enter: Option<String>,
    pub exit: Option<String>,
    pub focus: Option<String>,
    pub blur: Option<String>,
}

impl WidgetScripts {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

/// The scripts of the whole document, and what they can ask about it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct DocumentScripts {
    /// Those in `/Names /JavaScript`, by name, which run when it opens.
    pub named: Vec<(String, String)>,
    /// What `/OpenAction` runs after them.
    pub open: Option<String>,
    /// The fields that are worked out, in the order they are: `/CO`.
    pub calculation_order: Vec<usize>,
    /// What runs when a page becomes the one being read, and when it stops being that.
    pub pages: Vec<(usize, Option<String>, Option<String>)>,
    /// `/Info`: title, author, subject, keywords, creator and producer.
    pub info: Vec<(&'static str, String)>,
}

/// What a widget does when it is pressed, apart from its field.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum WidgetAction {
    None,
    /// Puts all of the fields back as they were: `ResetForm`.
    Reset,
}

/// The place of a field on a page.
#[derive(Clone, Debug)]
pub(crate) struct Widget {
    pub field: usize,
    pub object: ObjRef,
    /// Where it is, in the space of the document.
    pub rect: [f64; 4],
    /// What is on for this one, of a check box or a radio button.
    pub on_state: Option<String>,
    pub hidden: bool,
    /// What its `/F` says.
    pub flags: u32,
    pub action: WidgetAction,
    pub scripts: WidgetScripts,
}

/// What kind of annotation something is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfAnnotationKind {
    Highlight,
    Underline,
    StrikeOut,
    Squiggly,
    Ink,
    FreeText,
    Note,
    /// Anything else, which can still be moved and removed.
    Other,
}

impl PdfAnnotationKind {
    /// It marks text, and goes with the text instead of being moved around.
    pub(crate) fn is_on_text(self) -> bool {
        matches!(
            self,
            Self::Highlight | Self::Underline | Self::StrikeOut | Self::Squiggly
        )
    }

    fn of(subtype: &[u8]) -> Option<Self> {
        Some(match subtype {
            b"Highlight" => Self::Highlight,
            b"Underline" => Self::Underline,
            b"StrikeOut" => Self::StrikeOut,
            b"Squiggly" => Self::Squiggly,
            b"Ink" => Self::Ink,
            b"FreeText" => Self::FreeText,
            b"Text" => Self::Note,
            // These aren't there for marking up, or don't show by themselves.
            b"Link" | b"Widget" | b"Popup" => return None,
            _ => Self::Other,
        })
    }
}

/// Which annotation something is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PdfAnnotationId {
    /// One that the document has: the page and its place in `/Annots`.
    Old(usize, usize),
    /// One that was added since it was opened.
    New(u64),
}

/// What makes an annotation that was added what it is.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum Shape {
    /// One that the document has, which goes by its dictionary.
    #[default]
    Old,
    /// The corners of each box: top left, top right, bottom left, bottom right.
    Quads(Vec<[f64; 8]>),
    /// The lines that were drawn, and how wide.
    Ink(Vec<Vec<(f64, f64)>>, f64),
    /// How big the text is.
    FreeText(f64),
    Note,
}

/// An annotation, in the space of the document.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Annot {
    pub id: PdfAnnotationId,
    pub page: usize,
    pub kind: PdfAnnotationKind,
    pub rect: [f64; 4],
    pub contents: String,
    pub color: [f32; 3],
    pub shape: Arc<Shape>,
    pub removed: bool,
    /// When it was made, as a PDF says it.
    pub date: String,
}

impl Annot {
    /// The same in another box, with what it is made of going along.
    pub fn moved_to(mut self, rect: [f64; 4]) -> Self {
        let from = self.rect;
        let (sx, sy) = (
            (rect[2] - rect[0]) / (from[2] - from[0]).max(1e-6),
            (rect[3] - rect[1]) / (from[3] - from[1]).max(1e-6),
        );
        let map = |(x, y): (f64, f64)| (rect[0] + (x - from[0]) * sx, rect[1] + (y - from[1]) * sy);
        let shape = match &*self.shape {
            Shape::Quads(quads) => Shape::Quads(
                quads
                    .iter()
                    .map(|q| {
                        let mut out = *q;
                        for point in out.chunks_exact_mut(2) {
                            let (x, y) = map((point[0], point[1]));
                            point[0] = x;
                            point[1] = y;
                        }
                        out
                    })
                    .collect(),
            ),
            Shape::Ink(strokes, width) => Shape::Ink(
                strokes
                    .iter()
                    .map(|stroke| stroke.iter().map(|p| map(*p)).collect())
                    .collect(),
                *width,
            ),
            other => other.clone(),
        };
        self.shape = Arc::new(shape);
        self.rect = rect;
        self
    }
}

/// What a page has.
#[derive(Clone, Debug, Default)]
pub(crate) struct PageMarks {
    /// From the space of the document to the points of [`crate::text`].
    pub transform: [f64; 6],
    pub widgets: Vec<Widget>,
    pub annots: Vec<Annot>,
}

impl PageMarks {
    pub fn to_points(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let p = Affine::new(self.transform) * hayro::kurbo::Point::new(x, y);
        (p.x, p.y)
    }

    pub fn to_document(&self, (x, y): (f64, f64)) -> (f64, f64) {
        let p = Affine::new(self.transform).inverse() * hayro::kurbo::Point::new(x, y);
        (p.x, p.y)
    }

    /// A box of the document in the points of a page, with the top left first.
    pub fn box_to_points(&self, r: [f64; 4]) -> [f32; 4] {
        let a = self.to_points((r[0], r[1]));
        let b = self.to_points((r[2], r[3]));
        [
            a.0.min(b.0) as f32,
            a.1.min(b.1) as f32,
            a.0.max(b.0) as f32,
            a.1.max(b.1) as f32,
        ]
    }

    pub fn box_to_document(&self, r: [f32; 4]) -> [f64; 4] {
        let a = self.to_document((r[0] as f64, r[1] as f64));
        let b = self.to_document((r[2] as f64, r[3] as f64));
        [a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1)]
    }
}

/// All of it, for a document as it was opened.
#[derive(Clone, Debug, Default)]
pub(crate) struct Marks {
    pub editing: PdfEditing,
    pub signed: bool,
    pub fields: Vec<Field>,
    pub pages: Vec<PageMarks>,
    pub scripts: DocumentScripts,
}

impl Marks {
    /// Whether what shows for some field has to be made as soon as it is open.
    pub fn needs_looks(&self) -> bool {
        self.fields.iter().any(|f| f.needs_look)
    }

    /// Whether anything of it runs JavaScript. Links aren't counted: they start it when clicked.
    pub fn has_scripts(&self) -> bool {
        let s = &self.scripts;
        !s.named.is_empty()
            || s.open.is_some()
            || !s.pages.is_empty()
            || self.fields.iter().any(|f| !f.scripts.is_empty())
            || self
                .pages
                .iter()
                .any(|p| p.widgets.iter().any(|w| !w.scripts.is_empty()))
    }
}

/// The text of a script of a PDF: a text string or a stream.
fn script_text(object: Object<'_>) -> Option<String> {
    let text = match object {
        Object::String(s) => text_string(s.as_bytes()),
        Object::Stream(stream) => text_string(&stream_bytes(&stream)?),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

fn stream_bytes(stream: &Stream<'_>) -> Option<Vec<u8>> {
    stream.decoded().ok().map(|b| b.into_owned())
}

/// What JavaScript an action runs, with the actions that follow it by `/Next`.
pub(crate) fn javascript_of(action: &Dict<'_>) -> Option<String> {
    let mut all = Vec::new();
    collect_javascript(action, ACTION_DEPTH, &mut all);
    (!all.is_empty()).then(|| all.join("\n"))
}

fn collect_javascript(action: &Dict<'_>, depth: usize, all: &mut Vec<String>) {
    if depth == 0 {
        return;
    }
    if action
        .get::<Name<'_>>(b"S" as &[u8])
        .is_some_and(|s| s.as_ref() == b"JavaScript")
        && let Some(text) = action
            .get::<Object<'_>>(b"JS" as &[u8])
            .and_then(script_text)
    {
        all.push(text);
    }
    match action.get::<Object<'_>>(b"Next" as &[u8]) {
        Some(Object::Dict(next)) => collect_javascript(&next, depth - 1, all),
        Some(Object::Array(list)) => {
            for next in list.iter::<Dict<'_>>() {
                collect_javascript(&next, depth - 1, all);
            }
        }
        _ => {}
    }
}

/// The JavaScript of an entry of `/AA`.
fn additional(dict: &Dict<'_>, key: &[u8]) -> Option<String> {
    let actions = dict.get::<Dict<'_>>(b"AA" as &[u8])?;
    javascript_of(&actions.get::<Dict<'_>>(key)?)
}

/// All of the leaves of a tree of names, in order.
fn name_tree_leaves<'a>(node: &Dict<'a>, depth: usize, out: &mut Vec<(String, Object<'a>)>) {
    if depth == 0 {
        return;
    }
    if let Some(names) = node.get::<Array<'_>>(b"Names" as &[u8]) {
        let mut items = names.iter::<Object<'_>>();
        while let (Some(key), Some(value)) = (items.next(), items.next()) {
            if let Object::String(key) = key {
                out.push((text_string(key.as_bytes()), value));
            }
        }
    }
    if let Some(kids) = node.get::<Array<'_>>(b"Kids" as &[u8]) {
        for kid in kids.iter::<Dict<'_>>() {
            name_tree_leaves(&kid, depth - 1, out);
        }
    }
}

/// What the document runs by itself, apart from its fields.
fn document_scripts(pdf: &Pdf, catalog: Option<&Dict<'_>>) -> DocumentScripts {
    let mut scripts = DocumentScripts::default();
    if let Some(catalog) = catalog {
        if let Some(tree) = catalog
            .get::<Dict<'_>>(b"Names" as &[u8])
            .and_then(|n| n.get::<Dict<'_>>(b"JavaScript" as &[u8]))
        {
            let mut leaves = Vec::new();
            name_tree_leaves(&tree, ACTION_DEPTH, &mut leaves);
            for (name, action) in leaves {
                if let Object::Dict(action) = action
                    && let Some(text) = javascript_of(&action)
                {
                    scripts.named.push((name, text));
                }
            }
        }
        if let Some(Object::Dict(action)) = catalog.get::<Object<'_>>(b"OpenAction" as &[u8]) {
            scripts.open = javascript_of(&action);
        }
    }
    for (index, page) in pdf.pages().iter().enumerate() {
        let (open, close) = (additional(page.raw(), b"O"), additional(page.raw(), b"C"));
        if open.is_some() || close.is_some() {
            scripts.pages.push((index, open, close));
        }
    }
    let info = pdf.metadata();
    for (key, value) in [
        ("title", &info.title),
        ("author", &info.author),
        ("subject", &info.subject),
        ("keywords", &info.keywords),
        ("creator", &info.creator),
        ("producer", &info.producer),
    ] {
        if let Some(value) = value {
            scripts.info.push((key, text_string(value)));
        }
    }
    scripts
}

/// Goes up from a field for something it inherits.
fn inherited<'a>(dict: &Dict<'a>, key: &[u8]) -> Option<Object<'a>> {
    let mut at = dict.clone();
    for _ in 0..FIELD_DEPTH {
        if let Some(found) = at.get::<Object<'_>>(key) {
            return Some(found);
        }
        at = at.get::<Dict<'_>>(PARENT)?;
    }
    None
}

/// The whole name of a field and the dictionary that holds its value: that of a widget that has a
/// name of its own, and otherwise its parent.
fn name_and_holder<'a>(widget: &Dict<'a>, own: ObjRef) -> (String, ObjRef) {
    let holder = if widget.contains_key(b"T") {
        own
    } else {
        widget.get_ref(PARENT).unwrap_or(own)
    };
    let mut parts = Vec::new();
    let mut at = widget.clone();
    for _ in 0..FIELD_DEPTH {
        if let Some(part) = at.get::<PdfString<'_>>(b"T" as &[u8]) {
            parts.push(text_string(part.as_bytes()));
        }
        let Some(parent) = at.get::<Dict<'_>>(PARENT) else {
            break;
        };
        at = parent;
    }
    parts.reverse();
    (parts.join("."), holder)
}

fn number_of(object: Option<Object<'_>>) -> Option<f64> {
    match object {
        Some(Object::Number(n)) => Some(n.as_f64()),
        _ => None,
    }
}

fn text_of(object: &Object<'_>) -> Option<String> {
    match object {
        Object::String(s) => Some(text_string(s.as_bytes())),
        Object::Name(n) => Some(String::from_utf8_lossy(n.as_ref()).into_owned()),
        _ => None,
    }
}

fn kind_of(field_type: &[u8], flags: u32) -> PdfFieldKind {
    match field_type {
        b"Btn" if flags & PUSH_BUTTON != 0 => PdfFieldKind::Push,
        b"Btn" if flags & RADIO != 0 => PdfFieldKind::Radio,
        b"Btn" => PdfFieldKind::Check,
        b"Ch" if flags & COMBO != 0 => PdfFieldKind::Combo {
            editable: flags & EDIT != 0,
        },
        b"Ch" => PdfFieldKind::List {
            multiple: flags & MULTI_SELECT != 0,
        },
        b"Sig" => PdfFieldKind::Signature,
        _ => PdfFieldKind::Text,
    }
}

fn value_of(kind: PdfFieldKind, object: Option<Object<'_>>) -> PdfFieldValue {
    match kind {
        PdfFieldKind::Check | PdfFieldKind::Radio | PdfFieldKind::Push => {
            let on = object
                .as_ref()
                .and_then(text_of)
                .filter(|s| !s.is_empty() && s != "Off");
            PdfFieldValue::State(on)
        }
        PdfFieldKind::Combo { .. } | PdfFieldKind::List { .. } => {
            let picked = match &object {
                Some(Object::Array(items)) => items
                    .iter::<Object<'_>>()
                    .filter_map(|o| text_of(&o))
                    .collect(),
                Some(other) => text_of(other).into_iter().collect(),
                None => Vec::new(),
            };
            PdfFieldValue::Choice(picked)
        }
        PdfFieldKind::Text | PdfFieldKind::Signature => {
            PdfFieldValue::Text(object.as_ref().and_then(text_of).unwrap_or_default())
        }
    }
}

fn options_of(dict: &Dict<'_>) -> Vec<(String, String)> {
    let Some(Object::Array(options)) = inherited(dict, b"Opt") else {
        return Vec::new();
    };
    options
        .iter::<Object<'_>>()
        .filter_map(|option| match &option {
            Object::Array(pair) => {
                let mut pair = pair.iter::<Object<'_>>();
                let value = text_of(&pair.next()?)?;
                let shown = pair.next().as_ref().and_then(text_of);
                Some((value.clone(), shown.unwrap_or(value)))
            }
            other => {
                let value = text_of(other)?;
                Some((value.clone(), value))
            }
        })
        .collect()
}

/// What a check box or a radio button is called when it is on: what its look has besides `Off`.
fn on_state_of(widget: &Dict<'_>) -> Option<String> {
    let looks = widget.get::<Dict<'_>>(AP)?.get::<Dict<'_>>(N)?;
    looks
        .keys()
        .find(|k| k.as_ref() != b"Off")
        .map(|k| String::from_utf8_lossy(k.as_ref()).into_owned())
}

fn color_of(annot: &Dict<'_>) -> [f32; 3] {
    let Some(color) = annot.get::<Array<'_>>(b"C" as &[u8]) else {
        return [1.0, 0.85, 0.0];
    };
    let parts: Vec<f32> = color.iter::<f32>().collect();
    match parts[..] {
        [gray] => [gray; 3],
        [r, g, b] => [r, g, b],
        [c, m, y, k] => [
            (1.0 - c) * (1.0 - k),
            (1.0 - m) * (1.0 - k),
            (1.0 - y) * (1.0 - k),
        ],
        _ => [1.0, 0.85, 0.0],
    }
}

/// What each place of `/Annots` of a page holds.
pub(crate) fn annotations<'a>(page: &Dict<'a>) -> Vec<MaybeRef<Object<'a>>> {
    page.get::<Array<'_>>(ANNOTS)
        .map_or_else(Vec::new, |list| list.raw_iter().collect())
}

/// Reads what a document has. `tail` is what its end says, if that can be read.
pub(crate) fn read(pdf: &Pdf, tail: Option<&Tail>) -> Marks {
    let editing = match tail {
        Some(tail) if tail.encrypted => PdfEditing::Encrypted,
        Some(_) => PdfEditing::Yes,
        None => PdfEditing::Broken,
    };
    let xref = pdf.xref();
    let catalog = xref.get::<Dict<'_>>(xref.root_id());
    let form = catalog
        .as_ref()
        .and_then(|c| c.get::<Dict<'_>>(b"AcroForm" as &[u8]));
    let need_looks = form
        .as_ref()
        .is_some_and(|f| f.get::<bool>(b"NeedAppearances" as &[u8]).unwrap_or(false));
    let signed = form
        .as_ref()
        .and_then(|f| f.get::<i64>(b"SigFlags" as &[u8]))
        .is_some_and(|flags| flags & 1 != 0);

    let mut marks = Marks {
        editing,
        signed,
        ..Marks::default()
    };
    marks.scripts = document_scripts(pdf, catalog.as_ref());
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut by_holder: HashMap<(u32, i32), usize> = HashMap::new();
    for (index, page) in pdf.pages().iter().enumerate() {
        let mut shown = PageMarks {
            transform: page.initial_transform(true).to_kurbo().as_coeffs(),
            ..PageMarks::default()
        };
        for (place, item) in annotations(page.raw()).into_iter().enumerate() {
            let (dict, own) = match item {
                MaybeRef::Ref(r) => match xref.get::<Dict<'_>>(r.into()) {
                    Some(dict) => (dict, Some(r)),
                    None => continue,
                },
                MaybeRef::NotRef(Object::Dict(dict)) => (dict, None),
                MaybeRef::NotRef(_) => continue,
            };
            let Some(subtype) = dict.get::<Name<'_>>(SUBTYPE) else {
                continue;
            };
            let Some(rect) = dict.get::<Rect>(RECT).map(|r| r.to_kurbo()) else {
                continue;
            };
            let rect = [rect.x0, rect.y0, rect.x1, rect.y1];
            if subtype.as_ref() == b"Widget" {
                // A widget that isn't an object by itself can't be written without its page, which
                // nothing makes in practice.
                let Some(own) = own else {
                    continue;
                };
                read_widget(
                    &dict,
                    own,
                    rect,
                    index,
                    need_looks,
                    &mut shown,
                    &mut marks.fields,
                    &mut by_name,
                    &mut by_holder,
                );
            } else if let Some(kind) = PdfAnnotationKind::of(subtype.as_ref()) {
                shown.annots.push(Annot {
                    id: PdfAnnotationId::Old(index, place),
                    page: index,
                    kind,
                    rect,
                    contents: dict
                        .get::<PdfString<'_>>(b"Contents" as &[u8])
                        .map_or_else(String::new, |c| text_string(c.as_bytes())),
                    color: color_of(&dict),
                    shape: Arc::new(Shape::Old),
                    removed: false,
                    date: String::new(),
                });
            }
        }
        marks.pages.push(shown);
    }
    if let Some(order) = form
        .as_ref()
        .and_then(|f| f.get::<Array<'_>>(b"CO" as &[u8]))
    {
        for item in order.raw_iter() {
            if let MaybeRef::Ref(r) = item {
                let r: ObjRef = r;
                if let Some(&field) = by_holder.get(&(r.obj_number, r.gen_number))
                    && !marks.scripts.calculation_order.contains(&field)
                {
                    marks.scripts.calculation_order.push(field);
                }
            }
        }
    }
    marks
}

#[allow(clippy::too_many_arguments)]
fn read_widget(
    dict: &Dict<'_>,
    own: ObjRef,
    rect: [f64; 4],
    page: usize,
    need_looks: bool,
    shown: &mut PageMarks,
    fields: &mut Vec<Field>,
    by_name: &mut HashMap<String, usize>,
    by_holder: &mut HashMap<(u32, i32), usize>,
) {
    let (name, holder) = name_and_holder(dict, own);
    let key = (holder.obj_number, holder.gen_number);
    let known = by_holder.get(&key).copied().or_else(|| {
        (!name.is_empty())
            .then(|| by_name.get(&name).copied())
            .flatten()
    });
    let field = known.unwrap_or_else(|| {
        let field_type = match inherited(dict, b"FT") {
            Some(Object::Name(n)) => n.as_ref().to_vec(),
            _ => b"Tx".to_vec(),
        };
        let flags = number_of(inherited(dict, b"Ff")).unwrap_or(0.0) as u32;
        let kind = kind_of(&field_type, flags);
        let at = fields.len();
        fields.push(Field {
            info: PdfField {
                name: name.clone(),
                kind,
                read_only: flags & READ_ONLY != 0,
                required: flags & REQUIRED != 0,
                options: options_of(dict),
                states: Vec::new(),
                page,
            },
            value: value_of(kind, inherited(dict, b"V")),
            default: value_of(kind, inherited(dict, b"DV")),
            flags,
            max_len: number_of(inherited(dict, b"MaxLen")).map(|n| n as u32),
            da: match inherited(dict, b"DA") {
                Some(Object::String(s)) => s.as_bytes().to_vec(),
                _ => Vec::new(),
            },
            quadding: number_of(inherited(dict, b"Q")).unwrap_or(0.0) as u8,
            holders: Vec::new(),
            widgets: Vec::new(),
            needs_look: false,
            scripts: FieldScripts::default(),
        });
        at
    });
    by_holder.insert(key, field);
    if !name.is_empty() {
        by_name.insert(name, field);
    }
    let flags = dict.get::<u32>(b"F" as &[u8]).unwrap_or(0);
    let info = &mut fields[field];
    // What the field runs is in the dictionary of the field, which is the widget's when they are one.
    if info.scripts.is_empty() {
        let holder_dict = if holder == own {
            Some(dict.clone())
        } else {
            dict.get::<Dict<'_>>(PARENT)
        };
        if let Some(d) = holder_dict {
            info.scripts = FieldScripts {
                keystroke: additional(&d, b"K"),
                format: additional(&d, b"F"),
                validate: additional(&d, b"V"),
                calculate: additional(&d, b"C"),
            };
        }
    }
    if !info.holders.contains(&holder) {
        info.holders.push(holder);
    }
    info.widgets.push((page, shown.widgets.len()));
    let has_look = dict
        .get::<Dict<'_>>(AP)
        .is_some_and(|ap| ap.contains_key(N));
    let is_button = matches!(info.info.kind, PdfFieldKind::Check | PdfFieldKind::Radio);
    let on_state = if is_button {
        on_state_of(dict).or_else(|| {
            dict.get::<Name<'_>>(AS)
                .filter(|s| s.as_ref() != b"Off")
                .map(|s| String::from_utf8_lossy(s.as_ref()).into_owned())
        })
    } else {
        None
    };
    if let Some(state) = &on_state
        && !info.info.states.contains(state)
    {
        info.info.states.push(state.clone());
    }
    let has_value = match &info.value {
        PdfFieldValue::Text(t) => !t.is_empty(),
        PdfFieldValue::State(s) => s.is_some(),
        PdfFieldValue::Choice(c) => !c.is_empty(),
    };
    let is_look_kind = !matches!(info.info.kind, PdfFieldKind::Push | PdfFieldKind::Signature);
    if is_look_kind && ((has_value && need_looks && !is_button) || (has_value && !has_look)) {
        info.needs_look = true;
    }
    let pressed = dict.get::<Dict<'_>>(b"A" as &[u8]);
    let action = pressed
        .as_ref()
        .and_then(|a| a.get::<Name<'_>>(b"S" as &[u8]))
        .filter(|s| s.as_ref() == b"ResetForm")
        .map_or(WidgetAction::None, |_| WidgetAction::Reset);
    let up = [
        pressed.as_ref().and_then(javascript_of),
        additional(dict, b"U"),
    ]
    .into_iter()
    .flatten()
    .reduce(|a, b| format!("{a}\n{b}"));
    let scripts = WidgetScripts {
        down: additional(dict, b"D"),
        up,
        enter: additional(dict, b"E"),
        exit: additional(dict, b"X"),
        focus: additional(dict, b"Fo"),
        blur: additional(dict, b"Bl"),
    };
    shown.widgets.push(Widget {
        field,
        object: own,
        rect,
        on_state,
        hidden: flags & (HIDDEN | NO_VIEW) != 0,
        flags,
        action,
        scripts,
    });
}
