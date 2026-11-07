//! A desktop shell: a bar on every output with its workspaces and the window with the keyboard, a note that is as big as what it says, a launcher
//! that takes the keyboard, and an overview of the windows with live thumbnails. Each output runs on
//! a thread of its own. See `docs/design/37-wayland-shell.md`.
//!
//! `cargo run --release -p anti-webui --features wayland-shell --example shell_bar`
//!
//! `ANTIWEBUI_SHELL_SAME=1` puts every output on the main thread instead, and
//! `ANTIWEBUI_CAPTURE=hyprland` takes the thumbnails with Hyprland's protocol when there is a choice.
//! `ANTIWEBUI_SHELL_OVERVIEW=1` starts with the overview open.
//!
//! With `--features shell-all` a bar along the bottom shows the battery, the power profile, CPU and
//! memory, the volume, Wi-Fi, Bluetooth, the brightness, what is playing and the tray, and
//! notifications pop up at the top
//! right, when this is the notification daemon (`dbus-run-session` gives it a bus of its own to try
//! that next to another one).

use anti_webui::prelude::*;
use anti_webui::shell::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

fn main() {
    let same = std::env::var_os("ANTIWEBUI_SHELL_SAME").is_some();
    let result = Shell::new().namespace("anti-webui-bar").run(move || {
        #[cfg(feature = "shell-notifications")]
        notification_popups();
        if same {
            for_each_output(output_ui);
        } else {
            spawn_for_each_output(output_ui);
        }
    });
    if let Err(e) = result {
        eprintln!("{e}");
    }
}

/// What each output shows.
fn output_ui(output: Output) {
    let launcher_open = signal(false);
    let overview_open = signal(std::env::var_os("ANTIWEBUI_SHELL_OVERVIEW").is_some());
    let note = signal(String::from("A note as big as what it says"));
    open_layer(
        LayerOptions::bar(Anchor::TOP, 40).keyboard(Keyboard::OnDemand),
        {
            let output = output.clone();
            move || bar(output, launcher_open, overview_open, note)
        },
    );
    // What the services say, along the bottom, when the example was built with any.
    if cfg!(any(
        feature = "shell-battery",
        feature = "shell-power-profiles",
        feature = "shell-network",
        feature = "shell-bluetooth",
        feature = "shell-session",
        feature = "shell-media",
        feature = "shell-tray",
        feature = "shell-system",
        feature = "shell-audio",
    )) {
        open_layer(LayerOptions::bar(Anchor::BOTTOM, 40), || {
            page().child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .px(12.0)
                    .size_full()
                    .child(services()),
            )
        });
    }
    open_layer(
        LayerOptions::new()
            .anchor(Anchor::BOTTOM)
            .margin(Margin {
                bottom: 24,
                ..Default::default()
            })
            .click_through(),
        move || page().p(12.0).child(text(move || note.get())),
    );
    // The launcher is there while it is open, and closes with the effect that opened it.
    effect(move || {
        if launcher_open.get() {
            open_layer(
                LayerOptions::new()
                    .layer(Layer::Overlay)
                    .keyboard(Keyboard::Exclusive)
                    .size(480, 140),
                move || launcher(launcher_open, note),
            );
        }
    });
    effect(move || {
        if overview_open.get() {
            let scale = output.scale;
            open_layer(
                LayerOptions::new()
                    .layer(Layer::Overlay)
                    .keyboard(Keyboard::Exclusive)
                    .size(1040, 640),
                move || overview(overview_open, scale),
            );
        }
    });
}

fn bar(
    output: Output,
    launcher_open: Signal<bool>,
    overview_open: Signal<bool>,
    note: Signal<String>,
) -> Div {
    let size = signal(1);
    let thread = format!("{:?}", std::thread::current().id());
    let away = idle(Duration::from_secs(10));
    // The workspaces of this output; clicking one shows it.
    let on = output.clone();
    let workspace_buttons = dyn_view(move || {
        let mut row = div().flex().flex_row().gap(4.0);
        for w in workspaces().on(&on) {
            let name = w.name().to_owned();
            let b = if w.is_active() {
                primary_button(name)
            } else {
                button(name)
            };
            row = row.child(b.on_click(move |_| {
                w.activate();
            }));
        }
        row
    });
    page().child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(12.0)
            .px(12.0)
            .size_full()
            .whitespace_nowrap()
            .overflow_hidden()
            .child(workspace_buttons)
            .child(label(format!(
                "{} ×{:.2} on {thread}",
                output.name, output.scale
            )))
            .child(text(move || {
                let title = toplevels().active().map(|t| t.title).unwrap_or_default();
                title.chars().take(24).collect::<String>()
            }))
            .child(div().flex_1())
            .child(text(|| local_time(Every::Second).format("%H:%M:%S")))
            .child(text(move || if away.is_idle() { "(away)" } else { "" }))
            .child(div().flex_1())
            .child(select(
                size,
                vec![
                    (0, "Small".into()),
                    (1, "Medium".into()),
                    (2, "Large".into()),
                ],
            ))
            .child(button("Longer note").on_click(move |_| {
                note.update(|n| n.push_str(", and more"));
            }))
            .child(button("Launcher").on_click(move |_| launcher_open.update(|o| *o = !*o)))
            .child(button("Windows").on_click(move |_| overview_open.update(|o| *o = !*o)))
            .child(button("Lock").on_click(|_| {
                lock();
            }))
            .child(button("Quit").on_click(|_| quit())),
    )
}

fn launcher(open: Signal<bool>, note: Signal<String>) -> Div {
    let query = signal(String::new());
    let input = node_ref();
    request_frame(move |_| input.focus());
    page()
        .p(16.0)
        .gap(12.0)
        .child(label("Type a note, Enter to show it, Escape to close"))
        .child(
            text_field(query)
                .node_ref(input)
                .on_submit(move |text| {
                    note.set(text.to_owned());
                    open.set(false);
                })
                .on_key_down(move |e| {
                    if e.key == Key::Named(NamedKey::Escape) {
                        open.set(false);
                    }
                }),
        )
}

/// The windows, each as a live thumbnail; clicking one brings it up.
fn overview(open: Signal<bool>, scale: f32) -> Div {
    let prefer = match std::env::var("ANTIWEBUI_CAPTURE").as_deref() {
        Ok("hyprland") => CaptureSource::Hyprland,
        _ => CaptureSource::Ext,
    };
    // The thumbnails are 240 by 150 logical pixels: taken at that many of the output's.
    let (w, h) = (240.0, 150.0);
    let options = CaptureOptions::default()
        .max_size((w * scale) as u32, (h * scale) as u32)
        .every(Duration::from_millis(250))
        .prefer(prefer);
    let root = node_ref();
    request_frame(move |_| root.focus());
    page()
        .node_ref(root)
        .p(16.0)
        .gap(12.0)
        .on_key_down(move |e| {
            if e.key == Key::Named(NamedKey::Escape) {
                open.set(false);
            }
        })
        .child(label("Windows: click one to bring it up, Escape to close"))
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap(12.0)
                .child(for_each(
                    || toplevels().all(),
                    |t| t.id(),
                    move |t| {
                        // Kept while the window is: a new title doesn't take a new picture.
                        let shot = capture(&t, options.clone());
                        let id = t.id();
                        div()
                            .flex()
                            .flex_col()
                            .gap(4.0)
                            .w(w)
                            .child(
                                bitmap_view(move || shot.picture())
                                    .w(w)
                                    .h(h)
                                    .rounded(6.0)
                                    .object_fit(ObjectFit::Contain)
                                    .bg(rgb(0x202020)),
                            )
                            .child(text(move || {
                                let all = toplevels().all();
                                let title = all.iter().find(|t| t.id() == id).map(|t| &t.title);
                                title
                                    .map(|t| t.chars().take(32).collect::<String>())
                                    .unwrap_or_default()
                            }))
                            .on_click(move |_| {
                                t.activate();
                                open.set(false);
                            })
                    },
                )),
        )
}

/// What the D-Bus services the example was built with say.
fn services() -> Div {
    #[allow(unused_mut)]
    // The bar is narrow for all of this: nothing wraps or gives way.
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(12.0)
        .flex_none()
        .whitespace_nowrap();
    #[cfg(feature = "shell-battery")]
    {
        let b = battery();
        row = row.child(text(move || {
            if !b.available() {
                return String::new();
            }
            let plug = if b.charging() { "+" } else { "" };
            format!("{:.0}%{plug}", b.percentage())
        }));
    }
    #[cfg(feature = "shell-power-profiles")]
    {
        let p = power_profiles();
        row = row.child(button(move || p.active()).on_click(move |_| {
            // Goes round the profiles.
            let all = p.profiles();
            let at = all.iter().position(|x| *x == p.active()).unwrap_or(0);
            if let Some(next) = all.get((at + 1) % all.len().max(1)) {
                p.set_active(next);
            }
        }));
    }
    #[cfg(feature = "shell-system")]
    {
        let sys = system();
        row = row.child(text(move || {
            let m = sys.memory();
            let gib = |b: u64| b as f64 / (1u64 << 30) as f64;
            format!(
                "CPU {:.0}% · {:.1}/{:.0} GiB",
                sys.cpu() * 100.0,
                gib(m.used),
                gib(m.total)
            )
        }));
    }
    #[cfg(feature = "shell-audio")]
    {
        let a = audio();
        row = row.child(dyn_view(move || {
            let Some(sink) = a.default_sink() else {
                return div();
            };
            let shown = if sink.muted {
                "muted".to_owned()
            } else {
                format!("vol {:.0}%", sink.volume * 100.0)
            };
            let (down, up) = (sink.clone(), sink.clone());
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(4.0)
                .child(button("-").on_click(move |_| {
                    down.set_volume((down.volume - 0.05).max(0.0));
                }))
                .child(button(shown).on_click(move |_| {
                    sink.set_muted(!sink.muted);
                }))
                .child(button("+").on_click(move |_| {
                    up.set_volume((up.volume + 0.05).min(1.0));
                }))
        }));
    }
    #[cfg(feature = "shell-network")]
    {
        let net = network();
        row = row.child(text(move || match net.wifi() {
            Some(ap) => format!("{} {}%", ap.ssid, ap.strength),
            None if net.wired().is_some_and(|w| w.connected) => "wired".into(),
            None if net.available() => "offline".into(),
            None => String::new(),
        }));
    }
    #[cfg(feature = "shell-bluetooth")]
    {
        let bt = bluetooth();
        row = row.child(text(move || {
            let on = bt.adapter().is_some_and(|a| a.powered);
            let connected = bt.devices().iter().filter(|d| d.connected).count();
            match (bt.available(), on) {
                (false, _) => String::new(),
                (true, false) => "BT off".into(),
                (true, true) => format!("BT {connected}"),
            }
        }));
    }
    #[cfg(feature = "shell-session")]
    {
        let s = session();
        row = row.child(text(move || {
            s.brightness()
                .map(|b| format!("☀{:.0}%", b * 100.0))
                .unwrap_or_default()
        }));
    }
    #[cfg(feature = "shell-media")]
    {
        let m = media();
        row = row.child(dyn_view(move || {
            let Some(p) = m.current() else {
                return div();
            };
            let mark = if p.status == PlaybackStatus::Playing {
                "⏸"
            } else {
                "▶"
            };
            let what = format!("{} – {}", p.title, p.artists.join(", "));
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(4.0)
                .child(button(mark).on_click(move |_| {
                    p.play_pause();
                }))
                .child(label(what.chars().take(40).collect::<String>()))
        }));
    }
    #[cfg(feature = "shell-tray")]
    {
        row = row.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(4.0)
                .child(for_each(|| tray().items(), |item| item.key(), tray_icon)),
        );
    }
    row
}

/// An item of the tray: its icon, activated by a click, with its menu on the secondary button.
#[cfg(feature = "shell-tray")]
fn tray_icon(item: TrayItem) -> Div {
    let open = signal(false);
    let key = item.key();
    // The item as it is now, for an icon that changes.
    let now = move || tray().items().into_iter().find(|i| i.key() == key);
    let shown = now.clone();
    let clicked = item.clone();
    div()
        .w(24.0)
        .h(24.0)
        .child(dyn_view(move || {
            match shown().and_then(|i| i.icon.best(48)) {
                Some(picture) => bitmap_view(move || Some(picture.clone()))
                    .w(24.0)
                    .h(24.0)
                    .into_view(),
                // No picture: the first letter of its title.
                None => label(
                    shown()
                        .and_then(|i| i.title.chars().next())
                        .map(String::from)
                        .unwrap_or_default(),
                )
                .into_view(),
            }
        }))
        .on_pointer_down(move |e| match e.button {
            Some(PointerButton::Secondary) if clicked.has_menu => open.set(true),
            Some(PointerButton::Primary) if clicked.item_is_menu => open.set(true),
            Some(PointerButton::Primary) => {
                clicked.activate(0, 0);
            }
            _ => {}
        })
        .child(tray_menu(&item, open))
}

/// Notifications as they come, at the top right of the first output, when this is the
/// notification daemon.
#[cfg(feature = "shell-notifications")]
fn notification_popups() {
    let server = match notification_server() {
        Ok(server) => server,
        Err(e) => {
            eprintln!("no notifications: {e}");
            return;
        }
    };
    open_layer(
        LayerOptions::new()
            .layer(Layer::Overlay)
            .anchor(Anchor::TOP | Anchor::RIGHT)
            .margin(Margin {
                top: 48,
                right: 12,
                ..Default::default()
            })
            .width(360)
            .fit_content()
            .input_region(InputRegion::Full),
        move || {
            let server = server.clone();
            div().flex().flex_col().gap(8.0).child(for_each(
                move || server.popups(),
                |n| n.id,
                |n| {
                    let mut card = page()
                        .p(12.0)
                        .gap(4.0)
                        .rounded(8.0)
                        .child(label(format!("{} · {}", n.app_name, n.summary)))
                        .child(text(n.body_text()));
                    let mut actions = div().flex().flex_row().gap(4.0);
                    for (key, name) in n.actions.clone() {
                        let n = n.clone();
                        actions = actions.child(button(name).on_click(move |_| n.invoke(&key)));
                    }
                    let gone = n.clone();
                    card = card
                        .child(actions)
                        .child(button("Dismiss").on_click(move |_| gone.dismiss()));
                    card
                },
            ))
        },
    );
}

/// Locks the session with a clock and a button on every output. A real lock screen would ask for
/// a password (PAM) before unlocking; this one only shows how it goes. If this program dies while
/// it is locked, the compositor keeps it locked, as the protocol says.
fn lock() {
    let lock: Rc<RefCell<Option<SessionLock>>> = Rc::default();
    let unlock = lock.clone();
    *lock.borrow_mut() = Some(lock_session(move |output: Output| {
        let unlock = unlock.clone();
        page()
            .size_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(16.0)
            .child(text(|| local_time(Every::Second).format("%H:%M:%S")).text_size(64.0))
            .child(label(format!("Locked ({})", output.name)))
            .child(primary_button("Unlock").on_click(move |_| {
                if let Some(l) = unlock.borrow_mut().take() {
                    l.unlock();
                }
            }))
    }));
}
