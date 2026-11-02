//! Renders a frame after synthetic input: a cross-element selection and an IME composition.

use anti_webui_core::event::*;
use anti_webui_core::geometry::PointF;
use anti_webui_core::prelude::*;

fn main() {
    let out = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "interaction.png".into());
    let scale = 1.5;
    let (w, h) = (720, 420);
    let a = node_ref();
    let b = node_ref();
    let field = node_ref();
    let pixels = anti_webui_app::headless::render_driven_rgba(
        w,
        h,
        scale,
        move || {
            div()
                .flex()
                .flex_col()
                .gap(12.0)
                .p(16.0)
                .h(pct(100.0))
                .child(div().node_ref(a).child(text("First paragraph: the selection starts in the middle of this line and continues.")))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap(12.0)
                        .child(div().flex_1().p(8.0).bg(rgb(0xf6f8fa)).rounded(6.0).node_ref(b).child(text("左のカード：日本語の文章も選択できます。")))
                        .child(div().flex_1().p(8.0).bg(rgb(0xf6f8fa)).rounded(6.0).child(text("Right card text that is not selected."))),
                )
                .child(
                    text_input()
                        .node_ref(field)
                        .w(300.0)
                        .px(8.0)
                        .py(5.0)
                        .border(1.0)
                        .border_color(rgb(0x0969da))
                        .rounded(6.0)
                        .value(signal(String::from("入力中："))),
                )
        },
        move |ui| {
            let ra = a.rect().unwrap();
            let rb = b.rect().unwrap();
            let start = PointF::new(ra.x as f32 + 120.0, ra.y as f32 + 8.0);
            let end = PointF::new(rb.x as f32 + 140.0, rb.y as f32 + rb.height as f32 / 2.0);
            let m = Modifiers::default();
            ui.handle_event(WindowEvent::PointerButton { position: start, button: PointerButton::Primary, pressed: true, modifiers: m, time: 0 });
            ui.handle_event(WindowEvent::PointerMoved { position: end, modifiers: m });
            ui.handle_event(WindowEvent::PointerButton { position: end, button: PointerButton::Primary, pressed: false, modifiers: m, time: 10 });
            // Focus the input with Tab and start an IME composition.
            ui.handle_event(WindowEvent::Key(KeyEvent { key: Key::Named(NamedKey::Tab), text: None, modifiers: m, pressed: true, repeat: false }));
            ui.update();
            ui.handle_event(WindowEvent::Key(KeyEvent { key: Key::Named(NamedKey::End), text: None, modifiers: m, pressed: true, repeat: false }));
            ui.update();
            ui.handle_event(WindowEvent::Ime(ImeUpdate { preedit: Some(("にほんご".into(), Some((0, 6)))), ..Default::default() }));
            let _ = field;
        },
    )
    .unwrap();
    anti_webui_app::headless::write_png(&out, w, h, &pixels).unwrap();
    println!("wrote {out}");
}
