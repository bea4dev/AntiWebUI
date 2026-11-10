//! Fingers on a precision touchpad, through DirectManipulation (see "タッチパッド" in
//! `docs/design/39-platform-windows.md`).
//!
//! Without it, Windows turns the fingers into `WM_MOUSEWHEEL` at a rate of its own, which isn't
//! that of the screen, without a beginning or an end, and with inertia that can't be told from
//! the fingers. DirectManipulation instead follows the fingers as a transform of a viewport that
//! is never drawn, sampled once per vertical blank, and tells when they land, lift and when the
//! inertia it computes stops. Chromium does the same.

use crate::keyboard;
use crate::wndproc::Shared;
use anti_webui_core::event::{PinchPhase, ScrollPhase, ScrollSource, WindowEvent};
use anti_webui_core::geometry::PointF;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::DirectManipulation::*;
use windows::Win32::Graphics::Gdi::ScreenToClient;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::SystemInformation::GetTickCount;
use windows::Win32::UI::Input::Pointer::GetPointerType;
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, POINTER_INPUT_TYPE, PT_TOUCHPAD};
use windows::core::{Ref, implement};

/// The viewport's size. It is never drawn, so any will do; the content moves without bounds.
const SIDE: f32 = 1000.0;

/// How many vertical blanks the viewport is sampled for after fingers land, waiting for it to
/// say it runs. Fingers that rest without moving never make it run.
const LANDING: u32 = 30;

/// What one sample of the viewport, or a change of its status, comes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Gesture {
    Scroll {
        dx: f32,
        dy: f32,
        phase: ScrollPhase,
    },
    Pinch {
        scale: f32,
        phase: PinchPhase,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// Fingers landed; what they do isn't known yet.
    Undecided,
    Scroll,
    Pinch,
    /// A pinch was lifted; what follows of it is ignored.
    Over,
}

/// Turns the viewport's transforms and statuses into gestures. A gesture is a scroll or a pinch
/// from its first motion to its end, as in Chromium.
#[derive(Debug)]
pub(crate) struct Tracker {
    status: DIRECTMANIPULATION_STATUS,
    kind: Kind,
    /// The last transform: translation and scale.
    x: f32,
    y: f32,
    scale: f32,
}

impl Default for Tracker {
    fn default() -> Self {
        Self {
            status: DIRECTMANIPULATION_READY,
            kind: Kind::Undecided,
            x: 0.0,
            y: 0.0,
            scale: 1.0,
        }
    }
}

impl Tracker {
    /// The viewport's status changed. Also whether its transform has to go back to identity.
    pub fn status(
        &mut self,
        current: DIRECTMANIPULATION_STATUS,
        previous: DIRECTMANIPULATION_STATUS,
        out: &mut Vec<Gesture>,
    ) -> bool {
        self.status = current;
        if current == previous {
            return false;
        }
        match current {
            DIRECTMANIPULATION_RUNNING => {
                if previous == DIRECTMANIPULATION_INERTIA && self.kind == Kind::Scroll {
                    // Fingers came back and stopped the inertia; their motion begins anew.
                    out.push(scroll(0.0, 0.0, ScrollPhase::Held));
                }
                self.kind = Kind::Undecided;
                false
            }
            DIRECTMANIPULATION_INERTIA => {
                match self.kind {
                    Kind::Scroll => {
                        out.push(scroll(0.0, 0.0, ScrollPhase::Lifted { fling: false }))
                    }
                    Kind::Pinch => {
                        out.push(Gesture::Pinch {
                            scale: 1.0,
                            phase: PinchPhase::Ended,
                        });
                        self.kind = Kind::Over;
                    }
                    Kind::Undecided | Kind::Over => self.kind = Kind::Over,
                }
                false
            }
            DIRECTMANIPULATION_READY => {
                match self.kind {
                    Kind::Scroll => {
                        if previous == DIRECTMANIPULATION_RUNNING {
                            out.push(scroll(0.0, 0.0, ScrollPhase::Lifted { fling: false }));
                        }
                        out.push(scroll(0.0, 0.0, ScrollPhase::MomentumEnded));
                    }
                    Kind::Pinch => out.push(Gesture::Pinch {
                        scale: 1.0,
                        phase: PinchPhase::Ended,
                    }),
                    Kind::Undecided | Kind::Over => {}
                }
                self.kind = Kind::Undecided;
                let moved = (self.x, self.y, self.scale) != (0.0, 0.0, 1.0);
                (self.x, self.y, self.scale) = (0.0, 0.0, 1.0);
                moved
            }
            _ => false,
        }
    }

    /// A new transform of the content: its translation in physical pixels, and its scale.
    pub fn content(&mut self, x: f32, y: f32, scale: f32) -> Option<Gesture> {
        // Windows has been seen to give a scale of 0.
        if scale == 0.0 {
            return None;
        }
        let moving = matches!(
            self.status,
            DIRECTMANIPULATION_RUNNING | DIRECTMANIPULATION_INERTIA
        );
        let (dx, dy, by) = (self.x - x, self.y - y, scale / self.scale);
        if !moving {
            // Going back to identity, or something left of a gesture.
            (self.x, self.y, self.scale) = (x, y, scale);
            return None;
        }
        let scaled = (by - 1.0).abs() > 1e-4;
        if !scaled && dx == 0.0 && dy == 0.0 {
            return None;
        }
        (self.x, self.y, self.scale) = (x, y, scale);
        let inertia = self.status == DIRECTMANIPULATION_INERTIA;
        match self.kind {
            Kind::Undecided if inertia => None,
            Kind::Undecided if scaled => {
                self.kind = Kind::Pinch;
                Some(Gesture::Pinch {
                    scale: by,
                    phase: PinchPhase::Began,
                })
            }
            Kind::Undecided => {
                self.kind = Kind::Scroll;
                Some(scroll(dx, dy, ScrollPhase::Began))
            }
            Kind::Scroll if inertia => Some(scroll(dx, dy, ScrollPhase::Momentum)),
            Kind::Scroll => Some(scroll(dx, dy, ScrollPhase::Moved)),
            Kind::Pinch if !inertia && scaled => Some(Gesture::Pinch {
                scale: by,
                phase: PinchPhase::Moved,
            }),
            Kind::Pinch | Kind::Over => None,
        }
    }

    pub fn active(&self) -> bool {
        self.status != DIRECTMANIPULATION_READY
    }
}

/// Content that follows the fingers moves the other way from what it shows: the deltas are what
/// a scroll moves the content up and left by.
fn scroll(dx: f32, dy: f32, phase: ScrollPhase) -> Gesture {
    Gesture::Scroll { dx, dy, phase }
}

/// What the event handler and the window share.
struct Inner {
    shared: Rc<Shared>,
    tracker: RefCell<Tracker>,
    /// Fingers are on the touchpad, or inertia goes on: the viewport wants updates every frame.
    active: Cell<bool>,
}

impl Inner {
    fn deliver(&self, gestures: Vec<Gesture>) {
        if gestures.is_empty() {
            return;
        }
        let hwnd = self.shared.hwnd.get();
        let mut p = POINT::default();
        // SAFETY: plain queries; our window.
        let time = unsafe {
            let _ = GetCursorPos(&mut p);
            let _ = ScreenToClient(hwnd, &mut p);
            // The clock of the messages' times.
            GetTickCount()
        };
        let position = PointF::new(p.x as f32, p.y as f32);
        let modifiers = keyboard::modifiers();
        for g in gestures {
            self.shared.push(match g {
                Gesture::Scroll { dx, dy, phase } => WindowEvent::Scroll {
                    position,
                    delta_x: dx,
                    delta_y: dy,
                    modifiers,
                    source: ScrollSource::Touchpad,
                    phase,
                    time,
                },
                Gesture::Pinch { scale, phase } => WindowEvent::Pinch {
                    position,
                    scale,
                    modifiers,
                    phase,
                },
            });
        }
    }
}

#[implement(IDirectManipulationViewportEventHandler)]
struct Handler(Rc<Inner>);

impl IDirectManipulationViewportEventHandler_Impl for Handler_Impl {
    fn OnViewportStatusChanged(
        &self,
        viewport: Ref<IDirectManipulationViewport>,
        current: DIRECTMANIPULATION_STATUS,
        previous: DIRECTMANIPULATION_STATUS,
    ) -> windows::core::Result<()> {
        let inner = &self.0;
        let mut out = Vec::new();
        let Ok(mut tracker) = inner.tracker.try_borrow_mut() else {
            return Ok(());
        };
        let reset = tracker.status(current, previous, &mut out);
        inner.active.set(tracker.active());
        drop(tracker);
        inner.deliver(out);
        if reset && let Some(v) = viewport.as_ref() {
            // So that every gesture starts from identity. Updates to it come while ready, and
            // are ignored.
            // SAFETY: the viewport that called.
            unsafe {
                let _ = v.ZoomToRect(0.0, 0.0, SIDE, SIDE, false);
            }
        }
        Ok(())
    }

    fn OnViewportUpdated(
        &self,
        _viewport: Ref<IDirectManipulationViewport>,
    ) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnContentUpdated(
        &self,
        _viewport: Ref<IDirectManipulationViewport>,
        content: Ref<IDirectManipulationContent>,
    ) -> windows::core::Result<()> {
        let Some(content) = content.as_ref() else {
            return Ok(());
        };
        let mut m = [0f32; 6];
        // SAFETY: room for the 6 values of the matrix.
        unsafe { content.GetContentTransform(&mut m)? };
        let inner = &self.0;
        let Ok(mut tracker) = inner.tracker.try_borrow_mut() else {
            return Ok(());
        };
        let g = tracker.content(m[4], m[5], m[0]);
        drop(tracker);
        inner.deliver(g.into_iter().collect());
        Ok(())
    }
}

/// DirectManipulation for a window.
pub(crate) struct Manipulation {
    hwnd: HWND,
    manager: IDirectManipulationManager,
    update: IDirectManipulationUpdateManager,
    viewport: IDirectManipulationViewport,
    cookie: u32,
    inner: Rc<Inner>,
    /// Samples left before giving up on fingers that landed (see [`LANDING`]).
    landing: Cell<u32>,
}

impl Manipulation {
    /// `None` where DirectManipulation is missing; the touchpad then comes as a wheel.
    pub fn new(hwnd: HWND, shared: Rc<Shared>) -> Option<Self> {
        // SAFETY: COM on this thread (see `Session::connect`); our window.
        unsafe {
            let manager: IDirectManipulationManager =
                CoCreateInstance(&DirectManipulationManager, None, CLSCTX_INPROC_SERVER).ok()?;
            let update: IDirectManipulationUpdateManager = manager.GetUpdateManager().ok()?;
            let viewport: IDirectManipulationViewport = manager.CreateViewport(None, hwnd).ok()?;
            viewport
                .ActivateConfiguration(
                    DIRECTMANIPULATION_CONFIGURATION_INTERACTION
                        | DIRECTMANIPULATION_CONFIGURATION_TRANSLATION_X
                        | DIRECTMANIPULATION_CONFIGURATION_TRANSLATION_Y
                        | DIRECTMANIPULATION_CONFIGURATION_TRANSLATION_INERTIA
                        | DIRECTMANIPULATION_CONFIGURATION_RAILS_X
                        | DIRECTMANIPULATION_CONFIGURATION_RAILS_Y
                        | DIRECTMANIPULATION_CONFIGURATION_SCALING,
                )
                .ok()?;
            // Updates come when asked for, once per vertical blank.
            viewport
                .SetViewportOptions(DIRECTMANIPULATION_VIEWPORT_OPTIONS_MANUALUPDATE)
                .ok()?;
            let inner = Rc::new(Inner {
                shared,
                tracker: RefCell::new(Tracker::default()),
                active: Cell::new(false),
            });
            let handler: IDirectManipulationViewportEventHandler = Handler(inner.clone()).into();
            let cookie = viewport.AddEventHandler(Some(hwnd), &handler).ok()?;
            let rect = RECT {
                left: 0,
                top: 0,
                right: SIDE as i32,
                bottom: SIDE as i32,
            };
            let this = Self {
                hwnd,
                manager,
                update,
                viewport,
                cookie,
                inner,
                landing: Cell::new(0),
            };
            this.viewport.SetViewportRect(&rect).ok()?;
            this.manager.Activate(hwnd).ok()?;
            this.viewport.Enable().ok()?;
            Some(this)
        }
    }

    /// The viewport, for `DM_POINTERHITTEST` to hand it the fingers.
    pub fn viewport(&self) -> IDirectManipulationViewport {
        self.viewport.clone()
    }

    /// Fingers landed on the viewport.
    pub fn touched(&self) {
        self.landing.set(LANDING);
    }

    /// Whether the viewport wants [`Manipulation::update`] every frame.
    pub fn active(&self) -> bool {
        self.inner.active.get() || self.landing.get() > 0
    }

    /// Samples the viewport: its event handler turns what changed into events.
    pub fn update(&self) {
        self.landing.set(self.landing.get().saturating_sub(1));
        // SAFETY: our update manager; the handler doesn't reach back here.
        unsafe {
            let _ = self.update.Update(None);
        }
        if self.inner.active.get() {
            self.landing.set(0);
        }
    }
}

impl Drop for Manipulation {
    fn drop(&mut self) {
        // SAFETY: what `new` set up, before the window goes.
        unsafe {
            let _ = self.viewport.RemoveEventHandler(self.cookie);
            let _ = self.viewport.Abandon();
            let _ = self.manager.Deactivate(self.hwnd);
        }
    }
}

/// `DM_POINTERHITTEST`: fingers landed on the touchpad over the window. Whether DirectManipulation
/// took them; otherwise they come as a wheel.
pub(crate) fn hit_test(viewport: &IDirectManipulationViewport, pointer: u32) -> bool {
    let mut kind = POINTER_INPUT_TYPE::default();
    // SAFETY: plain query; the pointer came with the message.
    unsafe {
        GetPointerType(pointer, &mut kind).is_ok()
            && kind == PT_TOUCHPAD
            && viewport.SetContact(pointer).is_ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &mut Tracker, current: DIRECTMANIPULATION_STATUS) -> (Vec<Gesture>, bool) {
        let mut out = Vec::new();
        let previous = t.status;
        let reset = t.status(current, previous, &mut out);
        (out, reset)
    }

    #[test]
    fn a_scroll_with_inertia() {
        let mut t = Tracker::default();
        assert_eq!(run(&mut t, DIRECTMANIPULATION_RUNNING).0, vec![]);
        // Fingers move up: the content follows them up, and scrolls down.
        assert_eq!(
            t.content(0.0, -10.0, 1.0),
            Some(scroll(0.0, 10.0, ScrollPhase::Began))
        );
        assert_eq!(t.content(0.0, -10.0, 1.0), None);
        assert_eq!(
            t.content(-2.0, -15.0, 1.0),
            Some(scroll(2.0, 5.0, ScrollPhase::Moved))
        );
        assert_eq!(
            run(&mut t, DIRECTMANIPULATION_INERTIA).0,
            vec![scroll(0.0, 0.0, ScrollPhase::Lifted { fling: false })]
        );
        assert_eq!(
            t.content(-2.0, -25.0, 1.0),
            Some(scroll(0.0, 10.0, ScrollPhase::Momentum))
        );
        let (out, reset) = run(&mut t, DIRECTMANIPULATION_READY);
        assert_eq!(out, vec![scroll(0.0, 0.0, ScrollPhase::MomentumEnded)]);
        assert!(reset);
        // Going back to identity says nothing.
        assert_eq!(t.content(0.0, 0.0, 1.0), None);
        assert!(!t.active());
    }

    #[test]
    fn lifted_without_inertia() {
        let mut t = Tracker::default();
        run(&mut t, DIRECTMANIPULATION_RUNNING);
        t.content(0.0, 4.0, 1.0);
        let (out, _) = run(&mut t, DIRECTMANIPULATION_READY);
        assert_eq!(
            out,
            vec![
                scroll(0.0, 0.0, ScrollPhase::Lifted { fling: false }),
                scroll(0.0, 0.0, ScrollPhase::MomentumEnded),
            ]
        );
    }

    #[test]
    fn fingers_stop_the_inertia() {
        let mut t = Tracker::default();
        run(&mut t, DIRECTMANIPULATION_RUNNING);
        t.content(0.0, -10.0, 1.0);
        run(&mut t, DIRECTMANIPULATION_INERTIA);
        t.content(0.0, -20.0, 1.0);
        assert_eq!(
            run(&mut t, DIRECTMANIPULATION_RUNNING).0,
            vec![scroll(0.0, 0.0, ScrollPhase::Held)]
        );
        // The transform goes on from where it was.
        assert_eq!(
            t.content(0.0, -23.0, 1.0),
            Some(scroll(0.0, 3.0, ScrollPhase::Began))
        );
    }

    #[test]
    fn a_pinch() {
        let mut t = Tracker::default();
        run(&mut t, DIRECTMANIPULATION_RUNNING);
        assert_eq!(
            t.content(-5.0, -5.0, 1.25),
            Some(Gesture::Pinch {
                scale: 1.25,
                phase: PinchPhase::Began
            })
        );
        // Moving while pinching doesn't scroll.
        assert_eq!(t.content(-9.0, -9.0, 1.25), None);
        assert_eq!(
            t.content(-9.0, -9.0, 1.5),
            Some(Gesture::Pinch {
                scale: 1.2,
                phase: PinchPhase::Moved
            })
        );
        assert_eq!(
            run(&mut t, DIRECTMANIPULATION_INERTIA).0,
            vec![Gesture::Pinch {
                scale: 1.0,
                phase: PinchPhase::Ended
            }]
        );
        // The inertia of the fingers' motion is no scroll.
        assert_eq!(t.content(-19.0, -19.0, 1.5), None);
        let (out, reset) = run(&mut t, DIRECTMANIPULATION_READY);
        assert_eq!(out, vec![]);
        assert!(reset);
    }
}
