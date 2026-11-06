//! The thread the scripts run on: the engine, the API of Acrobat in `api.js`, timers and dialogs.
//! Whatever a script does, it can only stop this thread for as long as each event may take.

use super::json::Json;
use super::quickjs::Js;
use super::{
    Ask, Command, Effect, Message, Mouse, PdfAnswer, PdfButtons, PdfDialog, PdfDialogKind, Reply,
    Shared, Start,
};
use crate::edits::Display;
use crate::marks::{Field, Marks, PdfFieldKind, PdfFieldValue};
use anti_webui_core::channel::{UiSender, UiWork};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const API: &str = include_str!("api.js");
/// How much memory the scripts of a document can take.
const MEMORY: usize = 64 << 20;
/// How long the scripts can take for one event, not counting waiting for a dialog.
const TIME: Duration = Duration::from_secs(2);
/// The stack of the thread, which has room for the engine's own limit and what calls it.
const THREAD_STACK: usize = 4 << 20;

pub(super) fn spawn(
    start: Start,
    commands: Receiver<Command>,
    shared: Arc<Shared>,
    out: UiSender<Message>,
) -> std::io::Result<()> {
    // Opening is coming to something.
    let opening = out.work();
    std::thread::Builder::new()
        .name("pdf-scripts".to_owned())
        .stack_size(THREAD_STACK)
        .spawn(move || {
            if let Err(e) = run(start, commands, &shared, &out, opening) {
                log::warn!("the scripts of a PDF stopped: {e}");
            }
        })
        .map(|_| ())
}

fn run(
    start: Start,
    commands: Receiver<Command>,
    shared: &Arc<Shared>,
    out: &UiSender<Message>,
    opening: UiWork,
) -> Result<(), String> {
    let commands = Rc::new(commands);
    // What says an answer is coming, which waiting for a person doesn't.
    let working = Rc::new(RefCell::new(Some(opening)));
    // What came while a dialog was up, which goes after it.
    let later: Rc<RefCell<VecDeque<Command>>> = Rc::default();
    let deadline = Rc::new(Cell::new(Instant::now() + TIME));
    let ended = Rc::new(Cell::new(false));

    let stop = {
        let deadline = deadline.clone();
        move || Instant::now() > deadline.get()
    };
    let ask = {
        let (commands, later, deadline, ended, working) = (
            commands.clone(),
            later.clone(),
            deadline.clone(),
            ended.clone(),
            working.clone(),
        );
        let (shared, out) = (shared.clone(), out.clone());
        move |request: String| -> String {
            let Some(dialog) = Json::parse(&request).ok().as_ref().and_then(dialog_of) else {
                return "null".to_owned();
            };
            let question = matches!(dialog.kind, PdfDialogKind::Question { .. });
            let since = Instant::now();
            working.borrow_mut().take();
            shared.set_asking(true);
            out.send(Message::Dialog(dialog));
            let answer = loop {
                match commands.recv() {
                    Ok(Command::Answer(answer)) => break Some(answer),
                    Ok(Command::Ask(ticket, ask, _)) => {
                        later
                            .borrow_mut()
                            .push_back(Command::Ask(ticket, ask, None));
                    }
                    Err(_) => {
                        ended.set(true);
                        break None;
                    }
                }
            };
            shared.set_asking(false);
            deadline.set(deadline.get() + since.elapsed());
            let json = match answer {
                Some(PdfAnswer::Text(text)) if question => Json::String(text),
                _ if question => Json::Null,
                Some(PdfAnswer::Ok) | None => Json::Number(1.0),
                Some(PdfAnswer::Cancel) => Json::Number(2.0),
                Some(PdfAnswer::No) => Json::Number(3.0),
                Some(PdfAnswer::Yes) => Json::Number(4.0),
                Some(PdfAnswer::Text(_)) => Json::Number(1.0),
            };
            json.text()
        }
    };

    let started = Instant::now();
    let js = Js::new(MEMORY, stop, ask)?;
    js.eval(API)?;
    let kinds: Vec<PdfFieldKind> = start.marks.fields.iter().map(|f| f.info.kind).collect();
    let call = |message: Json| -> (Vec<Effect>, Option<Duration>) {
        deadline.set(Instant::now() + TIME);
        match js.call("__handle", &message.text()) {
            Ok(text) => effects_of(&text, &kinds),
            Err(e) => {
                log::warn!("a script of a PDF failed: {e}");
                // What it was doing may have been left half done; the timers stay as they were.
                (Vec::new(), None)
            }
        }
    };
    let reply = |reply: Reply| {
        shared.reply(reply);
        out.send(Message::Replied);
    };

    let (effects, _) = call(init(&start));
    log::debug!(
        "the scripts of a PDF started in {:.1} ms and take {} KiB",
        started.elapsed().as_secs_f64() * 1000.0,
        js.memory() >> 10
    );
    reply((0, effects));
    let (effects, mut timer) = call(Json::object([("t", "opened".into())]));
    reply((0, effects));
    working.borrow_mut().take();
    loop {
        if ended.get() {
            return Ok(());
        }
        let next = later.borrow_mut().pop_front();
        let command = match next {
            Some(command) => command,
            None => {
                let got = match timer {
                    Some(wait) => commands.recv_timeout(wait),
                    None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
                };
                match got {
                    Ok(command) => command,
                    Err(RecvTimeoutError::Timeout) => {
                        let (effects, next) = call(Json::object([("t", "timers".into())]));
                        timer = next;
                        reply((0, effects));
                        continue;
                    }
                    Err(RecvTimeoutError::Disconnected) => return Ok(()),
                }
            }
        };
        match command {
            Command::Ask(ticket, ask, work) => {
                *working.borrow_mut() = work;
                let (effects, next) = call(ask_json(&ask));
                timer = next;
                reply((ticket, effects));
                working.borrow_mut().take();
            }
            // For a dialog that is gone.
            Command::Answer(_) => {}
        }
    }
}

fn dialog_of(request: &Json) -> Option<PdfDialog> {
    let text = |key: &str| {
        request
            .get(key)
            .and_then(Json::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    let kind = match request.get("t").and_then(Json::as_str)? {
        "alert" => PdfDialogKind::Alert(
            match request.get("buttons").and_then(Json::as_usize).unwrap_or(0) {
                1 => PdfButtons::OkCancel,
                2 => PdfButtons::YesNo,
                3 => PdfButtons::YesNoCancel,
                _ => PdfButtons::Ok,
            },
        ),
        "response" => PdfDialogKind::Question {
            default: text("default"),
            password: request
                .get("password")
                .and_then(Json::as_bool)
                .unwrap_or(false),
        },
        _ => return None,
    };
    Some(PdfDialog {
        title: text("title"),
        message: text("message"),
        kind,
    })
}

fn value_json(value: &PdfFieldValue) -> Json {
    match value {
        PdfFieldValue::Text(text) => Json::str(text.clone()),
        PdfFieldValue::State(state) => Json::str(state.clone().unwrap_or_else(|| "Off".into())),
        PdfFieldValue::Choice(picked) => {
            Json::Array(picked.iter().map(|p| Json::str(p.clone())).collect())
        }
    }
}

/// What a script says a field of a kind has.
fn value_from(json: &Json, kind: PdfFieldKind) -> Option<PdfFieldValue> {
    let text = |json: &Json| match json {
        Json::String(s) => Some(s.clone()),
        Json::Number(n) => Some(n.to_string()),
        Json::Null => Some(String::new()),
        Json::Bool(b) => Some(b.to_string()),
        _ => None,
    };
    Some(match kind {
        PdfFieldKind::Check | PdfFieldKind::Radio | PdfFieldKind::Push => {
            let state = text(json)?;
            PdfFieldValue::State((!state.is_empty() && state != "Off").then_some(state))
        }
        PdfFieldKind::Combo { .. } | PdfFieldKind::List { .. } => match json {
            Json::Array(items) => PdfFieldValue::Choice(items.iter().filter_map(text).collect()),
            other => {
                PdfFieldValue::Choice(text(other).into_iter().filter(|s| !s.is_empty()).collect())
            }
        },
        PdfFieldKind::Text | PdfFieldKind::Signature => match json {
            Json::Array(items) => {
                PdfFieldValue::Text(items.iter().filter_map(text).collect::<Vec<_>>().join(","))
            }
            other => PdfFieldValue::Text(text(other)?),
        },
    })
}

fn type_name(kind: PdfFieldKind) -> &'static str {
    match kind {
        PdfFieldKind::Text => "text",
        PdfFieldKind::Check => "checkbox",
        PdfFieldKind::Radio => "radiobutton",
        PdfFieldKind::Combo { .. } => "combobox",
        PdfFieldKind::List { .. } => "listbox",
        PdfFieldKind::Push => "button",
        PdfFieldKind::Signature => "signature",
    }
}

fn script(s: &Option<String>) -> Json {
    s.clone().into()
}

fn field_json(marks: &Marks, start: &Start, at: usize, field: &Field) -> Json {
    let edits = &start.edits;
    let value = edits.fields.get(&at).unwrap_or(&field.value);
    let read_only = edits
        .read_only
        .get(&at)
        .copied()
        .unwrap_or(field.info.read_only);
    let widgets = field
        .widgets
        .iter()
        .map(|&(page, place)| {
            let w = &marks.pages[page].widgets[place];
            let display = edits
                .display
                .get(&at)
                .copied()
                .unwrap_or_else(|| Display::of_flags(w.flags));
            let s = &w.scripts;
            Json::object([
                ("page", page.into()),
                ("state", w.on_state.clone().into()),
                (
                    "rect",
                    Json::Array(vec![
                        w.rect[0].into(),
                        w.rect[3].into(),
                        w.rect[2].into(),
                        w.rect[1].into(),
                    ]),
                ),
                ("display", (display.number() as f64).into()),
                (
                    "scripts",
                    Json::object([
                        ("down", script(&s.down)),
                        ("up", script(&s.up)),
                        ("enter", script(&s.enter)),
                        ("exit", script(&s.exit)),
                        ("focus", script(&s.focus)),
                        ("blur", script(&s.blur)),
                    ]),
                ),
            ])
        })
        .collect();
    let s = &field.scripts;
    let (editable, multiple) = match field.info.kind {
        PdfFieldKind::Combo { editable } => (editable, false),
        PdfFieldKind::List { multiple } => (false, multiple),
        _ => (false, false),
    };
    Json::object([
        ("name", field.info.name.clone().into()),
        ("type", type_name(field.info.kind).into()),
        ("value", value_json(value)),
        ("default", value_json(&field.default)),
        ("readonly", read_only.into()),
        ("required", field.info.required.into()),
        ("multiline", field.multiline().into()),
        ("password", field.password().into()),
        ("comb", field.comb().is_some().into()),
        ("charLimit", field.max_len.map(|n| n as usize).into()),
        ("editable", editable.into()),
        ("multiple", multiple.into()),
        ("shown", edits.shown.get(&at).cloned().into()),
        ("needsLook", field.needs_look.into()),
        (
            "options",
            Json::Array(
                field
                    .info
                    .options
                    .iter()
                    .map(|(v, s)| Json::Array(vec![v.clone().into(), s.clone().into()]))
                    .collect(),
            ),
        ),
        (
            "states",
            Json::Array(field.info.states.iter().map(|s| s.clone().into()).collect()),
        ),
        (
            "scripts",
            Json::object([
                ("K", script(&s.keystroke)),
                ("F", script(&s.format)),
                ("V", script(&s.validate)),
                ("C", script(&s.calculate)),
            ]),
        ),
        ("widgets", Json::Array(widgets)),
    ])
}

fn init(start: &Start) -> Json {
    let marks = &start.marks;
    let s = &marks.scripts;
    Json::object([
        ("t", "init".into()),
        ("pages", marks.pages.len().into()),
        ("page", start.page.into()),
        ("file", start.file_name.clone().into()),
        (
            "info",
            Json::Object(
                s.info
                    .iter()
                    .map(|(k, v)| ((*k).to_owned(), v.clone().into()))
                    .collect(),
            ),
        ),
        (
            "named",
            Json::Array(
                s.named
                    .iter()
                    .map(|(n, js)| Json::Array(vec![n.clone().into(), js.clone().into()]))
                    .collect(),
            ),
        ),
        ("open", script(&s.open)),
        (
            "order",
            Json::Array(s.calculation_order.iter().map(|&i| i.into()).collect()),
        ),
        (
            "pageScripts",
            Json::Array(
                s.pages
                    .iter()
                    .map(|(page, open, close)| {
                        Json::Array(vec![(*page).into(), script(open), script(close)])
                    })
                    .collect(),
            ),
        ),
        (
            "fields",
            Json::Array(
                marks
                    .fields
                    .iter()
                    .enumerate()
                    .map(|(at, field)| field_json(marks, start, at, field))
                    .collect(),
            ),
        ),
    ])
}

fn ask_json(ask: &Ask) -> Json {
    match ask {
        Ask::Keystroke {
            field,
            value,
            change,
            selection,
        } => Json::object([
            ("t", "keystroke".into()),
            ("f", (*field).into()),
            ("value", value.clone().into()),
            ("change", change.clone().into()),
            ("selStart", selection.0.into()),
            ("selEnd", selection.1.into()),
        ]),
        Ask::Commit { field, value } => Json::object([
            ("t", "commit".into()),
            ("f", (*field).into()),
            ("v", value_json(value)),
        ]),
        Ask::Mouse {
            field,
            widget,
            what,
            shift,
            modifier,
        } => Json::object([
            ("t", "mouse".into()),
            ("f", (*field).into()),
            ("w", (*widget).into()),
            (
                "name",
                match what {
                    Mouse::Down => "Mouse Down",
                    Mouse::Up => "Mouse Up",
                    Mouse::Enter => "Mouse Enter",
                    Mouse::Exit => "Mouse Exit",
                    Mouse::Focus => "Focus",
                    Mouse::Blur => "Blur",
                }
                .into(),
            ),
            ("shift", (*shift).into()),
            ("modifier", (*modifier).into()),
        ]),
        Ask::Link(js) => Json::object([("t", "link".into()), ("js", js.clone().into())]),
        Ask::Page { page, open } => Json::object([
            ("t", "page".into()),
            ("n", (*page).into()),
            ("open", (*open).into()),
        ]),
        Ask::Sync {
            values,
            display,
            read_only,
            calculate,
        } => Json::object([
            ("t", "sync".into()),
            (
                "values",
                Json::Array(
                    values
                        .iter()
                        .map(|(f, v)| Json::Array(vec![(*f).into(), value_json(v)]))
                        .collect(),
                ),
            ),
            (
                "display",
                Json::Array(
                    display
                        .iter()
                        .map(|(f, d)| Json::Array(vec![(*f).into(), (d.number() as f64).into()]))
                        .collect(),
                ),
            ),
            (
                "readonly",
                Json::Array(
                    read_only
                        .iter()
                        .map(|(f, b)| Json::Array(vec![(*f).into(), (*b).into()]))
                        .collect(),
                ),
            ),
            ("calculate", (*calculate).into()),
        ]),
    }
}

/// What a call of `__handle` gave back: what the scripts did, and when the next timer is due.
fn effects_of(text: &str, kinds: &[PdfFieldKind]) -> (Vec<Effect>, Option<Duration>) {
    let json = match Json::parse(text) {
        Ok(json) => json,
        Err(e) => {
            log::warn!("the scripts of a PDF said something that isn't JSON: {e}");
            return (Vec::new(), None);
        }
    };
    let timer = json
        .get("timer")
        .and_then(Json::as_f64)
        .filter(|ms| *ms >= 0.0)
        .map(|ms| Duration::from_secs_f64(ms / 1000.0));
    let mut effects = Vec::new();
    for e in json.get("effects").and_then(Json::as_array).unwrap_or(&[]) {
        let field = e
            .get("f")
            .and_then(Json::as_usize)
            .filter(|f| *f < kinds.len());
        let shown = || e.get("s").and_then(Json::as_str).map(str::to_owned);
        let effect = match (e.get("t").and_then(Json::as_str), field) {
            (Some("value"), Some(field)) => e
                .get("v")
                .and_then(|v| value_from(v, kinds[field]))
                .map(|value| Effect::Value {
                    field,
                    value,
                    shown: shown(),
                }),
            (Some("refused"), Some(field)) => e
                .get("v")
                .and_then(|v| value_from(v, kinds[field]))
                .map(|value| Effect::Refused {
                    field,
                    value,
                    shown: shown(),
                }),
            (Some("display"), Some(field)) => e
                .get("d")
                .and_then(Json::as_f64)
                .and_then(|d| Display::from_number(d as i64))
                .map(|display| Effect::Display { field, display }),
            (Some("readonly"), Some(field)) => e
                .get("on")
                .and_then(Json::as_bool)
                .map(|on| Effect::ReadOnly { field, on }),
            (Some("focus"), Some(field)) => Some(Effect::Focus(field)),
            (Some("look"), Some(field)) => shown().map(|shown| Effect::Look { field, shown }),
            (Some("typed"), _) => {
                let sel = e.get("sel").and_then(Json::as_array).unwrap_or(&[]);
                let at = |i: usize| sel.get(i).and_then(Json::as_usize).unwrap_or(0);
                Some(Effect::Typed {
                    accepted: e.get("ok").and_then(Json::as_bool).unwrap_or(true),
                    text: e
                        .get("text")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                    selection: (at(0), at(1)),
                })
            }
            (Some("page"), _) => e.get("n").and_then(Json::as_usize).map(Effect::GoToPage),
            (Some("url"), _) => e
                .get("url")
                .and_then(Json::as_str)
                .map(|u| Effect::Url(u.to_owned())),
            (Some("log"), _) => {
                let text = e.get("text").and_then(Json::as_str).unwrap_or_default();
                log::info!("PDF script: {text}");
                None
            }
            _ => None,
        };
        effects.extend(effect);
    }
    (effects, timer)
}
