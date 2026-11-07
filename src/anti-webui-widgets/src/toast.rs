//! Short messages that go away by themselves.

use crate::display::Tone;
use crate::theme::{QUICK, Theme, theme, themed};
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::Duration;

const LASTS: Duration = Duration::from_millis(5000);

/// A message, with something to do about it if there is.
#[derive(Clone)]
pub struct Toast {
    text: String,
    tone: Tone,
    action: Option<(String, Rc<dyn Fn()>)>,
    lasts: Option<Duration>,
}

impl Toast {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            tone: Tone::Neutral,
            action: None,
            lasts: Some(LASTS),
        }
    }

    /// What it means, which its color says.
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }

    /// Something to do. It then stays until it is closed, unless told otherwise.
    pub fn action(mut self, label: impl Into<String>, f: impl Fn() + 'static) -> Self {
        self.action = Some((label.into(), Rc::new(f)));
        self.lasts = None;
        self
    }

    /// Stays for this long. The pointer on it makes it wait.
    pub fn lasting(mut self, duration: Duration) -> Self {
        self.lasts = Some(duration);
        self
    }
}

/// What a toast's action does.
pub type ToastAction = Rc<dyn Fn()>;

/// Messages that come and go, in the order they came: what toasts and snackbars are made from. It
/// is a handle: copy it to where messages are shown from, and show [`list`](Self::list) somewhere.
pub struct MessageQueue<T: 'static> {
    entries: Signal<Vec<(u64, T)>>,
    next: Signal<u64>,
}

impl<T> Clone for MessageQueue<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for MessageQueue<T> {}

/// A queue of messages, owned where it is made.
pub fn message_queue<T: Clone + 'static>() -> MessageQueue<T> {
    MessageQueue {
        entries: signal(Vec::new()),
        next: signal(0),
    }
}

impl<T: Clone + 'static> MessageQueue<T> {
    /// Adds a message at the end, and gives what it is known by.
    pub fn push(&self, message: T) -> u64 {
        let id = self.next.get_untracked();
        self.next.set(id + 1);
        self.entries.update(|all| all.push((id, message)));
        id
    }

    pub fn close(&self, id: u64) {
        self.entries.update(|all| all.retain(|(i, _)| *i != id));
    }

    /// Closes the first message, so that the next comes if only one is shown at a time.
    pub fn close_first(&self) {
        self.entries.update(|all| {
            if !all.is_empty() {
                all.remove(0);
            }
        });
    }

    /// The messages, oldest first. Reading it tracks it.
    pub fn list(&self) -> Vec<(u64, T)> {
        self.entries.get()
    }

    /// Closes message `id` after `lasts`, which the pointer on `el` makes wait for as long as it
    /// is there. Without a time it stays until it is closed. What it waits for goes with `el`.
    pub fn timed(&self, id: u64, lasts: Option<Duration>, el: Div) -> Div {
        let Some(after) = lasts else {
            return el;
        };
        let queue = *self;
        let timer: Rc<Cell<Option<Timeout>>> = Rc::default();
        let start: Rc<dyn Fn()> = {
            let timer = timer.clone();
            Rc::new(move || timer.set(Some(set_timeout(after, move || queue.close(id)))))
        };
        start();
        let cleanup = timer.clone();
        on_cleanup(move || {
            if let Some(running) = cleanup.take() {
                running.cancel();
            }
        });
        el.on_hover(move |inside| {
            if let Some(running) = timer.take() {
                running.cancel();
            }
            if !inside {
                start();
            }
        })
    }
}

/// What is up, and whether anything ever was: there is no portal before that.
#[derive(Clone, Copy)]
struct Queue {
    messages: MessageQueue<Toast>,
    used: Signal<bool>,
}

thread_local! {
    static QUEUE: Cell<Option<Queue>> = const { Cell::new(None) };
}

fn queue() -> Queue {
    QUEUE.with(|q| {
        q.get().unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever asks for it first.
            let (queue, _) = create_root(|| Queue {
                messages: message_queue(),
                used: signal(false),
            });
            q.set(Some(queue));
            queue
        })
    })
}

/// Shows a message for a few seconds. [`page`](crate::page) shows them; without it,
/// [`toast_host`] has to be somewhere.
pub fn toast(text: impl Into<String>) {
    toast_with(Toast::new(text));
}

/// Shows a message with more to it.
pub fn toast_with(toast: Toast) {
    let q = queue();
    q.used.set_if_changed(true);
    q.messages.push(toast);
}

/// The queue the toasts are in, for a host of one's own ([`toast_host_with`]).
pub fn toasts() -> MessageQueue<Toast> {
    queue().messages
}

impl Toast {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn tone_of(&self) -> Tone {
        self.tone
    }

    /// The label of what can be done, and what that does.
    pub fn action_of(&self) -> Option<(&str, ToastAction)> {
        self.action.as_ref().map(|(l, f)| (l.as_str(), f.clone()))
    }

    /// How long it stays.
    pub fn lasts(&self) -> Option<Duration> {
        self.lasts
    }
}

fn close(id: u64) {
    queue().messages.close(id);
}

fn toast_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.overlay)
            .border_color(t.border)
            .text_color(t.text)
            .shadow_lg()
    })
}

fn view(id: u64, toast: Toast) -> Div {
    let lasts = toast.lasts;
    let tone = toast.tone;
    let action = toast.action.map(|(name, f)| {
        crate::link(name).on_click(move |_| {
            f();
            close(id);
        })
    });
    let el = div()
        .access(|a| a.role(Role::Status))
        .flex()
        .flex_row()
        .items_center()
        .gap(12.0)
        .min_w(240.0)
        .max_w(420.0)
        .px(12.0)
        .py(8.0)
        .rounded(8.0)
        .border(1.0)
        .pointer_events_auto()
        .sheet(|| themed(toast_sheet))
        .children((tone != Tone::Neutral).then(|| {
            div()
                .flex_none()
                .w(4.0)
                .self_stretch()
                .rounded(2.0)
                .dyn_style(move |s| {
                    let t = theme();
                    s.bg(tone.colors(&t).1)
                })
        }))
        .child(div().flex_1().min_w(0.0).child(text(toast.text)))
        .children(action)
        .child(
            crate::quiet_button("×")
                .flex_none()
                .px(6.0)
                .py(0.0)
                .on_click(move |_| close(id)),
        );
    queue().messages.timed(id, lasts, el)
}

/// Where toasts show: the lower right of the window, above everything. [`page`](crate::page) has
/// one.
pub fn toast_host() -> Show {
    toast_host_with(view)
}

/// A host of toasts that look like whatever `look` builds from what each one is known by and says.
/// They come up from the lower right; [`MessageQueue::timed`] on [`toasts`] makes one go by itself.
pub fn toast_host_with(look: impl Fn(u64, Toast) -> Div + 'static) -> Show {
    let q = queue();
    let look = Rc::new(look);
    show(
        move || q.used.get(),
        move || {
            let look = look.clone();
            portal().child(
                div()
                    .absolute()
                    .right(16.0)
                    .bottom(16.0)
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(8.0)
                    .pointer_events_none()
                    .child(
                        for_each(
                            move || q.messages.list(),
                            |(id, _)| *id,
                            move |(id, toast)| look(id, toast),
                        )
                        .enter(|s| s.opacity(0.0).translate_y(8.0))
                        .exit(|s| s.opacity(0.0))
                        .moves(QUICK)
                        .transition(QUICK),
                    ),
            )
        },
    )
}
