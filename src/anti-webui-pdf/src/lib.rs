//! A fast PDF viewer: `pdf_view` shows a document in a box that scrolls and zooms. Pages are drawn
//! on other threads with `hayro`, and only those that show and come next. See
//! `docs/design/26-pdf.md`.

mod document;
mod engine;
mod fonts;
mod plan;

use anti_webui_core::channel::ui_channel;
use anti_webui_core::event::{Key, KeyboardEvent, NamedKey, ScrollEvent};
use anti_webui_core::geometry::PointF;
use anti_webui_core::images::Bitmap;
use anti_webui_core::prelude::*;
use engine::{Engine, Event};
use plan::{Direction, PageTable};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

/// How many physical pixels a point takes at 100% and a scale of 1, as in CSS.
const PX_PER_PT: f64 = 96.0 / 72.0;
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 16.0;

/// Why a document can't be shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfError {
    /// It takes a password: see [`PdfViewer::unlock`].
    NeedsPassword,
    /// That wasn't the password.
    WrongPassword,
    /// It can't be read.
    Source(String),
    /// It isn't a PDF, or it is too broken to read.
    Invalid,
    /// It is a kind of PDF this can't read, like an unknown kind of encryption.
    Unsupported(String),
}

impl std::fmt::Display for PdfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PdfError::NeedsPassword => f.write_str("this document needs a password"),
            PdfError::WrongPassword => f.write_str("that isn't the password"),
            PdfError::Source(e) => write!(f, "can't read the document: {e}"),
            PdfError::Invalid => f.write_str("this isn't a PDF, or it is too broken to read"),
            PdfError::Unsupported(e) => write!(f, "can't read this kind of PDF: {e}"),
        }
    }
}

impl std::error::Error for PdfError {}

/// How big a document shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PdfZoom {
    /// 1.0 is 100%: a point is `96 / 72` logical pixels, as in CSS.
    Percent(f32),
    /// The widest page takes the width of the box.
    FitWidth,
    /// The first page fits in the box.
    FitPage,
}

/// How far along a document is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfState {
    Loading,
    Ready,
    Failed(PdfError),
}

/// What a [`pdf_view`] shows.
#[derive(Clone, Debug)]
pub enum PdfSource {
    Path(PathBuf),
    Bytes(Arc<[u8]>),
}

impl From<&str> for PdfSource {
    fn from(path: &str) -> Self {
        Self::Path(path.into())
    }
}

impl From<String> for PdfSource {
    fn from(path: String) -> Self {
        Self::Path(path.into())
    }
}

impl From<&Path> for PdfSource {
    fn from(path: &Path) -> Self {
        Self::Path(path.into())
    }
}

impl From<PathBuf> for PdfSource {
    fn from(path: PathBuf) -> Self {
        Self::Path(path)
    }
}

impl From<Vec<u8>> for PdfSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes.into())
    }
}

impl From<Arc<[u8]>> for PdfSource {
    fn from(bytes: Arc<[u8]>) -> Self {
        Self::Bytes(bytes)
    }
}

/// Controls a [`pdf_view`] and says how far it has come. It can be copied into closures like a
/// signal.
#[derive(Clone, Copy)]
pub struct PdfViewer {
    node: NodeRef,
    zoom: Signal<PdfZoom>,
    percent: Signal<f32>,
    page: Signal<usize>,
    count: Signal<usize>,
    state: Signal<PdfState>,
    go_to: Signal<Option<usize>>,
    password: Signal<Option<String>>,
}

/// Makes what controls a [`pdf_view`], to hand it with [`PdfView::viewer`].
pub fn pdf_viewer() -> PdfViewer {
    PdfViewer {
        node: node_ref(),
        zoom: signal(PdfZoom::FitWidth),
        percent: signal(1.0),
        page: signal(0),
        count: signal(0),
        state: signal(PdfState::Loading),
        go_to: signal(None),
        password: signal(None),
    }
}

impl PdfViewer {
    pub fn zoom(&self) -> PdfZoom {
        self.zoom.get()
    }

    pub fn set_zoom(&self, zoom: PdfZoom) {
        self.zoom.set(zoom);
    }

    /// Makes it this many times bigger than it is now, keeping what is in the middle there.
    pub fn zoom_by(&self, factor: f32) {
        let now = self.percent.get_untracked();
        self.zoom
            .set(PdfZoom::Percent((now * factor).clamp(MIN_ZOOM, MAX_ZOOM)));
    }

    /// How big it shows now, whatever the zoom was: 1.0 is 100%.
    pub fn percent(&self) -> f32 {
        self.percent.get()
    }

    /// The page in the middle of the box, from 0.
    pub fn page(&self) -> usize {
        self.page.get()
    }

    /// How many pages there are, once it has been opened.
    pub fn page_count(&self) -> usize {
        self.count.get()
    }

    /// Scrolls to the top of a page, from 0, as soon as there is one.
    pub fn go_to_page(&self, page: usize) {
        self.go_to.set(Some(page));
    }

    pub fn state(&self) -> PdfState {
        self.state.get()
    }

    /// Gives it the keyboard: PageUp, PageDown, Home, End and the arrows move, and Ctrl with `+`, `-`
    /// and `0` zooms.
    pub fn focus(&self) {
        self.node.focus();
    }

    /// Opens it again with a password, after [`PdfError::NeedsPassword`].
    pub fn unlock(&self, password: impl Into<String>) {
        self.password.set(Some(password.into()));
    }
}

type ErrorListener = Rc<dyn Fn(&PdfError)>;

/// A document in a box that scrolls. See [`pdf_view`].
pub struct PdfView {
    root: Div,
    source: PdfSource,
    viewer: Option<PdfViewer>,
    on_error: Option<ErrorListener>,
    page_style: Option<Rc<dyn Fn(Style) -> Style>>,
    gap: f32,
    workers: usize,
    cache_bytes: usize,
}

/// Shows a PDF. It takes the box the style gives it, and opens without holding up the UI.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use anti_webui_pdf::{PdfZoom, pdf_view, pdf_viewer};
///
/// # fn app() -> impl IntoView {
/// let viewer = pdf_viewer();
/// viewer.set_zoom(PdfZoom::Percent(1.25));
/// pdf_view("paper.pdf").viewer(viewer).w_full().h_full()
/// # }
/// ```
pub fn pdf_view(source: impl Into<PdfSource>) -> PdfView {
    PdfView {
        root: div(),
        source: source.into(),
        viewer: None,
        on_error: None,
        page_style: None,
        gap: 8.0,
        workers: 2,
        cache_bytes: 256 << 20,
    }
}

impl Styled for PdfView {
    fn style_mut(&mut self) -> &mut Style {
        self.root.style_mut()
    }
}

impl PdfView {
    pub fn viewer(mut self, viewer: PdfViewer) -> Self {
        self.viewer = Some(viewer);
        self
    }

    /// Called when it can't be shown, and with why.
    pub fn on_error(mut self, f: impl Fn(&PdfError) + 'static) -> Self {
        self.on_error = Some(Rc::new(f));
        self
    }

    /// Styles each page, like with a shadow. A page is white unless this says otherwise.
    pub fn page_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.page_style = Some(Rc::new(f));
        self
    }

    /// The room between the pages and around them, in logical pixels. 8 unless this says otherwise.
    pub fn gap(mut self, gap: f32) -> Self {
        self.gap = gap;
        self
    }

    /// How many threads draw at most. Two unless this says otherwise: more draws a little faster
    /// and takes a lot more memory, since each has its own caches.
    pub fn workers(mut self, workers: usize) -> Self {
        self.workers = workers;
        self
    }

    /// How many bytes the pictures of the pages may take, 256 MiB unless this says otherwise. What
    /// shows stays even if that is more.
    pub fn cache_bytes(mut self, bytes: usize) -> Self {
        self.cache_bytes = bytes;
        self
    }
}

impl IntoView for PdfView {
    fn into_view(self) -> View {
        let viewer = self.viewer.unwrap_or_else(pdf_viewer);
        let (source, workers, budget) = (self.source, self.workers, self.cache_bytes);
        let page_style = self.page_style;
        let gap = self.gap;
        let on_error = self.on_error;

        let pages = signal(Rc::new(Vec::<(f32, f32)>::new()));
        let pictures = signal(HashMap::<usize, Arc<Bitmap>>::new());
        let failed = signal(HashSet::<usize>::new());
        let viewport = signal((0.0f32, 0.0f32));
        let offset = signal(PointF::new(0.0, 0.0));
        let direction = Rc::new(Cell::new(Direction::Down));
        let scroller = viewer.node;
        // Where the pointer is in the box, in physical pixels: what zooming with it keeps in place.
        let pointer = Rc::new(Cell::new(None::<(i32, i32)>));

        let px_per_pt = memo(move || {
            let scale = window_scale();
            let css = PX_PER_PT * scale.as_f32() as f64;
            let (width, height) = viewport.get();
            let (width, height) = (scale.px(width) as f64, scale.px(height) as f64);
            let gap = scale.px(gap) as f64;
            let fit_width = move |pages: &[(f32, f32)]| {
                let widest = pages.iter().map(|p| p.0).fold(0.0, f32::max) as f64;
                ((width - 2.0 * gap) / widest.max(1.0)).max(0.0)
            };
            let wanted = pages.with(|pages| match viewer.zoom.get() {
                PdfZoom::Percent(p) => p as f64 * css,
                PdfZoom::FitWidth => fit_width(pages),
                PdfZoom::FitPage => {
                    let tall = pages.first().map_or(1.0, |p| p.1) as f64;
                    fit_width(pages).min(((height - 2.0 * gap) / tall.max(1.0)).max(0.0))
                }
            });
            wanted.clamp(MIN_ZOOM as f64 * css, MAX_ZOOM as f64 * css)
        });
        effect(move || {
            let css = PX_PER_PT * window_scale().as_f32() as f64;
            viewer
                .percent
                .set_if_changed((px_per_pt.get() / css) as f32);
        });

        let table = memo(move || {
            let scale = window_scale();
            let width = scale.px(viewport.get().0);
            Rc::new(
                pages.with(|pages| PageTable::new(pages, px_per_pt.get(), scale.px(gap), width)),
            )
        });

        // Whatever makes the pages move keeps the same place on the same page in the same place of
        // the box: what is under the pointer or in the middle when zooming, and what is at the top
        // when the box changes. It runs before the next layout, which then scrolls with the new
        // sizes.
        {
            let last = RefCell::new(Rc::new(PageTable::default()));
            let last_size = Cell::new((0.0f32, 0.0f32));
            let pointer = pointer.clone();
            effect(move || {
                let new = table.get();
                let old = std::mem::replace(&mut *last.borrow_mut(), new.clone());
                let (width, height) = untrack(|| viewport.get());
                let was = last_size.replace((width, height));
                // Nothing showed before the box had a size, so there is no place to keep.
                if old.is_empty() || *old == *new || was.1 <= 0.0 {
                    return;
                }
                let scale = untrack(window_scale);
                let at = untrack(|| offset.get());
                let (ax, ay) = if was != (width, height) {
                    (0, 0)
                } else {
                    pointer
                        .get()
                        .unwrap_or((scale.px(width) / 2, scale.px(height) / 2))
                };
                let (x, y) = old.move_point(&new, scale.px(at.x) + ax, scale.px(at.y) + ay);
                let (x, y) = ((x - ax).max(0), (y - ay).max(0));
                scroller.scroll_to_after_layout(x, y);
                offset.set(PointF::new(
                    scale.to_logical(x as f32),
                    scale.to_logical(y as f32),
                ));
            });
        }

        // The page that is being read.
        effect(move || {
            let scale = window_scale();
            let top = scale.px(offset.get().y);
            let height = scale.px(viewport.get().1);
            let t = table.get();
            if !t.is_empty() {
                viewer.page.set_if_changed(t.page_at_middle(top, height));
            }
        });

        effect(move || {
            let Some(page) = viewer.go_to.get() else {
                return;
            };
            let t = table.get();
            if t.is_empty() || viewport.get().1 <= 0.0 {
                return;
            }
            let scale = untrack(window_scale);
            let y = t.top(page.min(t.len() - 1));
            let x = scale.px(untrack(|| offset.get()).x);
            scroller.scroll_to_after_layout(x, y);
            offset.set(PointF::new(
                scale.to_logical(x as f32),
                scale.to_logical(y as f32),
            ));
            viewer.go_to.set(None);
        });

        // What comes back from the workers. A picture at another size than is wanted now only helps
        // a page that has nothing to show.
        let out = {
            let on_error = on_error.clone();
            ui_channel(move |event: Event| match event {
                Event::Opened(sizes) => {
                    viewer.count.set(sizes.len());
                    pages.set(Rc::new(sizes));
                    viewer.state.set(PdfState::Ready);
                }
                Event::Failed(e) => {
                    if let Some(f) = &on_error {
                        f(&e);
                    }
                    viewer.state.set(PdfState::Failed(e));
                }
                Event::NotDrawn(job) => {
                    log::warn!("couldn't draw page {} of a PDF", job.page + 1);
                    failed.update(|f| {
                        f.insert(job.page);
                    });
                }
                Event::Drawn(job, bitmap) => {
                    let t = untrack(|| table.get());
                    if job.page >= t.len() {
                        return;
                    }
                    let right = plan::picture_size(t.size(job.page)) == (job.width, job.height);
                    let scale = untrack(window_scale);
                    let top = scale.px(untrack(|| offset.get()).y);
                    let height = scale.px(untrack(|| viewport.get()).1);
                    let keep = t.visible(top, height);
                    let center = t.page_at_middle(top, height);
                    pictures.update(|all| {
                        if !right && all.contains_key(&job.page) {
                            return;
                        }
                        all.insert(job.page, bitmap);
                        let sizes = all
                            .iter()
                            .map(|(page, b)| (*page, b.width() as usize * b.height() as usize * 4));
                        for page in plan::evict(sizes, &keep, center, budget) {
                            all.remove(&page);
                        }
                    });
                }
            })
        };

        let engine = Rc::new(RefCell::new(None::<Engine>));
        {
            let engine = engine.clone();
            effect(move || {
                let password = viewer.password.get();
                let source = source.clone();
                let out = out.clone();
                let opening = Engine::open(
                    move || document::open(&source, password.as_deref()),
                    out,
                    workers,
                );
                // Opening again is only for a password, when there are no pages yet.
                viewer.state.set(PdfState::Loading);
                *engine.borrow_mut() = Some(opening);
            });
        }
        {
            let engine = engine.clone();
            on_cleanup(move || *engine.borrow_mut() = None);
        }

        // Says what to draw whenever that may have changed.
        {
            let direction = direction.clone();
            effect(move || {
                let t = table.get();
                let scale = window_scale();
                let top = scale.px(offset.get().y);
                let height = scale.px(viewport.get().1);
                let jobs = failed.with(|failed| {
                    pictures.with(|have| {
                        plan::plan(&t, top, height, direction.get(), |page| {
                            if failed.contains(&page) {
                                return Some(plan::picture_size(t.size(page)));
                            }
                            have.get(&page).map(|b| (b.width(), b.height()))
                        })
                    })
                });
                if let Some(engine) = &*engine.borrow() {
                    engine.want(jobs);
                }
            });
        }

        let on_scrolled = {
            let direction = direction.clone();
            move |at: PointF| {
                let old = offset.get_untracked();
                if at.y > old.y {
                    direction.set(Direction::Down);
                } else if at.y < old.y {
                    direction.set(Direction::Up);
                }
                offset.set_if_changed(at);
            }
        };

        let on_scroll = move |e: &ScrollEvent| {
            if e.modifiers.ctrl {
                // A notch of a wheel is about a tenth, and a touchpad gets there in small steps.
                viewer.zoom_by((-e.delta_y / 1000.0).exp());
                e.stop_propagation();
            }
        };

        let on_key = move |e: &KeyboardEvent| {
            let scale = untrack(window_scale);
            let at = scroller.scroll_offset();
            let height = viewport.get_untracked().1;
            let total = scale.to_logical(table.with(|t| t.height) as f32);
            if e.modifiers.ctrl {
                match &e.key {
                    Key::Character(c) if c == "=" || c == "+" => viewer.zoom_by(1.25),
                    Key::Character(c) if c == "-" => viewer.zoom_by(0.8),
                    Key::Character(c) if c == "0" => viewer.set_zoom(PdfZoom::FitWidth),
                    _ => return,
                }
                e.stop_propagation();
                return;
            }
            let y = match &e.key {
                Key::Named(NamedKey::PageDown) => at.y + height * 0.9,
                Key::Named(NamedKey::PageUp) => at.y - height * 0.9,
                Key::Named(NamedKey::ArrowDown) => at.y + 40.0,
                Key::Named(NamedKey::ArrowUp) => at.y - 40.0,
                Key::Named(NamedKey::Home) => 0.0,
                Key::Named(NamedKey::End) => total,
                _ => return,
            };
            scroller.scroll_to(at.x, y);
            e.stop_propagation();
        };

        let page_view = move |page: usize| {
            let page_style = page_style.clone();
            div()
                .absolute()
                .dyn_style(move |s| {
                    let t = table.get();
                    if page >= t.len() {
                        return s;
                    }
                    let (w, h) = t.size(page);
                    let s = s
                        .left(Length::Device(t.left(page)))
                        .top(Length::Device(t.top(page)))
                        .w(Length::Device(w))
                        .h(Length::Device(h))
                        .bg(Color::WHITE);
                    match &page_style {
                        Some(f) => f(s),
                        None => s,
                    }
                })
                .child(
                    bitmap_view(move || pictures.with(|all| all.get(&page).cloned()))
                        .w_full()
                        .h_full(),
                )
        };

        self.root
            .overflow_scroll()
            .focusable()
            .node_ref(scroller)
            .on_scrolled(on_scrolled)
            .on_resized(move |w, h| viewport.set_if_changed((w, h)))
            .on_scroll(on_scroll)
            .on_key_down(on_key)
            .on_pointer_move({
                let pointer = pointer.clone();
                move |e| pointer.set(Some((e.local.x as i32, e.local.y as i32)))
            })
            .on_hover(move |inside| {
                if !inside {
                    pointer.set(None);
                }
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .dyn_style(move |s| {
                        let t = table.get();
                        s.w(Length::Device(t.width)).h(Length::Device(t.height))
                    })
                    .child(for_each(
                        move || {
                            let scale = window_scale();
                            let top = scale.px(offset.get().y);
                            let height = scale.px(viewport.get().1);
                            table.with(|t| {
                                let visible = t.visible(top, height);
                                let first = visible.start.saturating_sub(1);
                                let last = (visible.end + 1).min(t.len());
                                (first..last).collect::<Vec<_>>()
                            })
                        },
                        |page| *page,
                        page_view,
                    )),
            )
            .into_view()
    }
}
