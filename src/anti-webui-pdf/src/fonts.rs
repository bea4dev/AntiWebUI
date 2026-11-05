//! Fonts for what a document doesn't embed. Without these a lot of Japanese simply isn't there. See
//! "フォント" in `docs/design/26-pdf.md`.

use fontique::{
    Collection, CollectionOptions, FontStyle, FontWeight, FontWidth, SourceCache,
    SourceCacheOptions,
};
use hayro::hayro_interpret::FontResolverFn;
use hayro::hayro_interpret::font::{FontData, FontQuery, StandardFont};
use hayro::hayro_interpret::hayro_cmap::CidFamily;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

const SERIF_JA: &[&str] = &[
    "Noto Serif CJK JP",
    "Source Han Serif JP",
    "Source Han Serif",
    "IPAexMincho",
    "IPAMincho",
    "Yu Mincho",
    "MS Mincho",
    "Hiragino Mincho ProN",
];
const SANS_JA: &[&str] = &[
    "Noto Sans CJK JP",
    "Source Han Sans JP",
    "Source Han Sans",
    "IPAexGothic",
    "IPAGothic",
    "Yu Gothic",
    "Meiryo",
    "MS Gothic",
    "Hiragino Sans",
];
const SERIF_ZH_HANS: &[&str] = &[
    "Noto Serif CJK SC",
    "Source Han Serif SC",
    "SimSun",
    "STSong",
];
const SANS_ZH_HANS: &[&str] = &[
    "Noto Sans CJK SC",
    "Source Han Sans SC",
    "Microsoft YaHei",
    "PingFang SC",
];
const SERIF_ZH_HANT: &[&str] = &[
    "Noto Serif CJK TC",
    "Source Han Serif TC",
    "PMingLiU",
    "Songti TC",
];
const SANS_ZH_HANT: &[&str] = &[
    "Noto Sans CJK TC",
    "Source Han Sans TC",
    "Microsoft JhengHei",
    "PingFang TC",
];
const SERIF_KO: &[&str] = &[
    "Noto Serif CJK KR",
    "Source Han Serif KR",
    "Batang",
    "AppleMyungjo",
];
const SANS_KO: &[&str] = &[
    "Noto Sans CJK KR",
    "Source Han Sans KR",
    "Malgun Gothic",
    "Apple SD Gothic Neo",
];

/// Fonts that take the same room as the standard ones, as fontconfig has them.
fn stand_ins(font: StandardFont) -> &'static [&'static str] {
    use StandardFont::*;
    match font {
        Helvetica | HelveticaBold | HelveticaOblique | HelveticaBoldOblique => {
            &["Liberation Sans", "Arimo", "Arial", "Helvetica"]
        }
        Courier | CourierBold | CourierOblique | CourierBoldOblique => {
            &["Liberation Mono", "Cousine", "Courier New"]
        }
        TimesRoman | TimesBold | TimesItalic | TimesBoldItalic => {
            &["Liberation Serif", "Tinos", "Times New Roman"]
        }
        _ => &[],
    }
}

fn standard_look(font: StandardFont) -> (bool, bool) {
    use StandardFont::*;
    match font {
        HelveticaBold | CourierBold | TimesBold => (true, false),
        HelveticaOblique | CourierOblique | TimesItalic => (false, true),
        HelveticaBoldOblique | CourierBoldOblique | TimesBoldItalic => (true, true),
        _ => (false, false),
    }
}

/// The fonts of the system, which are looked at once a document asks.
struct Book {
    collection: Collection,
    cache: SourceCache,
    found: HashMap<(String, bool, bool), Option<(FontData, u32)>>,
}

impl Book {
    fn new() -> Self {
        Self {
            collection: Collection::new(CollectionOptions::default()),
            cache: SourceCache::new(SourceCacheOptions::default()),
            found: HashMap::new(),
        }
    }

    fn find(&mut self, name: &str, bold: bool, italic: bool) -> Option<(FontData, u32)> {
        let key = (name.to_owned(), bold, italic);
        if let Some(found) = self.found.get(&key) {
            return found.clone();
        }
        let found = self.load(name, bold, italic);
        self.found.insert(key, found.clone());
        found
    }

    fn load(&mut self, name: &str, bold: bool, italic: bool) -> Option<(FontData, u32)> {
        let family = self.collection.family_by_name(name)?;
        let font = family.match_font(
            FontWidth::NORMAL,
            if italic {
                FontStyle::Italic
            } else {
                FontStyle::Normal
            },
            if bold {
                FontWeight::BOLD
            } else {
                FontWeight::NORMAL
            },
            false,
        )?;
        let blob = font.load(Some(&mut self.cache))?;
        // The file is mapped, so that what isn't used takes no memory.
        Some((Arc::new(blob) as FontData, font.index()))
    }

    fn first_of(&mut self, names: &[&str], bold: bool, italic: bool) -> Option<(FontData, u32)> {
        names.iter().find_map(|name| self.find(name, bold, italic))
    }

    fn standard(&mut self, font: StandardFont) -> (FontData, u32) {
        let (bold, italic) = standard_look(font);
        self.first_of(stand_ins(font), bold, italic)
            .unwrap_or_else(|| font.get_font_data())
    }
}

fn book() -> &'static Mutex<Book> {
    static BOOK: OnceLock<Mutex<Book>> = OnceLock::new();
    BOOK.get_or_init(|| Mutex::new(Book::new()))
}

fn resolve(query: &FontQuery) -> Option<(FontData, u32)> {
    let mut book = book().lock().unwrap_or_else(|e| e.into_inner());
    match query {
        FontQuery::Standard(font) => Some(book.standard(*font)),
        FontQuery::Fallback(f) => {
            let (bold, italic) = (f.is_bold, f.is_italic);
            let named = [&f.font_family, &f.font_name, &f.post_script_name]
                .into_iter()
                .flatten()
                .find_map(|name| book.find(name, bold, italic));
            if let Some(font) = named {
                return Some(font);
            }
            let list =
                f.character_collection
                    .as_ref()
                    .and_then(|c| match (&c.family, f.is_serif) {
                        (CidFamily::AdobeJapan1, true) => Some(SERIF_JA),
                        (CidFamily::AdobeJapan1, false) => Some(SANS_JA),
                        (CidFamily::AdobeGB1, true) => Some(SERIF_ZH_HANS),
                        (CidFamily::AdobeGB1, false) => Some(SANS_ZH_HANS),
                        (CidFamily::AdobeCNS1, true) => Some(SERIF_ZH_HANT),
                        (CidFamily::AdobeCNS1, false) => Some(SANS_ZH_HANT),
                        (CidFamily::AdobeKorea1, true) => Some(SERIF_KO),
                        (CidFamily::AdobeKorea1, false) => Some(SANS_KO),
                        _ => None,
                    });
            if let Some(font) = list.and_then(|list| book.first_of(list, bold, italic)) {
                return Some(font);
            }
            Some(book.standard(f.pick_standard_font()))
        }
    }
}

pub(crate) fn resolver() -> FontResolverFn {
    Arc::new(resolve)
}

/// The fonts to write text with that the fonts of a document can't write, in the order to try them.
const WRITING: &[&[&str]] = &[
    &[
        "Liberation Sans",
        "Arimo",
        "Arial",
        "Helvetica",
        "DejaVu Sans",
        "Noto Sans",
    ],
    SANS_JA,
    SANS_ZH_HANS,
    SANS_ZH_HANT,
    SANS_KO,
];

/// A font of the system that has all of the characters, or as many of them as there are.
pub(crate) fn covering(chars: &BTreeSet<char>) -> Option<(FontData, u32)> {
    let mut book = book().lock().unwrap_or_else(|e| e.into_inner());
    let mut best: Option<((FontData, u32), usize)> = None;
    for names in WRITING {
        let Some(font) = book.first_of(names, false, false) else {
            continue;
        };
        let has = crate::embed::coverage((*font.0).as_ref(), font.1, chars);
        if has == chars.len() {
            return Some(font);
        }
        if best.as_ref().is_none_or(|b| has > b.1) {
            best = Some((font, has));
        }
    }
    best.map(|b| b.0)
}
