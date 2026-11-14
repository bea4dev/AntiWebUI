//! What RaTeX doesn't read, taken out of a formula before it is: `\label{…}`, which says what
//! the number of its line is called, and `\ref{…}` and `\eqref{…}`, which show such a number.

/// A formula with its labels taken out and its references written as what they show.
#[derive(Debug, PartialEq)]
pub(crate) struct Prepared {
    pub tex: String,
    /// Each label, with the line it is on: the line of the outermost environment, counted by
    /// `\\`, or 0 outside of one.
    pub labels: Vec<(String, usize)>,
    /// The label of each reference, in the order of the colors that mark them.
    pub refs: Vec<String>,
}

/// The color of the reference `k` while it is laid out, by which it is found after: no one writes
/// these, and there are no more than 256 references in a formula that keep them.
pub(crate) fn mark(k: usize) -> String {
    format!("#7a0e{:02x}", k.min(255))
}

/// Which reference a color marks.
pub(crate) fn marked(r: f32, g: f32, b: f32) -> Option<usize> {
    let byte = |v: f32| (v * 255.0).round() as i32;
    (byte(r) == 0x7a && byte(g) == 0x0e).then(|| byte(b) as usize)
}

/// Takes out what RaTeX doesn't read. `number` tells what the line called `label` is numbered,
/// as `\ref` shows it.
pub(crate) fn prepare(tex: &str, number: &dyn Fn(&str) -> Option<String>) -> Prepared {
    // Most formulas have none of these.
    if !tex.contains("\\label") && !tex.contains("ref") {
        return Prepared {
            tex: tex.to_owned(),
            labels: Vec::new(),
            refs: Vec::new(),
        };
    }
    let mut out = String::with_capacity(tex.len());
    let mut labels = Vec::new();
    let mut refs: Vec<String> = Vec::new();
    let (mut depth, mut line) = (0usize, 0usize);
    let mut rest = tex;
    while let Some(c) = rest.chars().next() {
        if c == '%' {
            // A comment, to the end of the line.
            let end = rest.find('\n').unwrap_or(rest.len());
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        }
        if c != '\\' {
            out.push(c);
            rest = &rest[c.len_utf8()..];
            continue;
        }
        let after = &rest[1..];
        let len = match after.chars().next() {
            Some(n) if n.is_ascii_alphabetic() => after
                .find(|n: char| !n.is_ascii_alphabetic())
                .unwrap_or(after.len()),
            Some(n) => n.len_utf8(),
            None => 0,
        };
        let name = &after[..len];
        let command = &rest[..1 + len];
        rest = &after[len..];
        match name {
            "begin" => {
                depth += 1;
                if depth == 1 {
                    line = 0;
                }
                out.push_str(command);
            }
            "end" => {
                depth = depth.saturating_sub(1);
                out.push_str(command);
            }
            "\\" | "cr" | "newline" => {
                if depth == 1 {
                    line += 1;
                }
                out.push_str(command);
            }
            "label" | "ref" | "eqref" => {
                let Some((key, after)) = argument(rest) else {
                    out.push_str(command);
                    continue;
                };
                rest = after;
                if name == "label" {
                    labels.push((key.to_owned(), line));
                    continue;
                }
                // What is shown is upright text, in the color that finds it again.
                let shown = number(key).unwrap_or_else(|| "??".into());
                let shown: String = shown
                    .chars()
                    .filter(|c| !matches!(c, '\\' | '{' | '}' | '$' | '%' | '#' | '&'))
                    .collect();
                let shown = if name == "eqref" {
                    format!("({shown})")
                } else {
                    shown
                };
                out.push_str(&format!(
                    "\\textcolor{{{}}}{{\\text{{{shown}}}}}",
                    mark(refs.len())
                ));
                refs.push(key.to_owned());
            }
            _ => out.push_str(command),
        }
    }
    Prepared {
        tex: out,
        labels,
        refs,
    }
}

/// The text of a `{…}` after spaces, and what is after it.
fn argument(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start();
    let s = s.strip_prefix('{')?;
    let end = s.find('}')?;
    Some((s[..end].trim(), &s[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_taken_out_with_their_lines() {
        let p = prepare(
            r"\begin{align} a &= b \label{one} \\ c &= \begin{matrix} 1 \\ 2 \end{matrix} \\ e \label{three} \end{align}",
            &|_| None,
        );
        assert_eq!(p.labels, vec![("one".into(), 0), ("three".into(), 2)]);
        assert!(!p.tex.contains("label"));
        assert!(p.refs.is_empty());
    }

    #[test]
    fn references_show_their_numbers_in_a_color_of_their_own() {
        let n = |k: &str| (k == "e").then(|| "3".to_owned());
        let p = prepare(r"x \eqref{e} + \ref{e} + \ref{none}", &n);
        assert_eq!(
            p.tex,
            format!(
                r"x \textcolor{{{}}}{{\text{{(3)}}}} + \textcolor{{{}}}{{\text{{3}}}} + \textcolor{{{}}}{{\text{{??}}}}",
                mark(0),
                mark(1),
                mark(2)
            )
        );
        assert_eq!(p.refs, vec!["e", "e", "none"]);
        assert_eq!(
            marked(0x7a as f32 / 255.0, 0x0e as f32 / 255.0, 2.0 / 255.0),
            Some(2)
        );
    }

    #[test]
    fn what_has_none_stays_as_it_is() {
        let t = r"\frac{a}{b} \\ \text{refers}";
        assert_eq!(prepare(t, &|_| None).tex, t);
    }
}
