//! The pages of the tour.

use anti_webui::prelude::*;
use std::rc::Rc;

/// Whether `WIDGETS_OPEN` asks for what is known by this name to be open.
fn asked(name: &str) -> bool {
    std::env::var("WIDGETS_OPEN").is_ok_and(|v| v == name)
}

fn section(title: &str, body: impl IntoView) -> Div {
    card()
        .child(heading(title.to_owned()).text_size(15.0))
        .child(body)
}

fn row() -> Div {
    div().flex().flex_row().items_center().gap(8.0)
}

fn col() -> Div {
    div().flex().flex_col().gap(8.0)
}

fn sections() -> Div {
    div().flex().flex_col().gap(16.0)
}

pub fn buttons() -> Div {
    let count = signal(0);
    let off = signal(true);
    let bold = signal(true);
    let italic = signal(false);
    sections()
        .child(section(
            "Kinds",
            row()
                .child(button("Button").on_click(move |_| count.update(|c| *c += 1)))
                .child(primary_button("Primary").on_click(move |_| count.update(|c| *c += 1)))
                .child(danger_button("Danger").on_click(move |_| count.update(|c| *c -= 1)))
                .child(quiet_button("Quiet"))
                .child(toggle_button(bold, "Toggle"))
                .child(link("Link").on_click(|_| toast("A link was pressed")))
                .child(text(move || format!("Pressed: {}", count.get()))),
        ))
        .child(section(
            "Disabled",
            col()
                .child(
                    row()
                        .child(switch(off))
                        .child(text("Disable what is below")),
                )
                .child(
                    row()
                        .child(button("Button").disabled(move || off.get()))
                        .child(primary_button("Primary").disabled(move || off.get()))
                        .child(danger_button("Danger").disabled(move || off.get()))
                        .child(quiet_button("Quiet").disabled(move || off.get()))
                        .child(toggle_button(italic, "Toggle").disabled(move || off.get()))
                        .child(link("Link").disabled(move || off.get())),
                ),
        ))
        .child(section(
            "Styled directly, as with CSS",
            col()
                .child(
                    row()
                        .child(
                            button("bg stays while hovered")
                                .bg(rgb(0xc0392b))
                                .text_color(Color::WHITE),
                        )
                        .child(
                            button("with its own hover")
                                .bg(rgb(0x8250df))
                                .border_color(rgb(0x8250df))
                                .text_color(Color::WHITE)
                                .hover(|s| s.bg(rgb(0x6f42c1)))
                                .active(|s| s.bg(rgb(0x5a32a3))),
                        )
                        .child(
                            button("square, no border")
                                .rounded(0.0)
                                .border(0.0)
                                .px(24.0),
                        )
                        .child(primary_button("with shadow").rounded(999.0).shadow_md()),
                )
                .child(
                    row()
                        .accent_color(rgb(0xe8590c))
                        .child(checkbox(signal(true), "accent_color"))
                        .child(switch(signal(true)))
                        .child(radio(signal(1), 1, "goes down the tree"))
                        .child(slider(signal(60.0f32), 0.0, 100.0).w(120.0))
                        .child(progress_bar(|| 0.6).w(100.0)),
                ),
        ))
}

pub fn selection() -> Div {
    let a = signal(true);
    let b = signal(false);
    let mixed = signal(CheckState::Mixed);
    let on = signal(true);
    let group = signal(1u32);
    let single = signal(1u32);
    let level = signal(40.0f32);
    let stepped = signal(50.0f32);
    let fruit = signal(1u32);
    let picked = signal(1u32);
    let many = signal(vec![0u32, 2]);
    let names = || -> Vec<(u32, String)> {
        [
            "Apple",
            "Orange",
            "Grape",
            "Peach",
            "Pear",
            "Persimmon",
            "Strawberry",
            "Melon",
        ]
        .iter()
        .enumerate()
        .map(|(i, n)| (i as u32, (*n).to_owned()))
        .collect()
    };
    sections()
        .child(section(
            "Checkboxes and switches",
            col()
                .child(checkbox(a, "On"))
                .child(checkbox(b, "Off"))
                .child(tri_checkbox(mixed, "A mix"))
                .child(checkbox(signal(true), "Disabled").disabled(|| true))
                .child(checkbox(signal(false), "Disabled").disabled(|| true))
                .child(row().child(switch(on)).child(text("Switch")))
                .child(
                    row()
                        .child(switch(signal(true)).disabled(|| true))
                        .child(text("Disabled")),
                ),
        ))
        .child(section(
            "Radio buttons",
            row()
                .gap(32.0)
                .items_start()
                .child(radio_group(
                    group,
                    vec![
                        (0, "One stop for Tab".to_owned()),
                        (1, "Arrow keys go through".to_owned()),
                        (2, "Third".to_owned()),
                    ],
                ))
                .child(
                    col()
                        .child(radio(single, 0, "On its own"))
                        .child(radio(single, 1, "Each is a stop"))
                        .child(radio(single, 2, "Disabled").disabled(|| true)),
                ),
        ))
        .child(section(
            "Sliders",
            col()
                .child(
                    row()
                        .child(slider(level, 0.0, 100.0).w(240.0))
                        .child(text(move || format!("{:.0}", level.get()))),
                )
                .child(
                    row()
                        .child(stepped_slider(stepped, 0.0, 100.0, 25.0).w(240.0))
                        .child(text(move || format!("{:.0}", stepped.get()))),
                )
                .child(
                    slider(signal(30.0f32), 0.0, 100.0)
                        .w(240.0)
                        .disabled(|| true),
                ),
        ))
        .child(section(
            "Lists",
            row()
                .items_start()
                .gap(16.0)
                .child(select(fruit, names()).w(180.0))
                .child(select(signal(0u32), names()).w(120.0).disabled(|| true))
                .child(list_box(picked, names()).w(160.0).max_h(160.0))
                .child(multi_list_box(many, names()).w(160.0).max_h(160.0)),
        ))
}

pub fn inputs() -> Div {
    let name = signal("Ada".to_owned());
    let empty = signal(String::new());
    let notes = signal("Several\nlines".to_owned());
    let amount = signal(12.5f64);
    let whole = signal(3f64);
    let search = signal(String::new());
    sections()
        .child(section(
            "Text",
            col()
                .child(text_field(name).w(280.0))
                .child(text_field(empty).placeholder("Placeholder").w(280.0))
                .child(
                    text_field(signal("Disabled".to_owned()))
                        .w(280.0)
                        .disabled(|| true),
                )
                .child(text_area_field(notes).rows(3).w(280.0)),
        ))
        .child(section(
            "Numbers",
            col()
                .child(
                    row()
                        .child(number_field(amount, 0.0, 100.0, 0.5).w(120.0))
                        .child(text(move || format!("Value: {}", amount.get()))),
                )
                .child(number_field(whole, 0.0, 10.0, 1.0).w(120.0))
                .child(
                    number_field(signal(1.0), 0.0, 10.0, 1.0)
                        .w(120.0)
                        .disabled(|| true),
                ),
        ))
        .child(section(
            "Search",
            col()
                .child(search_field(search, "Search").w(280.0))
                .child(text(move || format!("Looking for: {}", search.get())))
                .child(
                    search_field(signal(String::new()), "Disabled")
                        .w(280.0)
                        .disabled(|| true),
                ),
        ))
}

pub fn feedback() -> Div {
    let value = signal(0.35f32);
    sections()
        .child(section(
            "Progress",
            col()
                .child(progress_bar(move || value.get()).w(320.0))
                .child(
                    row()
                        .child(
                            button("Less")
                                .on_click(move |_| value.update(|v| *v = (*v - 0.1).max(0.0))),
                        )
                        .child(
                            button("More")
                                .on_click(move |_| value.update(|v| *v = (*v + 0.1).min(1.0))),
                        ),
                )
                .child(indeterminate_progress_bar().w(320.0))
                .child(
                    row()
                        .child(spinner())
                        .child(sized_spinner(24.0))
                        .child(sized_spinner(40.0)),
                ),
        ))
        .child(section(
            "Badges",
            row()
                .child(badge("Neutral"))
                .child(toned_badge("Accent", Tone::Accent))
                .child(toned_badge("Success", Tone::Success))
                .child(toned_badge("Warning", Tone::Warning))
                .child(toned_badge("Danger", Tone::Danger)),
        ))
        .child(section(
            "Tooltips",
            row()
                .child(tooltip(button("Rest on me"), "This shows after a moment"))
                .child(tooltip(primary_button("And me"), "Also from the keyboard")),
        ))
        .child(section(
            "Toasts",
            row()
                .child(button("Toast").on_click(|_| toast("Saved")))
                .child(
                    button("Success")
                        .on_click(|_| toast_with(Toast::new("Uploaded").tone(Tone::Success))),
                )
                .child(
                    button("Danger")
                        .on_click(|_| toast_with(Toast::new("It didn't work").tone(Tone::Danger))),
                )
                .child(button("With an action").on_click(|_| {
                    toast_with(
                        Toast::new("Moved to the trash").action("Undo", || toast("Put back")),
                    )
                })),
        ))
        .children(asked("toast").then(|| {
            toast("A toast");
            toast_with(Toast::new("With a tone").tone(Tone::Success));
            toast_with(Toast::new("With an action").action("Undo", || {}));
            div()
        }))
}

pub fn structure() -> Div {
    let current = signal(0usize);
    let open = signal(true);
    let closed = signal(false);
    let asking = signal(asked("dialog"));
    let bold = signal(true);
    let italic = signal(false);
    sections()
        .child(section(
            "Tabs",
            tabs(
                current,
                vec![
                    tab("First", || {
                        div().py(12.0).child(text("What the first tab shows"))
                    }),
                    tab("Second", || {
                        div().py(12.0).child(text("What the second tab shows"))
                    }),
                    tab("Third", || {
                        div().py(12.0).child(text("What the third tab shows"))
                    }),
                ],
            ),
        ))
        .child(section(
            "Parts that open",
            col()
                .child(disclosure(open, "Open from the start", || {
                    text("What is under it, which takes the room it needs.")
                }))
                .child(disclosure(closed, "Closed from the start", || {
                    col()
                        .child(text("It only exists while it shows."))
                        .child(button("And can have anything in it"))
                }))
                .child(divider())
                .child(accordion(vec![
                    (
                        "One at a time".to_owned(),
                        Rc::new(|| text("The first part").into_view()) as Rc<dyn Fn() -> View>,
                    ),
                    (
                        "Opening one".to_owned(),
                        Rc::new(|| text("The second part").into_view()),
                    ),
                    (
                        "Closes the other".to_owned(),
                        Rc::new(|| text("The third part").into_view()),
                    ),
                ])),
        ))
        .child(section(
            "Dialogs",
            row()
                .child(button("Ask").on_click(move |_| asking.set(true)))
                .child(alert_dialog(
                    move || asking.get(),
                    move || asking.set(false),
                    "Delete this file?",
                    "It can't be brought back. Tab stays in here, and Escape closes it.",
                    move || {
                        vec![
                            button("Cancel").on_click(move |_| asking.set(false)),
                            danger_button("Delete").on_click(move |_| {
                                asking.set(false);
                                toast("Deleted");
                            }),
                        ]
                    },
                )),
        ))
        .child(section(
            "Toolbars",
            div()
                .rounded(6.0)
                .border(1.0)
                .dyn_style(|s| s.border_color(theme().border))
                .child(
                    toolbar()
                        .child(quiet_button("Cut"))
                        .child(quiet_button("Copy"))
                        .child(quiet_button("Paste").disabled(|| true))
                        .child(toolbar_separator())
                        .child(toggle_button(bold, "Bold"))
                        .child(toggle_button(italic, "Italic"))
                        .child(toolbar_spacer())
                        .child(quiet_button("Settings")),
                )
                .child(div().p(12.0).child(text("What the toolbar is for"))),
        ))
}

pub fn menus() -> Div {
    let wrap = signal(true);
    let open = signal(asked("menu"));
    let saved = |what: &'static str| move || toast(what);
    let entries = move || -> Vec<MenuEntry> {
        vec![
            menu_item("New")
                .shortcut("Ctrl+N")
                .on_click(saved("New"))
                .into(),
            menu_item("Open")
                .shortcut("Ctrl+O")
                .on_click(saved("Open"))
                .into(),
            menu_item("Save")
                .shortcut("Ctrl+S")
                .disabled_when(|| true)
                .into(),
            menu_separator(),
            menu_item("Wrap lines")
                .checked_when(move || wrap.get())
                .on_click(move || wrap.update(|v| *v = !*v))
                .into(),
            submenu(
                "Export",
                vec![
                    menu_item("As PDF").on_click(saved("PDF")).into(),
                    menu_item("As PNG").on_click(saved("PNG")).into(),
                    submenu(
                        "More",
                        vec![menu_item("As SVG").on_click(saved("SVG")).into()],
                    ),
                ],
            ),
            menu_separator(),
            menu_item("Quit").on_click(saved("Quit")).into(),
        ]
    };
    sections()
        .child(section(
            "Menus",
            row().child(dropdown_button("File", entries())).child(
                button("Opens from the code")
                    .on_click(move |_| open.set(true))
                    .child(menu(move || open.get(), move || open.set(false), entries())),
            ),
        ))
        .child(section(
            "Context menus",
            context_menu(
                div()
                    .flex()
                    .items_center()
                    .justify_center()
                    .h(120.0)
                    .rounded(6.0)
                    .border(1.0)
                    .dyn_style(|s| {
                        s.border_color(theme().border)
                            .text_color(theme().text_muted)
                    })
                    .child(text("Press here with the secondary button")),
                entries(),
            ),
        ))
}

#[derive(Clone)]
struct Person {
    id: u32,
    name: String,
    city: &'static str,
    score: f32,
}

fn people(count: u32) -> Vec<Person> {
    const CITIES: [&str; 6] = ["Tokyo", "Osaka", "Nagoya", "Sapporo", "Fukuoka", "Kyoto"];
    (0..count)
        .map(|id| Person {
            id,
            name: format!("Person {id}"),
            city: CITIES[(id as usize * 7) % CITIES.len()],
            score: ((id * 2654435761u32.wrapping_mul(id + 1)) % 1000) as f32 / 10.0,
        })
        .collect()
}

pub fn data() -> Div {
    let rows = signal(people(100_000));
    let selected = signal(None::<usize>);
    let tree_selected = signal(Some("src"));
    sections()
        .child(section(
            "Table of 100 000 rows",
            col()
                .child(
                    table(
                        rows,
                        vec![
                            column("ID", |p: &Person| p.id.to_string())
                                .width(80.0)
                                .align_end()
                                .sort_by(|a: &Person, b: &Person| a.id.cmp(&b.id)),
                            column("Name", |p: &Person| p.name.clone())
                                .width(200.0)
                                .sort_by(|a: &Person, b: &Person| a.name.cmp(&b.name)),
                            column("City", |p: &Person| p.city.to_owned())
                                .width(140.0)
                                .sort_by(|a: &Person, b: &Person| a.city.cmp(b.city)),
                            column("Score", |p: &Person| format!("{:.1}", p.score))
                                .width(90.0)
                                .align_end()
                                .sort_by(|a: &Person, b: &Person| a.score.total_cmp(&b.score)),
                            custom_column("State", |p: &Person| {
                                if p.score > 50.0 {
                                    toned_badge("High", Tone::Success).into_view()
                                } else {
                                    toned_badge("Low", Tone::Neutral).into_view()
                                }
                            })
                            .width(90.0)
                            .fill(),
                        ],
                        selected,
                    )
                    .h(320.0),
                )
                .child(text(move || match selected.get() {
                    Some(i) => format!("Selected: row {i}"),
                    None => "Nothing is selected".to_owned(),
                })),
        ))
        .child(section(
            "Tree",
            tree_view(
                vec![
                    tree_item("src", "src")
                        .child(
                            tree_item("core", "anti-webui-core")
                                .child(tree_item("view", "view.rs"))
                                .child(tree_item("ui", "ui.rs"))
                                .child(tree_item("layout", "layout.rs")),
                        )
                        .child(
                            tree_item("widgets", "anti-webui-widgets")
                                .child(tree_item("theme", "theme.rs"))
                                .child(tree_item("menu", "menu.rs")),
                        ),
                    tree_item("docs", "docs").child(tree_item("design", "design")),
                    tree_item("readme", "README.md"),
                ],
                tree_selected,
            )
            .h(220.0),
        ))
        .child(section(
            "A list of a million rows",
            virtual_list(
                || 1_000_000,
                24.0,
                |i| {
                    div()
                        .h_full()
                        .flex()
                        .items_center()
                        .px(8.0)
                        .child(text(format!("Row {i}")))
                },
            )
            .h(160.0)
            .rounded(6.0)
            .border(1.0)
            .dyn_style(|s| s.border_color(theme().border)),
        ))
}

pub fn layout() -> Div {
    let page_at = signal(4usize);
    let few = signal(1usize);
    let across = signal(0.35f32);
    let down = signal(0.5f32);
    let pane = |name: &'static str| {
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .min_w(0.0)
            .min_h(0.0)
            .child(text(name))
    };
    sections()
        .child(section(
            "Panes",
            split_row(
                across,
                pane("Left"),
                split_col(down, pane("Top right"), pane("Bottom right")).flex_1(),
            )
            .h(240.0)
            .rounded(6.0)
            .border(1.0)
            .overflow_clip()
            .dyn_style(|s| s.border_color(theme().border)),
        ))
        .child(section(
            "Breadcrumbs",
            breadcrumb(vec![
                crumb("Home").on_click(|| toast("Home")),
                crumb("Documents").on_click(|| toast("Documents")),
                crumb("Design"),
            ]),
        ))
        .child(section(
            "Pages",
            col()
                .child(pagination(page_at, || 20))
                .child(text(move || format!("Page {} of 20", page_at.get() + 1)))
                .child(pagination(few, || 5)),
        ))
}
