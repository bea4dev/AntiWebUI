//! What the editor does with the scripts of a document: telling them what happens to the fields,
//! and doing what they say. See "JavaScript" in `docs/design/26-pdf.md`.

use super::{ActiveKind, Editor, PdfFieldChange};
use crate::edits::{Display, Edits};
use crate::marks::{Marks, PdfFieldValue};
use crate::script::{
    Ask, Effect, Message, Mouse, PdfAnswer, PdfButtons, PdfDialog, PdfDialogKind, Reply,
    ScriptHost, Start, byte_offset, keystroke_of,
};
use anti_webui_core::channel::ui_channel;
use anti_webui_core::event::{Key, NamedKey, PointerEvent};
use anti_webui_core::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

/// How long the UI thread waits for the scripts before going on without them. What they say later
/// still comes.
const WAIT: Duration = Duration::from_millis(30);

/// What came back for the thing that was waited for.
#[derive(Default)]
struct Applied {
    typed: Option<(bool, String, (usize, usize))>,
}

impl Editor {
    /// Gets ready for scripts: what has to be made with the view, before it is known whether there
    /// are any. `file_name` is what they get as the name of the file.
    pub fn watch_scripts(self: &Rc<Self>, file_name: String) {
        *self.file_name.borrow_mut() = file_name;
        let weak = Rc::downgrade(self);
        let out = ui_channel(move |message: Message| {
            if let Some(this) = weak.upgrade() {
                this.received(message);
            }
        });
        *self.script_out.borrow_mut() = Some(out);

        let viewer = self.viewer;
        let weak = Rc::downgrade(self);
        effect(move || {
            let edits = viewer.edits.get();
            if let Some(this) = weak.upgrade() {
                untrack(|| this.tell(&edits));
            }
        });
        let weak = Rc::downgrade(self);
        let last = Cell::new(None::<usize>);
        effect(move || {
            let page = viewer.page.get();
            let Some(this) = weak.upgrade() else {
                return;
            };
            let host = this.scripts.borrow();
            let Some(host) = host.as_ref() else {
                return;
            };
            // The page it started on has been told.
            let was = last.replace(Some(page)).unwrap_or(this.start_page.get());
            if was != page {
                host.send(Ask::Page {
                    page: was,
                    open: false,
                });
                host.send(Ask::Page { page, open: true });
            }
        });
        let weak = Rc::downgrade(self);
        effect(move || {
            let Some(answer) = viewer.dialog_answer.get() else {
                return;
            };
            untrack(|| {
                viewer.dialog_answer.set(None);
                viewer.dialog.set(None);
                if let Some(this) = weak.upgrade()
                    && let Some(host) = &*this.scripts.borrow()
                {
                    host.answer(answer);
                }
            });
        });
    }

    /// Starts the scripts of a document that has some, once its marks have been read.
    pub fn start_scripts(&self) {
        let marks = self.marks();
        if !marks.has_scripts() {
            return;
        }
        self.start_host();
    }

    fn start_host(&self) -> bool {
        if self.scripts.borrow().is_some() {
            return true;
        }
        let Some(out) = self.script_out.borrow().clone() else {
            return false;
        };
        let edits = self.viewer.edits.get_untracked();
        let page = self.viewer.page.get_untracked();
        let start = Start {
            marks: self.marks(),
            edits: edits.clone(),
            file_name: self.file_name.borrow().clone(),
            page,
        };
        let Some(host) = ScriptHost::start(start, out) else {
            return false;
        };
        *self.told.borrow_mut() = edits;
        self.start_page.set(page);
        *self.scripts.borrow_mut() = Some(host);
        true
    }

    pub(super) fn has_scripts(&self) -> bool {
        self.scripts.borrow().is_some()
    }

    fn received(&self, message: Message) {
        match message {
            Message::Replied => {
                let replies = match &*self.scripts.borrow() {
                    Some(host) => host.take(),
                    None => return,
                };
                self.apply(replies, None, 0);
            }
            Message::Dialog(dialog) => self.viewer.dialog.set(Some(dialog)),
        }
    }

    /// Sends something and waits a moment for what comes back, which is one thing to undo if
    /// `undo`. Nothing comes back if the scripts took longer.
    fn ask(&self, ask: Ask, undo: bool) -> Option<Applied> {
        let (ticket, replies) = {
            let host = self.scripts.borrow();
            let host = host.as_ref()?;
            let ticket = host.send(ask);
            (ticket, host.wait(ticket, WAIT))
        };
        match replies {
            Some(replies) => Some(self.apply(replies, undo.then_some(ticket), ticket)),
            None => {
                log::debug!("the scripts of a PDF are taking longer than {WAIT:?}");
                None
            }
        }
    }

    /// Commits a value through the scripts, if there are any: they check it and work out the rest.
    /// It says whether the value changed.
    pub(super) fn commit_with_scripts(&self, field: usize, value: PdfFieldValue) -> Option<bool> {
        if !self.has_scripts() {
            return None;
        }
        let before = self.viewer.field_value_at(field);
        let asked = self.ask(
            Ask::Commit {
                field,
                value: value.clone(),
            },
            true,
        );
        if asked.is_none() {
            // It goes in as it is, and what the scripts say about it comes on top. They are told
            // already.
            let marks = self.marks();
            let mut next = (*self.viewer.edits.get_untracked()).clone();
            set_value(&mut next, &marks, field, value, None);
            *self.told.borrow_mut() = Arc::new(next.clone());
            self.viewer.change_edits(|edits| *edits = next);
        }
        let after = self.viewer.field_value_at(field);
        if after == before {
            return Some(false);
        }
        if let Some(f) = &self.on_change {
            f(&PdfFieldChange {
                name: self.marks().fields[field].info.name.clone(),
                value: after,
            });
        }
        Some(true)
    }

    /// Lets the scripts of a field check what was just typed in it, and puts it back or changes it
    /// if they say so.
    pub(super) fn keystroke(
        &self,
        field: usize,
        typed: &RefCell<String>,
        value: Signal<String>,
        node: NodeRef,
    ) {
        let new = value.get_untracked();
        let checks = self.has_scripts()
            && self
                .marks()
                .fields
                .get(field)
                .is_some_and(|f| f.scripts.keystroke.is_some());
        let old = typed.replace(new.clone());
        if !checks || old == new {
            return;
        }
        let (change, selection) = keystroke_of(&old, &new);
        let asked = self.ask(
            Ask::Keystroke {
                field,
                value: old.clone(),
                change,
                selection,
            },
            false,
        );
        let Some((accepted, text, selection)) = asked.and_then(|a| a.typed) else {
            return;
        };
        let result = if accepted { text } else { old };
        if result != new {
            let selection = (
                byte_offset(&result, selection.0),
                byte_offset(&result, selection.1),
            );
            node.set_text(&result, selection);
            value.set(result.clone());
        }
        *typed.borrow_mut() = result;
    }

    /// Tells the scripts of a widget what happened to it, if one wants to know. A press and a click
    /// wait for them, and what they do is one thing to undo.
    pub(super) fn mouse(&self, page: usize, widget: usize, what: Mouse, e: Option<&PointerEvent>) {
        if !self.has_scripts() {
            return;
        }
        let marks = self.marks();
        let Some(w) = marks.pages.get(page).and_then(|p| p.widgets.get(widget)) else {
            return;
        };
        let s = &w.scripts;
        let script = match what {
            Mouse::Down => &s.down,
            Mouse::Up => &s.up,
            Mouse::Enter => &s.enter,
            Mouse::Exit => &s.exit,
            Mouse::Focus => &s.focus,
            Mouse::Blur => &s.blur,
        };
        if script.is_none() {
            return;
        }
        let Some(index) = marks.fields[w.field]
            .widgets
            .iter()
            .position(|&at| at == (page, widget))
        else {
            return;
        };
        let ask = Ask::Mouse {
            field: w.field,
            widget: index,
            what,
            shift: e.is_some_and(|e| e.modifiers.shift),
            modifier: e.is_some_and(|e| e.modifiers.primary()),
        };
        if matches!(what, Mouse::Down | Mouse::Up) {
            self.ask(ask, true);
        } else if let Some(host) = &*self.scripts.borrow() {
            host.send(ask);
        }
    }

    /// Tells the scripts which widget the pointer went into and out of.
    pub fn hover(&self, e: &PointerEvent) {
        if !self.has_scripts() {
            return;
        }
        let now = self
            .reader
            .on_page(e.local)
            .and_then(|(page, x, y)| self.widget_at(page, x, y).map(|w| (page, w)));
        let was = self.hovered.replace(now);
        if was == now {
            return;
        }
        if let Some((page, widget)) = was {
            self.mouse(page, widget, Mouse::Exit, Some(e));
        }
        if let Some((page, widget)) = now {
            self.mouse(page, widget, Mouse::Enter, Some(e));
        }
    }

    /// Runs what a link of the document says.
    pub fn run_link(&self, script: String) {
        if self.marks().editing == crate::marks::PdfEditing::NotYet || !self.start_host() {
            return;
        }
        self.ask(Ask::Link(script), true);
    }

    /// Tells the scripts what changed in the fields other than by them.
    fn tell(&self, edits: &Arc<Edits>) {
        let host = self.scripts.borrow();
        let Some(host) = host.as_ref() else {
            return;
        };
        let told = self.told.replace(edits.clone());
        if Arc::ptr_eq(&told, edits) || *told == **edits {
            return;
        }
        let marks = self.marks();
        let value =
            |e: &Edits, i: usize| e.fields.get(&i).unwrap_or(&marks.fields[i].value).clone();
        let display = |e: &Edits, i: usize| {
            e.display
                .get(&i)
                .copied()
                .unwrap_or_else(|| original_display(&marks, i))
        };
        let read_only = |e: &Edits, i: usize| {
            e.read_only
                .get(&i)
                .copied()
                .unwrap_or(marks.fields[i].info.read_only)
        };
        let mut values = Vec::new();
        let mut displays = Vec::new();
        let mut read_onlys = Vec::new();
        let keys = |a: &Edits, b: &Edits, of: fn(&Edits) -> Vec<usize>| {
            let mut all = of(a);
            all.extend(of(b));
            all.sort_unstable();
            all.dedup();
            all
        };
        for i in keys(&told, edits, |e| e.fields.keys().copied().collect()) {
            if value(&told, i) != value(edits, i) {
                values.push((i, value(edits, i)));
            }
        }
        for i in keys(&told, edits, |e| e.display.keys().copied().collect()) {
            if display(&told, i) != display(edits, i) {
                displays.push((i, display(edits, i)));
            }
        }
        for i in keys(&told, edits, |e| e.read_only.keys().copied().collect()) {
            if read_only(&told, i) != read_only(edits, i) {
                read_onlys.push((i, read_only(edits, i)));
            }
        }
        let calculate = self.viewer.calculate_next.get_untracked();
        if calculate {
            self.viewer.calculate_next.set(false);
        }
        if values.is_empty() && displays.is_empty() && read_onlys.is_empty() {
            return;
        }
        host.send(Ask::Sync {
            values,
            display: displays,
            read_only: read_onlys,
            calculate,
        });
    }

    /// Does what the scripts said, in order. What came for `undo` is one thing to undo; the rest
    /// follows from what was there. It gives what came for `mine`.
    fn apply(&self, replies: Vec<Reply>, undo: Option<u64>, mine: u64) -> Applied {
        let mut applied = Applied::default();
        let marks = self.marks();
        let mut then = Vec::new();
        for (ticket, effects) in replies {
            let mut next = (*self.viewer.edits.get_untracked()).clone();
            let mut looks = Vec::new();
            for effect in effects {
                match effect {
                    Effect::Value {
                        field,
                        value,
                        shown,
                    }
                    | Effect::Refused {
                        field,
                        value,
                        shown,
                    } => set_value(&mut next, &marks, field, value, shown),
                    Effect::Display { field, display } => {
                        if display == original_display(&marks, field) {
                            next.display.remove(&field);
                        } else {
                            next.display.insert(field, display);
                        }
                    }
                    Effect::ReadOnly { field, on } => {
                        if on == marks.fields[field].info.read_only {
                            next.read_only.remove(&field);
                        } else {
                            next.read_only.insert(field, on);
                        }
                    }
                    Effect::Typed {
                        accepted,
                        text,
                        selection,
                    } => {
                        // What was typed after that is there now, so a late answer is no use.
                        if ticket == mine {
                            applied.typed = Some((accepted, text, selection));
                        }
                    }
                    Effect::Look { field, shown } => {
                        next.shown.insert(field, shown.clone());
                        looks.push((field, shown));
                    }
                    other => then.push(other),
                }
            }
            if !looks.is_empty() {
                // They are what the file has, as far as being changed goes.
                let mut saved = (*self.viewer.saved.get_untracked()).clone();
                saved.shown.extend(looks);
                self.viewer.saved.set(Arc::new(saved));
            }
            *self.told.borrow_mut() = Arc::new(next.clone());
            if undo == Some(ticket) {
                self.viewer.change_edits(|edits| *edits = next);
            } else {
                self.viewer.amend_edits(next);
            }
        }
        for effect in then {
            match effect {
                Effect::GoToPage(page) => self.viewer.go_to_page(page),
                Effect::Url(url) => {
                    if let Some(f) = &self.reader.on_link {
                        f(&url);
                    }
                }
                Effect::Focus(field) => match self.focus_after.get() {
                    // What is being committed would close it again.
                    Some(_) => self.focus_after.set(Some(Some(field))),
                    None => self.focus_field(field),
                },
                _ => {}
            }
        }
        applied
    }

    /// Opens the first widget of a field that shows, as `setFocus` asks.
    pub(super) fn focus_field(&self, field: usize) {
        let marks = self.marks();
        let Some(info) = marks.fields.get(field) else {
            return;
        };
        let Some(&(page, widget)) = info
            .widgets
            .iter()
            .find(|&&(p, w)| !self.viewer.widget_hidden(&marks, p, w))
        else {
            return;
        };
        let current = self.active.with_untracked(|a| a.as_ref().map(|a| a.kind));
        if let Some(ActiveKind::Field { field: f, .. }) = current
            && f == field
        {
            return;
        }
        let table = untrack(|| self.table.get());
        if page < table.len() {
            let area = self.reader.to_pixels(
                &table,
                page,
                &marks.pages[page].box_to_points(marks.pages[page].widgets[widget].rect),
            );
            self.reader.bring_into_view(&table, page, area);
        }
        self.use_widget(page, widget);
    }

    /// The box a script shows and waits for, over all of the window.
    pub fn dialog_view(self: &Rc<Self>) -> View {
        let viewer = self.viewer;
        show(
            move || viewer.dialog.with(Option::is_some),
            move || {
                let Some(dialog) = viewer.dialog.get_untracked() else {
                    return div().into_view();
                };
                dialog_box(viewer, dialog).into_view()
            },
        )
        .into_view()
    }
}

/// What the document says about whether a field shows, going by its first widget.
fn original_display(marks: &Marks, field: usize) -> Display {
    marks.fields[field]
        .widgets
        .first()
        .map_or(Display::Visible, |&(p, w)| {
            Display::of_flags(marks.pages[p].widgets[w].flags)
        })
}

/// Says what a field has, and what shows for it.
fn set_value(
    edits: &mut Edits,
    marks: &Marks,
    field: usize,
    value: PdfFieldValue,
    shown: Option<String>,
) {
    if marks.fields[field].value == value {
        edits.fields.remove(&field);
    } else {
        edits.fields.insert(field, value);
    }
    match shown {
        Some(shown) => edits.shown.insert(field, shown),
        None => edits.shown.remove(&field),
    };
}

fn answer_label(answer: &PdfAnswer) -> &'static str {
    match answer {
        PdfAnswer::Ok | PdfAnswer::Text(_) => "OK",
        PdfAnswer::Cancel => "Cancel",
        PdfAnswer::No => "No",
        PdfAnswer::Yes => "Yes",
    }
}

fn dialog_box(viewer: crate::PdfViewer, dialog: PdfDialog) -> impl IntoView {
    let text = signal(match &dialog.kind {
        PdfDialogKind::Question { default, .. } => default.clone(),
        PdfDialogKind::Alert(_) => String::new(),
    });
    let question = matches!(dialog.kind, PdfDialogKind::Question { .. });
    let answers: Vec<PdfAnswer> = match &dialog.kind {
        PdfDialogKind::Alert(buttons) => buttons.answers().to_vec(),
        PdfDialogKind::Question { .. } => PdfButtons::OkCancel.answers().to_vec(),
    };
    // Enter is the first answer and Escape the last, as the buttons go from left to right.
    let first = answers[0].clone();
    let last = answers[answers.len() - 1].clone();
    let finish = Rc::new(move |answer: PdfAnswer| {
        let answer = match answer {
            PdfAnswer::Ok if question => PdfAnswer::Text(text.get_untracked()),
            other => other,
        };
        viewer.answer_dialog(answer);
    });
    let card = node_ref();
    let field = node_ref();
    let buttons = answers.into_iter().enumerate().map({
        let finish = finish.clone();
        move |(i, answer)| {
            let finish = finish.clone();
            let label = answer_label(&answer);
            let main = i == 0;
            div()
                .px(16.0)
                .py(6.0)
                .rounded(4.0)
                .cursor(CursorIcon::Pointer)
                .focusable()
                .bg(if main {
                    Color::from_rgb(0x1a73e8)
                } else {
                    Color::from_rgb(0xe8eaed)
                })
                .text_color(if main { Color::WHITE } else { Color::BLACK })
                .on_click(move |_| finish(answer.clone()))
                .child(text_node(label))
        }
    });
    let title = if dialog.title.is_empty() {
        "This document says".to_owned()
    } else {
        dialog.title.clone()
    };
    let mut body = div()
        .node_ref(card)
        .focusable()
        .flex()
        .flex_col()
        .gap(12.0)
        .p(20.0)
        .min_w(320.0)
        .max_w(520.0)
        .rounded(8.0)
        .bg(Color::WHITE)
        .text_color(Color::BLACK)
        .shadow_md()
        .on_key_down({
            let finish = finish.clone();
            move |e| match &e.key {
                Key::Named(NamedKey::Enter) => {
                    finish(first.clone());
                    e.stop_propagation();
                }
                Key::Named(NamedKey::Escape) => {
                    finish(last.clone());
                    e.stop_propagation();
                }
                _ => {}
            }
        })
        .child(text_node(title).bold())
        .child(text_node(dialog.message.clone()));
    if let PdfDialogKind::Question { password, .. } = &dialog.kind {
        let input = text_input()
            .node_ref(field)
            .value(text)
            .border(1.0)
            .border_color(Color::from_rgb(0x9aa0a6))
            .px(6.0)
            .py(4.0);
        let input = if *password {
            input.purpose(anti_webui_core::platform::ImePurpose::Password)
        } else {
            input
        };
        body = body.child(input);
    }
    body = body.child(div().flex().justify_end().gap(8.0).children(buttons));
    if question {
        field.focus();
    } else {
        card.focus();
    }
    portal().modal().child(
        div()
            .absolute()
            .inset(Length::Px(0.0))
            .flex()
            .items_center()
            .justify_center()
            .bg(Color::from_rgba(0x00000066))
            .on_pointer_down(|e| e.stop_propagation())
            .child(body),
    )
}

fn text_node(s: impl Into<String>) -> Text {
    text(s.into())
}
