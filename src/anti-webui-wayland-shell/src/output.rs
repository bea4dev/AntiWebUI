//! Outputs, and following them as they come and go (see "ディスプレイ" in
//! `docs/design/37-wayland-shell.md`).

use anti_webui_core::geometry::{PointI, SizeI};
use anti_webui_core::reactive::{Owner, create_root, effect, on_cleanup, untrack};
use anti_webui_core::window::{UiThreadHandle, spawn_ui_thread};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

/// How an output is turned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum OutputTransform {
    #[default]
    Normal,
    Rotated90,
    Rotated180,
    Rotated270,
    Flipped,
    Flipped90,
    Flipped180,
    Flipped270,
}

impl OutputTransform {
    pub(crate) fn from_wayland(t: wayland_client::protocol::wl_output::Transform) -> Self {
        use wayland_client::protocol::wl_output::Transform as T;
        match t {
            T::_90 => Self::Rotated90,
            T::_180 => Self::Rotated180,
            T::_270 => Self::Rotated270,
            T::Flipped => Self::Flipped,
            T::Flipped90 => Self::Flipped90,
            T::Flipped180 => Self::Flipped180,
            T::Flipped270 => Self::Flipped270,
            _ => Self::Normal,
        }
    }

    /// Whether width and height trade places.
    pub fn is_sideways(self) -> bool {
        matches!(
            self,
            Self::Rotated90 | Self::Rotated270 | Self::Flipped90 | Self::Flipped270
        )
    }
}

/// An output, as it was when read. Outputs are the same output when their names are.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Output {
    /// The connector, like `DP-1`.
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    /// Where it is in the desktop, in logical pixels.
    pub position: PointI,
    pub logical_size: SizeI,
    /// The mode it is in, in pixels, and its refresh rate in millihertz.
    pub mode: SizeI,
    pub refresh_mhz: i32,
    /// Pixels of the mode to a logical pixel. Surfaces draw at the scale the compositor gives each,
    /// so this is to show, not to draw by.
    pub scale: f32,
    pub transform: OutputTransform,
}

impl Output {
    /// The output as it is now, if it is still there. Reading it tracks it.
    pub fn current(&self) -> Option<Output> {
        outputs().into_iter().find(|o| o.name == self.name)
    }
}

/// The outputs there are now. Reading it tracks it, on any thread.
pub fn outputs() -> Vec<Output> {
    crate::conn::shared().outputs.get()
}

thread_local! {
    /// The outputs of the `for_each_output` calls going on, innermost last.
    static CURRENT: RefCell<Vec<Output>> = const { RefCell::new(Vec::new()) };
    /// The output of a thread from `spawn_for_each_output`.
    static THREAD_OUTPUT: RefCell<Option<Output>> = const { RefCell::new(None) };
}

/// The output of the [`for_each_output`] this runs in, or of the thread from
/// [`spawn_for_each_output`]. Layers opened without one go there.
pub fn current_output() -> Option<Output> {
    CURRENT
        .with(|c| c.borrow().last().cloned())
        .or_else(|| THREAD_OUTPUT.with(|t| t.borrow().clone()))
}

fn with_output<R>(output: Output, f: impl FnOnce() -> R) -> R {
    CURRENT.with(|c| c.borrow_mut().push(output));
    struct Pop;
    impl Drop for Pop {
        fn drop(&mut self) {
            CURRENT.with(|c| c.borrow_mut().pop());
        }
    }
    let _pop = Pop;
    f()
}

/// Keeps something per output, made when one comes and ended when it goes or when the owner this
/// was called under is.
fn follow<T: 'static>(start: impl Fn(Output) -> T + 'static, end: impl Fn(T) + 'static) {
    let live: Rc<RefCell<Vec<(String, T)>>> = Rc::default();
    let end = Rc::new(end);
    let (l, e) = (live.clone(), end.clone());
    effect(move || {
        let now = outputs();
        untrack(|| {
            let gone: Vec<T> = {
                let mut live = l.borrow_mut();
                let mut gone = Vec::new();
                let mut kept = Vec::new();
                for (name, t) in live.drain(..) {
                    if now.iter().any(|o| o.name == name) {
                        kept.push((name, t));
                    } else {
                        gone.push(t);
                    }
                }
                *live = kept;
                gone
            };
            for t in gone {
                e(t);
            }
            for o in now {
                if !l.borrow().iter().any(|(name, _)| *name == o.name) {
                    let name = o.name.clone();
                    let t = start(o);
                    l.borrow_mut().push((name, t));
                }
            }
        });
    });
    on_cleanup(move || {
        let all: Vec<T> = live.borrow_mut().drain(..).map(|(_, t)| t).collect();
        for t in all {
            end(t);
        }
    });
}

/// Calls `f` for every output, now and as they are plugged in, on this thread. What `f` makes, and
/// the layers it opens, go when the output does.
///
/// [`current_output`] gives the output while `f` runs, so layers opened then go on it. Layers
/// opened later, as from a handler, say which with [`LayerOptions::output`](crate::LayerOptions::output).
pub fn for_each_output(f: impl Fn(Output) + 'static) {
    follow(
        move |output| {
            let ((), owner) = create_root(|| with_output(output.clone(), || f(output)));
            owner
        },
        Owner::dispose,
    );
}

/// Calls `f` for every output, now and as they are plugged in, each on a UI thread of its own, so
/// that what one output shows never holds up another. The thread ends when the output goes.
/// Every layer opened on that thread goes on its output unless told otherwise.
pub fn spawn_for_each_output(f: impl Fn(Output) + Send + Sync + 'static) {
    let f = Arc::new(f);
    follow(
        move |output| {
            let f = f.clone();
            spawn_ui_thread(move || {
                THREAD_OUTPUT.with(|t| *t.borrow_mut() = Some(output.clone()));
                f(output)
            })
        },
        |thread: UiThreadHandle| thread.stop(),
    );
}
