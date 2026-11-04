//! Short messages that go away by themselves.

use crate::display::Tone;
use crate::theme::{QUICK, Theme, shared, theme};
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

#[derive(Clone)]
struct Entry {
    id: u64,
    toast: Toast,
}

/// What is up, and whether anything ever was: there is no portal before that.
#[derive(Clone, Copy)]
struct Queue {
    entries: Signal<Vec<Entry>>,
    used: Signal<bool>,
    next: Signal<u64>,
}

thread_local! {
    static QUEUE: Cell<Option<Queue>> = const { Cell::new(None) };
}

fn queue() -> Queue {
    QUEUE.with(|q| {
        q.get().unwrap_or_else(|| {
            // Under a root of its own: it outlives whatever asks for it first.
            let (queue, _) = create_root(|| Queue {
                entries: signal(Vec::new()),
                used: signal(false),
                next: signal(0),
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
    let id = q.next.get_untracked();
    q.next.set(id + 1);
    q.used.set_if_changed(true);
    q.entries.update(|all| all.push(Entry { id, toast }));
}

fn close(id: u64) {
    queue().entries.update(|all| all.retain(|e| e.id != id));
}

fn toast_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        s.bg(t.overlay)
            .border_color(t.border)
            .text_color(t.text)
            .shadow_lg()
    })
}

fn view(entry: Entry) -> Div {
    let Entry { id, toast } = entry;
    let timer: Rc<Cell<Option<Timeout>>> = Rc::default();
    let start: Rc<dyn Fn()> = {
        let timer = timer.clone();
        let lasts = toast.lasts;
        Rc::new(move || {
            if let Some(after) = lasts {
                timer.set(Some(set_timeout(after, move || close(id))));
            }
        })
    };
    start();
    let cleanup = timer.clone();
    on_cleanup(move || {
        if let Some(running) = cleanup.take() {
            running.cancel();
        }
    });
    let tone = toast.tone;
    let action = toast.action.map(|(name, f)| {
        crate::link(name).on_click(move |_| {
            f();
            close(id);
        })
    });
    div()
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
        .sheet(|| shared(toast_sheet))
        .on_hover(move |inside| {
            if inside {
                if let Some(running) = timer.take() {
                    running.cancel();
                }
            } else {
                start();
            }
        })
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
        )
}

/// Where toasts show: the lower right of the window, above everything. [`page`](crate::page) has
/// one.
pub fn toast_host() -> Show {
    let q = queue();
    show(
        move || q.used.get(),
        move || {
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
                        for_each(move || q.entries.get(), |e| e.id, view)
                            .enter(|s| s.opacity(0.0).translate_y(8.0))
                            .exit(|s| s.opacity(0.0))
                            .moves(QUICK)
                            .transition(QUICK),
                    ),
            )
        },
    )
}
