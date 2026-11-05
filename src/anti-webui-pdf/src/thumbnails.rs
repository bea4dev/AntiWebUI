//! A list of small pictures of the pages to go to them with, next to a [`crate::pdf_view`]. Only what
//! shows gets a node and a picture. See `docs/design/26-pdf.md`.

use crate::PdfViewer;
use crate::plan::{Job, PageTable};
use anti_webui_core::geometry::PointF;
use anti_webui_core::images::Bitmap;
use anti_webui_core::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

/// How wide they are unless it says otherwise, in logical pixels, as in Chromium's viewer.
const WIDTH: f32 = 108.0;
/// A page that is much taller than wide doesn't take more than this many widths.
const MOST_TALL: f32 = 2.0;
/// The room for the number of the page, and between the pages, in logical pixels.
const LABEL: f32 = 22.0;
const GAP: f32 = 10.0;
/// How much the pictures take at most. What shows stays even if that is more. With 200 by 280
/// pixels each, this is about 150 of them.
const BUDGET: usize = 32 << 20;
/// What shows how far you are, unless it says otherwise.
const CURRENT: u32 = 0x7cacf8ff;

/// See [`pdf_thumbnails`].
pub struct PdfThumbnails {
    root: Div,
    viewer: PdfViewer,
    width: f32,
    item_style: Option<Rc<dyn Fn(Style, bool) -> Style>>,
}

/// Shows a small picture of each page of what `viewer` shows, one below the other, and goes to a
/// page when clicked. It scrolls by itself to keep up with the page being read. It takes the box the
/// style gives it, and only draws anything while it is there.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
/// use anti_webui_pdf::{pdf_thumbnails, pdf_view, pdf_viewer};
///
/// # fn app() -> impl IntoView {
/// let viewer = pdf_viewer();
/// div()
///     .flex()
///     .w_full()
///     .h_full()
///     .child(pdf_thumbnails(viewer).w(140.0).h_full())
///     .child(pdf_view("paper.pdf").viewer(viewer).flex_1())
/// # }
/// ```
pub fn pdf_thumbnails(viewer: PdfViewer) -> PdfThumbnails {
    PdfThumbnails {
        root: div(),
        viewer,
        width: WIDTH,
        item_style: None,
    }
}

impl Styled for PdfThumbnails {
    fn style_mut(&mut self) -> &mut Style {
        self.root.style_mut()
    }
}

impl PdfThumbnails {
    /// How wide the pictures are, in logical pixels. 108 unless this says otherwise.
    pub fn thumbnail_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    /// Styles the picture of each page, with whether it is the one being read. Without this, that
    /// one has an outline.
    pub fn item_style(mut self, f: impl Fn(Style, bool) -> Style + 'static) -> Self {
        self.item_style = Some(Rc::new(f));
        self
    }
}

/// How big the picture of a page that takes `width` is, which is less wide if it is very tall.
fn fitted((w, h): (f32, f32), width: f32) -> (f32, f32) {
    let (w, h) = (w.max(1.0), h.max(1.0));
    let shrink = (width / w).min(MOST_TALL * width / h);
    (w * shrink, h * shrink)
}

/// How many bytes the pictures take now, and which of them go to stay within `budget`: those that
/// don't show or come next, the furthest from the page being read first.
fn over_budget<'a>(
    pictures: impl Iterator<Item = (usize, &'a Job)>,
    keep: &std::ops::Range<usize>,
    center: usize,
    budget: usize,
) -> Vec<usize> {
    let mut total = 0;
    let mut droppable = Vec::new();
    for (page, job) in pictures {
        total += job.bytes();
        if !keep.contains(&page) {
            droppable.push((page, job.bytes()));
        }
    }
    droppable.sort_by_key(|(page, _)| std::cmp::Reverse(page.abs_diff(center)));
    let mut dropped = Vec::new();
    for (page, bytes) in droppable {
        if total <= budget {
            break;
        }
        total -= bytes;
        dropped.push(page);
    }
    dropped
}

impl IntoView for PdfThumbnails {
    fn into_view(self) -> View {
        let viewer = self.viewer;
        let (width, item_style) = (self.width, self.item_style);
        let viewport = signal((0.0f32, 0.0f32));
        let offset = signal(PointF::new(0.0, 0.0));
        let scroller = node_ref();

        // A row is a picture with its number below it.
        let table = memo(move || {
            let scale = window_scale();
            let (wide, label) = (scale.px(width) as f32, scale.px(LABEL) as f32);
            let turn = viewer.turn.get();
            let rows: Vec<(f32, f32)> = viewer.pages.with(|pages| {
                pages
                    .iter()
                    .map(|&page| {
                        let (w, h) = fitted(turn.size(page), wide);
                        (w, h.round() + label)
                    })
                    .collect()
            });
            let mut table = PageTable::new(&rows, 1.0, scale.px(GAP), scale.px(viewport.get().0));
            table.turn = turn;
            Rc::new(table)
        });
        let picture_of = move |t: &PageTable, page: usize| {
            let (w, h) = t.size(page);
            let label = untrack(window_scale).px(LABEL);
            Job {
                page,
                width: w as u32,
                height: (h - label).max(1) as u32,
                area: None,
                turn: t.turn,
            }
        };

        viewer.thumbnail_lists.update(|n| *n += 1);
        on_cleanup(move || {
            // The viewer can go before this does, when all of it goes.
            if viewer.thumbnail_lists.is_alive() {
                viewer.thumbnail_lists.update(|n| *n -= 1);
                if viewer.thumbnail_lists.get_untracked() == 0 {
                    viewer.thumbnails.set(Default::default());
                    viewer.thumbnails_wanted.set(Vec::new());
                }
            }
        });

        // What shows comes first, and then a screen below and above that.
        effect(move || {
            let t = table.get();
            if t.is_empty() {
                return;
            }
            let scale = window_scale();
            let top = scale.px(offset.get().y);
            let height = scale.px(viewport.get().1);
            if height <= 0 {
                return;
            }
            let visible = t.visible(top, height);
            let keep = t.visible((top - height).max(0), height * 3);
            let order = visible
                .clone()
                .chain(visible.end..keep.end)
                .chain((keep.start..visible.start).rev());
            let wanted: Vec<Job> = viewer.thumbnails.with(|have| {
                order
                    .map(|page| picture_of(&t, page))
                    .filter(|job| have.get(&job.page).is_none_or(|(there, _)| there != job))
                    .collect()
            });
            viewer.thumbnails_wanted.set_if_changed(wanted);
            let center = viewer.page.get();
            let dropped = viewer.thumbnails.with(|have| {
                over_budget(
                    have.iter().map(|(&page, (job, _))| (page, job)),
                    &keep,
                    center,
                    BUDGET,
                )
            });
            if !dropped.is_empty() {
                viewer.thumbnails.update(|have| {
                    for page in dropped {
                        have.remove(&page);
                    }
                });
            }
        });

        // Keeps up with the page being read. Scrolling this by hand doesn't change that page, so
        // this doesn't undo it.
        effect(move || {
            let page = viewer.page.get();
            let t = table.get();
            let height = viewport.get().1;
            if page >= t.len() || height <= 0.0 {
                return;
            }
            let scale = untrack(window_scale);
            let height = scale.px(height);
            let top = scale.px(untrack(|| offset.get()).y);
            let (row, tall) = (t.top(page), t.size(page).1);
            if row >= top && row + tall <= top + height {
                return;
            }
            let y = (row + tall / 2 - height / 2).max(0);
            scroller.scroll_to_after_layout(0, y);
            offset.set(PointF::new(0.0, scale.to_logical(y as f32)));
        });

        let item_style = item_style.unwrap_or_else(|| {
            Rc::new(|s: Style, current: bool| {
                if current {
                    s.outline(2.0).outline_color(Color::from_rgba(CURRENT))
                } else {
                    s
                }
            })
        });

        let row = move |page: usize| {
            let item_style = item_style.clone();
            let thumbnail = move || -> Option<Arc<Bitmap>> {
                viewer
                    .thumbnails
                    .with(|all| all.get(&page).and_then(|t| t.1.clone()))
            };
            div()
                .absolute()
                .flex()
                .flex_col()
                .items_center()
                .cursor(CursorIcon::Pointer)
                .on_click(move |_| viewer.go_to_page(page))
                .dyn_style(move |s| {
                    let t = table.get();
                    if page >= t.len() {
                        return s;
                    }
                    let (w, h) = t.size(page);
                    s.left(Length::Device(t.left(page)))
                        .top(Length::Device(t.top(page)))
                        .w(Length::Device(w))
                        .h(Length::Device(h))
                })
                .child(
                    div()
                        .flex_none()
                        .bg(Color::WHITE)
                        .dyn_style(move |s| {
                            let t = table.get();
                            if page >= t.len() {
                                return s;
                            }
                            let job = picture_of(&t, page);
                            let s = s
                                .w(Length::Device(job.width as i32))
                                .h(Length::Device(job.height as i32));
                            item_style(s, viewer.page.get() == page)
                        })
                        .child(bitmap_view(thumbnail).w_full().h_full()),
                )
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .items_center()
                        .child(text(format!("{}", page + 1))),
                )
        };

        self.root
            .overflow_scroll()
            .select_none()
            .node_ref(scroller)
            .on_scrolled(move |at| offset.set_if_changed(at))
            .on_resized(move |w, h| viewport.set_if_changed((w, h)))
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
                            table.with(|t| t.visible(top, height).collect::<Vec<_>>())
                        },
                        |page| *page,
                        row,
                    )),
            )
            .into_view()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Turn;

    #[test]
    fn a_very_tall_page_takes_less_than_its_width() {
        assert_eq!(fitted((100.0, 150.0), 200.0), (200.0, 300.0));
        assert_eq!(fitted((100.0, 400.0), 200.0), (100.0, 400.0));
        assert_eq!(fitted((400.0, 100.0), 200.0), (200.0, 50.0));
    }

    #[test]
    fn what_goes_first_is_furthest_from_the_page_being_read() {
        let job = |page| Job {
            page,
            width: 10,
            height: 10,
            area: None,
            turn: Turn::default(),
        };
        let jobs: Vec<Job> = [0, 1, 2, 3, 9].map(job).to_vec();
        let drop = |keep: std::ops::Range<usize>, budget| {
            over_budget(jobs.iter().map(|j| (j.page, j)), &keep, 2, budget)
        };
        assert_eq!(drop(2..3, 2000), Vec::<usize>::new());
        assert_eq!(drop(2..3, 1200), [9, 0]);
        // What shows stays, even over the budget.
        assert_eq!(drop(0..10, 0), Vec::<usize>::new());
    }
}
