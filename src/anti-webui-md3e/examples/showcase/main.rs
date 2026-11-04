//! Every component of `anti_webui_md3e` in one app.
//!
//! `cargo run --release -p anti-webui-md3e --example showcase [page]`
//!
//! With `--shot <page> <file.png> [--dark]` it writes a page to a file without a window, and
//! `SHOWCASE_OPEN=<name>` opens what the page knows by that name first. `--bench` prints
//! what going through the pages costs, without a window.

mod pages;

use anti_webui::app::headless;
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    AppBarKind, AppBarScroll, IconButtonStyle, TopAppBar, icon_button, icon_toggle_button,
    nav_item, top_app_bar_over, wide_navigation_rail,
};
use anti_webui_md3e::{ColorScheme, color_scheme, icons, set_color_scheme};

const PAGES: [(&str, &str); 6] = [
    ("actions", "Actions"),
    ("selection", "Selection"),
    ("communication", "Feedback"),
    ("containment", "Containers"),
    ("navigation", "Navigation"),
    ("inputs", "Inputs"),
];

fn page(name: &str) -> Div {
    match name {
        "actions" => pages::actions::page(),
        "selection" => pages::selection::page(),
        "communication" => pages::communication::page(),
        "containment" => pages::containment::page(),
        "navigation" => pages::navigation::page(),
        "inputs" => pages::inputs::page(),
        _ => div(),
    }
}

/// The whole app: a rail that opens, a bar with the title and the theme, and the page.
fn app(first: usize, dark_at_start: bool) -> Div {
    app_at(signal(first), dark_at_start)
}

fn app_at(current: Signal<usize>, dark_at_start: bool) -> Div {
    let dark = signal(dark_at_start);
    let rail_open = signal(false);
    effect(move || {
        set_color_scheme(if dark.get() {
            ColorScheme::dark()
        } else {
            ColorScheme::light()
        });
    });
    let destinations = vec![
        nav_item(icons::TOUCH_APP, PAGES[0].1),
        nav_item(icons::TOGGLE_ON, PAGES[1].1),
        nav_item(icons::NOTIFICATIONS, PAGES[2].1),
        nav_item(icons::DASHBOARD, PAGES[3].1),
        nav_item(icons::EXPLORE, PAGES[4].1),
        nav_item(icons::KEYBOARD, PAGES[5].1),
    ];
    let content = for_each(
        move || vec![current.get()],
        |i| *i,
        move |i| {
            let bar = TopAppBar::new(AppBarKind::LargeFlexible, PAGES[i].1)
                .subtitle("Material 3 Expressive on AntiWebUI")
                .action(
                    icon_toggle_button(
                        icons::DARK_MODE,
                        move || dark.get(),
                        IconButtonStyle::standard(),
                    )
                    .on_click(move |_| dark.update(|v| *v = !*v)),
                )
                .action(icon_button(icons::MORE_VERT));
            top_app_bar_over(
                bar,
                AppBarScroll::ExitUntilCollapsed,
                div()
                    .flex()
                    .flex_col()
                    .child(page(PAGES[i].0).p(24.0).pb(120.0)),
            )
            .absolute()
            .left(0.0)
            .top(0.0)
            .right(0.0)
            .bottom(0.0)
            .dyn_style(|s| s.bg(color_scheme().surface))
        },
    )
    .enter(|s| s.opacity(0.0).translate_y(24.0))
    .exit(|s| s.opacity(0.0))
    .transition(Transition::ms(220).cubic_bezier(0.05, 0.7, 0.1, 1.0));
    div()
        .size_full()
        .flex()
        .flex_row()
        .dyn_style(|s| {
            let c = color_scheme();
            s.bg(c.surface).text_color(c.on_surface)
        })
        .child(wide_navigation_rail(
            destinations,
            move || current.get(),
            move |i| current.set(i),
            move || rail_open.get(),
            Some(
                div().pl(28.0).child(
                    icon_button(icons::MENU).on_click(move |_| rail_open.update(|v| *v = !*v)),
                ),
            ),
        ))
        .child(
            div()
                .relative()
                .flex_grow(1.0)
                .flex_basis(0.0)
                .min_w(0.0)
                .overflow_hidden()
                .child(content),
        )
}

/// Goes through the pages: what it costs to build each, and to play the frames after that.
fn bench() {
    use anti_webui::core::Ui;
    use anti_webui::core::event::{Modifiers, PointerButton, WindowEvent};
    use anti_webui::core::geometry::{PointF, Scale, SizeI};
    use std::time::{Duration, Instant};

    let current = signal(0usize);
    let mut ui = Ui::new(move || app_at(current, false));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(2340, 1616),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    let start = Instant::now();
    let mut clock = start;
    let mut frame = |ui: &mut Ui| {
        clock += Duration::from_micros(8333);
        let begin = Instant::now();
        ui.animate(clock);
        ui.update();
        let scene = ui.paint(clock);
        std::hint::black_box(&scene);
        begin.elapsed().as_secs_f64() * 1000.0
    };
    println!("page            build    first    p50    max  (ms of the UI thread, 240 frames)");
    for (i, (name, _)) in PAGES.iter().enumerate().cycle().take(PAGES.len() * 2) {
        let begin = Instant::now();
        current.set(i);
        let build = begin.elapsed().as_secs_f64() * 1000.0;
        let first = frame(&mut ui);
        // A press somewhere on the page starts what a press starts.
        let position = PointF::new(1000.0, 800.0);
        ui.handle_event(WindowEvent::PointerMoved {
            position,
            modifiers: Modifiers::default(),
        });
        for pressed in [true, false] {
            ui.handle_event(WindowEvent::PointerButton {
                position,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::default(),
                time: 0,
            });
        }
        let mut times: Vec<f64> = (0..240).map(|_| frame(&mut ui)).collect();
        times.sort_by(f64::total_cmp);
        println!(
            "{name:<15} {build:6.2} {first:8.2} {:6.2} {:6.2}",
            times[120], times[239]
        );
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--bench") {
        bench();
        return;
    }
    if args.first().is_some_and(|a| a == "--shot") {
        let dark = args.iter().any(|a| a == "--dark");
        let (w, h, scale) = (2340, 1800, 1.8);
        let name = args[1].clone();
        let first = PAGES.iter().position(|p| p.0 == name).unwrap_or(0);
        headless::render_png(&args[2], w, h, scale, move || app(first, dark)).expect("a shot");
        return;
    }
    let first = args
        .first()
        .and_then(|name| PAGES.iter().position(|p| p.0 == name))
        .unwrap_or(0);
    App::new()
        .title("Material 3 Expressive")
        .size(1300, 900)
        .run(move || app(first, false))
        .unwrap();
}
