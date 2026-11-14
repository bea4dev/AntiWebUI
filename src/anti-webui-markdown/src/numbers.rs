//! The numbers of a document's formulas on lines of their own, counted on from one to the next
//! as KaTeX counts them through a page, and what their labels refer to. See "式番号" in
//! `docs/design/40-math.md`.

use crate::parse::{Block, Inlines, Piece, Top};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

#[derive(Debug, Default)]
pub(crate) struct Numbers {
    /// Where each formula starts in the source, in order.
    at: Vec<usize>,
    /// What is found from the formulas' sources alone, which stays while only where they are
    /// changes, as when typing somewhere else.
    found: Rc<Found>,
}

#[derive(Debug, Default)]
struct Found {
    /// Of the sources of the formulas, in order.
    sources: u64,
    /// The first automatic number of each formula.
    first: Vec<u32>,
    /// What refers to each label shows, and which formula it is in.
    labels: HashMap<String, (String, usize)>,
    /// Of all of the labels, to know when what refers to them changes.
    labels_hash: u64,
}

impl Numbers {
    /// From the formulas in the order of the document, with where each starts. What `before`
    /// found is used again when the formulas are the same.
    pub fn of(before: &Numbers, formulas: &[(usize, &str)]) -> Self {
        let mut h = DefaultHasher::new();
        formulas.iter().for_each(|(_, tex)| tex.hash(&mut h));
        let sources = h.finish();
        let found = if before.found.sources == sources && !before.at.is_empty() {
            before.found.clone()
        } else {
            Rc::new(Found::of(sources, formulas))
        };
        Self {
            at: formulas.iter().map(|(at, _)| *at).collect(),
            found,
        }
    }

    fn index(&self, at: usize) -> Option<usize> {
        self.at.binary_search(&at).ok()
    }

    /// The first automatic number of the formula that starts at `at`.
    pub fn first_at(&self, at: usize) -> u32 {
        self.index(at)
            .and_then(|i| self.found.first.get(i).copied())
            .unwrap_or(1)
    }

    /// What refers to `label` shows.
    pub fn number(&self, label: &str) -> Option<String> {
        self.found.labels.get(label).map(|(n, _)| n.clone())
    }

    /// The labels of the formula that starts at `at`.
    pub fn labels_at(&self, at: usize) -> Vec<&str> {
        let Some(i) = self.index(at) else {
            return Vec::new();
        };
        self.found
            .labels
            .iter()
            .filter(|(_, (_, f))| *f == i)
            .map(|(l, _)| l.as_str())
            .collect()
    }

    /// What of the numbers shows in the source `range` of the document, which is `source`: what
    /// is made of it has to be made again when this changes.
    pub fn hash_of(&self, range: Range<usize>, source: &str) -> u64 {
        let (a, b) = (
            self.at.partition_point(|&x| x < range.start),
            self.at.partition_point(|&x| x < range.end),
        );
        let refers = !self.found.labels.is_empty() && source.contains("ref");
        if a == b && !refers {
            return 0;
        }
        let mut h = DefaultHasher::new();
        self.found.first.get(a..b).hash(&mut h);
        if refers {
            self.found.labels_hash.hash(&mut h);
        }
        h.finish()
    }
}

impl Found {
    fn of(sources: u64, formulas: &[(usize, &str)]) -> Self {
        let mut out = Self {
            sources,
            ..Self::default()
        };
        #[cfg(feature = "math")]
        {
            let mut next = 1;
            let mut h = DefaultHasher::new();
            for (i, (_, tex)) in formulas.iter().enumerate() {
                out.first.push(next);
                // Most number nothing, which is seen without laying them out.
                if !(tex.contains("\\begin") || tex.contains("\\tag") || tex.contains("\\label")) {
                    continue;
                }
                out.first.pop();
                let e = anti_webui_math::equations(tex, next);
                out.first.push(next);
                next += e.numbered;
                for (label, shown) in e.labels {
                    (&label, &shown).hash(&mut h);
                    out.labels.entry(label).or_insert((shown, i));
                }
            }
            out.labels_hash = h.finish();
        }
        #[cfg(not(feature = "math"))]
        let _ = formulas;
        out
    }
}

/// The formulas on lines of their own in a document as it is read into blocks, with where each
/// starts.
pub(crate) fn of_blocks(tops: &[Top]) -> Vec<(usize, &str)> {
    fn inl<'a>(p: &'a Inlines, out: &mut Vec<(usize, &'a str)>) {
        for piece in p {
            if let Piece::Math {
                tex,
                display: true,
                at,
            } = piece
            {
                out.push((*at, tex));
            }
        }
    }
    fn walk<'a>(blocks: &mut dyn Iterator<Item = &'a Block>, out: &mut Vec<(usize, &'a str)>) {
        for b in blocks {
            match b {
                Block::Paragraph(p) | Block::Plain(p) | Block::Heading(_, p) => inl(p, out),
                Block::Quote(_, bs) | Block::Footnote(_, bs) => walk(&mut bs.iter(), out),
                Block::List { items, .. } => {
                    for i in items {
                        walk(&mut i.blocks.iter(), out);
                    }
                }
                Block::Table { head, rows, .. } => {
                    for c in head.iter().chain(rows.iter().flatten()) {
                        inl(c, out);
                    }
                }
                _ => {}
            }
        }
    }
    let mut out = Vec::new();
    walk(&mut tops.iter().map(|t| &t.block), &mut out);
    out
}

#[cfg(all(test, feature = "math"))]
mod tests {
    use super::*;

    #[test]
    fn equations_are_numbered_one_after_the_other() {
        let doc = "$$\n\\begin{equation} a \\label{a} \\end{equation}\n$$\n\nText $x$.\n\n```math\n\\begin{align} b \\label{b} \\\\ c \\tag{C} \\label{c} \\\\ d \\end{align}\n```\n\n- $$\\begin{equation} e \\label{e} \\end{equation}$$\n";
        let tops = crate::parse::parse(doc);
        let formulas = of_blocks(&tops);
        assert_eq!(formulas.len(), 3);
        let n = Numbers::of(&Numbers::default(), &formulas);
        assert_eq!(n.first_at(formulas[0].0), 1);
        assert_eq!(n.first_at(formulas[1].0), 2);
        // `\tag` takes no number: the third is 4.
        assert_eq!(n.first_at(formulas[2].0), 4);
        let shown: Vec<_> = ["a", "b", "c", "e", "none"]
            .iter()
            .map(|l| n.number(l))
            .collect();
        assert_eq!(
            shown,
            vec![
                Some("1".into()),
                Some("2".into()),
                Some("C".into()),
                Some("4".into()),
                None
            ]
        );
        assert_eq!(n.labels_at(formulas[0].0), vec!["a"]);
        // What shows the numbers changes when they do; what doesn't, doesn't.
        let other = Numbers::of(&Numbers::default(), &[(formulas[1].0, formulas[1].1)]);
        let range = formulas[1].0..formulas[1].0 + 1;
        assert_ne!(
            n.hash_of(range.clone(), ""),
            other.hash_of(range.clone(), "")
        );
        assert_eq!(n.hash_of(0..0, "text"), other.hash_of(0..0, "text"));
        // Moved, the same formulas keep what was found of them.
        let moved: Vec<_> = formulas.iter().map(|(a, t)| (a + 5, *t)).collect();
        let m = Numbers::of(&n, &moved);
        assert!(Rc::ptr_eq(&m.found, &n.found));
        assert_eq!(m.first_at(moved[2].0), 4);
    }
}
