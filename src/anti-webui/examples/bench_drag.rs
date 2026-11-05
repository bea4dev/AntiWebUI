//! What moving things costs. Without a window: `cargo run --release -p anti-webui --example bench_drag`
//!
//! Prints what a frame takes on the UI thread and the GPU while something is dragged, while an item
//! of a list is moved, and how much of the window it repaints.

use anti_webui::core::Ui;
use anti_webui::core::damage;
use anti_webui::core::event::{Key, KeyEvent, Modifiers, PointerButton, WindowEvent};
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::{Duration, Instant};

const SCREEN: (u32, u32) = (2340, 1616);

fn mount<V: IntoView>(f: impl FnOnce() -> V) -> Ui {
    let mut ui = Ui::new(f);
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(SCREEN.0 as i32, SCREEN.1 as i32),
        scale: Scale::from_f64(1.8),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    ui
}

fn pointer(ui: &mut Ui, x: f32, y: f32) {
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, y),
        modifiers: Modifiers::default(),
    });
}

fn button(ui: &mut Ui, x: f32, y: f32, pressed: bool) {
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, y),
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::default(),
        time: 0,
    });
}

struct Report {
    ui: Vec<f64>,
    gpu: Vec<f64>,
    damage: Vec<f64>,
}

fn percentile(values: &mut [f64], p: f64) -> f64 {
    values.sort_by(f64::total_cmp);
    values[((values.len() - 1) as f64 * p) as usize]
}

/// Moves the pointer along a slow diagonal, drawing what changes each time.
fn run(name: &str, gpu: &Gpu, ui: &mut Ui, from: (f32, f32), to: (f32, f32)) {
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(gpu, format, SCREEN.0, SCREEN.1);
    pointer(ui, from.0, from.1);
    button(ui, from.0, from.1, true);
    ui.update();
    let mut prev = ui.paint(Instant::now());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    let mut report = Report {
        ui: Vec::new(),
        gpu: Vec::new(),
        damage: Vec::new(),
    };
    let start = Instant::now();
    const STEPS: usize = 600;
    for i in 1..=STEPS {
        let t = i as f32 / STEPS as f32;
        let (x, y) = (from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
        let began = Instant::now();
        pointer(ui, x, y);
        ui.animate(start + Duration::from_millis(i as u64 * 8));
        ui.update();
        let scene = ui.paint(Instant::now());
        report.ui.push(began.elapsed().as_secs_f64() * 1e3);
        let d = damage::diff(&prev, &scene);
        report
            .damage
            .push(d.area() as f64 / (SCREEN.0 as f64 * SCREEN.1 as f64) * 100.0);
        let began = Instant::now();
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render_damaged(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &target.view,
            &scene,
            &d,
        );
        gpu.queue.submit([encoder.finish()]);
        gpu.device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("the GPU finishes");
        report.gpu.push(began.elapsed().as_secs_f64() * 1e3);
        prev = scene;
    }
    button(ui, to.0, to.1, false);
    let begin = Instant::now();
    ui.update();
    let released = begin.elapsed().as_secs_f64() * 1e3;
    println!(
        "{name:<34} UI p50 {:.3} p99 {:.3} max {:.3}  GPU p50 {:.3} p99 {:.3}  repaint p50 {:.1}%  release {:.3} ms",
        percentile(&mut report.ui, 0.5),
        percentile(&mut report.ui, 0.99),
        percentile(&mut report.ui, 1.0),
        percentile(&mut report.gpu, 0.5),
        percentile(&mut report.gpu, 0.99),
        percentile(&mut report.damage, 0.5),
        released,
    );
}

#[derive(Clone)]
struct Card;

/// Rows of 16 cards.
fn lots_of_cards(n: usize) -> Div {
    let card = |i: usize| {
        div()
            .w(70.0)
            .h(32.0)
            .bg(rgb(0xeeeeee))
            .rounded(4.0)
            .draggable(|| Card)
            .dragging(|s| s.opacity(0.4))
            .drop_target(|_: &Card, _| {})
            .drag_over(|s| s.bg(rgb(0xccddff)))
            .child(text(format!("Card {i}")))
    };
    div()
        .bg(Color::WHITE)
        .h(pct(100.0))
        .flex()
        .flex_col()
        .gap(4.0)
        .children((0..n / 16).map(|r| {
            div()
                .flex()
                .flex_row()
                .gap(4.0)
                .children((0..16).map(|c| card(r * 16 + c)))
        }))
}

/// What it takes to start dragging all of a long text, and then to carry it.
fn selected_text(gpu: &Gpu) {
    for (paragraphs, lines) in [(100, 10), (5_000, 10)] {
        let words = "The quick brown fox jumps over the lazy dog. ".repeat(lines * 3);
        let mut ui = mount(|| {
            let mut page = div().flex().flex_col().gap(8.0).p(16.0);
            for _ in 0..paragraphs {
                page = page.child(text(words.clone()));
            }
            div()
                .bg(Color::WHITE)
                .h(pct(100.0))
                .overflow_y_scroll()
                .child(page)
        });
        ui.handle_event(WindowEvent::Key(KeyEvent {
            key: Key::Character("a".into()),
            text: None,
            modifiers: Modifiers {
                ctrl: true,
                ..Default::default()
            },
            pressed: true,
            repeat: false,
        }));
        ui.update();
        let (x, y) = (200.0, 80.0);
        pointer(&mut ui, x, y);
        button(&mut ui, x, y, true);
        ui.update();
        let started = Instant::now();
        pointer(&mut ui, x + 20.0, y);
        ui.update();
        let scene = ui.paint(Instant::now());
        let cost = started.elapsed();
        let glyphs = scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                anti_webui::core::scene::Primitive::Layer(l) if l.key == u64::MAX - 1 => {
                    Some(l.content.primitives.len())
                }
                _ => None,
            })
            .sum::<usize>();
        println!(
            "start dragging {} KB of text selected in {paragraphs} paragraphs: {:.2} ms, {glyphs} runs in the picture",
            words.len() * paragraphs / 1024,
            cost.as_secs_f64() * 1000.0,
        );
        run(
            &format!("carry {paragraphs} selected paragraphs"),
            gpu,
            &mut ui,
            (x + 20.0, y),
            (2200.0, 1500.0),
        );
    }
}

fn main() {
    let gpu = Gpu::headless().expect("a GPU");
    println!("{} ({:?})", gpu.adapter.get_info().name, SCREEN);

    selected_text(&gpu);

    let mut ui = mount(|| lots_of_cards(600));
    run(
        "drag a card over 600",
        &gpu,
        &mut ui,
        (100.0, 30.0),
        (2200.0, 1500.0),
    );
    drop(ui);

    let mut ui = mount(|| {
        lots_of_cards(600).child(
            div()
                .absolute()
                .left(0.0)
                .top(0.0)
                .w(120.0)
                .h(32.0)
                .draggable(|| Card)
                .drag_preview(|_: &Card| div().w(200.0).h(80.0).bg(rgb(0x2266ff)).rounded(8.0)),
        )
    });
    run(
        "the same with its own preview",
        &gpu,
        &mut ui,
        (100.0, 30.0),
        (2200.0, 1500.0),
    );
    drop(ui);

    for n in [50u32, 500] {
        let items = signal((0..n).collect::<Vec<_>>());
        let mut ui = mount(|| {
            div()
                .bg(Color::WHITE)
                .h(pct(100.0))
                .overflow_y_scroll()
                .child(
                    sortable_list(
                        items,
                        |v: &u32| *v,
                        |v, _| {
                            div()
                                .h(32.0)
                                .bg(rgb(0xeeeeee))
                                .rounded(4.0)
                                .child(text(format!("Row {v}")))
                        },
                    )
                    .gap(4.0),
                )
        });
        run(
            &format!("sortable_list of {n}"),
            &gpu,
            &mut ui,
            (200.0, 30.0),
            (200.0, 1500.0),
        );
        drop(ui);
    }
}
