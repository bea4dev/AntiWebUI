//! Writing PDF: objects and what gets added to the end of a file. `hayro` only reads, and all that
//! edits a document here comes through this. See "フォームと注釈" in `docs/design/26-pdf.md`.

use hayro::hayro_syntax::object::{Dict, MaybeRef, ObjRef, Object};
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// How deep objects within objects go. A document can only break this by being broken.
const MOST_DEPTH: usize = 64;
/// How far from the end the last `startxref` can be.
const TAIL: usize = 4096;
/// How far into a file its header can be, with what comes before it not counting for the offsets.
const HEADER: usize = 1024;

/// An object of a PDF.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(Vec<u8>),
    Str(Vec<u8>),
    Array(Vec<Value>),
    Dict(Entries),
    Ref(ObjRef),
}

impl Value {
    pub fn name(name: impl AsRef<[u8]>) -> Self {
        Self::Name(name.as_ref().to_vec())
    }

    pub fn str(bytes: impl AsRef<[u8]>) -> Self {
        Self::Str(bytes.as_ref().to_vec())
    }

    /// A text string: ASCII as it is, as that means the same in every encoding, and UTF-16 for the
    /// rest.
    pub fn text(text: &str) -> Self {
        if text.chars().all(|c| c.is_ascii() && c != '\0') {
            return Self::Str(text.as_bytes().to_vec());
        }
        let mut bytes = vec![0xfe, 0xff];
        for unit in text.encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        Self::Str(bytes)
    }

    pub fn reals(numbers: impl IntoIterator<Item = f64>) -> Self {
        Self::Array(numbers.into_iter().map(Self::Real).collect())
    }

    /// An object as it was read, with what it refers to left as references.
    pub fn of(object: &MaybeRef<Object<'_>>) -> Self {
        Self::read(object, MOST_DEPTH)
    }

    fn read(object: &MaybeRef<Object<'_>>, depth: usize) -> Self {
        match object {
            MaybeRef::Ref(r) => Self::Ref(*r),
            MaybeRef::NotRef(object) if depth > 0 => Self::of_object(object, depth - 1),
            MaybeRef::NotRef(_) => Self::Null,
        }
    }

    fn of_object(object: &Object<'_>, depth: usize) -> Self {
        match object {
            Object::Boolean(b) => Self::Bool(*b),
            Object::Number(n) => {
                let real = n.as_f64();
                if real.fract() == 0.0 && real.abs() < 1e15 {
                    Self::Int(real as i64)
                } else {
                    Self::Real(real)
                }
            }
            Object::String(s) => Self::Str(s.as_bytes().to_vec()),
            Object::Name(n) => Self::Name(n.as_ref().to_vec()),
            Object::Dict(d) => Self::Dict(Entries::read(d, depth)),
            Object::Array(a) => {
                Self::Array(a.raw_iter().map(|item| Self::read(&item, depth)).collect())
            }
            // A stream can only be an object by itself, so it can't get here.
            _ => Self::Null,
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Null => out.extend_from_slice(b"null"),
            Self::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
            Self::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
            Self::Real(r) => out.extend_from_slice(number(*r).as_bytes()),
            Self::Name(name) => write_name(name, out),
            Self::Str(bytes) => {
                out.push(b'<');
                for b in bytes {
                    out.extend_from_slice(format!("{b:02x}").as_bytes());
                }
                out.push(b'>');
            }
            Self::Array(items) => {
                out.push(b'[');
                for (i, item) in items.iter().enumerate() {
                    if i > 0 {
                        out.push(b' ');
                    }
                    item.write(out);
                }
                out.push(b']');
            }
            Self::Dict(entries) => entries.write(out),
            Self::Ref(r) => out.extend_from_slice(r.to_string().as_bytes()),
        }
    }
}

/// A number as a PDF writes it, which can't be in the notation with an exponent.
pub(crate) fn number(n: f64) -> String {
    let n = if n.is_finite() { n } else { 0.0 };
    let mut text = format!("{n:.4}");
    if text.contains('.') {
        let kept = text.trim_end_matches('0').trim_end_matches('.').len();
        text.truncate(kept);
    }
    if text == "-0" { "0".to_owned() } else { text }
}

fn write_name(name: &[u8], out: &mut Vec<u8>) {
    out.push(b'/');
    for &b in name {
        let regular = b.is_ascii_graphic()
            && !matches!(
                b,
                b'#' | b'/' | b'%' | b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}'
            );
        if regular {
            out.push(b);
        } else {
            out.extend_from_slice(format!("#{b:02x}").as_bytes());
        }
    }
}

/// What is in a dictionary, in the order it gets written.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Entries(Vec<(Vec<u8>, Value)>);

impl Entries {
    pub fn new() -> Self {
        Self::default()
    }

    /// What a dictionary has, with what it refers to left as references.
    pub fn of(dict: &Dict<'_>) -> Self {
        Self::read(dict, MOST_DEPTH)
    }

    fn read(dict: &Dict<'_>, depth: usize) -> Self {
        Self(
            dict.entries()
                .map(|(key, value)| (key.as_ref().to_vec(), Value::read(&value, depth)))
                .collect(),
        )
    }

    /// Adds something for a key, instead of what it had.
    pub fn set(&mut self, key: impl AsRef<[u8]>, value: Value) {
        let key = key.as_ref();
        match self.0.iter_mut().find(|(k, _)| k == key) {
            Some(entry) => entry.1 = value,
            None => self.0.push((key.to_vec(), value)),
        }
    }

    pub fn with(mut self, key: impl AsRef<[u8]>, value: Value) -> Self {
        self.set(key, value);
        self
    }

    pub fn remove(&mut self, key: impl AsRef<[u8]>) {
        let key = key.as_ref();
        self.0.retain(|(k, _)| k != key);
    }

    #[cfg(test)]
    pub fn get(&self, key: impl AsRef<[u8]>) -> Option<&Value> {
        let key = key.as_ref();
        self.0.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"<<");
        for (key, value) in &self.0 {
            write_name(key, out);
            out.push(b' ');
            value.write(out);
            out.push(b' ');
        }
        out.extend_from_slice(b">>");
    }
}

/// What the last part of a file says, which an update goes on from.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Tail {
    /// The next number for an object.
    pub size: u32,
    /// Where the last section of the index is.
    prev: usize,
    /// It is a stream rather than a table, which is what the next one has to be too.
    stream: bool,
    /// How far into the file the offsets start.
    base: usize,
    pub encrypted: bool,
    /// What goes on in each trailer as it is: `/Root`, `/Info` and `/ID`.
    kept: Vec<(&'static [u8], Vec<u8>)>,
}

impl Tail {
    /// Reads the end of a file. It can't be read if the file is broken in such a way that `hayro`
    /// repaired it, and then there is nothing to go on from.
    pub fn read(data: &[u8]) -> Option<Self> {
        let from = data.len().saturating_sub(TAIL);
        let at = from + rfind(&data[from..], b"startxref")?;
        let (prev, _) = read_int(data, skip_space(data, at + b"startxref".len()))?;
        let prev = usize::try_from(prev).ok()?;
        let header = find(&data[..data.len().min(HEADER)], b"%PDF-")?;
        [0, header]
            .into_iter()
            .find_map(|base| Self::read_section(data, base, prev))
    }

    fn read_section(data: &[u8], base: usize, prev: usize) -> Option<Self> {
        let start = skip_space(data, base.checked_add(prev)?);
        let rest = data.get(start..)?;
        let (dict, stream) = if rest.starts_with(b"xref") {
            let trailer = find(rest, b"trailer")?;
            (&rest[trailer + b"trailer".len()..], false)
        } else {
            let (_, after) = read_int(rest, 0)?;
            let (_, after) = read_int(rest, skip_space(rest, after))?;
            let after = skip_space(rest, after);
            if !rest[after..].starts_with(b"obj") {
                return None;
            }
            (&rest[after + 3..], true)
        };
        let entries = scan_dict(dict)?;
        let get = |key: &[u8]| {
            entries
                .iter()
                .find(|(k, _)| k.as_slice() == key)
                .map(|(_, v)| *v)
        };
        let (size, _) = read_int(get(b"Size")?, 0)?;
        let root = get(b"Root")?;
        if stream && get(b"Type").is_none_or(|t| !t.starts_with(b"/XRef")) {
            return None;
        }
        let mut kept: Vec<(&'static [u8], Vec<u8>)> = vec![(b"Root", root.to_vec())];
        for key in [b"Info" as &'static [u8], b"ID"] {
            if let Some(value) = get(key) {
                kept.push((key, value.to_vec()));
            }
        }
        Some(Self {
            size: u32::try_from(size).ok()?,
            prev,
            stream,
            base,
            encrypted: get(b"Encrypt").is_some(),
            kept,
        })
    }
}

fn find(data: &[u8], what: &[u8]) -> Option<usize> {
    data.windows(what.len()).position(|w| w == what)
}

fn rfind(data: &[u8], what: &[u8]) -> Option<usize> {
    data.windows(what.len()).rposition(|w| w == what)
}

fn skip_space(data: &[u8], mut at: usize) -> usize {
    loop {
        match data.get(at) {
            Some(0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ') => at += 1,
            Some(b'%') => {
                while !matches!(data.get(at), None | Some(b'\n' | b'\r')) {
                    at += 1;
                }
            }
            _ => return at,
        }
    }
}

fn read_int(data: &[u8], at: usize) -> Option<(i64, usize)> {
    let digits = data
        .get(at..)?
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let text = std::str::from_utf8(&data[at..at + digits]).ok()?;
    Some((text.parse().ok()?, at + digits))
}

/// The keys of the dictionary at the start of `data` and their values as they are written.
fn scan_dict(data: &[u8]) -> Option<Vec<(Vec<u8>, &[u8])>> {
    let mut at = skip_space(data, 0);
    if !data.get(at..)?.starts_with(b"<<") {
        return None;
    }
    at += 2;
    let mut entries = Vec::new();
    loop {
        at = skip_space(data, at);
        if data.get(at..)?.starts_with(b">>") {
            return Some(entries);
        }
        let key = skip_value(data, at, 0)?;
        let name = data[at..key].strip_prefix(b"/")?.to_vec();
        let start = skip_space(data, key);
        let end = skip_value(data, start, 0)?;
        // A reference is three things: the object, its generation and an `R`.
        let end = reference_end(data, start, end).unwrap_or(end);
        entries.push((name, &data[start..end]));
        at = end;
    }
}

fn reference_end(data: &[u8], start: usize, end: usize) -> Option<usize> {
    read_int(data, start)?;
    let second = skip_space(data, end);
    let (_, after) = read_int(data, second).filter(|(_, after)| *after > second)?;
    let r = skip_space(data, after);
    (data.get(r) == Some(&b'R')).then_some(r + 1)
}

/// Where the value at `at` ends.
fn skip_value(data: &[u8], at: usize, depth: usize) -> Option<usize> {
    if depth > MOST_DEPTH {
        return None;
    }
    let rest = data.get(at..)?;
    match *rest.first()? {
        b'<' if rest.starts_with(b"<<") => {
            let mut at = at + 2;
            loop {
                at = skip_space(data, at);
                if data.get(at..)?.starts_with(b">>") {
                    return Some(at + 2);
                }
                at = skip_value(data, at, depth + 1)?;
            }
        }
        b'<' => Some(at + find(rest, b">")? + 1),
        b'[' => {
            let mut at = at + 1;
            loop {
                at = skip_space(data, at);
                if data.get(at) == Some(&b']') {
                    return Some(at + 1);
                }
                at = skip_value(data, at, depth + 1)?;
            }
        }
        b'(' => {
            let (mut open, mut i) = (0usize, 0);
            loop {
                match *rest.get(i)? {
                    b'\\' => i += 1,
                    b'(' => open += 1,
                    b')' => {
                        open -= 1;
                        if open == 0 {
                            return Some(at + i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
        }
        _ => {
            let length = rest[1..]
                .iter()
                .position(|b| {
                    matches!(
                        b,
                        0 | b'\t'
                            | b'\n'
                            | 0x0c
                            | b'\r'
                            | b' '
                            | b'/'
                            | b'<'
                            | b'>'
                            | b'['
                            | b']'
                            | b'('
                            | b')'
                            | b'%'
                    )
                })
                .map_or(rest.len(), |p| p + 1);
            Some(at + length)
        }
    }
}

/// What gets added to the end of a file.
pub(crate) struct Update {
    tail: Tail,
    next: u32,
    /// The objects by number, with their generation and what they are.
    objects: BTreeMap<u32, (i32, Vec<u8>)>,
}

impl Update {
    pub fn new(tail: Tail) -> Self {
        let next = tail.size.max(1);
        Self {
            tail,
            next,
            objects: BTreeMap::new(),
        }
    }

    /// The next object's reference, for objects that say where each other are.
    pub fn reserve(&mut self) -> ObjRef {
        let number = self.next;
        self.next += 1;
        ObjRef::new(number, 0)
    }

    pub fn add(&mut self, value: &Value) -> ObjRef {
        let at = self.reserve();
        self.put(at, value);
        at
    }

    /// Writes an object, in place of what it was if it was something.
    pub fn put(&mut self, at: ObjRef, value: &Value) {
        let mut body = Vec::new();
        value.write(&mut body);
        self.objects.insert(at.obj_number, (at.gen_number, body));
    }

    /// Adds a stream with a dictionary saying what it is. The length is added to that.
    pub fn stream(&mut self, dict: Entries, data: &[u8]) -> ObjRef {
        let at = self.reserve();
        self.put_stream(at, dict, data);
        at
    }

    pub fn put_stream(&mut self, at: ObjRef, dict: Entries, data: &[u8]) {
        let dict = dict.with(b"Length", Value::Int(data.len() as i64));
        let mut body = Vec::new();
        dict.write(&mut body);
        body.extend_from_slice(b"\nstream\n");
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.objects.insert(at.obj_number, (at.gen_number, body));
    }

    /// The bytes that go at the end of a file of `length` bytes.
    pub fn finish(mut self, length: usize) -> Vec<u8> {
        let mut out = vec![b'\n'];
        let index_number = self.tail.stream.then(|| self.reserve());
        let mut offsets: BTreeMap<u32, (usize, i32)> = BTreeMap::new();
        for (number, (generation, body)) in &self.objects {
            offsets.insert(*number, (length + out.len() - self.tail.base, *generation));
            out.extend_from_slice(format!("{number} {generation} obj\n").as_bytes());
            out.extend_from_slice(body);
            out.extend_from_slice(b"\nendobj\n");
        }
        let start = length + out.len() - self.tail.base;
        let size = self.next.max(self.tail.size);
        match index_number {
            Some(at) => {
                offsets.insert(at.obj_number, (start, 0));
                self.index_stream(&mut out, at, &offsets, size);
            }
            None => self.index_table(&mut out, &offsets, size),
        }
        out.extend_from_slice(format!("startxref\n{start}\n%%EOF\n").as_bytes());
        out
    }

    fn index_table(&self, out: &mut Vec<u8>, offsets: &BTreeMap<u32, (usize, i32)>, size: u32) {
        out.extend_from_slice(b"xref\n");
        for run in runs(offsets) {
            out.extend_from_slice(format!("{} {}\n", run.0, run.1).as_bytes());
            for number in run.0..run.0 + run.1 {
                let (offset, generation) = offsets[&number];
                out.extend_from_slice(format!("{offset:010} {generation:05} n \n").as_bytes());
            }
        }
        out.extend_from_slice(b"trailer\n<<");
        self.write_kept(out, size);
        out.extend_from_slice(b">>\n");
    }

    fn index_stream(
        &self,
        out: &mut Vec<u8>,
        at: ObjRef,
        offsets: &BTreeMap<u32, (usize, i32)>,
        size: u32,
    ) {
        let mut text = String::new();
        let mut index = Vec::new();
        for run in runs(offsets) {
            write!(text, "{} {} ", run.0, run.1).expect("a string takes anything");
            for number in run.0..run.0 + run.1 {
                let (offset, generation) = offsets[&number];
                index.push(1u8);
                index.extend_from_slice(&(offset as u64).to_be_bytes()[4..]);
                index.extend_from_slice(&(generation as u16).to_be_bytes());
            }
        }
        out.extend_from_slice(format!("{} 0 obj\n<<", at.obj_number).as_bytes());
        out.extend_from_slice(b"/Type /XRef /W [1 4 2] /Index [");
        out.extend_from_slice(text.trim_end().as_bytes());
        out.extend_from_slice(format!("] /Length {} ", index.len()).as_bytes());
        self.write_kept(out, size);
        out.extend_from_slice(b">>\nstream\n");
        out.extend_from_slice(&index);
        out.extend_from_slice(b"\nendstream\nendobj\n");
    }

    fn write_kept(&self, out: &mut Vec<u8>, size: u32) {
        out.extend_from_slice(format!("/Size {size} /Prev {} ", self.tail.prev).as_bytes());
        for (key, value) in &self.tail.kept {
            out.push(b'/');
            out.extend_from_slice(key);
            out.push(b' ');
            out.extend_from_slice(value);
            out.push(b' ');
        }
    }
}

/// The numbers in `offsets` as runs without gaps: the first of each and how many there are.
fn runs(offsets: &BTreeMap<u32, (usize, i32)>) -> Vec<(u32, u32)> {
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for &number in offsets.keys() {
        match runs.last_mut() {
            Some((first, count)) if *first + *count == number => *count += 1,
            _ => runs.push((number, 1)),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use hayro::hayro_syntax::Pdf;
    use hayro::hayro_syntax::object::ObjectIdentifier;
    use hayro::hayro_syntax::object::dict::keys::{KIDS, PAGES};

    fn written(value: &Value) -> String {
        let mut out = Vec::new();
        value.write(&mut out);
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn numbers_never_use_an_exponent() {
        assert_eq!(number(1.5), "1.5");
        assert_eq!(number(2.0), "2");
        assert_eq!(number(-0.00001), "0");
        assert_eq!(number(1e-7), "0");
        assert_eq!(number(0.12345), "0.1235");
        assert_eq!(number(f64::NAN), "0");
        assert_eq!(number(1e7), "10000000");
    }

    #[test]
    fn names_and_strings_are_escaped() {
        assert_eq!(written(&Value::name("A B#")), "/A#20B#23");
        assert_eq!(written(&Value::str(b"a(")), "<6128>");
        assert_eq!(
            written(&Value::Array(vec![
                Value::Ref(ObjRef::new(3, 1)),
                Value::Null,
                Value::Bool(true)
            ])),
            "[3 1 R null true]"
        );
    }

    #[test]
    fn text_is_utf16_unless_it_is_ascii() {
        assert_eq!(Value::text("ab"), Value::Str(b"ab".to_vec()));
        assert_eq!(Value::text("日"), Value::Str(vec![0xfe, 0xff, 0x65, 0xe5]));
    }

    #[test]
    fn a_dictionary_is_scanned_with_its_references_and_strings() {
        let scanned =
            scan_dict(b"<</Size 12/Root 3 0 R /ID [<aa> <bb>] /T (a\\)b) /X<</Y 1>>>>rest")
                .unwrap();
        let got: Vec<(String, String)> = scanned
            .iter()
            .map(|(k, v)| {
                (
                    String::from_utf8_lossy(k).into_owned(),
                    String::from_utf8_lossy(v).into_owned(),
                )
            })
            .collect();
        assert_eq!(got[0], ("Size".to_owned(), "12".to_owned()));
        assert_eq!(got[1], ("Root".to_owned(), "3 0 R".to_owned()));
        assert_eq!(got[2].1, "[<aa> <bb>]");
        assert_eq!(got[3].1, "(a\\)b)");
        assert_eq!(got[4].1, "<</Y 1>>");
    }

    /// A document with a page and a table for an index.
    fn with_table() -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for body in [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Tag (old) >>",
        ] {
            offsets.push(out.len());
            let id = offsets.len();
            out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let at = out.len();
        out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
        for offset in &offsets {
            out.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        out.extend_from_slice(
            format!("trailer\n<< /Size 4 /Root 1 0 R >>\nstartxref\n{at}\n%%EOF\n").as_bytes(),
        );
        out
    }

    /// The same with a stream for an index.
    fn with_stream() -> Vec<u8> {
        let mut out = b"%PDF-1.7\n".to_vec();
        let mut offsets = Vec::new();
        for body in [
            "<< /Type /Catalog /Pages 2 0 R >>",
            "<< /Type /Pages /Count 1 /Kids [3 0 R] >>",
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Tag (old) >>",
        ] {
            offsets.push(out.len());
            let id = offsets.len();
            out.extend_from_slice(format!("{id} 0 obj\n{body}\nendobj\n").as_bytes());
        }
        let at = out.len();
        offsets.push(at);
        let mut index = vec![0u8, 0, 0, 0, 0, 0xff, 0xff];
        for offset in &offsets {
            index.push(1);
            index.extend_from_slice(&(*offset as u32).to_be_bytes());
            index.extend_from_slice(&[0, 0]);
        }
        out.extend_from_slice(
            format!(
                "4 0 obj\n<< /Type /XRef /Size 5 /W [1 4 2] /Root 1 0 R /Length {} >>\nstream\n",
                index.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&index);
        out.extend_from_slice(format!("\nendstream\nendobj\nstartxref\n{at}\n%%EOF\n").as_bytes());
        out
    }

    fn changes_a_page_and_adds_an_object(data: Vec<u8>, stream_at_end: bool) {
        let tail = Tail::read(&data).unwrap();
        let next = tail.size;
        let pdf = Pdf::new(data.clone()).unwrap();
        let page = pdf.pages()[0].raw().clone();
        let id = page.obj_id().unwrap();
        let mut entries = Entries::of(&page);
        entries.set(b"Tag", Value::str(b"new"));
        let mut update = Update::new(tail);
        update.put(id.into(), &Value::Dict(entries));
        let added = update.add(&Value::Dict(Entries::new().with(b"Mark", Value::Int(7))));
        assert_eq!(added.obj_number, next);
        let mut all = data.clone();
        let tail_bytes = update.finish(data.len());
        assert_eq!(
            tail_bytes.windows(4).any(|w| w == b"xref")
                && !tail_bytes.windows(8).any(|w| w == b"/XRef /W"),
            !stream_at_end
        );
        all.extend_from_slice(&tail_bytes);

        let pdf = Pdf::new(all.clone()).unwrap();
        let page = pdf.pages()[0].raw().clone();
        assert_eq!(
            page.get::<hayro::hayro_syntax::object::String<'_>>(b"Tag" as &[u8])
                .unwrap()
                .as_bytes(),
            b"new"
        );
        let id = ObjectIdentifier::new(added.obj_number, 0);
        let got = pdf.xref().get::<Dict<'_>>(id).unwrap();
        assert_eq!(got.get::<i64>(b"Mark" as &[u8]), Some(7));
        let catalog = pdf.xref().get::<Dict<'_>>(pdf.xref().root_id()).unwrap();
        assert!(catalog.get::<Dict<'_>>(PAGES).unwrap().contains_key(KIDS));
        // The next update goes on from this one.
        assert_eq!(Tail::read(&all).unwrap().stream, stream_at_end);
    }

    #[test]
    fn an_update_of_a_file_with_a_table_ends_in_a_table() {
        changes_a_page_and_adds_an_object(with_table(), false);
    }

    #[test]
    fn an_update_of_a_file_with_a_stream_ends_in_a_stream() {
        changes_a_page_and_adds_an_object(with_stream(), true);
    }

    #[test]
    fn what_comes_before_the_header_does_not_count() {
        let mut data = b"junk\n".to_vec();
        data.extend_from_slice(&with_table());
        let tail = Tail::read(&data).unwrap();
        assert_eq!(tail.base, 5);
        let mut update = Update::new(tail);
        update.add(&Value::Dict(Entries::new().with(b"Mark", Value::Int(7))));
        let added = update.finish(data.len());
        let text = String::from_utf8_lossy(&added);
        let first: usize = text.split("xref\n").nth(1).unwrap().lines().nth(1).unwrap()[..10]
            .parse()
            .unwrap();
        assert_eq!(first + 5, data.len() + 1);
    }

    #[test]
    fn what_says_it_is_encrypted_says_so() {
        let data = b"%PDF-1.7\nxref\n0 0\ntrailer\n<< /Size 4 /Root 1 0 R /Encrypt 9 0 R /ID [<aa><bb>] >>\nstartxref\n9\n%%EOF\n";
        let tail = Tail::read(data).unwrap();
        assert!(tail.encrypted);
        assert_eq!(tail.size, 4);
        assert_eq!(tail.kept[1].1, b"[<aa><bb>]");
    }

    #[test]
    fn a_file_without_a_usable_end_says_so() {
        assert!(Tail::read(b"%PDF-1.7\nnothing here").is_none());
        assert!(Tail::read(b"%PDF-1.7\nstartxref\n999\n%%EOF").is_none());
    }
}
