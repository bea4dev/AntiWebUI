//! What was changed in a document, and what gets added to the file for it. It is always all of the
//! changes on top of the document as it was opened, so going back is only saying less. See
//! "フォームと注釈" in `docs/design/26-pdf.md`.

use crate::appearance::{self, DefaultLook, Face, Frame, SimpleFont, TextStyle};
use crate::embed::{self, Subset, SubsetCache};
use crate::marks::{
    Annot, Field, HIDDEN, Marks, NO_VIEW, PRINT, PdfAnnotationId, PdfAnnotationKind, PdfFieldKind,
    PdfFieldValue, READ_ONLY, Shape, annotations,
};
use crate::writer::{Entries, Tail, Update, Value};
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::dict::keys::{ANNOTS, AP, N};
use hayro::hayro_syntax::object::{Array, Dict, MaybeRef, ObjRef, Object, String as PdfString};
use hayro::hayro_syntax::page::Rotation;
use std::collections::{BTreeMap, BTreeSet};

/// All that was changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Edits {
    /// What is in the fields that were changed, by their place in [`Marks::fields`].
    pub fields: BTreeMap<usize, PdfFieldValue>,
    /// The annotations that were added or changed, and those that were removed from the document.
    pub annots: BTreeMap<PdfAnnotationId, Annot>,
    /// What shows for the value of a field, when a script of the document says it shows otherwise:
    /// `$1,234.50` for `1234.5`.
    pub shown: BTreeMap<usize, String>,
    /// Whether a field shows, as a script said: [`Display`].
    pub display: BTreeMap<usize, Display>,
    /// Whether a field can be changed, as a script said.
    pub read_only: BTreeMap<usize, bool>,
}

/// Whether the widgets of a field show and get printed, as `display` of Acrobat's JavaScript has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Display {
    Visible,
    Hidden,
    NoPrint,
    NoView,
}

#[cfg_attr(not(feature = "js"), allow(dead_code))]
impl Display {
    pub fn from_number(n: i64) -> Option<Self> {
        Some(match n {
            0 => Self::Visible,
            1 => Self::Hidden,
            2 => Self::NoPrint,
            3 => Self::NoView,
            _ => return None,
        })
    }

    pub fn number(self) -> i64 {
        match self {
            Self::Visible => 0,
            Self::Hidden => 1,
            Self::NoPrint => 2,
            Self::NoView => 3,
        }
    }

    /// What `/F` of a widget says.
    pub fn of_flags(flags: u32) -> Self {
        if flags & HIDDEN != 0 {
            Self::Hidden
        } else if flags & NO_VIEW != 0 {
            Self::NoView
        } else if flags & PRINT == 0 {
            Self::NoPrint
        } else {
            Self::Visible
        }
    }

    /// `/F` of a widget that had `flags` before.
    pub fn flags(self, flags: u32) -> u32 {
        let rest = flags & !(HIDDEN | NO_VIEW | PRINT);
        match self {
            Self::Visible => rest | PRINT,
            Self::Hidden => rest | HIDDEN | PRINT,
            Self::NoPrint => rest,
            Self::NoView => rest | NO_VIEW | PRINT,
        }
    }

    pub fn shows(self) -> bool {
        matches!(self, Self::Visible | Self::NoPrint)
    }
}

impl Edits {
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
            && self.annots.is_empty()
            && self.shown.is_empty()
            && self.display.is_empty()
            && self.read_only.is_empty()
    }

    /// The pages that don't look the same with these as with those.
    pub fn pages_changed_from(&self, other: &Edits, marks: &Marks) -> BTreeSet<usize> {
        let mut pages = BTreeSet::new();
        let keys: BTreeSet<usize> = self
            .fields
            .keys()
            .chain(other.fields.keys())
            .chain(self.shown.keys())
            .chain(other.shown.keys())
            .chain(self.display.keys())
            .chain(other.display.keys())
            .copied()
            .collect();
        for key in keys {
            let differs = self.fields.get(&key) != other.fields.get(&key)
                || self.shown.get(&key) != other.shown.get(&key)
                || self.display.get(&key) != other.display.get(&key);
            if differs && let Some(field) = marks.fields.get(key) {
                pages.extend(field.widgets.iter().map(|w| w.0));
            }
        }
        let ids: BTreeSet<PdfAnnotationId> = self
            .annots
            .keys()
            .chain(other.annots.keys())
            .copied()
            .collect();
        for id in ids {
            let (a, b) = (self.annots.get(&id), other.annots.get(&id));
            if a != b {
                pages.extend(a.or(b).map(|annot| annot.page));
            }
        }
        pages
    }
}

/// The changes to objects of the document so far.
#[derive(Default)]
struct Rewrites(BTreeMap<u32, (ObjRef, Entries)>);

impl Rewrites {
    fn of<'a>(&'a mut self, pdf: &Pdf, at: ObjRef) -> Option<&'a mut Entries> {
        use std::collections::btree_map::Entry;
        match self.0.entry(at.obj_number) {
            Entry::Occupied(entry) => Some(&mut entry.into_mut().1),
            Entry::Vacant(entry) => {
                let dict = pdf.xref().get::<Dict<'_>>(at.into())?;
                Some(&mut entry.insert((at, Entries::of(&dict))).1)
            }
        }
    }
}

/// What writes text for something, if a font of the document can.
fn font_for(
    text: &str,
    look: &DefaultLook,
    widget: Option<&Dict<'_>>,
    form: Option<&Dict<'_>>,
) -> Option<SimpleFont> {
    if let Some(font) = appearance::font_named(widget, form, &look.font)
        && font.can_write(text)
    {
        return Some(font);
    }
    let helvetica = SimpleFont::helvetica();
    helvetica.can_write(text).then_some(helvetica)
}

/// Writes all of the changes as what goes at the end of the document.
pub(crate) fn build(
    pdf: &Pdf,
    tail: &Tail,
    marks: &Marks,
    edits: &Edits,
    cache: &SubsetCache,
) -> Result<Vec<u8>, String> {
    let xref = pdf.xref();
    let catalog = xref
        .get::<Dict<'_>>(xref.root_id())
        .ok_or("the document has no catalog")?;
    let form = catalog.get::<Dict<'_>>(b"AcroForm" as &[u8]);

    let fields: Vec<usize> = marks
        .fields
        .iter()
        .enumerate()
        .filter(|(i, f)| {
            f.needs_look || edits.fields.contains_key(i) || edits.shown.contains_key(i)
        })
        .map(|(i, _)| i)
        .collect();

    let mut builder = Builder {
        pdf,
        marks,
        form: form.as_ref(),
        update: Update::new(tail.clone()),
        rewrites: Rewrites::default(),
        subset: None,
        subset_value: Value::Null,
    };

    // First what all of it needs from the fonts of the system, so that there is only one part.
    let mut chars = BTreeSet::new();
    for &i in &fields {
        builder.chars_of_field(i, edits, &mut chars);
    }
    for annot in edits.annots.values() {
        builder.chars_of_annot(annot, &mut chars);
    }
    if !chars.is_empty() {
        let subset = cache.get(&chars);
        if subset.is_none() {
            log::warn!("no font on this system can write all of a PDF's new text");
        }
        builder.subset = subset;
        if let Some(subset) = builder.subset.clone() {
            builder.subset_value = embed::write(&mut builder.update, &subset);
        }
    }

    for &i in &fields {
        builder.write_field(i, edits)?;
    }
    builder.write_annots(edits)?;
    builder.write_flags(edits);

    // Those who read XFA would show what was in the fields before.
    if !edits.fields.is_empty() && form.as_ref().is_some_and(|f| f.contains_key(b"XFA")) {
        match catalog.get_ref(b"AcroForm" as &[u8]) {
            Some(at) => {
                if let Some(entries) = builder.rewrites.of(pdf, at) {
                    entries.remove(b"XFA");
                }
            }
            None => {
                let root: ObjRef = xref.root_id().into();
                let inner = form.as_ref().map(Entries::of).unwrap_or_default();
                let mut inner = inner;
                inner.remove(b"XFA");
                if let Some(entries) = builder.rewrites.of(pdf, root) {
                    entries.set(b"AcroForm", Value::Dict(inner));
                }
            }
        }
    }

    let Builder {
        mut update,
        rewrites,
        ..
    } = builder;
    for (at, entries) in rewrites.0.into_values() {
        update.put(at, &Value::Dict(entries));
    }
    let length = pdf.data().as_ref().len();
    Ok(update.finish(length))
}

struct Builder<'a, 'b> {
    pdf: &'a Pdf,
    marks: &'a Marks,
    form: Option<&'b Dict<'a>>,
    update: Update,
    rewrites: Rewrites,
    subset: Option<std::sync::Arc<Subset>>,
    subset_value: Value,
}

impl Builder<'_, '_> {
    fn widget_dict(&self, at: ObjRef) -> Option<Dict<'_>> {
        self.pdf.xref().get::<Dict<'_>>(at.into())
    }

    /// The face that writes `text`: a font of the document, or else the part of a font of the system.
    fn with_face<R>(
        &self,
        text: &str,
        look: &DefaultLook,
        widget: Option<&Dict<'_>>,
        f: impl FnOnce(Face<'_>) -> R,
    ) -> R {
        if let Some(font) = font_for(text, look, widget, self.form) {
            return f(Face::Simple(&font));
        }
        match &self.subset {
            Some(subset) => f(Face::Embedded(subset, &self.subset_value)),
            // With nothing to write it with, it comes out as blanks of no width.
            None => f(Face::Simple(&SimpleFont::helvetica())),
        }
    }

    fn field_value<'e>(&'e self, i: usize, edits: &'e Edits) -> &'e PdfFieldValue {
        edits.fields.get(&i).unwrap_or(&self.marks.fields[i].value)
    }

    /// What shows for a field, of its text and its list. `formatted` is what a script said shows.
    fn shown_text(field: &Field, value: &PdfFieldValue, formatted: Option<&String>) -> Vec<String> {
        match (value, field.info.kind) {
            (PdfFieldValue::Text(text), _) if field.password() => {
                vec!["*".repeat(text.chars().count())]
            }
            (_, PdfFieldKind::Text | PdfFieldKind::Combo { .. }) if formatted.is_some() => {
                formatted.into_iter().cloned().collect()
            }
            (PdfFieldValue::Text(text), _) => vec![text.clone()],
            (PdfFieldValue::Choice(_), PdfFieldKind::List { .. }) => {
                field.info.options.iter().map(|o| o.1.clone()).collect()
            }
            (PdfFieldValue::Choice(picked), _) => vec![
                picked
                    .first()
                    .map(|value| {
                        field
                            .info
                            .options
                            .iter()
                            .find(|o| &o.0 == value)
                            .map_or_else(|| value.clone(), |o| o.1.clone())
                    })
                    .unwrap_or_default(),
            ],
            _ => Vec::new(),
        }
    }

    fn chars_of_field(&self, i: usize, edits: &Edits, chars: &mut BTreeSet<char>) {
        let field = &self.marks.fields[i];
        let look = appearance::parse_default_look(&field.da);
        let value = self.field_value(i, edits);
        for text in Self::shown_text(field, value, edits.shown.get(&i)) {
            for &(page, widget) in &field.widgets {
                let widget = self.marks.pages[page].widgets[widget].object;
                let dict = self.widget_dict(widget);
                if font_for(&text, &look, dict.as_ref(), self.form).is_none() {
                    appearance::needs_chars(&text, chars);
                }
            }
        }
    }

    fn write_field(&mut self, i: usize, edits: &Edits) -> Result<(), String> {
        let field = self.marks.fields[i].clone();
        let value = self.field_value(i, edits).clone();
        if edits.fields.contains_key(&i) {
            for holder in &field.holders {
                let Some(entries) = self.rewrites.of(self.pdf, *holder) else {
                    continue;
                };
                write_value(entries, &field, &value);
            }
        }
        for &(page, widget) in &field.widgets {
            let widget = self.marks.pages[page].widgets[widget].clone();
            match field.info.kind {
                PdfFieldKind::Check | PdfFieldKind::Radio => self.write_button(
                    &field,
                    &widget.object,
                    widget.rect,
                    &value,
                    &widget.on_state,
                ),
                PdfFieldKind::Push | PdfFieldKind::Signature => {}
                _ => self.write_text_look(
                    &field,
                    &widget.object,
                    widget.rect,
                    &value,
                    edits.shown.get(&i),
                ),
            }
        }
        Ok(())
    }

    /// Whether the fields show and can be changed, as scripts said.
    fn write_flags(&mut self, edits: &Edits) {
        for (&i, display) in &edits.display {
            let Some(field) = self.marks.fields.get(i) else {
                continue;
            };
            for &(page, widget) in &field.widgets {
                let widget = &self.marks.pages[page].widgets[widget];
                if let Some(entries) = self.rewrites.of(self.pdf, widget.object) {
                    entries.set(b"F", Value::Int(display.flags(widget.flags) as i64));
                }
            }
        }
        for (&i, &on) in &edits.read_only {
            let Some(field) = self.marks.fields.get(i) else {
                continue;
            };
            let flags = if on {
                field.flags | READ_ONLY
            } else {
                field.flags & !READ_ONLY
            };
            for holder in &field.holders {
                if let Some(entries) = self.rewrites.of(self.pdf, *holder) {
                    entries.set(b"Ff", Value::Int(flags as i64));
                }
            }
        }
    }

    fn write_button(
        &mut self,
        field: &Field,
        object: &ObjRef,
        rect: [f64; 4],
        value: &PdfFieldValue,
        on_state: &Option<String>,
    ) {
        let on = match value {
            PdfFieldValue::State(Some(state)) => match on_state {
                Some(own) => own == state,
                None => true,
            },
            _ => false,
        };
        let name: Vec<u8> = match (on, on_state, value) {
            (true, Some(own), _) => own.as_bytes().to_vec(),
            (true, None, PdfFieldValue::State(Some(state))) => state.as_bytes().to_vec(),
            _ => b"Off".to_vec(),
        };
        let Some(dict) = self.widget_dict(*object) else {
            return;
        };
        let has_look = dict
            .get::<Dict<'_>>(AP)
            .is_some_and(|ap| ap.contains_key(N));
        let mut new_look = None;
        if !has_look {
            let frame = Frame::read(&dict, rect);
            let radio = field.info.kind == PdfFieldKind::Radio;
            let on_key = on_state.clone().unwrap_or_else(|| "Yes".to_owned());
            let dict_of = frame.stream_dict(Entries::new());
            let on_look = self
                .update
                .stream(dict_of.clone(), &appearance::button(&frame, true, radio));
            let off_look = self
                .update
                .stream(dict_of, &appearance::button(&frame, false, radio));
            new_look = Some(
                Entries::new()
                    .with(on_key, Value::Ref(on_look))
                    .with(b"Off", Value::Ref(off_look)),
            );
        }
        let Some(entries) = self.rewrites.of(self.pdf, *object) else {
            return;
        };
        entries.set(b"AS", Value::Name(name));
        if let Some(look) = new_look {
            entries.set(
                b"AP",
                Value::Dict(Entries::new().with(b"N", Value::Dict(look))),
            );
        }
    }

    fn write_text_look(
        &mut self,
        field: &Field,
        object: &ObjRef,
        rect: [f64; 4],
        value: &PdfFieldValue,
        formatted: Option<&String>,
    ) {
        let Some(dict) = self.widget_dict(*object) else {
            return;
        };
        let frame = Frame::read(&dict, rect);
        let look = appearance::parse_default_look(&field.da);
        let shown = Self::shown_text(field, value, formatted);
        let all = shown.join("\n");
        let (ops, resources) = self.with_face(&all, &look, Some(&dict), |face| {
            let style = TextStyle {
                face,
                size: look.size,
                color: &look.color,
                quadding: field.quadding,
            };
            let ops = match (field.info.kind, value) {
                (PdfFieldKind::List { .. }, PdfFieldValue::Choice(picked)) => {
                    let top = dict.get::<usize>(b"TI" as &[u8]).unwrap_or(0);
                    appearance::list(&frame, &field.info.options, picked, top, style)
                }
                _ if field.multiline() => appearance::multiple_lines(&frame, &all, style),
                _ => match field.comb() {
                    Some(cells) => appearance::comb(&frame, &all, cells, style),
                    None => appearance::single_line(&frame, &all, style),
                },
            };
            (ops, face.resources())
        });
        let stream = self.update.stream(frame.stream_dict(resources), &ops);
        let Some(entries) = self.rewrites.of(self.pdf, *object) else {
            return;
        };
        entries.set(
            b"AP",
            Value::Dict(Entries::new().with(b"N", Value::Ref(stream))),
        );
    }

    fn chars_of_annot(&self, annot: &Annot, chars: &mut BTreeSet<char>) {
        if annot.kind != PdfAnnotationKind::FreeText || annot.removed {
            return;
        }
        let (look, dict) = self.free_text_source(annot);
        if font_for(&annot.contents, &look, dict.as_ref(), self.form).is_none() {
            appearance::needs_chars(&annot.contents, chars);
        }
    }

    /// What says how text put on a page looks, and the annotation of the document if it is one.
    fn free_text_source(&self, annot: &Annot) -> (DefaultLook, Option<Dict<'_>>) {
        let dict = self.old_dict(annot);
        let look = match (&*annot.shape, &dict) {
            (Shape::FreeText(size), _) => DefaultLook {
                font: Vec::new(),
                size: *size,
                color: format!(
                    "{} {} {} rg",
                    crate::writer::number(annot.color[0] as f64),
                    crate::writer::number(annot.color[1] as f64),
                    crate::writer::number(annot.color[2] as f64)
                ),
            },
            (_, Some(dict)) => {
                let da = dict
                    .get::<PdfString<'_>>(b"DA" as &[u8])
                    .map(|s| s.as_bytes().to_vec())
                    .unwrap_or_default();
                let mut look = appearance::parse_default_look(&da);
                if look.size <= 0.0 {
                    look.size = 12.0;
                }
                look
            }
            _ => DefaultLook {
                font: Vec::new(),
                size: 12.0,
                color: "0 g".to_owned(),
            },
        };
        (look, dict)
    }

    fn old_dict(&self, annot: &Annot) -> Option<Dict<'_>> {
        let PdfAnnotationId::Old(page, place) = annot.id else {
            return None;
        };
        let raw = self.pdf.pages().get(page)?.raw();
        match annotations(raw).into_iter().nth(place)? {
            MaybeRef::Ref(r) => self.pdf.xref().get::<Dict<'_>>(r.into()),
            MaybeRef::NotRef(Object::Dict(dict)) => Some(dict),
            MaybeRef::NotRef(_) => None,
        }
    }

    fn write_annots(&mut self, edits: &Edits) -> Result<(), String> {
        let mut pages: BTreeMap<usize, Vec<&Annot>> = BTreeMap::new();
        for annot in edits.annots.values() {
            pages.entry(annot.page).or_default().push(annot);
        }
        for (page, annots) in pages {
            self.write_page_annots(page, &annots, edits)?;
        }
        Ok(())
    }

    fn write_page_annots(
        &mut self,
        page: usize,
        annots: &[&Annot],
        edits: &Edits,
    ) -> Result<(), String> {
        let pdf = self.pdf;
        let shown = pdf.pages().get(page).ok_or("a page isn't there")?;
        let raw = shown.raw().clone();
        let id = raw.obj_id().ok_or("a page isn't an object by itself")?;
        let rotation = match shown.rotation() {
            Rotation::None => 0,
            Rotation::Horizontal => 90,
            Rotation::Flipped => 180,
            Rotation::FlippedHorizontal => 270,
        };

        let items = annotations(&raw);
        // A note that was removed takes its popup along.
        let mut popups = Vec::new();
        for (place, item) in items.iter().enumerate() {
            let removed = edits
                .annots
                .get(&PdfAnnotationId::Old(page, place))
                .is_some_and(|a| a.removed);
            if !removed {
                continue;
            }
            let dict = match item {
                MaybeRef::Ref(r) => pdf.xref().get::<Dict<'_>>((*r).into()),
                MaybeRef::NotRef(Object::Dict(d)) => Some(d.clone()),
                MaybeRef::NotRef(_) => None,
            };
            if let Some(popup) = dict.and_then(|d| d.get_ref(b"Popup" as &[u8])) {
                popups.push(popup);
            }
        }

        let mut list = Vec::new();
        for (place, item) in items.iter().enumerate() {
            let edit = edits.annots.get(&PdfAnnotationId::Old(page, place));
            match (edit, item) {
                (Some(annot), _) if annot.removed => {}
                (Some(annot), MaybeRef::Ref(r)) => {
                    let dict = pdf.xref().get::<Dict<'_>>((*r).into());
                    if let Some(dict) = dict {
                        let entries = self.rewrite_annot(&dict, annot, rotation);
                        self.rewrites.0.insert(r.obj_number, (*r, entries));
                    }
                    list.push(Value::Ref(*r));
                }
                (Some(annot), MaybeRef::NotRef(Object::Dict(dict))) => {
                    list.push(Value::Dict(self.rewrite_annot(dict, annot, rotation)));
                }
                (_, MaybeRef::Ref(r)) if popups.contains(r) => {}
                (_, item) => list.push(Value::of(item)),
            }
        }
        for annot in annots {
            if matches!(annot.id, PdfAnnotationId::New(_)) && !annot.removed {
                let added = self.new_annot(annot, rotation);
                list.push(Value::Ref(added));
            }
        }
        if let Some(entries) = self.rewrites.of(pdf, id.into()) {
            entries.set(ANNOTS, Value::Array(list));
        }
        Ok(())
    }

    /// An annotation of the document with what was changed about it.
    fn rewrite_annot(&mut self, dict: &Dict<'_>, annot: &Annot, rotation: i32) -> Entries {
        let mut entries = Entries::of(dict);
        let old = dict
            .get::<Array<'_>>(b"Rect" as &[u8])
            .map(|r| r.iter::<f64>().collect::<Vec<_>>())
            .filter(|r| r.len() == 4)
            .map_or(annot.rect, |r| {
                [
                    r[0].min(r[2]),
                    r[1].min(r[3]),
                    r[0].max(r[2]),
                    r[1].max(r[3]),
                ]
            });
        if old != annot.rect {
            entries.set(b"Rect", Value::reals(annot.rect));
            for key in [b"QuadPoints" as &[u8], b"Vertices", b"L", b"CL"] {
                if let Some(numbers) = dict.get::<Array<'_>>(key) {
                    let moved = moved_points(numbers.iter::<f64>().collect(), old, annot.rect);
                    entries.set(key, Value::reals(moved));
                }
            }
            if let Some(lines) = dict.get::<Array<'_>>(b"InkList" as &[u8]) {
                let lines = lines
                    .iter::<Array<'_>>()
                    .map(|line| {
                        Value::reals(moved_points(line.iter::<f64>().collect(), old, annot.rect))
                    })
                    .collect();
                entries.set(b"InkList", Value::Array(lines));
            }
        }
        let contents = dict
            .get::<PdfString<'_>>(b"Contents" as &[u8])
            .map_or_else(String::new, |c| crate::links::text_string(c.as_bytes()));
        let new_text = contents != annot.contents;
        if new_text {
            entries.set(b"Contents", Value::text(&annot.contents));
            entries.remove(b"RC");
        }
        let resized = (old[2] - old[0] - (annot.rect[2] - annot.rect[0])).abs() > 1e-3
            || (old[3] - old[1] - (annot.rect[3] - annot.rect[1])).abs() > 1e-3;
        if annot.kind == PdfAnnotationKind::FreeText && (new_text || resized) {
            let look = self.free_text_stream(annot, Some(dict), rotation);
            entries.set(
                b"AP",
                Value::Dict(Entries::new().with(b"N", Value::Ref(look))),
            );
        }
        if !annot.date.is_empty() {
            entries.set(b"M", Value::str(annot.date.as_bytes()));
        }
        entries
    }

    fn free_text_stream(
        &mut self,
        annot: &Annot,
        dict: Option<&Dict<'_>>,
        rotation: i32,
    ) -> ObjRef {
        let (look, _) = self.free_text_source(annot);
        let turned = matches!(rotation, 90 | 270);
        let (w, h) = (annot.rect[2] - annot.rect[0], annot.rect[3] - annot.rect[1]);
        let size = if turned { (h, w) } else { (w, h) };
        let (ops, resources) = self.with_face(&annot.contents, &look, dict, |face| {
            (
                appearance::free_text(size, &annot.contents, face, look.size, &look.color),
                face.resources(),
            )
        });
        self.update
            .stream(appearance::form_dict(size, rotation, resources), &ops)
    }

    fn new_annot(&mut self, annot: &Annot, rotation: i32) -> ObjRef {
        let subtype = match annot.kind {
            PdfAnnotationKind::Highlight => "Highlight",
            PdfAnnotationKind::Underline => "Underline",
            PdfAnnotationKind::StrikeOut => "StrikeOut",
            PdfAnnotationKind::Squiggly => "Squiggly",
            PdfAnnotationKind::Ink => "Ink",
            PdfAnnotationKind::FreeText => "FreeText",
            PdfAnnotationKind::Note => "Text",
            PdfAnnotationKind::Other => "Square",
        };
        let color = Value::reals(annot.color.map(|c| c as f64));
        let mut entries = Entries::new()
            .with(b"Type", Value::name("Annot"))
            .with(b"Subtype", Value::name(subtype))
            .with(b"Rect", Value::reals(annot.rect))
            .with(b"F", Value::Int(4));
        if !annot.contents.is_empty() {
            entries.set(b"Contents", Value::text(&annot.contents));
        }
        if !annot.date.is_empty() {
            entries.set(b"M", Value::str(annot.date.as_bytes()));
            entries.set(b"CreationDate", Value::str(annot.date.as_bytes()));
        }
        let look = match &*annot.shape {
            Shape::Quads(quads) => {
                let moved: Vec<[f64; 8]> = quads.clone();
                let all: Vec<f64> = moved.iter().flatten().copied().collect();
                entries.set(b"C", color);
                entries.set(b"QuadPoints", Value::reals(all));
                let ops = appearance::markup(annot.kind, &moved, annot.color);
                let resources = appearance::markup_resources();
                let dict = Entries::new()
                    .with(b"Type", Value::name("XObject"))
                    .with(b"Subtype", Value::name("Form"))
                    .with(b"BBox", Value::reals(annot.rect))
                    .with(b"Resources", Value::Dict(resources));
                self.update.stream(dict, &ops)
            }
            Shape::Ink(strokes, width) => {
                entries.set(b"C", color);
                entries.set(
                    b"BS",
                    Value::Dict(Entries::new().with(b"W", Value::Real(*width))),
                );
                entries.set(
                    b"InkList",
                    Value::Array(
                        strokes
                            .iter()
                            .map(|s| Value::reals(s.iter().flat_map(|p| [p.0, p.1])))
                            .collect(),
                    ),
                );
                let ops = appearance::ink(strokes, *width, annot.color);
                let dict = Entries::new()
                    .with(b"Type", Value::name("XObject"))
                    .with(b"Subtype", Value::name("Form"))
                    .with(b"BBox", Value::reals(annot.rect));
                self.update.stream(dict, &ops)
            }
            Shape::FreeText(size) => {
                let (look, _) = self.free_text_source(annot);
                let font = format!("/Helv {} Tf {}", crate::writer::number(*size), look.color);
                entries.set(b"DA", Value::str(font.as_bytes()));
                self.free_text_stream(annot, None, rotation)
            }
            Shape::Note | Shape::Old => {
                entries.set(b"C", color);
                entries.set(b"Name", Value::name("Comment"));
                // It stays the same size and upright, however the page shows.
                entries.set(b"F", Value::Int(4 + 8 + 16));
                let size = (appearance::NOTE_SIZE, appearance::NOTE_SIZE);
                self.update.stream(
                    appearance::form_dict(size, rotation, Entries::new()),
                    &appearance::note(annot.color),
                )
            }
        };
        entries.set(
            b"AP",
            Value::Dict(Entries::new().with(b"N", Value::Ref(look))),
        );
        self.update.add(&Value::Dict(entries))
    }
}

/// Writes what is in a field where a holder of it says so.
fn write_value(entries: &mut Entries, field: &Field, value: &PdfFieldValue) {
    match value {
        PdfFieldValue::Text(text) => {
            if text.is_empty() {
                entries.remove(b"V");
            } else {
                entries.set(b"V", Value::text(text));
            }
            entries.remove(b"RV");
        }
        PdfFieldValue::State(state) => {
            entries.set(
                b"V",
                Value::name(state.as_deref().unwrap_or("Off").as_bytes()),
            );
        }
        PdfFieldValue::Choice(picked) => {
            match picked.as_slice() {
                [] => entries.remove(b"V"),
                [one] => entries.set(b"V", Value::text(one)),
                many => entries.set(
                    b"V",
                    Value::Array(many.iter().map(|p| Value::text(p)).collect()),
                ),
            }
            let mut indices: Vec<i64> = picked
                .iter()
                .filter_map(|p| field.info.options.iter().position(|o| &o.0 == p))
                .map(|i| i as i64)
                .collect();
            indices.sort_unstable();
            if indices.is_empty() {
                entries.remove(b"I");
            } else {
                entries.set(
                    b"I",
                    Value::Array(indices.into_iter().map(Value::Int).collect()),
                );
            }
        }
    }
}

/// Numbers that are x and y in turn, as one box going to another takes them.
fn moved_points(numbers: Vec<f64>, from: [f64; 4], to: [f64; 4]) -> Vec<f64> {
    let (fw, fh) = ((from[2] - from[0]).max(1e-6), (from[3] - from[1]).max(1e-6));
    let (sx, sy) = ((to[2] - to[0]) / fw, (to[3] - to[1]) / fh);
    numbers
        .chunks_exact(2)
        .flat_map(|p| [to[0] + (p[0] - from[0]) * sx, to[1] + (p[1] - from[1]) * sy])
        .collect()
}
