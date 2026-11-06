//! The JavaScript of documents: what the UI thread says to the scripts and what they say back. The
//! scripts run on a thread of their own, with the API of Acrobat written in JavaScript on top of a
//! thin engine. See "JavaScript" in `docs/design/26-pdf.md`.
// Without the engine there is nothing to send to or to hear from.
#![cfg_attr(not(feature = "js"), allow(dead_code))]

#[cfg(feature = "js")]
mod json;
#[cfg(feature = "js")]
mod quickjs;
#[cfg(feature = "js")]
mod sandbox;

use crate::edits::{Display, Edits};
use crate::marks::{Marks, PdfFieldValue};
use anti_webui_core::channel::{UiSender, UiWork};
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// A box with a message that a script shows, and waits for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PdfDialog {
    pub title: String,
    pub message: String,
    pub kind: PdfDialogKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfDialogKind {
    /// `app.alert`, with the buttons it has.
    Alert(PdfButtons),
    /// `app.response`, which takes text, with what is in it at first.
    Question { default: String, password: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfButtons {
    Ok,
    OkCancel,
    YesNo,
    YesNoCancel,
}

impl PdfButtons {
    pub fn answers(self) -> &'static [PdfAnswer] {
        match self {
            Self::Ok => &[PdfAnswer::Ok],
            Self::OkCancel => &[PdfAnswer::Ok, PdfAnswer::Cancel],
            Self::YesNo => &[PdfAnswer::Yes, PdfAnswer::No],
            Self::YesNoCancel => &[PdfAnswer::Yes, PdfAnswer::No, PdfAnswer::Cancel],
        }
    }
}

/// What was answered to a [`PdfDialog`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PdfAnswer {
    Ok,
    Cancel,
    No,
    Yes,
    /// What was written for a question.
    Text(String),
}

/// What happened to a widget, for its scripts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mouse {
    Down,
    Up,
    Enter,
    Exit,
    Focus,
    Blur,
}

/// What the UI thread tells the scripts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Ask {
    /// The text of a field that is being typed in changed from `value`: `change` took the place of
    /// what was between the two offsets, in UTF-16 units as JavaScript counts.
    Keystroke {
        field: usize,
        value: String,
        change: String,
        selection: (usize, usize),
    },
    /// Someone says this is what a field has.
    Commit {
        field: usize,
        value: PdfFieldValue,
    },
    /// `widget` is which of the field's widgets.
    Mouse {
        field: usize,
        widget: usize,
        what: Mouse,
        shift: bool,
        modifier: bool,
    },
    Link(String),
    /// The page being read, which became that or stopped being that.
    Page {
        page: usize,
        open: bool,
    },
    /// What changed outside of the scripts, which then work out what depends on it if `calculate`.
    Sync {
        values: Vec<(usize, PdfFieldValue)>,
        display: Vec<(usize, Display)>,
        read_only: Vec<(usize, bool)>,
        calculate: bool,
    },
}

/// What the scripts did.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Effect {
    /// What a field has now, and what shows for it if that is something else.
    Value {
        field: usize,
        value: PdfFieldValue,
        shown: Option<String>,
    },
    Display {
        field: usize,
        display: Display,
    },
    ReadOnly {
        field: usize,
        on: bool,
    },
    /// What a field that is being typed in has after a keystroke, and what is selected in it, in
    /// UTF-16 units. It wasn't taken unless `accepted`.
    Typed {
        accepted: bool,
        text: String,
        selection: (usize, usize),
    },
    /// What was committed wasn't taken: the field keeps this.
    Refused {
        field: usize,
        value: PdfFieldValue,
        shown: Option<String>,
    },
    GoToPage(usize),
    Url(String),
    Focus(usize),
    /// What a field that gets its look when the document opens shows, which isn't a change.
    Look {
        field: usize,
        shown: String,
    },
}

/// What the scripts send to the UI thread, apart from what it waits for.
pub(crate) enum Message {
    /// There are answers to take.
    Replied,
    Dialog(PdfDialog),
}

/// What there is to know about the document when the scripts start.
pub(crate) struct Start {
    pub marks: Arc<Marks>,
    pub edits: Arc<Edits>,
    pub file_name: String,
    pub page: usize,
}

/// What the scripts did for something, by the number it was sent with. 0 is for what they did by
/// themselves: when the document opened, and with timers.
pub(crate) type Reply = (u64, Vec<Effect>);

/// What the threads share: the answers and whether a dialog is up.
#[derive(Default)]
pub(crate) struct Shared {
    inbox: Mutex<Inbox>,
    ready: Condvar,
}

#[derive(Default)]
struct Inbox {
    replies: VecDeque<Reply>,
    asking: bool,
}

impl Shared {
    fn reply(&self, reply: Reply) {
        self.inbox
            .lock()
            .expect("no panics in here")
            .replies
            .push_back(reply);
        self.ready.notify_all();
    }

    fn set_asking(&self, asking: bool) {
        self.inbox.lock().expect("no panics in here").asking = asking;
        self.ready.notify_all();
    }
}

pub(crate) enum Command {
    /// With what says that an answer is coming, for tests and screenshots that wait for it.
    Ask(u64, Ask, Option<UiWork>),
    Answer(PdfAnswer),
}

/// The scripts of a document, from the UI thread. Dropping it ends them.
pub(crate) struct ScriptHost {
    commands: std::sync::mpsc::Sender<Command>,
    shared: Arc<Shared>,
    next: std::cell::Cell<u64>,
    out: UiSender<Message>,
}

impl ScriptHost {
    /// Starts the scripts on a thread, if this was built with them.
    #[cfg_attr(not(feature = "js"), allow(unused_variables))]
    pub fn start(start: Start, out: UiSender<Message>) -> Option<Self> {
        #[cfg(feature = "js")]
        {
            let (commands, received) = std::sync::mpsc::channel();
            let shared = Arc::new(Shared::default());
            match sandbox::spawn(start, received, shared.clone(), out.clone()) {
                Ok(()) => Some(Self {
                    commands,
                    shared,
                    next: std::cell::Cell::new(0),
                    out,
                }),
                Err(e) => {
                    log::warn!("couldn't start the scripts of a PDF: {e}");
                    None
                }
            }
        }
        #[cfg(not(feature = "js"))]
        None
    }

    /// Sends something, and gives the number its answer comes with.
    pub fn send(&self, ask: Ask) -> u64 {
        let ticket = self.next.get() + 1;
        self.next.set(ticket);
        let work = self.out.work();
        let _ = self.commands.send(Command::Ask(ticket, ask, Some(work)));
        ticket
    }

    pub fn answer(&self, answer: PdfAnswer) {
        let _ = self.commands.send(Command::Answer(answer));
    }

    /// Waits a moment for the answer to something, and gives it with all that came before it, in
    /// order. It gives nothing if that takes longer or a dialog is up, and the answers come later.
    pub fn wait(&self, ticket: u64, most: Duration) -> Option<Vec<Reply>> {
        let until = Instant::now() + most;
        let mut inbox = self.shared.inbox.lock().expect("no panics in here");
        loop {
            if let Some(at) = inbox.replies.iter().position(|r| r.0 == ticket) {
                return Some(inbox.replies.drain(..=at).collect());
            }
            let now = Instant::now();
            if inbox.asking || now >= until {
                return None;
            }
            inbox = self
                .shared
                .ready
                .wait_timeout(inbox, until - now)
                .expect("no panics in here")
                .0;
        }
    }

    /// All the answers that have come.
    pub fn take(&self) -> Vec<Reply> {
        let mut inbox = self.shared.inbox.lock().expect("no panics in here");
        inbox.replies.drain(..).collect()
    }
}

/// Where `new` differs from `old`, as a keystroke has it: what took the place of what, by UTF-16
/// offsets into `old`.
pub(crate) fn keystroke_of(old: &str, new: &str) -> (String, (usize, usize)) {
    let a: Vec<u16> = old.encode_utf16().collect();
    let b: Vec<u16> = new.encode_utf16().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let most = a.len().min(b.len()) - prefix;
    let suffix = a
        .iter()
        .rev()
        .zip(b.iter().rev())
        .take(most)
        .take_while(|(x, y)| x == y)
        .count();
    let change = String::from_utf16_lossy(&b[prefix..b.len() - suffix]);
    (change, (prefix, a.len() - suffix))
}

/// The byte offset of a UTF-16 offset into text.
pub(crate) fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (at, c) in text.char_indices() {
        if units >= utf16 {
            return at;
        }
        units += c.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_keystroke_is_what_changed_between_two_texts() {
        assert_eq!(keystroke_of("12", "123"), ("3".to_owned(), (2, 2)));
        assert_eq!(keystroke_of("13", "123"), ("2".to_owned(), (1, 1)));
        assert_eq!(keystroke_of("123", "13"), (String::new(), (1, 2)));
        assert_eq!(keystroke_of("aa", "aaa"), ("a".to_owned(), (2, 2)));
        assert_eq!(keystroke_of("abc", "xyz"), ("xyz".to_owned(), (0, 3)));
        assert_eq!(keystroke_of("日本", "日😀本"), ("😀".to_owned(), (1, 1)));
        assert_eq!(byte_offset("日😀本", 3), "日😀".len());
    }
}
