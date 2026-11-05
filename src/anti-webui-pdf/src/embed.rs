//! Part of a font of the system, for text that the fonts of a document can't write. Only the
//! characters that are used go in. See "フォント" in `docs/design/26-pdf.md`.

use crate::fonts;
use crate::writer::{Entries, Update, Value};
use read_fonts::{FontRef, TableProvider, types::Tag};
use std::collections::{BTreeSet, HashMap};
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};
use subsetter::GlyphRemapper;

/// How many characters a section of a character map can have.
const CMAP_SECTION: usize = 100;

/// The part of a font that goes into a document.
#[derive(Debug)]
pub(crate) struct Subset {
    data: Vec<u8>,
    /// It has the outlines of PostScript rather than TrueType.
    is_cff: bool,
    name: String,
    /// The character, its number in the part and how wide it is for a thousand units.
    glyphs: HashMap<char, (u16, u16)>,
    ascent: i32,
    descent: i32,
    /// What it has, which is more than `glyphs` for what it doesn't.
    chars: BTreeSet<char>,
}

impl Subset {
    pub fn width(&self, c: char) -> f64 {
        self.glyphs.get(&c).map_or(0.0, |g| g.1 as f64)
    }

    pub fn cid(&self, c: char) -> u16 {
        self.glyphs.get(&c).map_or(0, |g| g.0)
    }
}

/// The last part that was made, as it doesn't have to be made again until there are more characters.
#[derive(Default)]
pub(crate) struct SubsetCache(Mutex<Option<Arc<Subset>>>);

impl SubsetCache {
    pub fn get(&self, chars: &BTreeSet<char>) -> Option<Arc<Subset>> {
        let mut cached = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(subset) = cached.as_ref()
            && chars.is_subset(&subset.chars)
        {
            return Some(subset.clone());
        }
        // Whatever it had goes along, so that going back and forth doesn't make it again.
        let mut all = chars.clone();
        if let Some(subset) = cached.as_ref() {
            all.extend(subset.chars.iter().copied());
        }
        let subset = Arc::new(make(&all)?);
        *cached = Some(subset.clone());
        Some(subset)
    }
}

/// How many of the characters a font has.
pub(crate) fn coverage(data: &[u8], index: u32, chars: &BTreeSet<char>) -> usize {
    let Ok(font) = FontRef::from_index(data, index) else {
        return 0;
    };
    let Ok(cmap) = font.cmap() else {
        return 0;
    };
    chars
        .iter()
        .filter(|c| cmap.map_codepoint(**c).is_some_and(|g| g.to_u32() != 0))
        .count()
}

fn make(chars: &BTreeSet<char>) -> Option<Subset> {
    let (font_data, index) = fonts::covering(chars)?;
    let data: &[u8] = (*font_data).as_ref();
    let font = FontRef::from_index(data, index).ok()?;
    let cmap = font.cmap().ok()?;
    let hmtx = font.hmtx().ok()?;
    let head = font.head().ok()?;
    let hhea = font.hhea().ok()?;
    let units = head.units_per_em().max(1) as f64;
    let thousand = |v: f64| (v * 1000.0 / units).round() as i32;
    let number_of_metrics = hhea.number_of_h_metrics() as usize;
    let metrics = hmtx.h_metrics();
    let advance = |glyph: usize| {
        let last = number_of_metrics
            .saturating_sub(1)
            .min(metrics.len().saturating_sub(1));
        metrics.get(glyph.min(last)).map_or(0, |m| m.advance()) as f64
    };

    let mut remapper = GlyphRemapper::new();
    let mut glyphs = HashMap::new();
    for c in chars {
        let Some(glyph) = cmap.map_codepoint(*c).filter(|g| g.to_u32() != 0) else {
            continue;
        };
        let glyph = glyph.to_u32();
        let cid = remapper.remap(u16::try_from(glyph).ok()?);
        glyphs.insert(
            *c,
            (
                cid,
                thousand(advance(glyph as usize)).clamp(0, 65535) as u16,
            ),
        );
    }
    let subset = subsetter::subset(data, index, &remapper).ok()?;
    let is_cff = font.table_data(Tag::new(b"CFF ")).is_some();
    let kept = if is_cff {
        // What the part has is a whole font file with little in it, and a PDF only takes the outlines.
        FontRef::new(&subset)
            .ok()?
            .table_data(Tag::new(b"CFF "))?
            .as_bytes()
            .to_vec()
    } else {
        subset
    };
    let name = font
        .name()
        .ok()
        .and_then(|n| {
            n.name_record()
                .iter()
                .find(|r| r.name_id().to_u16() == 6)
                .and_then(|r| r.string(n.string_data()).ok())
                .map(|s| {
                    s.chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
                        .collect::<String>()
                })
        })
        .filter(|n: &String| !n.is_empty())
        .unwrap_or_else(|| "Embedded".to_owned());
    Some(Subset {
        data: kept,
        is_cff,
        name,
        glyphs,
        ascent: thousand(hhea.ascender().to_i16() as f64),
        descent: thousand(hhea.descender().to_i16() as f64),
        chars: chars.clone(),
    })
}

/// Writes the objects of a part of a font, and says what refers to it.
pub(crate) fn write(update: &mut Update, subset: &Subset) -> Value {
    // Six letters say it is only part of a font. It is always the same one here, and that's fine.
    let name = format!("AWUIPD+{}", subset.name);
    let file_key: &[u8] = if subset.is_cff {
        b"FontFile3"
    } else {
        b"FontFile2"
    };
    let file_dict = if subset.is_cff {
        Entries::new().with(b"Subtype", Value::name("CIDFontType0C"))
    } else {
        Entries::new().with(b"Length1", Value::Int(subset.data.len() as i64))
    };
    let file = update.stream(file_dict, &subset.data);

    let descriptor = Entries::new()
        .with(b"Type", Value::name("FontDescriptor"))
        .with(b"FontName", Value::name(&name))
        .with(b"Flags", Value::Int(4))
        .with(
            b"FontBBox",
            Value::Array(vec![
                Value::Int(-1000),
                Value::Int(subset.descent as i64),
                Value::Int(2000),
                Value::Int(subset.ascent as i64),
            ]),
        )
        .with(b"ItalicAngle", Value::Int(0))
        .with(b"Ascent", Value::Int(subset.ascent as i64))
        .with(b"Descent", Value::Int(subset.descent as i64))
        .with(b"CapHeight", Value::Int(subset.ascent as i64))
        .with(b"StemV", Value::Int(80))
        .with(file_key, Value::Ref(file));
    let descriptor = update.add(&Value::Dict(descriptor));

    let mut widths: Vec<(u16, u16)> = subset.glyphs.values().copied().collect();
    widths.sort_unstable();
    let mut w = Vec::new();
    for (cid, width) in widths {
        w.push(Value::Int(cid as i64));
        w.push(Value::Array(vec![Value::Int(width as i64)]));
    }
    let info = Entries::new()
        .with(b"Registry", Value::str(b"Adobe"))
        .with(b"Ordering", Value::str(b"Identity"))
        .with(b"Supplement", Value::Int(0));
    let mut descendant = Entries::new()
        .with(b"Type", Value::name("Font"))
        .with(
            b"Subtype",
            Value::name(if subset.is_cff {
                "CIDFontType0"
            } else {
                "CIDFontType2"
            }),
        )
        .with(b"BaseFont", Value::name(&name))
        .with(b"CIDSystemInfo", Value::Dict(info))
        .with(b"FontDescriptor", Value::Ref(descriptor))
        .with(b"DW", Value::Int(1000))
        .with(b"W", Value::Array(w));
    if !subset.is_cff {
        descendant.set(b"CIDToGIDMap", Value::name("Identity"));
    }
    let descendant = update.add(&Value::Dict(descendant));

    let to_unicode = update.stream(Entries::new(), to_unicode(subset).as_bytes());
    Value::Dict(
        Entries::new()
            .with(b"Type", Value::name("Font"))
            .with(b"Subtype", Value::name("Type0"))
            .with(b"BaseFont", Value::name(&name))
            .with(b"Encoding", Value::name("Identity-H"))
            .with(
                b"DescendantFonts",
                Value::Array(vec![Value::Ref(descendant)]),
            )
            .with(b"ToUnicode", Value::Ref(to_unicode)),
    )
}

/// The map that makes the text that was put in with a part of a font copy and search as what it says.
fn to_unicode(subset: &Subset) -> String {
    let mut map: Vec<(u16, char)> = subset.glyphs.iter().map(|(c, g)| (g.0, *c)).collect();
    map.sort_unstable();
    let mut out = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <ffff>\nendcodespacerange\n",
    );
    for section in map.chunks(CMAP_SECTION) {
        let _ = writeln!(out, "{} beginbfchar", section.len());
        for (cid, c) in section {
            let mut units = [0u16; 2];
            let hex: String = c
                .encode_utf16(&mut units)
                .iter()
                .map(|u| format!("{u:04x}"))
                .collect();
            let _ = writeln!(out, "<{cid:04x}> <{hex}>");
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out
}
