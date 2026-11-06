//! Parsing with tree-sitter and coloring what shows. The tree is kept up to date with each edit;
//! parsing again is tried on the UI thread for a moment and otherwise done on a thread of its own,
//! while the old tree, moved by the edits, keeps coloring. Only the lines that have rows are
//! colored. See "色分けの流れ" in `docs/design/31-code-editor.md`.

use crate::language::Language;
use std::collections::HashMap;
use std::ops::{ControlFlow, Range};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tree_sitter::{
    InputEdit, Node, ParseOptions, ParseState, Parser, Point, Query, QueryCursor,
    StreamingIterator, Tree,
};

/// A language's grammar with its queries, compiled once for the whole process.
pub(crate) struct Compiled {
    pub grammar: tree_sitter::Language,
    pub highlights: Query,
    /// How specific each pattern of `highlights` is: how long it is written.
    pub ranks: Vec<u32>,
    pub injections: Option<Query>,
}

type Table = Mutex<HashMap<Language, Option<Arc<Compiled>>>>;

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(Table::default)
}

/// The compiled language, compiling it if it isn't yet. Compiling takes 2 to 20 ms, so this is
/// for the worker.
pub(crate) fn compile(lang: Language) -> Option<Arc<Compiled>> {
    if let Some(c) = table().lock().expect("no panics in here").get(&lang) {
        return c.clone();
    }
    let compiled = lang.grammar().and_then(|g| {
        let highlights = Query::new(&g.language, &g.highlights)
            .map_err(|e| log::warn!("the highlights of {} don't compile: {e}", lang.name()))
            .ok()?;
        let injections = (!g.injections.is_empty())
            .then(|| Query::new(&g.language, g.injections))
            .and_then(|q| {
                q.map_err(|e| log::warn!("the injections of {} don't compile: {e}", lang.name()))
                    .ok()
            });
        let ranks = (0..highlights.pattern_count())
            .map(|i| {
                let text = &g.highlights
                    [highlights.start_byte_for_pattern(i)..highlights.end_byte_for_pattern(i)];
                text.lines()
                    .map(|l| l.split(';').next().unwrap_or("").trim().len())
                    .sum::<usize>() as u32
            })
            .collect();
        Some(Arc::new(Compiled {
            grammar: g.language,
            highlights,
            ranks,
            injections,
        }))
    });
    table()
        .lock()
        .expect("no panics in here")
        .insert(lang, compiled.clone());
    compiled
}

/// The compiled language if it is ready, without waiting for it.
pub(crate) fn compiled(lang: Language) -> Option<Option<Arc<Compiled>>> {
    table()
        .lock()
        .expect("no panics in here")
        .get(&lang)
        .cloned()
}

type Job = Box<dyn FnOnce() + Send>;

/// Runs on the thread that parses and compiles, one job after the other.
pub(crate) fn spawn(job: impl FnOnce() + Send + 'static) {
    static WORKER: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();
    let sender = WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        std::thread::Builder::new()
            .name("anti-webui-code".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("a thread for parsing");
        Mutex::new(tx)
    });
    let _ = sender
        .lock()
        .expect("no panics in here")
        .send(Box::new(job));
}

thread_local! {
    static PARSERS: std::cell::RefCell<HashMap<Language, Parser>> = Default::default();
}

/// Parses with this thread's parser for the language.
pub(crate) fn parse_here(
    lang: Language,
    compiled: &Compiled,
    text: &str,
    old: Option<&Tree>,
) -> Option<Tree> {
    PARSERS.with(|p| {
        let mut parsers = p.borrow_mut();
        let parser = parsers.entry(lang).or_insert_with(|| {
            let mut p = Parser::new();
            let _ = p.set_language(&compiled.grammar);
            p
        });
        parser.parse(text, old)
    })
}

/// The tree-sitter edit for a change of the text.
pub(crate) fn input_edit(e: &anti_webui_core::prelude::TextEdit) -> InputEdit {
    let p = |(row, column): (usize, usize)| Point { row, column };
    InputEdit {
        start_byte: e.start,
        old_end_byte: e.old_end,
        new_end_byte: e.new_end,
        start_position: p(e.start_point),
        old_end_position: p(e.old_end_point),
        new_end_position: p(e.new_end_point),
    }
}

/// How long parsing again may hold up the UI thread after an edit, and when the text is new.
const SYNC_AFTER_EDIT: Duration = Duration::from_millis(1);
/// After parsing on the UI thread took too long, it isn't tried there for this long.
const SLOW_FOR: Duration = Duration::from_secs(2);
/// How much of a long text is parsed first, to color where it opens.
const START: usize = 128 * 1024;
/// How many parts of other languages are kept parsed.
const INJECTED_KEPT: usize = 512;

/// What a byte is painted with: an index into the styles, and what lies over it.
pub(crate) const NONE: u16 = u16::MAX;

/// A part in another language, parsed on its own.
struct Injected {
    tree: Tree,
    used: u64,
}

/// The parsing and coloring of one editor.
pub(crate) struct Highlighter {
    pub language: Language,
    compiled: Option<Arc<Compiled>>,
    pub tree: Option<Tree>,
    /// The text changed since the tree was parsed.
    pub stale: bool,
    /// The parse out on the worker, and the edits made since its text was taken.
    job: Option<u64>,
    since_job: Vec<InputEdit>,
    jobs: u64,
    parser: Parser,
    slow_until: Option<Instant>,
    cursor: QueryCursor,
    /// For each compiled language, the style of each of its capture names.
    ids: HashMap<usize, Vec<u16>>,
    injected: HashMap<(Language, usize, usize, u64), Injected>,
    passes: u64,
    /// Texts longer than this aren't parsed.
    pub limit: usize,
    /// Asks for the compiled grammar and the first tree, and is told when they come.
    pub started: bool,
    send: SendFn,
}

/// What the worker sends back.
pub(crate) enum Done {
    Ready(Language),
    /// A tree of the start of a long text, which colors what shows when it opens until the whole
    /// of it is parsed.
    Start {
        job: u64,
        tree: Tree,
    },
    Parsed {
        job: u64,
        tree: Option<Tree>,
    },
}

impl Highlighter {
    pub fn new(language: Language, limit: usize, send: SendFn) -> Self {
        Self {
            language,
            compiled: None,
            tree: None,
            stale: true,
            job: None,
            since_job: Vec::new(),
            jobs: 0,
            parser: Parser::new(),
            slow_until: None,
            cursor: QueryCursor::new(),
            ids: HashMap::new(),
            injected: HashMap::new(),
            passes: 0,
            limit,
            started: false,
            send,
        }
    }

    /// The text changed.
    pub fn edit(&mut self, e: &InputEdit) {
        if let Some(t) = &mut self.tree {
            t.edit(e);
        }
        if self.job.is_some() {
            self.since_job.push(*e);
        }
        self.stale = true;
    }

    /// What came from the worker. Returns whether the colors may have changed.
    pub fn done(&mut self, done: Done) -> bool {
        match done {
            Done::Ready(lang) if lang == self.language => {
                self.compiled = compiled(lang).flatten();
                true
            }
            Done::Ready(_) => true,
            Done::Start { job, tree } => {
                if self.job != Some(job) || self.tree.is_some() {
                    return false;
                }
                // The edits made since its text was taken move it; the job still has them for the
                // whole tree.
                let mut tree = tree;
                for e in &self.since_job {
                    tree.edit(e);
                }
                self.tree = Some(tree);
                true
            }
            Done::Parsed { job, tree } => {
                if self.job != Some(job) {
                    return false;
                }
                self.job = None;
                let Some(mut tree) = tree else {
                    return false;
                };
                for e in self.since_job.drain(..) {
                    tree.edit(&e);
                    self.stale = true;
                }
                self.tree = Some(tree);
                true
            }
        }
    }

    /// Brings the tree up to date with the text if that is quick, and otherwise has it parsed on
    /// the worker, which `send` tells of. Call before coloring.
    pub fn prepare(&mut self, text: &str) {
        if text.len() > self.limit {
            self.tree = None;
            return;
        }
        if !self.started {
            self.started = true;
            self.start(text);
            return;
        }
        if !self.stale || self.job.is_some() {
            return;
        }
        let Some(compiled) = self.compiled.clone() else {
            return;
        };
        let quick = self.tree.is_some() && self.slow_until.is_none_or(|t| Instant::now() >= t);
        if quick {
            if self.parser.language().is_none() {
                let _ = self.parser.set_language(&compiled.grammar);
            }
            let deadline = Instant::now() + SYNC_AFTER_EDIT;
            let mut progress = |_: &ParseState| {
                if Instant::now() >= deadline {
                    ControlFlow::Break(())
                } else {
                    ControlFlow::Continue(())
                }
            };
            let bytes = text.as_bytes();
            let tree = self.parser.parse_with_options(
                &mut |i, _| bytes.get(i..).unwrap_or_default(),
                self.tree.as_ref(),
                Some(ParseOptions::new().progress_callback(&mut progress)),
            );
            match tree {
                Some(t) => {
                    self.tree = Some(t);
                    self.stale = false;
                    return;
                }
                None => {
                    self.parser.reset();
                    self.slow_until = Some(Instant::now() + SLOW_FOR);
                }
            }
        }
        self.parse_elsewhere(text, compiled);
    }

    /// The first time: the grammar is compiled and the text parsed on the worker.
    fn start(&mut self, text: &str) {
        let lang = self.language;
        if lang.grammar().is_none() {
            return;
        }
        let snapshot = text.to_owned();
        self.jobs += 1;
        let job = self.jobs;
        self.job = Some(job);
        self.since_job.clear();
        let send = self.send.clone();
        spawn(move || {
            let compiled = compile(lang);
            send(Done::Ready(lang));
            let Some(c) = compiled else {
                send(Done::Parsed { job, tree: None });
                return;
            };
            // A long text opens at its start: that is colored first.
            if snapshot.len() > 2 * START {
                let cut = snapshot.as_bytes()[..START]
                    .iter()
                    .rposition(|&b| b == b'\n')
                    .map_or(0, |i| i + 1);
                if let Some(tree) = parse_here(lang, &c, &snapshot[..cut], None) {
                    send(Done::Start { job, tree });
                }
            }
            let tree = parse_here(lang, &c, &snapshot, None);
            send(Done::Parsed { job, tree });
        });
        self.stale = false;
    }

    fn parse_elsewhere(&mut self, text: &str, compiled: Arc<Compiled>) {
        let lang = self.language;
        let snapshot = text.to_owned();
        let old = self.tree.clone();
        self.jobs += 1;
        let job = self.jobs;
        self.job = Some(job);
        self.since_job.clear();
        self.stale = false;
        let send = self.send.clone();
        spawn(move || {
            let tree = parse_here(lang, &compiled, &snapshot, old.as_ref());
            send(Done::Parsed { job, tree });
        });
    }

    /// Paints `range` of `text` with the styles of what the grammar finds, an index of `styles`
    /// for each byte (or `NONE`). `style_of` gives the index for a capture name.
    pub fn paint(
        &mut self,
        text: &str,
        range: Range<usize>,
        out: &mut [u16],
        style_of: &mut dyn FnMut(&str) -> u16,
    ) {
        let (Some(tree), Some(compiled)) = (self.tree.clone(), self.compiled.clone()) else {
            return;
        };
        self.passes += 1;
        paint_tree(
            &mut self.cursor,
            &mut self.ids,
            &compiled,
            tree.root_node(),
            text,
            range.clone(),
            out,
            style_of,
        );
        self.paint_injections(&compiled, &tree, text, range, out, style_of, 0);
        // Forget the parts of other languages that haven't shown for a while.
        if self.injected.len() > INJECTED_KEPT {
            let mut used: Vec<u64> = self.injected.values().map(|i| i.used).collect();
            used.sort_unstable();
            let cut = used[used.len() - INJECTED_KEPT / 2];
            self.injected.retain(|_, i| i.used >= cut);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_injections(
        &mut self,
        compiled: &Compiled,
        tree: &Tree,
        text: &str,
        range: Range<usize>,
        out: &mut [u16],
        style_of: &mut dyn FnMut(&str) -> u16,
        depth: u32,
    ) {
        let Some(query) = &compiled.injections else {
            return;
        };
        if depth > 2 {
            return;
        }
        let found = injections(query, tree.root_node(), text, range.clone());
        for (name, content) in found {
            let Some(lang) = Language::from_name(&name) else {
                continue;
            };
            let sub = match compiled_or_ask(lang, &self.send) {
                Some(c) => c,
                None => continue,
            };
            let key = (
                lang,
                content.start_byte,
                content.end_byte,
                hash(&text[content.start_byte..content.end_byte]),
            );
            let passes = self.passes;
            let entry = match self.injected.get_mut(&key) {
                Some(e) => {
                    e.used = passes;
                    e.tree.clone()
                }
                None => {
                    let mut p = Parser::new();
                    if p.set_language(&sub.grammar).is_err()
                        || p.set_included_ranges(&[content]).is_err()
                    {
                        continue;
                    }
                    let Some(t) = p.parse(text, None) else {
                        continue;
                    };
                    self.injected.insert(
                        key,
                        Injected {
                            tree: t.clone(),
                            used: passes,
                        },
                    );
                    t
                }
            };
            let a = range.start.max(content.start_byte);
            let b = range.end.min(content.end_byte);
            if a >= b {
                continue;
            }
            paint_tree(
                &mut self.cursor,
                &mut self.ids,
                &sub,
                entry.root_node(),
                text,
                a..b,
                &mut out[a - range.start..b - range.start],
                style_of,
            );
            self.paint_injections(
                &sub,
                &entry,
                text,
                a..b,
                &mut out[a - range.start..b - range.start],
                style_of,
                depth + 1,
            );
        }
    }
}

/// The compiled language, or `None` after asking the worker to compile it.
fn compiled_or_ask(lang: Language, send: &SendFn) -> Option<Arc<Compiled>> {
    match compiled(lang) {
        Some(c) => c,
        None => {
            let send = send.clone();
            spawn(move || {
                compile(lang);
                send(Done::Ready(lang));
            });
            None
        }
    }
}

/// How the worker reaches the editor.
pub(crate) type SendFn = Arc<dyn Fn(Done) + Send + Sync>;

fn hash(s: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Paints the captures of a tree in `range`: from the outside in, and of the captures of the same
/// node the most specific pattern wins (the longest), then the first. tree-sitter's own
/// highlighter takes the first, which colors a method call in Rust as a field, as the query's
/// `(field_identifier) @property` comes before its calls.
#[allow(clippy::too_many_arguments)]
fn paint_tree(
    cursor: &mut QueryCursor,
    ids: &mut HashMap<usize, Vec<u16>>,
    compiled: &Compiled,
    root: Node,
    text: &str,
    range: Range<usize>,
    out: &mut [u16],
    style_of: &mut dyn FnMut(&str) -> u16,
) {
    let query = &compiled.highlights;
    let ids = ids
        .entry(compiled as *const Compiled as usize)
        .or_insert_with(|| query.capture_names().iter().map(|n| style_of(n)).collect());
    let mut found: Vec<(usize, usize, usize, u16)> = Vec::new();
    cursor.set_byte_range(range.clone());
    let mut captures = cursor.captures(query, root, text.as_bytes());
    while let Some((m, i)) = captures.next() {
        let c = m.captures()[*i];
        let id = ids[c.index as usize];
        if id == NONE {
            continue;
        }
        let (s, e) = (c.node.start_byte(), c.node.end_byte());
        if e <= range.start || s >= range.end || s == e {
            continue;
        }
        found.push((s, e, m.pattern_index, id));
    }
    // Outer first, and of the same node the one that wins last.
    let rank = |p: usize| {
        (
            compiled.ranks.get(p).copied().unwrap_or(0),
            std::cmp::Reverse(p),
        )
    };
    found.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.1.cmp(&a.1))
            .then(rank(a.2).cmp(&rank(b.2)))
    });
    for (s, e, _, id) in found {
        let a = s.max(range.start) - range.start;
        let b = e.min(range.end) - range.start;
        out[a..b].fill(id);
    }
}

/// The parts in other languages in `range`: the language's name and where the part is.
fn injections(
    query: &Query,
    root: Node,
    text: &str,
    range: Range<usize>,
) -> Vec<(String, tree_sitter::Range)> {
    let content = query.capture_index_for_name("injection.content");
    let language = query.capture_index_for_name("injection.language");
    let mut cursor = QueryCursor::new();
    cursor.set_byte_range(range);
    let mut out = Vec::new();
    let mut matches = cursor.matches(query, root, text.as_bytes());
    while let Some(m) = matches.next() {
        let mut name = query
            .property_settings(m.pattern_index)
            .iter()
            .find(|p| &*p.key == "injection.language")
            .and_then(|p| p.value.as_deref().map(str::to_owned));
        let mut node = None;
        for c in m.captures() {
            if Some(c.index) == language {
                name = Some(text[c.node.byte_range()].to_owned());
            } else if Some(c.index) == content {
                node = Some(c.node);
            }
        }
        if let (Some(name), Some(node)) = (name, node)
            && node.start_byte() < node.end_byte()
        {
            out.push((name, node.range()));
        }
    }
    out
}

/// Where the bracket at or before `at` and its partner are, by the tree: brackets in strings and
/// comments aren't brackets there.
pub(crate) fn brackets(tree: &Tree, text: &str, at: usize) -> Option<(usize, usize)> {
    const PAIRS: [(&str, &str); 3] = [("(", ")"), ("[", "]"), ("{", "}")];
    let bytes = text.as_bytes();
    // The bracket after the caret, or else the one before it.
    let candidates = [Some(at), at.checked_sub(1)];
    for pos in candidates.into_iter().flatten() {
        let Some(&c) = bytes.get(pos) else { continue };
        if !b"()[]{}".contains(&c) {
            continue;
        }
        let node = tree.root_node().descendant_for_byte_range(pos, pos + 1)?;
        if node.is_named() || node.start_byte() != pos {
            continue;
        }
        let kind = node.kind();
        let parent = node.parent()?;
        let mut walk = parent.walk();
        let children: Vec<Node> = parent.children(&mut walk).collect();
        for (open, close) in PAIRS {
            if kind == open {
                let partner = children.iter().rev().find(|n| n.kind() == close)?;
                return Some((pos, partner.start_byte()));
            }
            if kind == close {
                let partner = children.iter().find(|n| n.kind() == open)?;
                return Some((partner.start_byte(), pos));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_language_compiles() {
        for &lang in Language::all()
            .iter()
            .chain([Language::MarkdownInline].iter())
        {
            let started = Instant::now();
            let c = compile(lang);
            assert!(c.is_some(), "{} doesn't compile", lang.name());
            eprintln!("{}: {:?}", lang.name(), started.elapsed());
        }
    }

    #[test]
    fn brackets_pair_up_by_the_tree() {
        let lang = Language::Rust;
        let c = compile(lang).unwrap();
        let text = "fn f() { let s = \"{\"; g(a[0]); }";
        let tree = parse_here(lang, &c, text, None).unwrap();
        let open = text.find('{').unwrap();
        let close = text.rfind('}').unwrap();
        assert_eq!(brackets(&tree, text, open), Some((open, close)));
        assert_eq!(brackets(&tree, text, close + 1), Some((open, close)));
        // The brace in the string is no bracket.
        let quoted = text.find("\"{").unwrap() + 1;
        assert_eq!(brackets(&tree, text, quoted), None);
        let sq = text.find('[').unwrap();
        assert_eq!(brackets(&tree, text, sq), Some((sq, sq + 2)));
    }

    /// The styles painted on a text, by name.
    fn painted(lang: Language, text: &str) -> Vec<(String, String)> {
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let mut h = Highlighter::new(
            lang,
            usize::MAX,
            Arc::new(move |d| {
                let _ = tx.lock().unwrap().send(d);
            }),
        );
        h.prepare(text);
        while h.tree.is_none() {
            let d = rx.recv_timeout(Duration::from_secs(10)).expect("parsed");
            h.done(d);
        }
        let mut names: Vec<String> = Vec::new();
        let mut out = vec![NONE; text.len()];
        // Injected languages compile on the worker: paint until they are there.
        for _ in 0..50 {
            out.fill(NONE);
            h.paint(text, 0..text.len(), &mut out, &mut |n| {
                let i = names.iter().position(|m| m == n).unwrap_or_else(|| {
                    names.push(n.to_owned());
                    names.len() - 1
                });
                i as u16
            });
            match rx.recv_timeout(Duration::from_millis(200)) {
                Ok(d) => {
                    h.done(d);
                }
                Err(_) => break,
            }
        }
        let mut runs = Vec::new();
        let mut i = 0;
        while i < out.len() {
            let mut j = i + 1;
            while j < out.len() && out[j] == out[i] {
                j += 1;
            }
            if out[i] != NONE {
                runs.push((text[i..j].to_owned(), names[out[i] as usize].clone()));
            }
            i = j;
        }
        runs
    }

    fn has(runs: &[(String, String)], text: &str, name: &str) -> bool {
        runs.iter().any(|(t, n)| t == text && n.starts_with(name))
    }

    #[test]
    fn code_is_colored_by_what_it_is() {
        let runs = painted(Language::Rust, "fn main() { let x = \"hi\"; // note\n}");
        assert!(has(&runs, "fn", "keyword"), "{runs:?}");
        assert!(has(&runs, "main", "function"), "{runs:?}");
        assert!(has(&runs, "\"hi\"", "string"), "{runs:?}");
        assert!(has(&runs, "// note", "comment"), "{runs:?}");
        let runs = painted(Language::Rust, "fn f() { a.lock(); a.field; }");
        assert!(has(&runs, "lock", "function"), "{runs:?}");
        assert!(has(&runs, "field", "property"), "{runs:?}");
        let runs = painted(Language::TypeScript, "const x: number = 1;");
        assert!(has(&runs, "const", "keyword"), "{runs:?}");
        assert!(has(&runs, "number", "type"), "{runs:?}");
    }

    #[test]
    fn code_in_markdown_takes_its_own_colors() {
        let md = "# Title\n\nSome *text*.\n\n```rust\nfn f() {}\n```\n";
        let runs = painted(Language::Markdown, md);
        assert!(
            runs.iter()
                .any(|(t, n)| t.contains("Title") && n.contains("title"))
                || has(&runs, "Title", "text.title"),
            "{runs:?}"
        );
        assert!(has(&runs, "fn", "keyword"), "{runs:?}");
        assert!(
            runs.iter()
                .any(|(t, n)| t.contains("text") && n.contains("emphasis")),
            "{runs:?}"
        );
    }
}
