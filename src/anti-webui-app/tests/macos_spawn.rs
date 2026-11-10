//! Windows of `spawn_window` on macOS end to end: AppKit's window on the main thread, the UI and
//! its drawing on a thread of its own (see "複数ウィンドウ" in `docs/design/38-platform-macos.md`).
//! Like `macos.rs`, this has no test harness: `cargo test -p anti-webui-app --test macos_spawn`.

#[cfg(not(target_os = "macos"))]
fn main() {}

#[cfg(target_os = "macos")]
fn main() {
    use anti_webui_core::prelude::*;
    use anti_webui_core::window::{WindowOptions, spawn_window};
    use anti_webui_macos::testing as t;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

    // What hangs fails.
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(40));
        println!("FAIL the program didn't end");
        std::process::exit(1);
    });

    #[derive(Default)]
    struct Spawned {
        clicks: AtomicUsize,
        popup_clicks: AtomicUsize,
        ticks: AtomicUsize,
        on_main_thread: AtomicBool,
        ended: AtomicBool,
    }

    let failures: Rc<RefCell<Vec<String>>> = Rc::default();
    let passed: Rc<RefCell<Vec<&'static str>>> = Rc::default();
    let (f, p) = (failures.clone(), passed.clone());
    let spawned = Arc::new(Spawned::default());
    let sp = spawned.clone();
    let main_thread = std::thread::current().id();

    let result = anti_webui_app::App::new()
        .title("main")
        .size(300, 200)
        .run(move || {
            let clicks = signal(0);
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
            let in_spawned = |f: &dyn Fn()| {
                t::use_window(Some("spawned"));
                f();
                t::use_window(Some("main"));
            };
            t::use_window(Some("main"));
            let ticks_before = Rc::new(std::cell::Cell::new(0));

            type Step = Box<dyn FnOnce()>;
            let s = |f: Step| f;
            let sp = sp.clone();
            let steps: Vec<Step> = vec![
                s(Box::new({
                    let sp = sp.clone();
                    move || {
                        spawn_window(
                            WindowOptions::new().title("spawned").size(300, 200),
                            move || spawned_root(sp, main_thread),
                        );
                    }
                })),
                s(Box::new(|| {})),
                s(Box::new({
                    let (check, sp) = (check.clone(), sp.clone());
                    move || {
                        let n = t::visible_windows();
                        check("a spawned window opens", n == 2, format!("{n} windows"));
                        t::use_window(Some("spawned"));
                        // Off the main window, which the clicks below are for too.
                        t::move_by(360.0, 0.0);
                        let scale = t::backing_scale();
                        let shows = t::shows_iosurfaces();
                        let size = t::layer_size();
                        t::click(50.0, 20.0);
                        t::use_window(Some("main"));
                        check(
                            "it shows IOSurfaces of its pixels",
                            shows && size == Some((300.0 * scale, 200.0 * scale)),
                            format!("IOSurfaces {shows}, {size:?}"),
                        );
                        let _ = &sp;
                    }
                })),
                s(Box::new({
                    let (check, sp, ticks_before) =
                        (check.clone(), sp.clone(), ticks_before.clone());
                    move || {
                        let c = sp.clicks.load(Ordering::SeqCst);
                        check("a click on it", c == 1, format!("{c}"));
                        check(
                            "its UI runs on its own thread",
                            !sp.on_main_thread.load(Ordering::SeqCst),
                            "on the main thread".into(),
                        );
                        // The main thread is busy; the spawned window's timers go on.
                        ticks_before.set(sp.ticks.load(Ordering::SeqCst));
                        std::thread::sleep(Duration::from_millis(500));
                        let ticks = sp.ticks.load(Ordering::SeqCst) - ticks_before.get();
                        check(
                            "it goes on while the main thread is busy",
                            ticks >= 10,
                            format!("{ticks} ticks in 500 ms"),
                        );
                    }
                })),
                // The steps due while the main thread slept run now.
                s(Box::new(|| {})),
                s(Box::new(move || in_spawned(&|| t::click(150.0, 20.0)))),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        let n = t::visible_windows();
                        check("its popup is a window", n == 3, format!("{n} windows"));
                        // Below its anchor at (150, 0, 100, 40).
                        in_spawned(&|| t::click(190.0, 60.0));
                    }
                })),
                s(Box::new({
                    let (check, sp) = (check.clone(), sp.clone());
                    move || {
                        let c = sp.popup_clicks.load(Ordering::SeqCst);
                        check("a click on its popup", c == 1, format!("{c}"));
                        // Blocks its thread for 600 ms.
                        in_spawned(&|| t::click(50.0, 120.0));
                        t::click(60.0, 20.0);
                    }
                })),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        check(
                            "the main window goes on while the spawned one is busy",
                            clicks.get_untracked() == 1,
                            format!("{}", clicks.get_untracked()),
                        );
                        in_spawned(&|| t::resize(350.0, 250.0));
                    }
                })),
                s(Box::new(|| {})),
                s(Box::new(|| {})),
                s(Box::new({
                    let check = check.clone();
                    move || {
                        t::use_window(Some("spawned"));
                        let size = t::layer_size();
                        let scale = t::backing_scale();
                        t::use_window(Some("main"));
                        let want = (350.0 * scale, 250.0 * scale);
                        check(
                            "it follows its size",
                            size == Some(want),
                            format!("{size:?}, wanted {want:?}"),
                        );
                        close_window();
                    }
                })),
            ];
            let mut at = Duration::from_millis(800);
            for step in steps {
                set_timeout(at, step);
                at += Duration::from_millis(300);
            }

            div().w_full().h_full().bg(rgb(0xf4f4f4)).child(
                div()
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .w(120.0)
                    .h(40.0)
                    .bg(rgb(0x3060d0))
                    .on_click(move |_| clicks.update(|c| *c += 1)),
            )
        });

    let ended = spawned.ended.load(Ordering::SeqCst);
    if ended {
        passed.borrow_mut().push("its thread ends with the program");
    } else {
        failures
            .borrow_mut()
            .push("its thread ends with the program: it didn't".into());
    }
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

    fn spawned_root(
        sp: Arc<Spawned>,
        main_thread: std::thread::ThreadId,
    ) -> impl anti_webui_core::view::IntoView {
        sp.on_main_thread
            .store(std::thread::current().id() == main_thread, Ordering::SeqCst);
        struct End(Arc<Spawned>);
        impl Drop for End {
            fn drop(&mut self) {
                self.0.ended.store(true, Ordering::SeqCst);
            }
        }
        let end = Rc::new(End(sp.clone()));
        // `end` goes with the timers, when the window does.
        #[allow(clippy::only_used_in_recursion)]
        fn tick(sp: Arc<Spawned>, end: Rc<End>) {
            set_timeout(Duration::from_millis(20), move || {
                sp.ticks.fetch_add(1, Ordering::SeqCst);
                tick(sp, end);
            });
        }
        tick(sp.clone(), end);
        let open = signal(false);
        let (a, b, c) = (sp.clone(), sp.clone(), sp);
        div()
            .w_full()
            .h_full()
            .bg(rgb(0xe8f0e8))
            .child(
                div()
                    .absolute()
                    .left(0.0)
                    .top(0.0)
                    .w(120.0)
                    .h(40.0)
                    .bg(rgb(0x30a060))
                    .on_click(move |_| {
                        a.clicks.fetch_add(1, Ordering::SeqCst);
                    }),
            )
            .child(
                div()
                    .absolute()
                    .left(150.0)
                    .top(0.0)
                    .w(100.0)
                    .h(40.0)
                    .bg(rgb(0xd0d0d0))
                    .on_click(move |_| open.set(true))
                    .child(show(
                        move || open.get(),
                        move || {
                            let c = c.clone();
                            popup().on_dismiss(move || open.set(false)).child(
                                div().w(100.0).h(40.0).bg(rgb(0xffffff)).on_click(move |_| {
                                    c.popup_clicks.fetch_add(1, Ordering::SeqCst);
                                }),
                            )
                        },
                    )),
            )
            .child(
                div()
                    .absolute()
                    .left(0.0)
                    .top(100.0)
                    .w(120.0)
                    .h(40.0)
                    .bg(rgb(0xa03030))
                    .on_click(move |_| {
                        let _ = &b;
                        std::thread::sleep(Duration::from_millis(600));
                    }),
            )
    }
}
