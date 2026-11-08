//! The Windows backend end to end: a real window, with messages made by the program and sent to its
//! window procedure or posted to its queue, which come in like the user's (see "確かめ方" in
//! `docs/design/39-platform-windows.md`). The window has to be the program's own and on this thread,
//! so this has no test harness: `cargo test -p anti-webui-app --test windows`.

#[cfg(not(windows))]
fn main() {}

#[cfg(windows)]
fn main() {
    use anti_webui_core::prelude::*;
    use anti_webui_core::view::{set_window_fullscreen, window_fullscreen};
    use anti_webui_core::window::{set_window_maximized, window_maximized};
    use anti_webui_windows::testing as t;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    const BACK: u16 = 0x08;
    const A: u16 = b'A' as u16;

    // A logical size the scale doesn't divide gets the nearest whole physical pixel.
    let near = |got: (f32, f32), want: (f32, f32)| {
        let one = 1.0 / t::scale();
        (got.0 - want.0).abs() <= one && (got.1 - want.1).abs() <= one
    };

    let failures: Rc<RefCell<Vec<String>>> = Rc::default();
    let passed: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let (f, p) = (failures.clone(), passed.clone());

    let result = anti_webui_app::App::new()
        .title("Windows backend test")
        .size(400, 300)
        .run(move || {
            let clicks = signal(0);
            let secondary = signal(0);
            let value = signal(String::new());
            let open = signal(false);
            let popup_clicks = signal(0);
            let size = signal((0.0f32, 0.0f32));
            let scrolled = signal(0.0f32);
            let close_requests = signal(0);
            let others: Rc<RefCell<Vec<WindowHandle>>> = Rc::default();
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
                        let scale = t::scale();
                        let got = t::client_size();
                        let want = (
                            (400.0 * scale).round() as i32,
                            (300.0 * scale).round() as i32,
                        );
                        // It was hidden until its first frame came.
                        check(
                            "the window shows once it is drawn",
                            !t::cloaked(),
                            String::new(),
                        );
                        check(
                            "the client area has the logical size at the screen's scale",
                            got == want,
                            format!("{got:?}, wanted {want:?}"),
                        );
                        let laid = size.get_untracked();
                        check(
                            "the core lays out the logical size",
                            laid == (400.0, 300.0),
                            format!("{laid:?}"),
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
                        t::right_click(60.0, 20.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        check(
                            "the secondary button",
                            secondary.get_untracked() == 1,
                            format!("{}", secondary.get_untracked()),
                        );
                        t::click(100.0, 65.0);
                    }
                })),
                s(Box::new(|| t::type_text("abc"))),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("keys type their characters", v == "abc", v);
                        t::key(BACK, false);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("Backspace deletes", v == "ab", v);
                        t::commit("日本");
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("characters without a key are typed", v == "ab日本", v);
                        t::key(A, true);
                        t::type_text("z");
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let v = value.get_untracked();
                        check("Control-A selects all", v == "z", v);
                        t::wheel(300.0, 200.0, 2);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let y = scrolled.get_untracked();
                        check("the wheel scrolls down", y > 0.0, format!("{y}"));
                        check(
                            "a touchpad goes through DirectManipulation",
                            t::touchpad_manipulated(),
                            String::new(),
                        );
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
                        // The popup is white, and behind its shadow's outer edge is the window's
                        // background, 0xf4f4f4.
                        match t::popup_colors() {
                            Some([corner, middle]) => {
                                check(
                                    "the screen shows the popup",
                                    middle == (0xff, 0xff, 0xff),
                                    format!("{middle:?} in the middle"),
                                );
                                check(
                                    "what is behind a popup's shadow shows through",
                                    corner.0 > 0xc0 && corner.1 > 0xc0 && corner.2 > 0xc0,
                                    format!("{corner:?} at the corner"),
                                );
                            }
                            None => println!("skip a popup's shadow: something is in front of it"),
                        }
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
                        t::resize(500.0, 350.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let got = size.get_untracked();
                        check(
                            "resizing lays out again",
                            near(got, (500.0, 350.0)),
                            format!("{got:?}"),
                        );
                        set_window_maximized(true);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let (_, zoomed) = t::fullscreen_and_maximized();
                        check(
                            "maximizing",
                            zoomed && window_maximized(),
                            format!("zoomed {zoomed}, said {}", window_maximized()),
                        );
                        set_window_maximized(false);
                        set_window_fullscreen(true);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let (covers, _) = t::fullscreen_and_maximized();
                        check(
                            "the window covers the screen",
                            covers && window_fullscreen(),
                            format!("covers {covers}, said {}", window_fullscreen()),
                        );
                        set_window_fullscreen(false);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    let others = others.clone();
                    move || {
                        let (covers, zoomed) = t::fullscreen_and_maximized();
                        let got = size.get_untracked();
                        check(
                            "it comes back from all of the screen",
                            !covers && !zoomed && near(got, (500.0, 350.0)),
                            format!("covers {covers}, zoomed {zoomed}, {got:?}"),
                        );
                        let a = open_window(
                            WindowOptions::new().title("Second").size(200, 120),
                            || div().w_full().h_full().bg(rgb(0xd0f0d0)),
                        );
                        let b = spawn_window(
                            WindowOptions::new().title("Spawned").size(200, 120),
                            || div().w_full().h_full().bg(rgb(0xf0d0d0)),
                        );
                        others.borrow_mut().extend([a, b]);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    let others = others.clone();
                    move || {
                        let got = t::top_level_windows();
                        check(
                            "a window opened here and one on a thread of its own",
                            got == (3, 2),
                            format!("{} windows on {} threads", got.0, got.1),
                        );
                        for w in others.borrow_mut().drain(..) {
                            w.close();
                        }
                    }
                })),
                // A thread lets go of its GPU as its window closes, which takes a while in a
                // debug build.
                s(Box::new(|| {})),
                s(Box::new(|| {})),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let got = t::top_level_windows();
                        check(
                            "they close",
                            got == (1, 1),
                            format!("{} windows on {} threads", got.0, got.1),
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
            let mut at = Duration::from_millis(1500);
            for step in steps {
                set_timeout(at, step);
                at += Duration::from_millis(300);
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
                        .top(50.0)
                        .w(200.0)
                        .h(30.0)
                        .child(text_input().value(value).w(200.0).h(30.0)),
                )
                .child(
                    div()
                        .absolute()
                        .left(250.0)
                        .top(100.0)
                        .w(120.0)
                        .h(150.0)
                        .overflow_y_scroll()
                        .on_scroll(move |e| scrolled.update(|y| *y += e.delta_y))
                        .child(div().h(1000.0).bg(rgb(0xe0e8f0))),
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
                                        .rounded(8.0_f32)
                                        .shadow_lg()
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
