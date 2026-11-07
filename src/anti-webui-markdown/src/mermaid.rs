//! Mermaid diagrams: made into SVG on a thread of their own, then into nodes by `svg::parse`. See
//! "Mermaid" in `docs/design/28-markdown.md`.
//!
//! Only the backend knows the crate that makes the SVG: `setup` on the thread, then `render`.

mod merman;
use self::merman as backend;

use anti_webui_core::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, OnceLock};

/// How many diagrams are kept once made, by their source.
const KEPT: usize = 64;

type Made = Result<Arc<str>, String>;

struct Job {
    source: String,
    done: Box<dyn FnOnce(Made) + Send>,
}

thread_local! {
    static MADE: RefCell<(HashMap<u64, Made>, Vec<u64>)> = RefCell::default();
}

/// The one thread diagrams are made on, started the first time one is wanted.
fn worker() -> &'static std::sync::Mutex<Sender<Job>> {
    static WORKER: OnceLock<std::sync::Mutex<Sender<Job>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("markdown-diagrams".into())
            .spawn(move || {
                backend::setup();
                for job in rx {
                    let made = render(&job.source);
                    // What `svg::parse` needs on the UI thread, which would stop it for as long.
                    if let Ok(svg) = &made {
                        svg::prepare_fonts_for(svg.as_bytes());
                    }
                    (job.done)(made);
                }
            })
            .expect("the thread starts");
        std::sync::Mutex::new(tx)
    })
}

/// Makes the SVG of a diagram. Public for measuring, through `mermaid_svg`.
pub(crate) fn render(source: &str) -> Made {
    let started = std::time::Instant::now();
    let made = backend::render(source).map(|svg| Arc::from(svg.as_str()));
    log::debug!(
        "made a diagram in {:.1} ms",
        started.elapsed().as_secs_f64() * 1000.0
    );
    made
}

/// Makes the SVG of a diagram on this thread, as the diagrams of a document are made. For
/// measuring and comparing backends.
#[doc(hidden)]
pub fn mermaid_svg(source: &str) -> Result<String, String> {
    thread_local! {
        static READY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    if !READY.with(|r| r.replace(true)) {
        backend::setup();
    }
    render(source).map(|s| s.to_string())
}

fn hash_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

fn keep(key: u64, made: Made) {
    MADE.with(|m| {
        let (map, order) = &mut *m.borrow_mut();
        if map.insert(key, made).is_none() {
            order.push(key);
        }
        if order.len() > KEPT {
            let old = order.remove(0);
            map.remove(&old);
        }
    });
}

/// A diagram from its Mermaid source. Until it is made, there is an empty box.
pub(crate) fn diagram(source: &str) -> View {
    let key = hash_of(source);
    let made = signal(MADE.with(|m| m.borrow().0.get(&key).cloned()));
    if made.get_untracked().is_none() {
        let tx = ui_channel(move |m: Made| {
            keep(key, m.clone());
            made.set(Some(m));
        });
        let work = tx.work();
        let job = Job {
            source: source.to_owned(),
            done: Box::new(move |m| {
                tx.send(m);
                drop(work);
            }),
        };
        let _ = worker().lock().map(|w| w.send(job));
    }
    let source = source.to_owned();
    div()
        .flex()
        .justify_center()
        .child(dyn_view(move || match made.get() {
            None => div().h(48.0).into_view(),
            Some(Ok(svg)) => match svg::parse(svg.as_bytes()) {
                Ok(picture) => picture.into_view(),
                Err(e) => failed(&source, &e),
            },
            Some(Err(e)) => failed(&source, &e),
        }))
        .into_view()
}

/// What shows when a diagram can't be made: why, and its source.
fn failed(source: &str, why: &str) -> View {
    log::debug!("a diagram can't be made: {why}");
    div()
        .w_full()
        .p(12.0)
        .rounded(6.0)
        .bg(Color::from_rgba(0xcf222e14))
        .child(
            text(format!("Mermaid: {why}"))
                .text_color(Color::from_rgb(0xcf222e))
                .text_size(13.0),
        )
        .child(
            text(source.to_owned())
                .font_family(FontFamily::from("monospace"))
                .text_size(13.0)
                .whitespace_nowrap(),
        )
        .into_view()
}
