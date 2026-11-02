//! Measures small changes in large documents: what one hover, caret blink or text change costs
//! in each phase of a frame.
//!
//! `cargo run --release -p anti-webui --example bench_changes`

use anti_webui::core::Ui;
use anti_webui::core::damage;
use anti_webui::core::event::*;
use anti_webui::core::geometry::{PointF, Scale, SizeI};
use anti_webui::core::layout::resolve_styles;
use anti_webui::core::paint::CARET_BLINK;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use anti_webui::wgpu::{Gpu, Offscreen, Renderer, wgpu};
use std::time::{Duration, Instant};

const W: u32 = 1620;
const H: u32 = 1616;
const STEPS: u32 = 120;

#[derive(Default)]
struct Times {
    event: Duration,
    style: Duration,
    layout: Duration,
    paint: Duration,
    diff: Duration,
    record: Duration,
    gpu: Duration,
    damage: i64,
}

struct Bench<'a> {
    ui: Ui,
    gpu: &'a Gpu,
    renderer: Renderer,
    target: Offscreen,
    prev: anti_webui::core::scene::Scene,
    now: Instant,
}

impl Bench<'_> {
    fn frame(&mut self, times: &mut Times, event: impl FnOnce(&mut Ui)) {
        let t0 = Instant::now();
        event(&mut self.ui);
        let t1 = Instant::now();
        with_doc(|d| {
            if d.dirty.style {
                resolve_styles(d);
                d.dirty.style = false;
            }
        });
        let t2 = Instant::now();
        self.ui.update();
        let t3 = Instant::now();
        self.now += CARET_BLINK;
        let scene = self.ui.paint(self.now);
        let t4 = Instant::now();
        let d = damage::diff(&self.prev, &scene);
        let t5 = Instant::now();
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        self.renderer.render_damaged(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &self.target.view,
            &scene,
            &d,
        );
        self.gpu.queue.submit([encoder.finish()]);
        let t6 = Instant::now();
        let _ = self.gpu.device.poll(wgpu::PollType::wait_indefinitely());
        let t7 = Instant::now();
        times.event += t1 - t0;
        times.style += t2 - t1;
        times.layout += t3 - t2;
        times.paint += t4 - t3;
        times.diff += t5 - t4;
        times.record += t6 - t5;
        times.gpu += t7 - t6;
        times.damage += d.area();
        self.prev = scene;
    }
}

fn report(name: &str, t: &Times) {
    let per = |d: Duration| d.as_secs_f64() * 1000.0 / STEPS as f64;
    let total = t.event + t.style + t.layout + t.paint + t.diff + t.record + t.gpu;
    println!(
        "  {name:<12} total {:>7.2} ms | event {:>6.2} style {:>6.2} layout+pos {:>6.2} paint {:>6.2} \
         diff {:>6.2} record {:>6.2} gpu {:>5.2} | damage {:.2}%",
        per(total),
        per(t.event),
        per(t.style),
        per(t.layout),
        per(t.paint),
        per(t.diff),
        per(t.record),
        per(t.gpu),
        t.damage as f64 * 100.0 / (STEPS as f64 * W as f64 * H as f64),
    );
}

fn bench(gpu: &Gpu, rows: usize) {
    let field = node_ref();
    let label = signal("row label".to_string());
    let mut ui = Ui::new(move || {
        div()
            .h_full()
            .overflow_y_scroll()
            .child(
                text_input()
                    .w(300.0)
                    .node_ref(field)
                    .value(signal("caret".to_string())),
            )
            .child(div().h(28.0).child(text(move || label.get())))
            .children((0..rows).map(|i| {
                div()
                    .flex()
                    .items_center()
                    .h(28.0)
                    .px(8.0)
                    .rounded(4.0)
                    .bg(if i % 2 == 0 {
                        rgb(0xf4f4f4)
                    } else {
                        rgb(0xffffff)
                    })
                    .hover(|s| s.bg(rgb(0xddeeff)))
                    .child(text(format!(
                        "Row {i}: the quick brown fox jumps over the lazy dog"
                    )))
            }))
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(W as i32, H as i32),
        scale: Scale(216),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let format = wgpu::TextureFormat::Bgra8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::new(gpu, format, W, H);
    let now = Instant::now();
    let prev = ui.paint(now);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &target.view, &prev);
    gpu.queue.submit([encoder.finish()]);
    let nodes = with_doc(|d| d.nodes.len());
    println!("{rows} rows, {nodes} nodes");
    let mut b = Bench {
        ui,
        gpu,
        renderer,
        target,
        prev,
        now,
    };

    let m = Modifiers::default();
    let mut t = Times::default();
    for i in 0..STEPS {
        let y = 300.0 + (i % 40) as f32 * 50.0 % 1200.0;
        b.frame(&mut t, |ui| {
            ui.handle_event(WindowEvent::PointerMoved {
                position: PointF::new(400.0, y),
                modifiers: m,
            })
        });
    }
    report("hover", &t);

    let r = field.rect().unwrap();
    let p = PointF::new(r.x as f32 + 20.0, r.y as f32 + 10.0);
    for pressed in [true, false] {
        b.ui.handle_event(WindowEvent::PointerButton {
            position: p,
            button: PointerButton::Primary,
            pressed,
            modifiers: m,
            time: 0,
        });
    }
    let mut t = Times::default();
    for _ in 0..STEPS {
        let now = b.now;
        b.frame(&mut t, |ui| ui.tick(now));
    }
    report("caret blink", &t);

    let mut t = Times::default();
    for i in 0..STEPS {
        b.frame(&mut t, |_| label.set(format!("row label {i}")));
    }
    report("text change", &t);

    let mut t = Times::default();
    for i in 0..STEPS {
        b.frame(&mut t, |ui| {
            ui.handle_event(WindowEvent::Scroll {
                position: PointF::new(400.0, 800.0),
                delta_x: 0.0,
                delta_y: if i % 120 < 60 { 24.0 } else { -24.0 },
                modifiers: m,
                source: ScrollSource::Unknown,
                phase: ScrollPhase::Single,
                time: 0,
            })
        });
    }
    report("scroll", &t);

    // At the top speed, which moves further than a screen every few frames.
    let mut t = Times::default();
    let mut fling = (std::time::Instant::now(), 0);
    let mut down = true;
    for _ in 0..STEPS {
        b.frame(&mut t, |ui| {
            if !ui.animating() {
                down = !down;
                let dy = if down { 300.0 } else { -300.0 };
                for (phase, time, dy) in [
                    (ScrollPhase::Began, 0, dy),
                    (ScrollPhase::Moved, 10, dy),
                    (ScrollPhase::Moved, 20, dy),
                    (ScrollPhase::Lifted { fling: true }, 25, 0.0),
                ] {
                    ui.handle_event(WindowEvent::Scroll {
                        position: PointF::new(400.0, 800.0),
                        delta_x: 0.0,
                        delta_y: dy,
                        modifiers: m,
                        source: ScrollSource::Touchpad,
                        phase,
                        time,
                    });
                }
                fling = (std::time::Instant::now(), 0);
            }
            fling.1 += 1;
            ui.animate(fling.0 + std::time::Duration::from_millis(fling.1 * 16));
        });
    }
    report("inertia", &t);
}

fn main() {
    let gpu = Gpu::headless().expect("GPU");
    println!("{:?}", gpu.adapter.get_info().name);
    bench(&gpu, 2000);
    bench(&gpu, 20000);
}
