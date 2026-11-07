//! The macOS backend end to end: a real window, with events made by the program and posted to its
//! own queue, which come in through `sendEvent:` like the user's (see "確かめ方" in
//! `docs/design/38-platform-macos.md`). AppKit runs on the main thread only, so this has no test
//! harness: `cargo test -p anti-webui-app --test macos`.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use anti_webui_core::prelude::*;
    use anti_webui_macos::testing as t;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    // kVK_ codes.
    const DELETE: u16 = 0x33;
    const A: u16 = 0x00;

    let failures: Rc<RefCell<Vec<String>>> = Rc::default();
    let passed: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let (f, p) = (failures.clone(), passed.clone());

    let result = anti_webui_app::App::new()
        .title("macOS backend test")
        .size(400, 300)
        .run(move || {
            let clicks = signal(0);
            let secondary = signal(0);
            let value = signal(String::new());
            let open = signal(false);
            let popup_clicks = signal(0);
            let size = signal((0.0f32, 0.0f32));
            let close_requests = signal(0);
            let dropped_text = signal(String::new());
            let dropped_files = signal(Vec::<String>::new());
            on_close_request(move |r| {
                close_requests.update(|c| *c += 1);
                r.prevent();
            });

            let check = {
                let (f, p) = (f.clone(), p.clone());
                move |name: &'static str, ok: bool, detail: String| {
                    if ok {
                        p.borrow_mut().push(name);
                    } else {
                        f.borrow_mut().push(format!("{name}: {detail}"));
                    }
                }
            };

            type Step = Box<dyn FnOnce()>;
            let s = |f: Step| f;
            let steps: Vec<Step> = vec![
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let scale = t::backing_scale();
                        let layer = t::layer_size();
                        let want = (400.0 * scale, 300.0 * scale);
                        check(
                            "draws to a layer of the window's pixels",
                            layer == Some(want),
                            format!("{layer:?}, wanted {want:?}"),
                        );
                        t::click(60.0, 20.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        check(
                            "a click",
                            clicks.get_untracked() == 1,
                            format!("{}", clicks.get_untracked()),
                        );
                        t::click_with(60.0, 20.0, objc2_app_kit::NSEventModifierFlags::Control);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        check(
                            "Control-click is the secondary button",
                            secondary.get_untracked() == 1,
                            format!("{}", secondary.get_untracked()),
                        );
                        t::click(100.0, 65.0);
                    }
                })),
                s(Box::new(|| t::commit("abc"))),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("text from the input method", v == "abc", v);
                        t::key(
                            DELETE,
                            "\u{7f}",
                            objc2_app_kit::NSEventModifierFlags::empty(),
                        );
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("Delete deletes", v == "ab", v);
                        t::compose("に");
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("composed text isn't in the value yet", v == "ab", v);
                        let caret = t::caret_on_screen();
                        let top_left = t::content_top_left();
                        let inside = match (caret, top_left) {
                            (Some((x, y, _, h)), Some((left, top))) => {
                                x >= left && x <= left + 400.0 && y + h <= top && y >= top - 300.0
                            }
                            _ => false,
                        };
                        check(
                            "candidates go by the caret",
                            inside,
                            format!("{caret:?} in the content at {top_left:?}"),
                        );
                        t::commit("日");
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("composed text is committed", v == "ab日", v);
                        t::key(A, "a", objc2_app_kit::NSEventModifierFlags::Command);
                    }
                })),
                // Posted events come after what the input method does at once.
                s(Box::new(|| t::commit("z"))),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("Command-A selects all", v == "z", v);
                        open.set(true);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let n = t::visible_windows();
                        check(
                            "a popup is a window of its own",
                            n == 2,
                            format!("{n} windows"),
                        );
                        // Below its anchor at (250, 0, 100, 40).
                        t::click(290.0, 60.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        check(
                            "a click on a popup",
                            popup_clicks.get_untracked() == 1,
                            format!("{}", popup_clicks.get_untracked()),
                        );
                        t::click(60.0, 250.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let n = t::visible_windows();
                        check(
                            "a click outside closes the popup",
                            !open.get_untracked() && n == 1,
                            format!("open {}, {n} windows", open.get_untracked()),
                        );
                        t::drag_enter(Some("dropped"), &[], 50.0, 150.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let op = t::drag_move(60.0, 150.0);
                        check(
                            "a drop target takes text from another program",
                            op != "none",
                            op.into(),
                        );
                        let took = t::drag_drop();
                        check("the drop is taken", took, "not taken".into());
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let got = dropped_text.get_untracked();
                        check("the dropped text arrives", got == "dropped", got);
                        t::drag_enter(None, &["file:///tmp/anti-webui-drop.txt"], 50.0, 150.0);
                    }
                })),
                s(Box::new(|| {
                    t::drag_move(60.0, 150.0);
                    t::drag_drop();
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let got = dropped_files.get_untracked();
                        check(
                            "dropped files arrive as paths",
                            got == ["/tmp/anti-webui-drop.txt"],
                            format!("{got:?}"),
                        );
                        t::drag_enter(Some("elsewhere"), &[], 300.0, 250.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let op = t::drag_move(310.0, 250.0);
                        check("nothing takes it elsewhere", op == "none", op.into());
                        t::drag_exit();
                        t::resize(500.0, 350.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let got = size.get_untracked();
                        check(
                            "resizing lays out again",
                            got == (500.0, 350.0),
                            format!("{got:?}"),
                        );
                        let scale = t::backing_scale();
                        let layer = t::layer_size();
                        let want = (500.0 * scale, 350.0 * scale);
                        check(
                            "the layer follows the size",
                            layer == Some(want),
                            format!("{layer:?}, wanted {want:?}"),
                        );
                        t::press_close_button();
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        check(
                            "the close button asks the handlers",
                            close_requests.get_untracked() == 1,
                            format!("{}", close_requests.get_untracked()),
                        );
                        close_window();
                    }
                })),
            ];
            // One step at a time, after the window has had time to show.
            let mut at = Duration::from_millis(800);
            for step in steps {
                set_timeout(at, step);
                at += Duration::from_millis(250);
            }

            div()
                .w_full()
                .h_full()
                .bg(rgb(0xf4f4f4))
                .on_resized(move |w, h| size.set((w, h)))
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(0.0)
                        .w(120.0)
                        .h(40.0)
                        .bg(rgb(0x3060d0))
                        .on_click(move |_| clicks.update(|c| *c += 1))
                        .on_pointer_down(move |e| {
                            if e.button == Some(PointerButton::Secondary) {
                                secondary.update(|c| *c += 1);
                            }
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(100.0)
                        .w(200.0)
                        .h(100.0)
                        .bg(rgb(0xe0e8f0))
                        .on_drop_text(move |text, _| dropped_text.set(text.to_owned()))
                        .on_drop_files(move |files, _| {
                            dropped_files
                                .set(files.iter().map(|f| f.display().to_string()).collect())
                        }),
                )
                .child(
                    div()
                        .absolute()
                        .left(0.0)
                        .top(50.0)
                        .w(200.0)
                        .h(30.0)
                        .child(text_input().value(value).w(200.0).h(30.0)),
                )
                .child(
                    div()
                        .absolute()
                        .left(250.0)
                        .top(0.0)
                        .w(100.0)
                        .h(40.0)
                        .bg(rgb(0xd0d0d0))
                        .child(show(
                            move || open.get(),
                            move || {
                                popup().on_dismiss(move || open.set(false)).child(
                                    div()
                                        .w(100.0)
                                        .h(40.0)
                                        .bg(rgb(0xffffff))
                                        .on_click(move |_| popup_clicks.update(|c| *c += 1)),
                                )
                            },
                        )),
                )
        });

    for name in passed.borrow().iter() {
        println!("ok   {name}");
    }
    for failure in failures.borrow().iter() {
        println!("FAIL {failure}");
    }
    if let Err(e) = result {
        println!("FAIL the program ended with: {e}");
        std::process::exit(1);
    }
    let (ok, failed) = (passed.borrow().len(), failures.borrow().len());
    println!("{ok} passed, {failed} failed");
    if failed > 0 || ok == 0 {
        std::process::exit(1);
    }
}
