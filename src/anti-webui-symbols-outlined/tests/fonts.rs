//! What `tools/generate.py` made.

use anti_webui_symbols_outlined as icons;
use read_fonts::{FontRef, TableProvider};
use std::path::Path;

const AXES: [&str; 4] = ["FILL", "GRAD", "opsz", "wght"];

#[test]
fn every_file_holds_the_glyph_it_is_named_after_with_all_axes() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fonts");
    let mut count = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "ttf") {
            continue;
        }
        count += 1;
        let data = std::fs::read(&path).unwrap();
        let font = FontRef::new(&data).unwrap();
        let name = path.file_stem().unwrap().to_str().unwrap();
        let ch = u32::from_str_radix(name, 16).unwrap();
        let glyph = font.cmap().unwrap().map_codepoint(ch).unwrap();
        assert_ne!(glyph.to_u32(), 0, "{name}");
        let tags: Vec<String> = font
            .fvar()
            .unwrap()
            .axes()
            .unwrap()
            .iter()
            .map(|a| a.axis_tag().to_string())
            .collect();
        assert_eq!(tags, AXES, "{name}");
        // The box of the whole font, which says how far a glyph may reach on any of the axes.
        let head = font.head().unwrap();
        let reach = (head.x_min(), head.y_min(), head.x_max(), head.y_max());
        assert_eq!(reach, (-12, -91, 972, 961), "{name}");
    }
    assert_eq!(count, 4004);
}

#[test]
fn the_names_lead_to_the_right_files() {
    assert_eq!(icons::HOME.ch, '\u{e9b2}');
    assert_eq!(icons::HOME.family, "material-symbols-outlined-e9b2");
    assert_eq!(
        icons::HOME.font,
        std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/fonts/e9b2.ttf")).unwrap()
    );
    // A name that starts with a digit, and a second name of the same icon.
    assert_eq!(icons::_10K.ch, '\u{e951}');
    assert_eq!(icons::STAR.ch, icons::GRADE.ch);
}

#[test]
fn an_icon_has_one_address_wherever_it_is_used() {
    fn here() -> *const u8 {
        icons::HOME.font.as_ptr()
    }
    mod elsewhere {
        pub fn there() -> *const u8 {
            anti_webui_symbols_outlined::HOME.font.as_ptr()
        }
    }
    let on_a_thread = std::thread::spawn(|| icons::HOME.font.as_ptr() as usize)
        .join()
        .unwrap();
    assert_eq!(here(), elsewhere::there());
    assert_eq!(here() as usize, on_a_thread);
}
