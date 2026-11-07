//! Keys: AppKit's key codes and characters to the core's keys (see "入力" in
//! `docs/design/37-platform-macos.md`).

use anti_webui_core::event::{Key, Modifiers, NamedKey};
use objc2_app_kit::NSEventModifierFlags;

/// The named key of a key code (`kVK_*` of `HIToolbox/Events.h`), whatever the layout.
pub(crate) fn named_key(code: u16) -> Option<NamedKey> {
    Some(match code {
        0x24 | 0x4C => NamedKey::Enter,
        0x30 => NamedKey::Tab,
        0x31 => NamedKey::Space,
        0x33 => NamedKey::Backspace,
        0x75 => NamedKey::Delete,
        0x35 => NamedKey::Escape,
        0x7B => NamedKey::ArrowLeft,
        0x7C => NamedKey::ArrowRight,
        0x7D => NamedKey::ArrowDown,
        0x7E => NamedKey::ArrowUp,
        0x73 => NamedKey::Home,
        0x77 => NamedKey::End,
        0x74 => NamedKey::PageUp,
        0x79 => NamedKey::PageDown,
        // Help is where Insert is on keyboards made for other systems.
        0x72 => NamedKey::Insert,
        0x38 | 0x3C => NamedKey::Shift,
        0x3B | 0x3E => NamedKey::Control,
        0x3A | 0x3D => NamedKey::Alt,
        0x37 | 0x36 => NamedKey::Super,
        0x39 => NamedKey::CapsLock,
        0x7A => NamedKey::F(1),
        0x78 => NamedKey::F(2),
        0x63 => NamedKey::F(3),
        0x76 => NamedKey::F(4),
        0x60 => NamedKey::F(5),
        0x61 => NamedKey::F(6),
        0x62 => NamedKey::F(7),
        0x64 => NamedKey::F(8),
        0x65 => NamedKey::F(9),
        0x6D => NamedKey::F(10),
        0x67 => NamedKey::F(11),
        0x6F => NamedKey::F(12),
        0x69 => NamedKey::F(13),
        0x6B => NamedKey::F(14),
        0x71 => NamedKey::F(15),
        0x6A => NamedKey::F(16),
        0x40 => NamedKey::F(17),
        0x4F => NamedKey::F(18),
        0x50 => NamedKey::F(19),
        0x5A => NamedKey::F(20),
        _ => return None,
    })
}

/// The key of a key code, with `ignoring` (`charactersIgnoringModifiers`) for those without a
/// name: the character of the layout, which Shift changes and Option and Control don't.
pub(crate) fn key_of(code: u16, ignoring: &str) -> Key {
    if let Some(named) = named_key(code) {
        return Key::Named(named);
    }
    let mut chars = ignoring.chars().filter(|c| !is_not_text(*c));
    match (chars.next(), chars.next()) {
        (Some(c), None) => Key::Character(c.to_string()),
        _ => Key::Unidentified,
    }
}

/// Control characters, and AppKit's characters for keys without text (`NSUpArrowFunctionKey` and
/// the others, in the private use area from U+F700).
fn is_not_text(c: char) -> bool {
    c.is_control() || ('\u{F700}'..='\u{F8FF}').contains(&c)
}

/// The text a key types, given `characters` of its event: none with the modifiers of shortcuts, or
/// for keys that have no text. Option types characters, like `å`.
pub(crate) fn text_of(code: u16, characters: &str, modifiers: Modifiers) -> Option<String> {
    if modifiers.ctrl || modifiers.logo {
        return None;
    }
    // Space types itself; the other named keys are what the core acts on.
    if named_key(code).is_some_and(|k| k != NamedKey::Space) {
        return None;
    }
    (!characters.is_empty() && !characters.chars().any(is_not_text)).then(|| characters.to_owned())
}

pub(crate) fn modifiers_of(flags: NSEventModifierFlags) -> Modifiers {
    Modifiers {
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        logo: flags.contains(NSEventModifierFlags::Command),
    }
}

/// Whether the modifier key of `code` is down after a `flagsChanged:` with these flags. `None` if
/// it isn't a modifier key.
pub(crate) fn modifier_down(code: u16, flags: NSEventModifierFlags) -> Option<bool> {
    let flag = match named_key(code)? {
        NamedKey::Shift => NSEventModifierFlags::Shift,
        NamedKey::Control => NSEventModifierFlags::Control,
        NamedKey::Alt => NSEventModifierFlags::Option,
        NamedKey::Super => NSEventModifierFlags::Command,
        NamedKey::CapsLock => NSEventModifierFlags::CapsLock,
        _ => return None,
    };
    Some(flags.contains(flag))
}

/// The byte offset of a UTF-16 offset into `text`, clamped to it.
pub(crate) fn utf16_to_byte(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (i, c) in text.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    text.len()
}

/// The UTF-16 offset of a byte offset into `text`, clamped to it.
pub(crate) fn byte_to_utf16(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    let mut at = byte;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    text[..at].chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers {
        shift: false,
        ctrl: false,
        alt: false,
        logo: false,
    };

    #[test]
    fn named_keys_go_by_the_code_and_others_by_the_layout() {
        assert_eq!(key_of(0x7B, "\u{F702}"), Key::Named(NamedKey::ArrowLeft));
        assert_eq!(key_of(0x24, "\r"), Key::Named(NamedKey::Enter));
        assert_eq!(key_of(0x4C, "\u{3}"), Key::Named(NamedKey::Enter));
        assert_eq!(key_of(0x7A, "\u{F704}"), Key::Named(NamedKey::F(1)));
        // `a` on QWERTY, and the same key on another layout.
        assert_eq!(key_of(0x00, "a"), Key::Character("a".into()));
        assert_eq!(key_of(0x00, "A"), Key::Character("A".into()));
        assert_eq!(key_of(0x00, "ф"), Key::Character("ф".into()));
        assert_eq!(key_of(0x0A, ""), Key::Unidentified);
    }

    #[test]
    fn shortcuts_and_named_keys_type_nothing() {
        let cmd = Modifiers { logo: true, ..NONE };
        let ctrl = Modifiers { ctrl: true, ..NONE };
        let option = Modifiers { alt: true, ..NONE };
        assert_eq!(text_of(0x00, "a", NONE).as_deref(), Some("a"));
        assert_eq!(text_of(0x00, "a", cmd), None);
        assert_eq!(text_of(0x00, "\u{1}", ctrl), None);
        assert_eq!(text_of(0x00, "å", option).as_deref(), Some("å"));
        assert_eq!(text_of(0x31, " ", NONE).as_deref(), Some(" "));
        assert_eq!(text_of(0x24, "\r", NONE), None);
        assert_eq!(text_of(0x33, "\u{7f}", NONE), None);
        assert_eq!(text_of(0x7E, "\u{F700}", NONE), None);
    }

    #[test]
    fn modifier_keys_say_whether_they_are_down() {
        let shift = NSEventModifierFlags::Shift;
        assert_eq!(modifier_down(0x38, shift), Some(true));
        assert_eq!(
            modifier_down(0x3C, NSEventModifierFlags::empty()),
            Some(false)
        );
        assert_eq!(
            modifier_down(0x37, NSEventModifierFlags::Command),
            Some(true)
        );
        assert_eq!(modifier_down(0x00, shift), None);
        let m = modifiers_of(NSEventModifierFlags::Option | NSEventModifierFlags::Command);
        assert_eq!(
            m,
            Modifiers {
                alt: true,
                logo: true,
                ..NONE
            }
        );
    }

    #[test]
    fn utf16_offsets_and_bytes() {
        let s = "aあ😀b";
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 1), 1);
        assert_eq!(utf16_to_byte(s, 2), 4);
        assert_eq!(utf16_to_byte(s, 4), 8);
        assert_eq!(utf16_to_byte(s, 5), 9);
        assert_eq!(utf16_to_byte(s, 99), 9);
        assert_eq!(byte_to_utf16(s, 4), 2);
        assert_eq!(byte_to_utf16(s, 8), 4);
        assert_eq!(
            byte_to_utf16(s, 6),
            2,
            "inside a character goes to its start"
        );
        assert_eq!(byte_to_utf16(s, 99), 5);
    }
}
