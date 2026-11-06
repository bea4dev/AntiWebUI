//! What the keys of a code editor do to the text, as replacements of one range, so that each is
//! one step of undo. Everything here works on the text alone.

use std::ops::Range;

/// What to replace, and where the selection goes after it (`None`: after what was put in).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Replace {
    pub range: Range<usize>,
    pub with: String,
    pub select: Option<(usize, usize)>,
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |i| at + i)
}

/// The whitespace a line starts with.
pub(crate) fn indentation(line: &str) -> &str {
    &line[..line.len() - line.trim_start_matches([' ', '\t']).len()]
}

/// The lines a selection covers: from the start of its first line to the end of its last, where a
/// selection that ends at the start of a line doesn't take that line.
pub(crate) fn covered_lines(text: &str, a: usize, b: usize) -> Range<usize> {
    let start = line_start(text, a);
    let b = if b > a && line_start(text, b) == b {
        b - 1
    } else {
        b
    };
    start..line_end(text, b.max(start))
}

const OPENERS: [(char, char); 3] = [('{', '}'), ('(', ')'), ('[', ']')];

/// Enter: the new line keeps the indentation, one step more after an opening bracket (or `:` in
/// Python), and a closing bracket right after the caret goes to a line of its own.
pub(crate) fn enter(text: &str, a: usize, b: usize, unit: &str, colon: bool) -> Replace {
    let start = line_start(text, a);
    let before = &text[start..a];
    let indent = indentation(before).to_owned();
    let last = before.trim_end().chars().last();
    let opens =
        last.is_some_and(|c| OPENERS.iter().any(|(o, _)| *o == c)) || (colon && last == Some(':'));
    let next = text[b..].chars().next();
    let closes =
        opens && next.is_some_and(|n| OPENERS.iter().any(|(o, c)| Some(*o) == last && *c == n));
    if closes {
        let inner = format!("\n{indent}{unit}");
        let with = format!("{inner}\n{indent}");
        let caret = a + inner.len();
        return Replace {
            range: a..b,
            with,
            select: Some((caret, caret)),
        };
    }
    let with = if opens {
        format!("\n{indent}{unit}")
    } else {
        format!("\n{indent}")
    };
    Replace {
        range: a..b,
        with,
        select: None,
    }
}

/// Where a byte goes through edits (where, how much removed, how much put in), in order and in
/// the coordinates of before them.
fn follow(p: usize, edits: &[(usize, usize, usize)]) -> usize {
    let mut q = p as isize;
    for &(at, removed, inserted) in edits {
        if p >= at + removed {
            q += inserted as isize - removed as isize;
        } else if p > at {
            q -= (p - at) as isize;
        } else if p == at && removed == 0 {
            q += inserted as isize;
        }
    }
    q.max(0) as usize
}

/// Puts the edits into the lines, as one replacement with the selection following.
fn edit_lines(
    text: &str,
    lines: Range<usize>,
    edits: Vec<(usize, usize, String)>,
    a: usize,
    b: usize,
) -> Option<Replace> {
    if edits.is_empty() {
        return None;
    }
    let mut with = String::with_capacity(lines.len() + 16);
    let mut at = lines.start;
    for (pos, removed, inserted) in &edits {
        with.push_str(&text[at..*pos]);
        with.push_str(inserted);
        at = pos + removed;
    }
    with.push_str(&text[at..lines.end]);
    let shifts: Vec<(usize, usize, usize)> =
        edits.iter().map(|(p, r, i)| (*p, *r, i.len())).collect();
    Some(Replace {
        range: lines,
        with,
        select: Some((follow(a, &shifts), follow(b, &shifts))),
    })
}

/// The start of each line of a range, with the line.
fn lines_in(text: &str, lines: Range<usize>) -> impl Iterator<Item = (usize, &str)> {
    let mut at = lines.start;
    text[lines].split('\n').map(move |l| {
        let start = at;
        at += l.len() + 1;
        (start, l)
    })
}

/// Tab and Shift+Tab over the selected lines: one step of indentation more or less on each.
pub(crate) fn indent_lines(
    text: &str,
    a: usize,
    b: usize,
    unit: &str,
    back: bool,
) -> Option<Replace> {
    let lines = covered_lines(text, a, b);
    let mut edits = Vec::new();
    for (start, line) in lines_in(text, lines.clone()) {
        if back {
            let cut = if line.starts_with('\t') {
                1
            } else {
                line.bytes()
                    .take(unit.len().max(1))
                    .take_while(|&c| c == b' ')
                    .count()
            };
            if cut > 0 {
                edits.push((start, cut, String::new()));
            }
        } else if !line.is_empty() {
            edits.push((start, 0, unit.to_owned()));
        }
    }
    edit_lines(text, lines, edits, a, b)
}

/// Ctrl+/: comments the selected lines out, or back in when all of them are comments.
pub(crate) fn toggle_comment(
    text: &str,
    a: usize,
    b: usize,
    line: Option<&str>,
    block: Option<(&str, &str)>,
) -> Option<Replace> {
    let lines = covered_lines(text, a, b);
    let body = &text[lines.clone()];
    if let Some(mark) = line {
        let filled = || lines_in(text, lines.clone()).filter(|(_, l)| !l.trim().is_empty());
        let all = filled().all(|(_, l)| l.trim_start().starts_with(mark));
        // Comments start where the least indented line does.
        let col = filled()
            .map(|(_, l)| indentation(l).len())
            .min()
            .unwrap_or(0);
        let edits = filled()
            .map(|(start, l)| {
                if all {
                    let at = start + indentation(l).len();
                    let space = text[at + mark.len()..].starts_with(' ') as usize;
                    (at, mark.len() + space, String::new())
                } else {
                    (start + col, 0, format!("{mark} "))
                }
            })
            .collect();
        return edit_lines(text, lines, edits, a, b);
    }
    let (open, close) = block?;
    let trimmed = body.trim();
    let with = if trimmed.starts_with(open)
        && trimmed.ends_with(close)
        && trimmed.len() >= open.len() + close.len()
    {
        let lead = &body[..body.len() - body.trim_start().len()];
        let inner = &trimmed[open.len()..trimmed.len() - close.len()];
        format!("{lead}{}", inner.trim())
    } else {
        let lead = indentation(body);
        format!("{lead}{open} {} {close}", &body[lead.len()..])
    };
    Some(Replace {
        range: lines,
        with,
        select: None,
    })
}

/// A closing bracket typed where the line has only indentation: one step back first.
pub(crate) fn close_bracket(text: &str, at: usize, c: char, unit: &str) -> Option<Replace> {
    if !OPENERS.iter().any(|(_, close)| *close == c) {
        return None;
    }
    let start = line_start(text, at);
    let before = &text[start..at];
    if before.is_empty() || !before.chars().all(|c| c == ' ' || c == '\t') {
        return None;
    }
    let cut = if before.ends_with('\t') {
        1
    } else {
        before.len().min(unit.len().max(1))
    };
    let with = format!("{}{c}", &before[..before.len() - cut]);
    Some(Replace {
        range: start..at,
        with,
        select: None,
    })
}

/// Backspace in the indentation: back to the step before.
pub(crate) fn backspace(text: &str, at: usize, unit: &str) -> Option<Replace> {
    let n = unit.len();
    if n < 2 || !unit.bytes().all(|c| c == b' ') {
        return None;
    }
    let start = line_start(text, at);
    let before = &text[start..at];
    if before.is_empty() || !before.bytes().all(|c| c == b' ') {
        return None;
    }
    let cut = (before.len() - 1) % n + 1;
    Some(Replace {
        range: at - cut..at,
        with: String::new(),
        select: None,
    })
}

/// The lines that fold with line `line`: those after it that are more indented than it, blank
/// lines among them included, as editors do when the language says nothing of its folds.
pub(crate) fn fold_at<'a>(
    line_text: &dyn Fn(usize) -> &'a str,
    lines: usize,
    line: usize,
) -> Option<Range<usize>> {
    let width = |l: &str| -> usize {
        indentation(l)
            .chars()
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum()
    };
    let here = line_text(line);
    if here.trim().is_empty() {
        return None;
    }
    let base = width(here);
    let mut end = line;
    for i in line + 1..lines {
        let l = line_text(i);
        if l.trim().is_empty() {
            continue;
        }
        if width(l) <= base {
            break;
        }
        end = i;
    }
    (end > line).then_some(line..end + 1)
}

/// The fold around line `line`: its own, or that of the nearest line above it that it is in.
pub(crate) fn fold_around<'a>(
    line_text: &dyn Fn(usize) -> &'a str,
    lines: usize,
    line: usize,
) -> Option<Range<usize>> {
    (0..=line)
        .rev()
        .take(5000)
        .find_map(|l| fold_at(line_text, lines, l).filter(|r| r.contains(&line)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(text: &str, r: &Replace) -> String {
        format!(
            "{}{}{}",
            &text[..r.range.start],
            r.with,
            &text[r.range.end..]
        )
    }

    #[test]
    fn enter_keeps_the_indentation_and_opens_blocks() {
        let t = "    let x = 1;";
        assert_eq!(enter(t, t.len(), t.len(), "    ", false).with, "\n    ");
        let t = "fn f() {";
        assert_eq!(enter(t, t.len(), t.len(), "    ", false).with, "\n    ");
        let t = "fn f() {}";
        let r = enter(t, 8, 8, "    ", false);
        assert_eq!(apply(t, &r), "fn f() {\n    \n}");
        assert_eq!(r.select, Some((13, 13)));
        let t = "def f():";
        assert_eq!(enter(t, t.len(), t.len(), "    ", true).with, "\n    ");
        assert_eq!(enter(t, t.len(), t.len(), "    ", false).with, "\n");
    }

    #[test]
    fn tab_moves_the_selected_lines() {
        let t = "a\n  b\nc";
        let r = indent_lines(t, 0, 5, "  ", false).unwrap();
        assert_eq!(apply(t, &r), "  a\n    b\nc");
        let t2 = apply(t, &r);
        let r = indent_lines(&t2, 0, 9, "  ", true).unwrap();
        assert_eq!(apply(&t2, &r), "a\n  b\nc");
        // A selection that ends at the start of a line doesn't take it.
        let r = indent_lines(t, 0, 2, "  ", false).unwrap();
        assert_eq!(apply(t, &r), "  a\n  b\nc");
    }

    #[test]
    fn comments_go_on_and_off() {
        let t = "  a\n    b\n\n  c";
        let r = toggle_comment(t, 0, t.len(), Some("//"), None).unwrap();
        let on = apply(t, &r);
        assert_eq!(on, "  // a\n  //   b\n\n  // c");
        let r = toggle_comment(&on, 0, on.len(), Some("//"), None).unwrap();
        assert_eq!(apply(&on, &r), t);
        let t = "<p>x</p>";
        let r = toggle_comment(t, 0, 0, None, Some(("<!--", "-->"))).unwrap();
        let on = apply(t, &r);
        assert_eq!(on, "<!-- <p>x</p> -->");
        let r = toggle_comment(&on, 0, 0, None, Some(("<!--", "-->"))).unwrap();
        assert_eq!(apply(&on, &r), t);
    }

    #[test]
    fn a_closing_bracket_steps_back() {
        let t = "{\n        ";
        let r = close_bracket(t, t.len(), '}', "    ").unwrap();
        assert_eq!(apply(t, &r), "{\n    }");
        assert_eq!(close_bracket("{\n  x", 5, '}', "    "), None);
    }

    #[test]
    fn backspace_goes_back_a_step() {
        let t = "      ";
        let r = backspace(t, 6, "    ").unwrap();
        assert_eq!(apply(t, &r), "    ");
        assert_eq!(backspace("  x", 3, "    "), None);
    }

    #[test]
    fn folds_follow_the_indentation() {
        let t = "fn a() {\n    x;\n\n    y;\n}\nfn b() {}";
        let lines: Vec<&str> = t.split('\n').collect();
        let line = |i: usize| lines[i];
        assert_eq!(fold_at(&line, lines.len(), 0), Some(0..4));
        assert_eq!(fold_at(&line, lines.len(), 5), None);
        assert_eq!(fold_around(&line, lines.len(), 3), Some(0..4));
    }
}
