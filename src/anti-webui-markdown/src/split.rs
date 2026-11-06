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

/// Where the blocks shown are, to go from a line of the source to a height on the right and back.
struct Places {
    anchors: Rc<Anchors>,
    preview: NodeRef,
    /// How tall what scrolls on the right is, in logical pixels.
    height: Cell<f32>,
}

impl Places {
    /// The line each block starts on and how far down its top is on the right, for block `i`, and
    /// for the end after the last one. The first block starts where both sides start, so that
    /// the top of the source shows the top of the right side, room above the first block and all.
    fn at(&self, i: usize) -> Option<(f64, f32)> {
        if i == 0 {
            return Some((0.0, 0.0));
        }
        let blocks = self.anchors.blocks.borrow();
        let Some(&(line, key, gap)) = blocks.get(i) else {
            return Some((self.anchors.lines.get() as f64, self.height.get()));
        };
        let r = *self.anchors.boxes.borrow().get(&key)?;
        Some((line as f64, r.offset_in(self.preview)?.y + gap))
    }

    fn count(&self) -> usize {
        self.anchors.blocks.borrow().len()
    }

    /// How far down the right side shows a line of the source: between the tops of the blocks
    /// around it, as far as the line is between their first lines (as VS Code does).
    fn height_of(&self, line: f64) -> Option<f32> {
        let n = self.count();
        if n == 0 {
            return None;
        }
        let i = {
            let blocks = self.anchors.blocks.borrow();
            blocks.partition_point(|b| b.0 as f64 <= line)
        };
        let (l0, y0) = match i.checked_sub(1) {
            Some(i) => self.at(i)?,
            None => (0.0, 0.0),
        };
        let (l1, y1) = self.at(i)?;
        let part = if l1 > l0 {
            ((line - l0) / (l1 - l0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(y0 + (y1 - y0) * part as f32)
    }

    /// The line of the source a height on the right shows: the other way.
    fn line_at(&self, y: f32) -> Option<f64> {
        let n = self.count();
        if n == 0 {
            return None;
        }
        // The first block whose top is below `y`, looking at as few as it can.
        let (mut lo, mut hi) = (0, n);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.at(mid)?.1 <= y {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let (l0, y0) = match lo.checked_sub(1) {
            Some(i) => self.at(i)?,
            None => (0.0, 0.0),
        };
        let (l1, y1) = self.at(lo)?;
        let part = if y1 > y0 {
            ((y - y0) / (y1 - y0)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Some(l0 + (l1 - l0) * part as f64)
    }
}

impl IntoView for MarkdownSplitEditor {
    fn into_view(self) -> View {
        let source = self.source;
        let editor = self.editor_ref.unwrap_or_else(editor_ref);
        let preview = node_ref();
        let anchors = Rc::new(Anchors::default());
        let places = Rc::new(Places {
            anchors: anchors.clone(),
            preview,
            height: Cell::new(0.0),
        });
        let leader = Rc::new(Cell::new(Side::Source));
        let split = signal(self.split);
        let width = Rc::new(Cell::new(0.0f32));

        let follow_source = {
            let (places, leader) = (places.clone(), leader.clone());
            Rc::new(move || {
                if leader.get() != Side::Source {
                    return;
                }
                if let Some(y) = editor.top_line().and_then(|l| places.height_of(l)) {
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
                if let Some(line) = places.line_at(preview.scroll_offset().y) {
                    editor.scroll_to_line(line);
                }
            }
        };
        let lead = |side: Side| {
            let leader = leader.clone();
            move || leader.set(side)
        };

        let source_side = {
            let (l1, l2, l3) = (lead(Side::Source), lead(Side::Source), lead(Side::Source));
            let follow = follow_source.clone();
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
                        .on_key_down(move |e| {
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
        let preview_side = {
            let (l1, l2) = (lead(Side::Preview), lead(Side::Preview));
            let follow = follow_source.clone();
            let places = places.clone();
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
                .child(
                    // What is shown changes size as it is edited, or as blocks far away are laid
                    // out for the first time: the source is followed again.
                    div()
                        .px(32.0)
                        .py(24.0)
                        .on_resized(move |_, h| {
                            places.height.set(h);
                            follow();
                        })
                        .flex()
                        .justify_center()
                        .child(view.w_full().max_w(900.0)),
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
