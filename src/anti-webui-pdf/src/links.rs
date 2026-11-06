//! Where a document goes: the links of a page and its outline. `hayro` doesn't read them, so this
//! reads the dictionaries. See `docs/design/26-pdf.md`.

use hayro::hayro_interpret::RectExt;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::Pdf;
use hayro::hayro_syntax::object::dict::keys::{
    A, ANNOTS, D, DEST, DESTS, FIRST, KIDS, LIMITS, NAMES, NEXT, OUTLINES, RECT, S, SUBTYPE, TITLE,
    URI,
};
use hayro::hayro_syntax::object::{
    Array, Dict, Name, Object, ObjectIdentifier, Rect, String as PdfString,
};
use hayro::hayro_syntax::page::Page;
use hayro::kurbo::Point;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// How deep the tree of names can go, and how many outline items there can be, as a broken document
/// can make these go on forever.
const NAME_DEPTH: usize = 32;
const MOST_OUTLINE_ITEMS: usize = 100_000;

/// A place in a document.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfTarget {
    /// The page, from 0.
    pub page: usize,
    /// How far down the page, in points, when it says.
    pub top: Option<f32>,
}

/// An item of the outline of a document, which is what it has for a table of contents.
#[derive(Clone, Debug, PartialEq)]
pub struct PdfOutlineItem {
    pub title: String,
    /// None when it doesn't go anywhere in the document.
    pub target: Option<PdfTarget>,
    pub children: Vec<PdfOutlineItem>,
}

/// Where a link goes.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum LinkTarget {
    Place(PdfTarget),
    Url(String),
    /// JavaScript to run.
    Script(String),
}

/// A place on a page that goes somewhere when clicked, in the points of [`crate::text`].
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Link {
    pub bounds: [f32; 4],
    pub target: LinkTarget,
}

impl Link {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let b = &self.bounds;
        x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]
    }
}

/// Finds what the links and the outline of a document go to. A document keeps it, as the list of
/// pages takes a moment to go through.
#[derive(Default)]
pub(crate) struct Places {
    pages: OnceLock<HashMap<ObjectIdentifier, usize>>,
}

impl Places {
    fn page_of(&self, pdf: &Pdf, id: ObjectIdentifier) -> Option<usize> {
        let pages = self.pages.get_or_init(|| {
            let mut pages = HashMap::new();
            for (i, page) in pdf.pages().iter().enumerate() {
                if let Some(id) = page.raw().obj_id() {
                    pages.entry(id).or_insert(i);
                }
            }
            pages
        });
        pages.get(&id).copied()
    }

    /// A place from what a document calls a destination.
    fn target(&self, pdf: &Pdf, destination: Object<'_>, depth: usize) -> Option<PdfTarget> {
        if depth == 0 {
            return None;
        }
        match destination {
            Object::Array(array) => self.explicit(pdf, &array),
            Object::Dict(dict) => self.target(pdf, dict.get::<Object<'_>>(D)?, depth - 1),
            Object::Name(name) => {
                let named = catalog(pdf)?
                    .get::<Dict<'_>>(DESTS)?
                    .get::<Object<'_>>(name.as_ref())?;
                self.target(pdf, named, depth - 1)
            }
            Object::String(name) => {
                let tree = catalog(pdf)?
                    .get::<Dict<'_>>(NAMES)?
                    .get::<Dict<'_>>(DESTS)?;
                let named = in_name_tree(&tree, name.as_bytes(), NAME_DEPTH)?;
                self.target(pdf, named, depth - 1)
            }
            _ => None,
        }
    }

    /// `[page /XYZ left top zoom]` and the like.
    fn explicit(&self, pdf: &Pdf, array: &Array<'_>) -> Option<PdfTarget> {
        let mut items = array.iter::<Object<'_>>();
        let page = match items.next()? {
            Object::Dict(dict) => self.page_of(pdf, dict.obj_id()?)?,
            Object::Number(number) => number.as_i64().try_into().ok()?,
            _ => return None,
        };
        let shown = pdf.pages().get(page)?;
        let number = |o: Option<Object<'_>>| match o {
            Some(Object::Number(n)) => Some(n.as_f64()),
            _ => None,
        };
        let kind = match items.next() {
            Some(Object::Name(kind)) => kind,
            _ => return Some(PdfTarget { page, top: None }),
        };
        let top = match kind.as_ref() {
            b"XYZ" => {
                items.next();
                number(items.next())
            }
            b"FitH" | b"FitBH" => number(items.next()),
            b"FitR" => {
                items.next();
                items.next();
                items.next();
                number(items.next())
            }
            _ => None,
        };
        let transform = shown.initial_transform(true).to_kurbo();
        let top = top.map(|t| (transform * Point::new(0.0, t)).y as f32);
        Some(PdfTarget { page, top })
    }

    /// The links of a page.
    pub fn links(&self, pdf: &Pdf, page: &Page<'_>) -> Vec<Link> {
        let Some(annotations) = page.raw().get::<Array<'_>>(ANNOTS) else {
            return Vec::new();
        };
        let transform = page.initial_transform(true).to_kurbo();
        annotations
            .iter::<Dict<'_>>()
            .filter(|a| {
                a.get::<Name<'_>>(SUBTYPE)
                    .is_some_and(|s| s.as_ref() == b"Link")
            })
            .filter_map(|annotation| {
                let target = self.link_target(pdf, &annotation)?;
                let rect = annotation.get::<Rect>(RECT)?.to_kurbo();
                let r = transform.transform_rect_bbox(rect);
                Some(Link {
                    bounds: [r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32],
                    target,
                })
            })
            .collect()
    }

    fn link_target(&self, pdf: &Pdf, link: &Dict<'_>) -> Option<LinkTarget> {
        if let Some(destination) = link.get::<Object<'_>>(DEST) {
            return Some(LinkTarget::Place(self.target(
                pdf,
                destination,
                NAME_DEPTH,
            )?));
        }
        let action = link.get::<Dict<'_>>(A)?;
        match action.get::<Name<'_>>(S)?.as_ref() {
            b"URI" => {
                let url = action.get::<PdfString<'_>>(URI)?;
                Some(LinkTarget::Url(
                    String::from_utf8_lossy(url.as_bytes()).into_owned(),
                ))
            }
            b"GoTo" => Some(LinkTarget::Place(self.target(
                pdf,
                action.get::<Object<'_>>(D)?,
                NAME_DEPTH,
            )?)),
            b"JavaScript" => crate::marks::javascript_of(&action).map(LinkTarget::Script),
            _ => None,
        }
    }

    /// The outline of a document, which most don't have.
    pub fn outline(&self, pdf: &Pdf) -> Vec<PdfOutlineItem> {
        let Some(first) = catalog(pdf)
            .and_then(|c| c.get::<Dict<'_>>(OUTLINES))
            .and_then(|o| o.get::<Dict<'_>>(FIRST))
        else {
            return Vec::new();
        };
        let mut seen = HashSet::new();
        self.outline_items(pdf, first, &mut seen)
    }

    fn outline_items(
        &self,
        pdf: &Pdf,
        first: Dict<'_>,
        seen: &mut HashSet<ObjectIdentifier>,
    ) -> Vec<PdfOutlineItem> {
        let mut items = Vec::new();
        let mut next = Some(first);
        while let Some(item) = next {
            // Items that are seen again would go around forever.
            if !item.obj_id().is_none_or(|id| seen.insert(id)) || seen.len() > MOST_OUTLINE_ITEMS {
                break;
            }
            let target = match item.get::<Object<'_>>(DEST) {
                Some(destination) => self.target(pdf, destination, NAME_DEPTH),
                None => self.action_target(pdf, &item),
            };
            let children = item
                .get::<Dict<'_>>(FIRST)
                .map_or_else(Vec::new, |first| self.outline_items(pdf, first, seen));
            items.push(PdfOutlineItem {
                title: item
                    .get::<PdfString<'_>>(TITLE)
                    .map_or_else(String::new, |t| text_string(t.as_bytes())),
                target,
                children,
            });
            next = item.get::<Dict<'_>>(NEXT);
        }
        items
    }

    fn action_target(&self, pdf: &Pdf, item: &Dict<'_>) -> Option<PdfTarget> {
        match self.link_target(pdf, item)? {
            LinkTarget::Place(target) => Some(target),
            LinkTarget::Url(_) | LinkTarget::Script(_) => None,
        }
    }
}

fn catalog(pdf: &Pdf) -> Option<Dict<'_>> {
    let xref = pdf.xref();
    xref.get::<Dict<'_>>(xref.root_id())
}

/// Looks up a name in a name tree: `/Names [name value ...]` at the leaves and `/Kids` above them.
fn in_name_tree<'a>(node: &Dict<'a>, name: &[u8], depth: usize) -> Option<Object<'a>> {
    if depth == 0 {
        return None;
    }
    if let Some(names) = node.get::<Array<'_>>(NAMES) {
        let mut items = names.iter::<Object<'_>>();
        while let (Some(key), Some(value)) = (items.next(), items.next()) {
            if matches!(&key, Object::String(key) if key.as_bytes() == name) {
                return Some(value);
            }
        }
    }
    let kids = node.get::<Array<'_>>(KIDS)?;
    kids.iter::<Dict<'_>>()
        .filter(|kid| may_have(kid, name))
        .find_map(|kid| in_name_tree(&kid, name, depth - 1))
}

/// Whether a node of a name tree can have a name, going by the first and the last it says it has.
fn may_have(node: &Dict<'_>, name: &[u8]) -> bool {
    let Some(limits) = node.get::<Array<'_>>(LIMITS) else {
        return true;
    };
    let mut limits = limits.iter::<PdfString<'_>>();
    match (limits.next(), limits.next()) {
        (Some(first), Some(last)) => first.as_bytes() <= name && name <= last.as_bytes(),
        _ => true,
    }
}

/// What a text string of a PDF says: UTF-16 or UTF-8 after a mark that says so, and otherwise an
/// encoding that is close to Latin-1.
pub(crate) fn text_string(bytes: &[u8]) -> String {
    if let Some(utf16) = bytes.strip_prefix(&[0xfe, 0xff]) {
        let units = utf16
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]));
        return char::decode_utf16(units)
            .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
    }
    if let Some(utf8) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(utf8).into_owned();
    }
    bytes.iter().map(|&b| b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_strings_say_how_they_are_written() {
        assert_eq!(text_string(b"Intro"), "Intro");
        assert_eq!(text_string(&[0xfe, 0xff, 0x65, 0xe5, 0x67, 0x2c]), "日本");
        assert_eq!(text_string(&[0xef, 0xbb, 0xbf, 0xe6, 0x97, 0xa5]), "日");
        assert_eq!(text_string(&[0xe9]), "é");
    }
}
