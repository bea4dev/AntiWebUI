//! Keys and text from Win32's keyboard messages (see "キーボード" in
//! `docs/design/39-platform-windows.md`).

use anti_webui_core::event::{Key, Modifiers, NamedKey};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, ToUnicodeEx, VIRTUAL_KEY, VK_BACK, VK_CAPITAL, VK_CONTROL,
    VK_DELETE, VK_DOWN, VK_END, VK_ESCAPE, VK_F1, VK_F24, VK_HOME, VK_INSERT, VK_LCONTROL, VK_LEFT,
    VK_LMENU, VK_LSHIFT, VK_LWIN, VK_MENU, VK_NEXT, VK_PRIOR, VK_RCONTROL, VK_RETURN, VK_RIGHT,
    VK_RMENU, VK_RSHIFT, VK_RWIN, VK_SHIFT, VK_SPACE, VK_TAB, VK_UP,
};

/// The key of a virtual-key code that has a name of its own.
pub(crate) fn named_key(vk: u16) -> Option<NamedKey> {
    let vk = VIRTUAL_KEY(vk);
    Some(match vk {
        VK_RETURN => NamedKey::Enter,
        VK_TAB => NamedKey::Tab,
        VK_SPACE => NamedKey::Space,
        VK_BACK => NamedKey::Backspace,
        VK_DELETE => NamedKey::Delete,
        VK_ESCAPE => NamedKey::Escape,
        VK_LEFT => NamedKey::ArrowLeft,
        VK_RIGHT => NamedKey::ArrowRight,
        VK_UP => NamedKey::ArrowUp,
        VK_DOWN => NamedKey::ArrowDown,
        VK_HOME => NamedKey::Home,
        VK_END => NamedKey::End,
        VK_PRIOR => NamedKey::PageUp,
        VK_NEXT => NamedKey::PageDown,
        VK_INSERT => NamedKey::Insert,
        VK_SHIFT | VK_LSHIFT | VK_RSHIFT => NamedKey::Shift,
        VK_CONTROL | VK_LCONTROL | VK_RCONTROL => NamedKey::Control,
        VK_MENU | VK_LMENU | VK_RMENU => NamedKey::Alt,
        VK_LWIN | VK_RWIN => NamedKey::Super,
        VK_CAPITAL => NamedKey::CapsLock,
        _ if (VK_F1.0..=VK_F24.0).contains(&vk.0) => NamedKey::F((vk.0 - VK_F1.0 + 1) as u8),
        _ => return None,
    })
}

/// The key, with what the layout makes of it without Ctrl and Alt for those without a name.
pub(crate) fn key_of(vk: u16, scan: u32) -> Key {
    if let Some(named) = named_key(vk) {
        return Key::Named(named);
    }
    let typed = character_of(vk, scan);
    let mut chars = typed.chars().filter(|c| !c.is_control());
    match (chars.next(), chars.next()) {
        (Some(c), None) => Key::Character(c.to_string()),
        _ => Key::Unidentified,
    }
}

/// What the key types with Shift and Caps Lock as they are, and neither Ctrl nor Alt. The state of
/// the keyboard, like a dead key pressed before, stays as it is.
fn character_of(vk: u16, scan: u32) -> String {
    let mut state = [0u8; 256];
    for key in [VK_SHIFT, VK_LSHIFT, VK_RSHIFT] {
        if down(key) {
            state[key.0 as usize] = 0x80;
        }
    }
    // SAFETY: plain query.
    if unsafe { GetKeyState(VK_CAPITAL.0 as i32) } & 1 != 0 {
        state[VK_CAPITAL.0 as usize] = 0x01;
    }
    let mut buf = [0u16; 8];
    // SAFETY: valid buffers; flag 4 leaves the keyboard state alone (Windows 10 1607 and later).
    let n = unsafe {
        let layout = GetKeyboardLayout(0);
        ToUnicodeEx(vk as u32, scan, &state, &mut buf, 0x4, Some(layout))
    };
    // A dead key gives the character it puts on the next (as -1).
    let n = n.unsigned_abs() as usize;
    String::from_utf16_lossy(&buf[..n.min(buf.len())])
}

fn down(key: VIRTUAL_KEY) -> bool {
    // SAFETY: plain query.
    unsafe { GetKeyState(key.0 as i32) < 0 }
}

/// The modifiers held as the message being handled was sent.
pub(crate) fn modifiers() -> Modifiers {
    Modifiers {
        shift: down(VK_SHIFT),
        ctrl: down(VK_CONTROL),
        alt: down(VK_MENU),
        logo: down(VK_LWIN) || down(VK_RWIN),
    }
}

/// What a key event types, from the characters `TranslateMessage` made of it. Control characters
/// type nothing, and neither do shortcuts. AltGr comes as Ctrl+Alt: what it types is text, and the
/// modifiers it is made of are taken off.
pub(crate) fn text_of(vk: u16, chars: &str, modifiers: &mut Modifiers) -> Option<String> {
    if named_key(vk).is_some_and(|k| k != NamedKey::Space) {
        return None;
    }
    let text: String = chars.chars().filter(|c| !c.is_control()).collect();
    if text.is_empty() {
        return None;
    }
    if modifiers.ctrl && modifiers.alt {
        modifiers.ctrl = false;
        modifiers.alt = false;
    }
    modifiers.types_text().then_some(text)
}

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
    fn named_keys_go_by_the_virtual_key() {
        assert_eq!(named_key(VK_RETURN.0), Some(NamedKey::Enter));
        assert_eq!(named_key(VK_F1.0), Some(NamedKey::F(1)));
        assert_eq!(named_key(VK_F24.0), Some(NamedKey::F(24)));
        assert_eq!(named_key(VK_RMENU.0), Some(NamedKey::Alt));
        assert_eq!(named_key(b'A' as u16), None);
    }

    #[test]
    fn shortcuts_and_named_keys_type_nothing() {
        let ctrl = Modifiers { ctrl: true, ..NONE };
        let alt = Modifiers { alt: true, ..NONE };
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            ..NONE
        };
        let a = b'A' as u16;
        assert_eq!(text_of(a, "a", &mut NONE.clone()).as_deref(), Some("a"));
        assert_eq!(text_of(a, "\u{1}", &mut ctrl.clone()), None);
        assert_eq!(text_of(a, "a", &mut alt.clone()), None);
        assert_eq!(
            text_of(VK_SPACE.0, " ", &mut NONE.clone()).as_deref(),
            Some(" ")
        );
        assert_eq!(text_of(VK_RETURN.0, "\r", &mut NONE.clone()), None);
        let mut m = altgr;
        assert_eq!(text_of(b'E' as u16, "€", &mut m).as_deref(), Some("€"));
        assert_eq!(m, NONE, "AltGr isn't Ctrl and Alt for what it typed");
    }

    #[test]
    fn utf16_offsets_become_byte_offsets() {
        let s = "aあ😀b";
        assert_eq!(utf16_to_byte(s, 0), 0);
        assert_eq!(utf16_to_byte(s, 1), 1);
        assert_eq!(utf16_to_byte(s, 2), 4);
        assert_eq!(utf16_to_byte(s, 4), 8);
        assert_eq!(utf16_to_byte(s, 9), s.len());
    }
}
