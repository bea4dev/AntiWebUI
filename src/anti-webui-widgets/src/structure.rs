//! What organizes the rest: tabs, parts that open, toolbars.

use crate::theme::{QUICK, Theme, focus_ring, theme, themed};
use anti_webui_core::prelude::*;
use std::rc::Rc;

/// A tab: what it is called and what it shows, which is built each time it is chosen.
pub struct Tab {
    label: String,
    content: Rc<dyn Fn() -> View>,
}

/// A tab of [`tabs`].
pub fn tab<V: IntoView>(label: impl Into<String>, content: impl Fn() -> V + 'static) -> Tab {
    Tab {
        label: label.into(),
        content: Rc::new(move || content().into_view()),
    }
}

fn tab_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        crate::theme::smooth(s)
            .text_color(t.text_muted)
            .border_color(Color::TRANSPARENT)
            .cursor(CursorIcon::Pointer)
    })
    .hover(|s| s.text_color(t.text).border_color(t.border_strong))
    .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

fn current_tab_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        crate::theme::smooth(s)
            .text_color(t.text)
            .border_color(t.accent)
            .cursor(CursorIcon::Pointer)
    })
    .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
    .accent_border()
}

fn ringed_tab_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| {
        focus_ring(crate::theme::smooth(s), t)
            .rounded(4.0)
            .text_color(t.text)
            .border_color(t.accent)
            .cursor(CursorIcon::Pointer)
    })
    .accent_border()
}

/// What the look of a row of tabs gets to see. Reading it tracks it.
#[derive(Clone, Copy)]
pub struct TabsState {
    selected: Binding<usize>,
    count: usize,
    keyed: Signal<bool>,
}

impl TabsState {
    /// The index of the chosen tab.
    pub fn selected(&self) -> usize {
        self.selected.get()
    }

    pub fn is_selected(&self, i: usize) -> bool {
        self.selected.get() == i
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// The row has the focus from the keyboard. The chosen tab is the one to show it on.
    pub fn focus_visible(&self) -> bool {
        self.keyed.get()
    }

    /// Makes `el` tab `i`: clicking it chooses it.
    pub fn tab(&self, i: usize, el: Div) -> Div {
        let selected = self.selected;
        el.access(move |a| a.role(Role::Tab).selected(selected.get() == i))
            .on_click(move |_| selected.set(i))
    }
}

type TabsPart = Rc<dyn Fn(Div, TabsState) -> Div>;
type TabPart = Rc<dyn Fn(Div, TabsState, usize) -> Div>;

/// Changes to the parts of the standard tabs. Each gets the part with its standard styles on and
/// returns it; what it says goes above the theme in every state.
#[derive(Clone, Default)]
pub struct TabsParts {
    row: Option<TabsPart>,
    tab: Option<TabPart>,
    panel: Option<TabsPart>,
}

impl TabsParts {
    pub fn new() -> Self {
        Self::default()
    }

    /// The row of tabs, which is the stop for Tab.
    pub fn row(mut self, f: impl Fn(Div, TabsState) -> Div + 'static) -> Self {
        self.row = Some(Rc::new(f));
        self
    }

    /// Each tab, with its index.
    pub fn tab(mut self, f: impl Fn(Div, TabsState, usize) -> Div + 'static) -> Self {
        self.tab = Some(Rc::new(f));
        self
    }

    /// What holds the chosen tab's content.
    pub fn panel(mut self, f: impl Fn(Div, TabsState) -> Div + 'static) -> Self {
        self.panel = Some(Rc::new(f));
        self
    }
}

/// Tabs, bound to the index of the one that is chosen. They are one stop for Tab, and the arrow
/// keys go through them.
pub fn tabs(selected: impl Into<Binding<usize>>, items: Vec<Tab>) -> Div {
    styled_tabs(selected, items, TabsParts::new())
}

/// Tabs with the standard look, with their parts changed by `parts`.
pub fn styled_tabs(selected: impl Into<Binding<usize>>, items: Vec<Tab>, parts: TabsParts) -> Div {
    let selected = selected.into();
    let count = items.len();
    let contents: Vec<_> = items.iter().map(|t| t.content.clone()).collect();
    let panel_part = parts.panel.clone();
    let mut state = None;
    let row = tabs_with(selected, count, |s| {
        state = Some(s);
        let heads = items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let head = div()
                    .flex_none()
                    .px(12.0)
                    .py(6.0)
                    .border_b(2.0)
                    .select_none()
                    .sheet(move || {
                        themed(if !s.is_selected(i) {
                            tab_sheet
                        } else if s.focus_visible() {
                            ringed_tab_sheet
                        } else {
                            current_tab_sheet
                        })
                    })
                    .child(text(item.label));
                let head = s.tab(i, head);
                match &parts.tab {
                    Some(f) => f(head, s, i),
                    None => head,
                }
            })
            .collect::<Vec<_>>();
        let row = div()
            .flex()
            .flex_row()
            .flex_none()
            .border_b(1.0)
            .dyn_style(|s| s.border_color(theme().border))
            .children(heads);
        match &parts.row {
            Some(f) => f(row, s),
            None => row,
        }
    });
    let s = state.expect("the look is built at once");
    let panel = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(0.0)
        .access(|a| a.role(Role::TabPanel))
        .child(for_each(
            move || vec![selected.get()],
            |i| *i,
            move |i| match contents.get(i) {
                Some(content) => content(),
                None => fragment(Vec::<View>::new()),
            },
        ));
    let panel = match &panel_part {
        Some(f) => f(panel, s),
        None => panel,
    };
    div().flex().flex_col().child(row).child(panel)
}

/// A row of tabs that looks like whatever `look` builds; `look` makes each tab with
/// [`TabsState::tab`]. The row is one stop for Tab: the arrow keys go through the tabs, around
/// at the ends, and Home and End go to the first and the last. Only the row is made: what each tab
/// shows is up to the program.
pub fn tabs_with(
    selected: impl Into<Binding<usize>>,
    count: usize,
    look: impl FnOnce(TabsState) -> Div,
) -> Div {
    let state = TabsState {
        selected: selected.into(),
        count,
        keyed: signal(false),
    };
    look(state)
        .focusable()
        .access(|a| a.role(Role::TabList))
        .on_focus_visible(move |v| state.keyed.set(v))
        .on_key_down(move |e| {
            let at = state.selected.get_untracked();
            let next = match e.key {
                Key::Named(NamedKey::ArrowRight) => (at + 1) % count.max(1),
                Key::Named(NamedKey::ArrowLeft) => (at + count - 1) % count.max(1),
                Key::Named(NamedKey::Home) => 0,
                Key::Named(NamedKey::End) => count.saturating_sub(1),
                _ => return,
            };
            e.prevent_default();
            state.selected.set(next);
        })
}

fn head_sheet(t: &Theme) -> Sheet {
    Sheet::new(|s| s.cursor(CursorIcon::Pointer).text_color(t.text))
        .hover(|s| s.bg(t.hover))
        .active(|s| s.bg(t.pressed))
        .focus_visible(|s| focus_ring(s, t).rounded(4.0))
        .disabled(|s| s.text_color(t.text_disabled).cursor(CursorIcon::Default))
}

/// A title that opens and closes what is under it, bound to `open`. What is under it only exists
/// while it shows.
pub fn disclosure<V: IntoView>(
    open: Signal<bool>,
    title: impl IntoTextSource,
    content: impl Fn() -> V + 'static,
) -> Div {
    disclosure_of(
        open,
        title,
        move || content().into_view(),
        move || open.update(|v| *v = !*v),
    )
}

fn disclosure_of(
    open: Signal<bool>,
    title: impl IntoTextSource,
    content: impl Fn() -> View + 'static,
    press: impl Fn() + 'static,
) -> Div {
    // How high what is under it is, once it was there. Before that, an open one takes what it
    // needs and a closed one has no height to start from.
    let measured = signal(None::<f32>);
    let built = signal(untrack(|| open.get()));
    let was_open = untrack(|| open.get());
    effect(move || {
        if open.get() {
            built.set_if_changed(true);
        } else {
            let timer = set_timeout(QUICK.duration, move || {
                if !open.get_untracked() {
                    built.set_if_changed(false);
                }
            });
            on_cleanup(move || timer.cancel());
        }
    });
    div()
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(6.0)
                .px(6.0)
                .py(5.0)
                .rounded(4.0)
                .select_none()
                .font_weight(FontWeight::MEDIUM)
                .focusable()
                .sheet(|| themed(head_sheet))
                .on_click(move |_| press())
                .child(
                    text("▸")
                        .text_size(14.0)
                        .transition_of([Prop::Rotate], QUICK)
                        .dyn_style(move |s| s.rotate(if open.get() { 90.0 } else { 0.0 })),
                )
                .child(text(title)),
        )
        .child(
            div()
                .overflow_clip()
                .transition_of([Prop::Height], QUICK)
                .dyn_style(move |s| match (open.get(), measured.get()) {
                    (true, Some(h)) => s.h(h),
                    (true, None) if was_open => s,
                    _ => s.h(0.0),
                })
                .child(show(move || built.get(), {
                    let content = Rc::new(content);
                    move || {
                        let content = content.clone();
                        div()
                            .flex()
                            .flex_col()
                            .flex_none()
                            .pl(24.0)
                            .pr(6.0)
                            .py(6.0)
                            .on_resized(move |_, h| measured.set_if_changed(Some(h)))
                            .child(content())
                    }
                })),
        )
}

/// Parts of which one is open at a time. Each has a title and what is under it.
pub fn accordion(items: Vec<(String, Rc<dyn Fn() -> View>)>) -> Div {
    let current = signal(None::<usize>);
    div().flex().flex_col().gap(2.0).children(
        items
            .into_iter()
            .enumerate()
            .map(|(i, (title, content))| {
                let open = signal(false);
                effect(move || open.set_if_changed(current.get() == Some(i)));
                disclosure_of(
                    open,
                    title,
                    move || content(),
                    move || current.update(|c| *c = if *c == Some(i) { None } else { Some(i) }),
                )
            })
            .collect::<Vec<_>>(),
    )
}

/// A row of controls for an area. [`quiet_button`](crate::quiet_button) and
/// [`toggle_button`](crate::toggle_button) go well in it.
pub fn toolbar() -> Div {
    div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .gap(2.0)
        .px(4.0)
        .py(4.0)
        .border_b(1.0)
        .dyn_style(|s| s.border_color(theme().border))
}

/// A line between what belongs together in a toolbar.
pub fn toolbar_separator() -> Div {
    div()
        .flex_none()
        .w(1.0)
        .h(18.0)
        .mx(4.0)
        .dyn_style(|s| s.bg(theme().border))
}

/// Pushes what comes after it to the other end of a toolbar.
pub fn toolbar_spacer() -> Div {
    div().flex_1()
}
