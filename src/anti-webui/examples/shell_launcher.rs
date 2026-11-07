//! A launcher: a box in the middle of the output that takes the keyboard, the installed
//! applications as you type, with their icons, and Enter to start the one picked. See
//! `docs/design/37-wayland-shell.md`.
//!
//! `cargo run --release -p anti-webui --features shell-apps --example shell_launcher`

use anti_webui::prelude::*;
use anti_webui::shell::prelude::*;

/// How many results show at once.
const SHOWN: usize = 8;

fn main() {
    let result = Shell::new().namespace("anti-webui-launcher").run(|| {
        open_layer(
            LayerOptions::new()
                .layer(Layer::Overlay)
                .keyboard(Keyboard::Exclusive)
                .size(560, 480),
            launcher,
        );
    });
    if let Err(e) = result {
        eprintln!("{e}");
    }
}

fn launcher() -> Div {
    let query = signal(String::new());
    let picked = signal(0usize);
    let all = apps();
    let results = move || {
        let found = all.search(&query.get());
        found.into_iter().take(SHOWN).collect::<Vec<_>>()
    };
    // Typing starts the list over.
    effect(move || {
        query.with(|_| ());
        picked.set(0);
    });
    let start = move |app: &Application| {
        app.launch().then(|r| {
            if let Err(e) = r {
                eprintln!("couldn't start it: {e}");
            }
            quit();
        });
    };
    let input = node_ref();
    request_frame(move |_| input.focus());
    page()
        .p(16.0)
        .gap(12.0)
        .child(
            text_field(query)
                .node_ref(input)
                .placeholder("Type to search, Enter to start, Escape to close")
                .on_submit(move |_| {
                    if let Some(app) = results().get(picked.get_untracked()) {
                        start(app);
                    }
                })
                .on_key_down(move |e| match e.key {
                    Key::Named(NamedKey::Escape) => quit(),
                    Key::Named(NamedKey::ArrowDown) => {
                        let n = results().len().max(1);
                        picked.update(|p| *p = (*p + 1) % n);
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        let n = results().len().max(1);
                        picked.update(|p| *p = (*p + n - 1) % n);
                    }
                    _ => {}
                }),
        )
        .child(text(move || {
            if all.loaded() {
                String::new()
            } else {
                "Reading the applications…".into()
            }
        }))
        .child(dyn_view(move || {
            let mut list = div().flex().flex_col().gap(2.0);
            for (i, app) in results().into_iter().enumerate() {
                let icon = app.icon_path(64);
                let mut row = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(12.0)
                    .px(8.0)
                    .py(6.0)
                    .rounded(6.0);
                if i == picked.get() {
                    row = row.bg(rgb(0xdde6f5));
                }
                row = match icon {
                    Some(path) => row.child(img(path).w(32.0).h(32.0)),
                    None => row.child(div().w(32.0).h(32.0)),
                };
                let detail = app
                    .generic_name
                    .clone()
                    .or_else(|| app.comment.clone())
                    .unwrap_or_default();
                let clicked = app.clone();
                list = list.child(
                    row.child(
                        div()
                            .flex()
                            .flex_col()
                            .child(text(app.name.clone()))
                            .child(label(detail)),
                    )
                    .on_click(move |_| start(&clicked)),
                );
            }
            list
        }))
}
