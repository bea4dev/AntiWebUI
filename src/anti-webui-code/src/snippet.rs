//! Coloring short pieces of code on the UI thread, such as the code blocks of a Markdown
//! document. See "コードブロックの色分け" in `docs/design/32-markdown-split.md`.

use crate::highlight::{self, Done, Highlighter, NONE, SendFn};
use crate::language::Language;
use crate::theme::CodeTheme;
use anti_webui_core::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

/// Code longer than this isn't colored: it is parsed on the UI thread, at about 10 MB a second.
pub const SNIPPET_LIMIT: usize = 64 * 1024;
/// How many pieces are kept colored, by their language and code.
const KEPT: usize = 512;

/// The colors of a piece: where each style goes, in order.
pub type Colors = Vec<(Range<usize>, Rc<Style>)>;

/// Colors pieces of code by their language's name, as a Markdown code block gives it (`rust`,
/// `sh`, `ts`, ...). The first piece in a language compiles its grammar on a thread of its own;
/// until then pieces in it aren't colored, and reading [`CodeHighlighter::ready`] (which
/// [`highlight`](CodeHighlighter::highlight) does then) tracks when they can be. Make it where
/// signals can be made, as in a view.
#[derive(Clone)]
pub struct CodeHighlighter(Rc<Inner>);

struct Inner {
    theme: CodeTheme,
    ready: Signal<u64>,
    send: SendFn,
    state: RefCell<State>,
}

#[derive(Default)]
struct State {
    by_name: HashMap<String, u16>,
    styles: Vec<Rc<Style>>,
    /// A highlighter for each language, which keeps what it parsed of other languages in it.
    highlighters: HashMap<Language, Highlighter>,
    kept: HashMap<(Language, u64), Rc<Colors>>,
    order: VecDeque<(Language, u64)>,
    /// The languages whose grammars have been asked for, once each.
    asked: HashSet<Language>,
}

impl CodeHighlighter {
    pub fn new(theme: CodeTheme) -> Self {
        let ready = signal(0u64);
        let sender = ui_channel(move |done: Done| {
            if matches!(done, Done::Ready(_)) {
                ready.update(|n| *n += 1);
            }
        });
        let send: SendFn = Arc::new(move |d| sender.send(d));
        Self(Rc::new(Inner {
            theme,
            ready,
            send,
            state: RefCell::default(),
        }))
    }

    /// Bumped whenever a grammar has been compiled, after which what wasn't colored may be.
    pub fn ready(&self) -> Signal<u64> {
        self.0.ready
    }

    /// The colors of `code` in the language named `lang`. Nothing for a language without a
    /// grammar here, for code longer than [`SNIPPET_LIMIT`], and while the grammar is compiled.
    pub fn highlight(&self, lang: &str, code: &str) -> Colors {
        let name = lang
            .trim_matches(|c| matches!(c, '{' | '}' | '.'))
            .split([',', ' '])
            .next()
            .unwrap_or("");
        let Some(lang) = Language::from_name(name) else {
            return Vec::new();
        };
        if code.is_empty() || code.len() > SNIPPET_LIMIT || lang == Language::PlainText {
            return Vec::new();
        }
        let key = (lang, hash_of(code));
        let inner = &*self.0;
        if let Some(c) = inner.state.borrow().kept.get(&key) {
            return (**c).clone();
        }
        let compiled = match highlight::compiled(lang) {
            Some(Some(c)) => c,
            Some(None) => return Vec::new(),
            None => {
                if inner.state.borrow_mut().asked.insert(lang) {
                    highlight::ask(lang, &inner.send);
                }
                inner.ready.get();
                return Vec::new();
            }
        };
        let Some(tree) = highlight::parse_here(lang, &compiled, code, None) else {
            return Vec::new();
        };
        let mut guard = inner.state.borrow_mut();
        let State {
            by_name,
            styles,
            highlighters,
            ..
        } = &mut *guard;
        let h = highlighters
            .entry(lang)
            .or_insert_with(|| Highlighter::new(lang, usize::MAX, inner.send.clone()));
        h.set_tree(compiled, tree);
        h.waiting = false;
        let mut ids = vec![NONE; code.len()];
        h.paint(code, 0..code.len(), &mut ids, &mut |name| {
            if let Some(&id) = by_name.get(name) {
                return id;
            }
            let id = match inner.theme.style_of(name) {
                Some(s) => {
                    styles.push(s);
                    (styles.len() - 1) as u16
                }
                None => NONE,
            };
            by_name.insert(name.to_owned(), id);
            id
        });
        let waiting = h.waiting;
        let mut out = Vec::new();
        let mut i = 0;
        while i < ids.len() {
            let id = ids[i];
            let mut j = i + 1;
            while j < ids.len() && ids[j] == id {
                j += 1;
            }
            if id != NONE {
                out.push((i..j, styles[id as usize].clone()));
            }
            i = j;
        }
        if waiting {
            // A part in another language comes later: not kept until it is there.
            drop(guard);
            inner.ready.get();
            return out;
        }
        guard.kept.insert(key, Rc::new(out.clone()));
        guard.order.push_back(key);
        if guard.order.len() > KEPT
            && let Some(old) = guard.order.pop_front()
        {
            guard.kept.remove(&old);
        }
        out
    }
}

fn hash_of(s: &str) -> u64 {
    let mut h = DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}
