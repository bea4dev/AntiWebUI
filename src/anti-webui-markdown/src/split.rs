//! A Markdown editor of two sides: the source on the left in a `code_editor`, and what it shows
//! on the right in a `markdown_view`, the side that isn't scrolled following the one that is. See
//! `docs/design/32-markdown-split.md`.

use crate::editor::list_keys;
use crate::theme::MarkdownTheme;
use crate::view::{Anchors, Emoji, Emojis, OnLink, Pictures, markdown_view};
use anti_webui_code::{CodeTheme, Language, code_editor};
use anti_webui_core::drag::GrabPhase;
use anti_webui_core::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

/// How narrow either side may be made, as a part of the whole.
const NARROWEST: f32 = 0.15;

/// A Markdown editor with the source on the left and what it shows on the right.
pub struct MarkdownSplitEditor {
    source: Signal<String>,
    theme: MarkdownTheme,
    code_theme: CodeTheme,
    on_link: OnLink,
    on_key_down: Option<Rc<dyn Fn(&KeyboardEvent)>>,
    image: Pictures,
    emoji: Emojis,
    editor_ref: Option<EditorRef>,
    line_numbers: bool,
    split: f32,
    style: Style,
}

/// A split editor of the Markdown in `source`, which it changes as it is edited.
pub fn markdown_split_editor(source: Signal<String>) -> MarkdownSplitEditor {
    MarkdownSplitEditor {
        source,
        theme: MarkdownTheme::default(),
        code_theme: CodeTheme::light(),
        on_link: None,
        on_key_down: None,
        image: None,
        emoji: None,
        editor_ref: None,
        line_numbers: true,
        split: 0.5,
        style: Style::default(),
    }
}

impl MarkdownSplitEditor {
    /// How the right side looks.
    pub fn theme(mut self, theme: MarkdownTheme) -> Self {
        self.theme = theme;
        self
    }

    /// The colors of the source, and of the code blocks on the right.
    pub fn code_theme(mut self, theme: CodeTheme) -> Self {
        self.code_theme = theme;
        self
    }

    /// Called with the target of a link clicked on the right.
    pub fn on_link(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_link = Some(Rc::new(f));
        self
    }

    /// Called before the split editor handles a key. Preventing default skips its built-in handling.
    pub fn on_key_down(mut self, f: impl Fn(&KeyboardEvent) + 'static) -> Self {
        self.on_key_down = Some(Rc::new(f));
        self
    }

    /// Where the picture of `![alt](src)` comes from. By default `src` is a path.
    pub fn image(mut self, f: impl Fn(&str) -> ImageSource + 'static) -> Self {
        self.image = Some(Rc::new(f));
        self
    }

    /// What `:name:` is on the right.
    pub fn emoji(mut self, f: impl Fn(&str) -> Option<Emoji> + 'static) -> Self {
        self.emoji = Some(Rc::new(f));
        self
    }

    /// To change the source and the selection from outside, as `code_editor`'s.
    pub fn editor_ref(mut self, r: EditorRef) -> Self {
        self.editor_ref = Some(r);
        self
    }

    pub fn line_numbers(mut self, on: bool) -> Self {
        self.line_numbers = on;
        self
    }

    /// How much of the width the source takes at first, from 0 to 1. Half by default; dragging
    /// the line between the sides changes it.
    pub fn split(mut self, part: f32) -> Self {
        self.split = part.clamp(NARROWEST, 1.0 - NARROWEST);
        self
    }
}

impl Styled for MarkdownSplitEditor {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

/// The side being scrolled, which the other follows.
#[derive(Clone, Copy, PartialEq)]
enum Side {
    Source,
    Preview,
}

/// Where a block is: its lines in the source (the first, and the one after its last) and its
/// top and bottom on the right, in logical pixels from the top of what scrolls there.
#[derive(Clone, Copy)]
struct Place {
    lines: (f64, f64),
    y: (f32, f32),
}

/// The value at `x` of the line through `a` and `b`, which `x` is between.
fn lerp(x: f64, a: (f64, f32), b: (f64, f32)) -> f32 {
    if b.0 <= a.0 {
        return a.1;
    }
    let part = ((x - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    a.1 + (b.1 - a.1) * part as f32
}

/// The other way: the `x` where the line through `a` and `b` is at `y`.
fn unlerp(y: f32, a: (f64, f32), b: (f64, f32)) -> f64 {
    if b.1 <= a.1 {
        return a.0;
    }
    let part = ((y - a.1) / (b.1 - a.1)).clamp(0.0, 1.0);
    a.0 + (b.0 - a.0) * part as f64
}

/// Where the blocks shown are, to go from a line of the source to a height on the right and back.
struct Places {
    anchors: Rc<Anchors>,
    preview: NodeRef,
    /// How tall what scrolls on the right is, in logical pixels.
    height: Cell<f32>,
}

impl Places {
    fn count(&self) -> usize {
        self.anchors.blocks.borrow().len()
    }

    /// Where block `i` is, as of the last layout. For scrolling (`scroll`), the first block starts
    /// where the right side does when it starts on the first line, so that the top of the source
    /// shows the top of the right side, room above the first block and all.
    fn place(&self, i: usize, scroll: bool) -> Option<Place> {
        let a = *self.anchors.blocks.borrow().get(i)?;
        let r = *self.anchors.boxes.borrow().get(&a.key)?;
        let top = r.offset_in(self.preview)?.y;
        let height = untrack(window_scale).to_logical(r.rect()?.height as f32);
        let start = if scroll && i == 0 && a.lines.0 == 0 {
            0.0
        } else {
            top + a.gap
        };
        Some(Place {
            lines: (a.lines.0 as f64, a.lines.1 as f64),
            y: (start, top + height),
        })
    }

    /// Where what follows block `i` starts: the next block, or the end.
    fn next(&self, i: usize, scroll: bool) -> Option<(f64, f32)> {
        if i + 1 < self.count() {
            let p = self.place(i + 1, scroll)?;
            Some((p.lines.0, p.y.0))
        } else {
            Some((self.anchors.lines.get() as f64, self.height.get()))
        }
    }

    /// The last block that starts on `line` or before it.
    fn by_line(&self, line: f64) -> Option<usize> {
        let blocks = self.anchors.blocks.borrow();
        blocks
            .partition_point(|b| b.lines.0 as f64 <= line)
            .checked_sub(1)
    }

    /// The last block whose top is at `y` or above it, looking at as few as it can.
    fn by_height(&self, y: f32, scroll: bool) -> Option<Option<usize>> {
        let (mut lo, mut hi) = (0, self.count());
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.place(mid, scroll)?.y.0 <= y {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        Some(lo.checked_sub(1))
    }

    /// How far down the right side shows a line of the source: in a block, as far as the line is
    /// through the block's lines, and between blocks, as far through the room between them (as
    /// VS Code does).
    fn height_of(&self, line: f64, scroll: bool) -> Option<f32> {
        if self.count() == 0 {
            return None;
        }
        let Some(i) = self.by_line(line) else {
            let first = self.place(0, scroll)?;
            return Some(lerp(line, (0.0, 0.0), (first.lines.0, first.y.0)));
        };
        let p = self.place(i, scroll)?;
        if line < p.lines.1 {
            return Some(lerp(line, (p.lines.0, p.y.0), (p.lines.1, p.y.1)));
        }
        Some(lerp(line, (p.lines.1, p.y.1), self.next(i, scroll)?))
    }

    /// The line of the source a height on the right shows: the other way.
    fn line_at(&self, y: f32, scroll: bool) -> Option<f64> {
        if self.count() == 0 {
            return None;
        }
        let Some(i) = self.by_height(y, scroll)? else {
            let first = self.place(0, scroll)?;
            return Some(unlerp(y, (0.0, 0.0), (first.lines.0, first.y.0)));
        };
        let p = self.place(i, scroll)?;
        if y < p.y.1 {
            return Some(unlerp(y, (p.lines.0, p.y.0), (p.lines.1, p.y.1)));
        }
        Some(unlerp(y, (p.lines.1, p.y.1), self.next(i, scroll)?))
    }

    /// Where on the right a line of the source is shown, top and bottom: its part of the block it
    /// is in. Nothing for a line between blocks.
    fn span_of(&self, line: usize) -> Option<(f32, f32)> {
        let line = line as f64;
        let p = self.place(self.by_line(line)?, false)?;
        if line >= p.lines.1 {
            return None;
        }
        let (a, b) = ((p.lines.0, p.y.0), (p.lines.1, p.y.1));
        Some((lerp(line, a, b), lerp(line + 1.0, a, b)))
    }
}

impl IntoView for MarkdownSplitEditor {
    fn into_view(self) -> View {
        let source = self.source;
        let on_key_down = self.on_key_down;
        let editor = self.editor_ref.unwrap_or_else(editor_ref);
        let preview = node_ref();
        let page = node_ref();
        let anchors = Rc::new(Anchors::default());
        let places = Rc::new(Places {
            anchors: anchors.clone(),
            preview,
            height: Cell::new(0.0),
        });
        let leader = Rc::new(Cell::new(Side::Source));
        let split = signal(self.split);
        let width = Rc::new(Cell::new(0.0f32));
        // Where the caret's line is shown on the right, from the top of the page.
        let mark = signal(None::<(f32, f32)>);

        let follow_source = {
            let (places, leader) = (places.clone(), leader.clone());
            Rc::new(move || {
                if leader.get() != Side::Source {
                    return;
                }
                if let Some(y) = editor.top_line().and_then(|l| places.height_of(l, true)) {
                    preview.scroll_to(0.0, y);
                }
            })
        };
        let follow_preview = {
            let (places, leader) = (places.clone(), leader.clone());
            move || {
                if leader.get() != Side::Preview {
                    return;
                }
                if let Some(line) = places.line_at(preview.scroll_offset().y, true) {
                    editor.scroll_to_line(line);
                }
            }
        };
        let show_caret = {
            let places = places.clone();
            Rc::new(move || {
                let at = editor
                    .with(|e| e.line_of(e.focus))
                    .and_then(|line| places.span_of(line))
                    .zip(page.offset_in(preview))
                    .map(|((a, b), p)| (a - p.y, b - p.y));
                mark.set_if_changed(at);
            })
        };
        let lead = |side: Side| {
            let leader = leader.clone();
            move || leader.set(side)
        };

        let source_side = {
            let (l1, l2, l3) = (lead(Side::Source), lead(Side::Source), lead(Side::Source));
            let follow = follow_source.clone();
            let show = show_caret.clone();
            div()
                .h_full()
                .min_w(0.0)
                .flex_none()
                .flex()
                .flex_col()
                .dyn_style(move |s| s.w(pct(split.get() * 100.0)))
                .on_pointer_down(move |_| l1())
                .on_scroll(move |_| l2())
                .child(
                    code_editor(String::new())
                        .value(source)
                        .language(Language::Markdown)
                        .theme(self.code_theme.clone())
                        .wrap(true)
                        .line_numbers(self.line_numbers)
                        .editor_ref(editor)
                        .on_scrolled(move |_| follow())
                        .on_select(move |_, _| {
                            // Now for a move, and once the right side is laid out again for an
                            // edit.
                            show();
                            let show = show.clone();
                            request_frame(move |_| {
                                if mark.is_alive() {
                                    show();
                                }
                            });
                        })
                        .on_key_down(move |e| {
                            if let Some(f) = &on_key_down {
                                f(e);
                                if e.is_default_prevented() {
                                    return;
                                }
                            }
                            l3();
                            list_keys(editor, e);
                        })
                        .flex_1()
                        .min_h(0.0),
                )
        };

        let line = self.code_theme.bracket_background;
        let divider = {
            let width = width.clone();
            let from = Cell::new(0.0f32);
            div()
                .w(5.0)
                .h_full()
                .flex_none()
                .flex()
                .justify_center()
                .cursor(CursorIcon::EwResize)
                .grab_threshold(0.0)
                .on_grab(move |g| match g.phase {
                    GrabPhase::Start => from.set(split.get_untracked()),
                    GrabPhase::Move | GrabPhase::End if width.get() > 0.0 => split.set(
                        (from.get() + g.total.x / width.get()).clamp(NARROWEST, 1.0 - NARROWEST),
                    ),
                    GrabPhase::Cancel => split.set(from.get()),
                    _ => {}
                })
                .child(div().w(1.0).h_full().bg(line))
        };

        let mut view = markdown_view(move || source.get())
            .theme(self.theme.clone())
            .code_theme(self.code_theme.clone())
            .anchors(anchors);
        if let Some(f) = self.on_link {
            view = view.on_link(move |u| f(u));
        }
        if let Some(f) = self.image {
            view = view.image(move |u| f(u));
        }
        if let Some(f) = self.emoji {
            view = view.emoji(move |u| f(u));
        }
        let accent = self
            .theme
            .link
            .color
            .unwrap_or(Color::from_rgb(0x0969da))
            .with_alpha(0.6);
        // A bar left of the text, along the caret's line.
        let caret_bar = div()
            .absolute()
            .left(-14.0)
            .w(3.0)
            .rounded(1.5)
            .bg(accent)
            .select_none()
            .dyn_style(move |s| match mark.get() {
                Some((a, b)) => s.top(a).h((b - a).max(2.0)),
                None => s.top(0.0).h(0.0),
            });
        let preview_side = {
            let (l1, l2) = (lead(Side::Preview), lead(Side::Preview));
            let follow = follow_source.clone();
            let show = show_caret.clone();
            let p1 = places.clone();
            let p2 = places.clone();
            div()
                .flex_1()
                .min_w(0.0)
                .h_full()
                .overflow_y_scroll()
                .bg(self.theme.root.background.unwrap_or(Color::WHITE))
                .node_ref(preview)
                .on_pointer_down(move |_| l1())
                .on_scroll(move |_| l2())
                .on_scrolled(move |_| follow_preview())
                // A double click takes the source to the line clicked, as in VS Code.
                .on_click(move |p| {
                    if p.click_count != 2 {
                        return;
                    }
                    let y = untrack(window_scale).to_logical(p.local.y) + preview.scroll_offset().y;
                    let Some(line) = p2.line_at(y, false) else {
                        return;
                    };
                    let at = editor.with(|e| {
                        let last = e.line_count().saturating_sub(1);
                        e.line((line as usize).min(last)).start
                    });
                    if let Some(at) = at {
                        // The source shows what the right side shows, so that the line lands
                        // about where it was clicked.
                        if let Some(top) = p2.line_at(preview.scroll_offset().y, true) {
                            editor.scroll_to_line(top);
                        }
                        editor.select(at, at);
                        editor.focus();
                    }
                })
                .child(
                    // What is shown changes size as it is edited, or as blocks far away are laid
                    // out for the first time: the source is followed again.
                    div()
                        .px(32.0)
                        .py(24.0)
                        .on_resized(move |_, h| {
                            p1.height.set(h);
                            follow();
                            show();
                        })
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .relative()
                                .w_full()
                                .max_w(900.0)
                                .node_ref(page)
                                .child(view.w_full())
                                .child(caret_bar),
                        ),
                )
        };

        div()
            .flex()
            .flex_row()
            .overflow_clip()
            .bg(self.code_theme.background)
            .style(&self.style)
            .on_resized(move |w, _| width.set(w))
            .child(source_side)
            .child(divider)
            .child(preview_side)
            .into_view()
    }
}
