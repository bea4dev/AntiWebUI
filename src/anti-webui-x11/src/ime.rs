//! Input methods through XIM, on the spot (see "IME" in `docs/design/35-platform-x11.md`).
//! fcitx5 and IBus both serve it. Keys go to the input method first while it is on, and come back
//! if it doesn't use them.

use anti_webui_core::event::ImeUpdate;
use anti_webui_core::geometry::RectI;
use anti_webui_core::platform::{ImePurpose, ImeState};
use std::rc::Rc;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{KeyPressEvent, Window};
use x11rb::xcb_ffi::XCBConnection;
use xim::x11rb::X11rbClient;
use xim::{
    AHashMap, AttributeName, Client, ClientError, ClientHandler, Feedback, ForwardEventFlag,
    InputStyle, Point, PreeditDrawStatus,
};

type XimClient = X11rbClient<Rc<XCBConnection>>;

/// What the input method says, for the state to pass on.
pub(crate) enum ImeOut {
    Update(ImeUpdate),
    /// A key it didn't use.
    Key(KeyPressEvent),
}

#[derive(Default)]
struct Handler {
    window: Window,
    im: Option<u16>,
    ic: Option<u16>,
    /// What is being composed, by characters as XIM counts them, and where the caret is in it.
    preedit: Vec<char>,
    caret: usize,
    out: Vec<ImeOut>,
}

impl Handler {
    fn tell_preedit(&mut self) {
        let text: String = self.preedit.iter().collect();
        let preedit = (!text.is_empty()).then(|| {
            let at = self.preedit[..self.caret.min(self.preedit.len())]
                .iter()
                .map(|c| c.len_utf8())
                .sum::<usize>();
            (text, Some((at, at)))
        });
        self.out.push(ImeOut::Update(ImeUpdate {
            preedit,
            commit: None,
            delete_surrounding: None,
        }));
    }
}

impl ClientHandler<XimClient> for Handler {
    fn handle_connect(&mut self, client: &mut XimClient) -> Result<(), ClientError> {
        client.open("en_US.UTF-8")
    }

    fn handle_open(&mut self, client: &mut XimClient, im: u16) -> Result<(), ClientError> {
        self.im = Some(im);
        client.get_im_values(im, &[AttributeName::QueryInputStyle])
    }

    fn handle_get_im_values(
        &mut self,
        client: &mut XimClient,
        im: u16,
        _attributes: AHashMap<AttributeName, Vec<u8>>,
    ) -> Result<(), ClientError> {
        let attributes = client
            .build_ic_attributes()
            .push(
                AttributeName::InputStyle,
                InputStyle::PREEDIT_CALLBACKS | InputStyle::STATUS_NOTHING,
            )
            .push(AttributeName::ClientWindow, self.window)
            .push(AttributeName::FocusWindow, self.window)
            .nested_list(AttributeName::PreeditAttributes, |b| {
                b.push(AttributeName::SpotLocation, Point { x: 0, y: 0 });
            })
            .build();
        client.create_ic(im, attributes)
    }

    fn handle_create_ic(
        &mut self,
        _client: &mut XimClient,
        _im: u16,
        ic: u16,
    ) -> Result<(), ClientError> {
        self.ic = Some(ic);
        Ok(())
    }

    fn handle_disconnect(&mut self) {
        self.im = None;
        self.ic = None;
    }

    fn handle_commit(
        &mut self,
        _client: &mut XimClient,
        _im: u16,
        _ic: u16,
        text: &str,
    ) -> Result<(), ClientError> {
        self.preedit.clear();
        self.caret = 0;
        self.out.push(ImeOut::Update(ImeUpdate {
            preedit: None,
            commit: Some(text.to_owned()),
            delete_surrounding: None,
        }));
        Ok(())
    }

    fn handle_forward_event(
        &mut self,
        _client: &mut XimClient,
        _im: u16,
        _ic: u16,
        _flag: ForwardEventFlag,
        xev: KeyPressEvent,
    ) -> Result<(), ClientError> {
        self.out.push(ImeOut::Key(xev));
        Ok(())
    }

    fn handle_preedit_draw(
        &mut self,
        _client: &mut XimClient,
        _im: u16,
        _ic: u16,
        caret: i32,
        chg_first: i32,
        chg_len: i32,
        _status: PreeditDrawStatus,
        preedit_string: &str,
        _feedbacks: Vec<Feedback>,
    ) -> Result<(), ClientError> {
        let first = (chg_first.max(0) as usize).min(self.preedit.len());
        let end = (first + chg_len.max(0) as usize).min(self.preedit.len());
        self.preedit.splice(first..end, preedit_string.chars());
        self.caret = caret.max(0) as usize;
        self.tell_preedit();
        Ok(())
    }

    fn handle_preedit_caret(
        &mut self,
        _client: &mut XimClient,
        _im: u16,
        _ic: u16,
        position: &mut i32,
        _direction: xim::CaretDirection,
        _style: xim::CaretStyle,
    ) -> Result<(), ClientError> {
        *position = self.caret as i32;
        Ok(())
    }

    fn handle_preedit_done(
        &mut self,
        _client: &mut XimClient,
        _im: u16,
        _ic: u16,
    ) -> Result<(), ClientError> {
        if !self.preedit.is_empty() {
            self.preedit.clear();
            self.caret = 0;
            self.tell_preedit();
        }
        Ok(())
    }
}

pub(crate) struct Ime {
    client: XimClient,
    handler: Handler,
    /// The input method has the keys.
    focused: bool,
    spot: Option<Point>,
}

impl Ime {
    /// `None` without an XIM server (`XMODIFIERS` names it).
    pub fn new(conn: Rc<XCBConnection>, screen: usize, window: Window) -> Option<Self> {
        let client = match X11rbClient::init(conn, screen, None) {
            Ok(c) => c,
            Err(e) => {
                log::debug!("no XIM: {e}");
                return None;
            }
        };
        Some(Self {
            client,
            handler: Handler {
                window,
                ..Default::default()
            },
            focused: false,
            spot: None,
        })
    }

    /// Lets the input method see the event first: what is for it doesn't go further.
    pub fn filter(&mut self, event: &Event) -> bool {
        match self.client.filter_event(event, &mut self.handler) {
            Ok(used) => used,
            Err(e) => {
                log::debug!("XIM: {e}");
                false
            }
        }
    }

    /// Whether keys go to the input method.
    pub fn takes_keys(&self) -> bool {
        self.focused && self.handler.ic.is_some()
    }

    pub fn forward(&mut self, key: &KeyPressEvent) {
        let (Some(im), Some(ic)) = (self.handler.im, self.handler.ic) else {
            return;
        };
        let _ = self
            .client
            .forward_event(im, ic, ForwardEventFlag::empty(), key);
    }

    /// Follows what the core wants: on for something editable, off otherwise and for passwords
    /// (Chromium turns it off there too).
    pub fn set(&mut self, state: Option<&ImeState>) {
        let (Some(im), Some(ic)) = (self.handler.im, self.handler.ic) else {
            return;
        };
        let on = state.is_some_and(|s| s.purpose != ImePurpose::Password);
        if on != self.focused {
            self.focused = on;
            let _ = if on {
                self.client.set_focus(im, ic)
            } else {
                self.client.unset_focus(im, ic)
            };
            if !on && !self.handler.preedit.is_empty() {
                self.handler.preedit.clear();
                self.handler.caret = 0;
                self.handler.tell_preedit();
            }
        }
        if let Some(rect) = state.filter(|_| on).map(|s| s.cursor_rect) {
            let spot = spot_of(rect);
            if self.spot.as_ref() != Some(&spot) {
                self.spot = Some(spot.clone());
                let attributes = self
                    .client
                    .build_ic_attributes()
                    .nested_list(AttributeName::PreeditAttributes, |b| {
                        b.push(AttributeName::SpotLocation, spot);
                    })
                    .build();
                let _ = self.client.set_ic_values(im, ic, attributes);
            }
        }
    }

    pub fn take_out(&mut self) -> Vec<ImeOut> {
        std::mem::take(&mut self.handler.out)
    }
}

/// Where the candidates go: under the caret, in window coordinates.
fn spot_of(rect: RectI) -> Point {
    let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
    Point {
        x: clamp(rect.x),
        y: clamp(rect.bottom()),
    }
}
