//! Finding in the text: plain text, with or without case, or a regular expression.

use std::ops::Range;

/// No more matches than this are found, so that a search for `e` in a big file stays quick.
pub(crate) const MOST_MATCHES: usize = 100_000;

/// What to find.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Query {
    pub text: String,
    pub case: bool,
    pub regex: bool,
}

/// Where the query is found, in order. `Err` when the regular expression doesn't compile.
pub(crate) fn find_all(text: &str, q: &Query) -> Result<Vec<Range<usize>>, String> {
    if q.text.is_empty() {
        return Ok(Vec::new());
    }
    if q.case && !q.regex {
        let n = q.text.len();
        return Ok(
            memchr::memmem::find_iter(text.as_bytes(), q.text.as_bytes())
                .take(MOST_MATCHES)
                .map(|i| i..i + n)
                .collect(),
        );
    }
    let pattern = if q.regex {
        q.text.clone()
    } else {
        regex::escape(&q.text)
    };
    let re = regex::RegexBuilder::new(&pattern)
        .case_insensitive(!q.case)
        .multi_line(true)
        .build()
        .map_err(|e| e.to_string())?;
    Ok(re
        .find_iter(text)
        .filter(|m| !m.is_empty())
        .take(MOST_MATCHES)
        .map(|m| m.range())
        .collect())
}

/// What a match is replaced with: the replacement, with `$1` and `${name}` filled in for a regular
/// expression.
pub(crate) fn replacement(text: &str, found: Range<usize>, q: &Query, with: &str) -> String {
    if !q.regex {
        return with.to_owned();
    }
    let Ok(re) = regex::RegexBuilder::new(&q.text)
        .case_insensitive(!q.case)
        .multi_line(true)
        .build()
    else {
        return with.to_owned();
    };
    match re.captures_at(text, found.start) {
        Some(c) if c.get(0).is_some_and(|m| m.range() == found) => {
            let mut out = String::new();
            c.expand(with, &mut out);
            out
        }
        _ => with.to_owned(),
    }
}

/// The match at or after `at`, or the first one.
pub(crate) fn next_from(matches: &[Range<usize>], at: usize) -> Option<usize> {
    if matches.is_empty() {
        return None;
    }
    let i = matches.partition_point(|m| m.start < at);
    Some(if i < matches.len() { i } else { 0 })
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn q(text: &str, case: bool, regex: bool) -> Query {
        Query {
            text: text.into(),
            case,
            regex,
        }
    }

    #[test]
    fn plain_text_with_and_without_case() {
        let t = "Foo foo FOO";
        assert_eq!(find_all(t, &q("foo", true, false)).unwrap(), [4..7]);
        assert_eq!(find_all(t, &q("foo", false, false)).unwrap().len(), 3);
        // What would mean something in a regular expression means nothing here.
        assert_eq!(find_all("a.b axb", &q("a.b", true, false)).unwrap(), [0..3]);
    }

    #[test]
    fn regular_expressions_and_their_groups() {
        let t = "let a = 1;\nlet bc = 22;";
        let found = find_all(t, &q(r"let (\w+) = (\d+)", true, true)).unwrap();
        assert_eq!(found.len(), 2);
        assert_eq!(
            replacement(
                t,
                found[1].clone(),
                &q(r"let (\w+) = (\d+)", true, true),
                "$1: $2"
            ),
            "bc: 22"
        );
        assert!(find_all(t, &q("(", true, true)).is_err());
    }

    #[test]
    fn the_next_match_goes_round() {
        let m = [1..2, 5..6];
        assert_eq!(next_from(&m, 0), Some(0));
        assert_eq!(next_from(&m, 3), Some(1));
        assert_eq!(next_from(&m, 9), Some(0));
    }
}
