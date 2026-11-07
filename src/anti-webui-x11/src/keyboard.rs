//! The keyboard through XKB and libxkbcommon-x11.

use anti_webui_core::event::{Key, KeyEvent, Modifiers, NamedKey};
use x11rb::xcb_ffi::XCBConnection;
use xkbcommon::xkb;
use xkbcommon::xkb::keysyms as ks;

pub(crate) struct Keyboard {
    context: xkb::Context,
    device: i32,
    state: Option<xkb::State>,
    pub modifiers: Modifiers,
}

impl Keyboard {
    /// `None` without the XKEYBOARD extension.
    pub fn new(conn: &XCBConnection) -> Option<Self> {
        let (mut major, mut minor, mut event, mut error) = (0, 0, 0, 0);
        let ok = xkb::x11::setup_xkb_extension(
            conn,
            xkb::x11::MIN_MAJOR_XKB_VERSION,
            xkb::x11::MIN_MINOR_XKB_VERSION,
            xkb::x11::SetupXkbExtensionFlags::NoFlags,
            &mut major,
            &mut minor,
            &mut event,
            &mut error,
        );
        if !ok {
            return None;
        }
        let mut kb = Self {
            context: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            device: -1,
            state: None,
            modifiers: Modifiers::default(),
        };
        kb.reload(conn);
        Some(kb)
    }

    /// The core keyboard's device, which XKB's requests and events name.
    pub fn device(&self) -> i32 {
        self.device
    }

    /// Reads the keymap again, after it or the keyboard changed.
    pub fn reload(&mut self, conn: &XCBConnection) {
        self.device = xkb::x11::get_core_keyboard_device_id(conn);
        if self.device < 0 {
            self.state = None;
            return;
        }
        let keymap = xkb::x11::keymap_new_from_device(
            &self.context,
            conn,
            self.device,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        );
        let state = xkb::x11::state_new_from_device(&keymap, conn, self.device);
        self.state = Some(state);
        self.read_modifiers();
    }

    /// What `XkbStateNotify` says.
    pub fn update(&mut self, mods: [u32; 3], groups: [i32; 3]) {
        let Some(state) = &mut self.state else { return };
        state.update_mask(
            mods[0],
            mods[1],
            mods[2],
            groups[0] as u32,
            groups[1] as u32,
            groups[2] as u32,
        );
        self.read_modifiers();
    }

    fn read_modifiers(&mut self) {
        let Some(state) = &self.state else { return };
        let active = |name: &str| state.mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE);
        self.modifiers = Modifiers {
            shift: active(xkb::MOD_NAME_SHIFT),
            ctrl: active(xkb::MOD_NAME_CTRL),
            alt: active(xkb::MOD_NAME_ALT),
            logo: active(xkb::MOD_NAME_LOGO),
        };
    }

    /// Translates an X keycode into a key event.
    pub fn key_event(&self, keycode: u8, pressed: bool, repeat: bool) -> Option<KeyEvent> {
        let state = self.state.as_ref()?;
        let code = xkb::Keycode::new(keycode as u32);
        let sym = state.key_get_one_sym(code);
        let key = map_keysym(sym);
        let text = if pressed && !self.modifiers.ctrl && !self.modifiers.alt && !self.modifiers.logo
        {
            let t = state.key_get_utf8(code);
            (!t.is_empty() && !t.chars().any(char::is_control)).then_some(t)
        } else {
            None
        };
        Some(KeyEvent {
            key,
            text,
            modifiers: self.modifiers,
            pressed,
            repeat,
        })
    }
}

/// The same as the Wayland backend's.
fn map_keysym(sym: xkb::Keysym) -> Key {
    let named = match sym.raw() {
        ks::KEY_Return | ks::KEY_KP_Enter => NamedKey::Enter,
        ks::KEY_Tab | ks::KEY_ISO_Left_Tab => NamedKey::Tab,
        ks::KEY_space => NamedKey::Space,
        ks::KEY_BackSpace => NamedKey::Backspace,
        ks::KEY_Delete | ks::KEY_KP_Delete => NamedKey::Delete,
        ks::KEY_Escape => NamedKey::Escape,
        ks::KEY_Left | ks::KEY_KP_Left => NamedKey::ArrowLeft,
        ks::KEY_Right | ks::KEY_KP_Right => NamedKey::ArrowRight,
        ks::KEY_Up | ks::KEY_KP_Up => NamedKey::ArrowUp,
        ks::KEY_Down | ks::KEY_KP_Down => NamedKey::ArrowDown,
        ks::KEY_Home | ks::KEY_KP_Home => NamedKey::Home,
        ks::KEY_End | ks::KEY_KP_End => NamedKey::End,
        ks::KEY_Page_Up | ks::KEY_KP_Page_Up => NamedKey::PageUp,
        ks::KEY_Page_Down | ks::KEY_KP_Page_Down => NamedKey::PageDown,
        ks::KEY_Insert | ks::KEY_KP_Insert => NamedKey::Insert,
        ks::KEY_Shift_L | ks::KEY_Shift_R => NamedKey::Shift,
        ks::KEY_Control_L | ks::KEY_Control_R => NamedKey::Control,
        ks::KEY_Alt_L | ks::KEY_Alt_R => NamedKey::Alt,
        ks::KEY_Super_L | ks::KEY_Super_R => NamedKey::Super,
        ks::KEY_Caps_Lock => NamedKey::CapsLock,
        s @ ks::KEY_F1..=ks::KEY_F24 => NamedKey::F((s - ks::KEY_F1 + 1) as u8),
        _ => {
            let c = xkb::keysym_to_utf32(sym);
            return match char::from_u32(c).filter(|c| *c != '\0' && !c.is_control()) {
                Some(c) => Key::Character(c.to_string()),
                None => Key::Unidentified,
            };
        }
    };
    Key::Named(named)
}
