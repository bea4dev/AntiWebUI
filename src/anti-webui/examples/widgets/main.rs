//! Every widget of AntiWebUI in one app.
//!
//! `cargo run --release -p anti-webui --example widgets [page]`
//!
//! With `--shot <page> <file.png> [--dark]` it writes a page to a file without a window, after what
//! comes in has come in, and each popup to `<file.png>.popup<n>.png`. `WIDGETS_OPEN=<name>` opens
//! what the page knows by that name first. `--bench` prints what going through the pages and
//! scrolling a long table costs, without a window.

mod pages;

use anti_webui::app::headless;
use anti_webui::prelude::*;

const PAGES: [(&str, &str); 10] = [
    ("buttons", "Buttons"),
    ("selection", "Selection"),
    ("inputs", "Inputs"),
    ("feedback", "Feedback"),
    ("structure", "Structure"),
    ("menus", "Menus"),
    ("data", "Data"),
    ("layout", "Layout"),
    ("images", "Images"),
    ("dragging", "Dragging"),
];

fn page_view(name: &str) -> Div {
    match name {
        "buttons" => pages::buttons(),
        "selection" => pages::selection(),
        "inputs" => pages::inputs(),
        "feedback" => pages::feedback(),
        "structure" => pages::structure(),
        "menus" => pages::menus(),
        "data" => pages::data(),
        "layout" => pages::layout(),
        "images" => pages::images(),
        "dragging" => pages::dragging(),
        _ => div(),
    }
}

fn app(current: Signal<usize>, dark_at_start: bool) -> Div {
    let dark = signal(dark_at_start);
    effect(move || {
        set_theme(if dark.get() {
            Theme::dark()
        } else {
            Theme::light()
        })
    });
    let names: Vec<(usize, String)> = PAGES
        .iter()
        .enumerate()
        .map(|(i, (_, title))| (i, (*title).to_owned()))
        .collect();
    page()
        .child(
            toolbar()
                .px(12.0)
                .child(heading("AntiWebUI widgets").text_size(14.0))
                .child(toolbar_spacer())
                .child(label("Dark"))
                .child(switch(dark).ml(6.0)),
        )
        .child(
            split_row(
                signal(0.16),
                div().flex().flex_col().gap(2.0).p(8.0).children(
                    names
                        .into_iter()
                        .map(move |(i, title)| toggle_like(current, i, title)),
                ),
                div().flex().flex_col().flex_1().min_h(0.0).child(for_each(
                    move || vec![current.get()],
                    |i| *i,
                    |i| {
                        scroll_view()
                            .flex_1()
                            .min_h(0.0)
                            .child(page_view(PAGES[i].0).p(24.0).max_w(960.0))
                    },
                )),
            )
            .flex_1(),
        )
}

/// A page in the list on the side.
fn toggle_like(current: Signal<usize>, i: usize, title: String) -> Div {
    quiet_button(title)
        .justify_start()
        .dyn_style(move |s| {
            if current.get() == i {
                s.bg(theme().selected)
            } else {
                s
            }
        })
        .on_click(move |_| current.set(i))
}

/// What going through the pages costs, and scrolling the long table of the data page.
fn bench() {
    use anti_webui::core::Ui;
    use anti_webui::core::event::{Modifiers, ScrollPhase, ScrollSource, WindowEvent};
    use anti_webui::core::geometry::{PointF, Scale, SizeI};
    use std::time::{Duration, Instant};

    let current = signal(0usize);
    let mut ui = Ui::new(move || app(current, false));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(2340, 1616),
        scale: Scale::from_f64(1.8),
    });
    ui.update();
    let mut clock = Instant::now();
    let mut frame = |ui: &mut Ui| {
        clock += Duration::from_micros(8333);
        let begin = Instant::now();
        ui.animate(clock);
        ui.update();
        std::hint::black_box(ui.paint(clock));
        begin.elapsed().as_secs_f64() * 1000.0
    };
    println!("page            build    first    p50    max  (ms of the UI thread, 240 frames)");
    for (i, (name, _)) in PAGES.iter().enumerate().cycle().take(PAGES.len() * 2) {
        let begin = Instant::now();
        current.set(i);
        let build = begin.elapsed().as_secs_f64() * 1000.0;
        let first = frame(&mut ui);
        let mut times: Vec<f64> = (0..240).map(|_| frame(&mut ui)).collect();
        times.sort_by(f64::total_cmp);
        println!(
            "{name:<15} {build:6.2} {first:8.2} {:6.2} {:6.2}",
            times[120], times[239]
        );
    }

    current.set(PAGES.iter().position(|p| p.0 == "data").unwrap());
    frame(&mut ui);
    let over_table = PointF::new(1400.0, 500.0);
    ui.handle_event(WindowEvent::PointerMoved {
        position: over_table,
        modifiers: Modifiers::default(),
    });
    let mut times = Vec::new();
    for _ in 0..1200 {
        let begin = Instant::now();
        ui.handle_event(WindowEvent::Scroll {
            position: over_table,
            delta_x: 0.0,
            delta_y: 40.0,
            modifiers: Modifiers::default(),
            source: ScrollSource::Unknown,
            phase: ScrollPhase::Single,
            time: 0,
        });
        let input = begin.elapsed().as_secs_f64() * 1000.0;
        times.push(input + frame(&mut ui));
    }
    times.sort_by(f64::total_cmp);
    println!(
        "scrolling a table of 100000 rows: p50 {:.2} p99 {:.2} max {:.2} (ms of the UI thread)",
        times[600], times[1188], times[1199]
    );
}

/// Writes a page to a file, playing the platform for what opens.
fn shot(first: usize, dark: bool, file: &str) {
    use anti_webui::core::Ui;
    use anti_webui::core::event::WindowEvent;
    use anti_webui::core::geometry::{PointI, Scale, SizeI};
    use anti_webui::core::platform::PlatformRequest;
    use anti_webui::wgpu::{Gpu, Renderer, wgpu::TextureFormat};
    use std::time::{Duration, Instant};

    let (w, h) = (2340, 1800);
    let gpu = Gpu::headless().expect("a GPU");
    let mut renderer = Renderer::new(&gpu.device, TextureFormat::Rgba8Unorm);
    let mut ui = Ui::new(move || app(signal(first), dark));
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(w, h),
        scale: Scale::from_f64(1.8),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.wait_for_images(Duration::from_secs(30));
    let start = Instant::now();
    let mut frame = None;
    for i in 0..40 {
        let now = start + Duration::from_millis(i * 16);
        ui.animate(now);
        ui.update();
        frame = Some(ui.paint_frame(now));
        for request in ui.take_requests() {
            if let PlatformRequest::ShowPopup(p) = request {
                ui.handle_event(WindowEvent::PopupConfigured {
                    id: p.id,
                    origin: PointI::new(
                        p.anchor.x - p.geometry.x,
                        p.anchor.bottom() + p.gap - p.geometry.y,
                    ),
                    size: p.wanted,
                });
            }
        }
    }
    let frame = frame.expect("a frame");
    let pixels = gpu.render_to_rgba(&mut renderer, &frame.window);
    headless::write_png(file, w as u32, h as u32, &pixels).expect("a shot");
    for (i, (_, scene)) in frame.popups.iter().enumerate() {
        let pixels = gpu.render_to_rgba(&mut renderer, scene);
        let path = format!("{file}.popup{i}.png");
        headless::write_png(path, scene.width, scene.height, &pixels).expect("a shot");
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
        let first = PAGES.iter().position(|p| p.0 == args[1]).unwrap_or(0);
        shot(first, dark, &args[2]);
        return;
    }
    let first = args
        .first()
        .and_then(|name| PAGES.iter().position(|p| p.0 == name))
        .unwrap_or(0);
    App::new()
        .title("AntiWebUI widgets")
        .size(1300, 900)
        .run(move || app(signal(first), false))
        .unwrap();
}
