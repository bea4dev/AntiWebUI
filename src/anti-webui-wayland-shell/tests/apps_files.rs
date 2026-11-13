//! Applications from `.desktop` files in directories of the test's own: what shows, shadowing,
//! languages, searching, icons, following changes and launching. Run with `--features apps`.

#![cfg(target_os = "linux")]
#![cfg(feature = "apps")]

use anti_webui_wayland_shell::{apps, icon_path};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::{Duration, Instant};

fn until(what: &str, mut done: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !done() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        anti_webui_core::channel::pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn entry(name: &str, extra: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nExec=true\n{extra}\n")
}

#[test]
fn applications_are_read_searched_followed_and_launched() {
    let root = std::env::temp_dir().join(format!("anti-webui-apps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (home, system) = (root.join("home"), root.join("system"));
    let launched = root.join("launched");
    // SAFETY: one test in this binary, before anything reads the environment.
    unsafe {
        std::env::set_var("HOME", &root);
        std::env::set_var("XDG_DATA_HOME", &home);
        std::env::set_var("XDG_DATA_DIRS", &system);
        std::env::set_var("XDG_CONFIG_HOME", root.join("config"));
        std::env::set_var("XDG_CURRENT_DESKTOP", "ShojiWM");
        std::env::set_var("LANG", "ja_JP.UTF-8");
        std::env::remove_var("LC_ALL");
        std::env::remove_var("LC_MESSAGES");
    }
    let apps_dir = home.join("applications");
    write(
        &apps_dir.join("alpha.desktop"),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Alpha Editor\nName[ja]=アルファ エディタ\nKeywords=write;\nIcon=alpha\nExec=sh -c \"echo %c > {}\"\n",
            launched.display()
        ),
    );
    write(
        &apps_dir.join("hidden.desktop"),
        &entry("Hidden", "NoDisplay=true"),
    );
    write(&apps_dir.join("kde/sub.desktop"), &entry("Sub", ""));
    write(
        &apps_dir.join("kdeonly.desktop"),
        &entry("KDE Only", "OnlyShowIn=KDE;"),
    );
    write(
        &apps_dir.join("ours.desktop"),
        &entry("Ours", "OnlyShowIn=ShojiWM;"),
    );
    write(
        &apps_dir.join("notours.desktop"),
        &entry("Not Ours", "NotShowIn=ShojiWM;"),
    );
    // Hidden by the one of the same id in the data home.
    write(
        &system.join("applications/alpha.desktop"),
        &entry("Shadowed", ""),
    );
    write(
        &system.join("applications/beta.desktop"),
        &entry("Beta Browser", "GenericName=Web Browser"),
    );

    let a = apps();
    until("the applications", || a.loaded());
    let names: Vec<String> = a.all().into_iter().map(|a| a.name).collect();
    assert_eq!(names, ["Beta Browser", "Ours", "Sub", "アルファ エディタ"]);
    assert_eq!(a.get("kde-sub.desktop").unwrap().name, "Sub");
    assert!(a.get("hidden.desktop").is_none());

    // Searching.
    let ids = |q: &str| a.search(q).into_iter().map(|a| a.id).collect::<Vec<_>>();
    assert_eq!(ids("bet"), ["beta.desktop"]);
    assert_eq!(ids("browser"), ["beta.desktop"]);
    // The generic name comes before the command, which has the temporary directory in it.
    assert_eq!(ids("web"), ["beta.desktop", "alpha.desktop"]);
    assert_eq!(ids("write"), ["alpha.desktop"]);
    assert_eq!(ids("").len(), 4);

    // Icons: the theme and hicolor in the data home, a pixmap, a path.
    let icons = home.join("icons/hicolor");
    write(
        &icons.join("index.theme"),
        "[Icon Theme]\nName=Hicolor\nDirectories=48x48/apps,scalable/apps\n\n[48x48/apps]\nSize=48\nType=Fixed\n\n[scalable/apps]\nSize=64\nType=Scalable\nMinSize=16\nMaxSize=512\n",
    );
    write(&icons.join("48x48/apps/alpha.png"), "png");
    write(&icons.join("scalable/apps/alpha.svg"), "svg");
    write(&icons.join("scalable/apps/beta.svg"), "svg");
    let alpha = a.get("alpha.desktop").unwrap();
    assert_eq!(
        alpha.icon_path(48),
        Some(icons.join("48x48/apps/alpha.png"))
    );
    assert_eq!(
        icon_path("alpha", 128),
        Some(icons.join("scalable/apps/alpha.svg"))
    );
    assert_eq!(
        icon_path("beta.svg", 48),
        Some(icons.join("scalable/apps/beta.svg"))
    );
    let abs = icons.join("48x48/apps/alpha.png");
    assert_eq!(icon_path(abs.to_str().unwrap(), 16), Some(abs.clone()));
    assert_eq!(icon_path("nothing-like-this", 48), None);
    // XPM isn't decoded, so it isn't found: what stands for a missing icon shows instead.
    write(&icons.join("48x48/apps/old.xpm"), "xpm");
    write(&icons.join("48x48/apps/older.xpm"), "xpm");
    write(&icons.join("48x48/apps/older.png"), "png");
    assert_eq!(icon_path("old", 48), None);
    assert_eq!(
        icon_path("older.xpm", 48),
        Some(icons.join("48x48/apps/older.png"))
    );
    let xpm = icons.join("48x48/apps/old.xpm");
    assert_eq!(icon_path(xpm.to_str().unwrap(), 48), None);

    // A new file is seen.
    write(&apps_dir.join("gamma.desktop"), &entry("Gamma", ""));
    until("the new application", || a.get("gamma.desktop").is_some());
    std::fs::remove_file(apps_dir.join("ours.desktop")).unwrap();
    until("the removed application to go", || {
        a.get("ours.desktop").is_none()
    });

    // Launching runs the command, with the field codes filled.
    let done = Rc::new(RefCell::new(None));
    let d = done.clone();
    alpha.launch().then(move |r| *d.borrow_mut() = Some(r));
    until("the launch", || done.borrow().is_some());
    assert_eq!(*done.borrow(), Some(Ok(())));
    until("what it wrote", || {
        std::fs::read_to_string(&launched).is_ok_and(|t| !t.is_empty())
    });
    assert_eq!(
        std::fs::read_to_string(&launched).unwrap().trim(),
        "アルファ エディタ"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Reads this machine's applications and icons, changing nothing. Run alone:
/// `-- --ignored this_machine`.
#[test]
#[ignore = "reads this machine's applications"]
fn this_machine() {
    let a = apps();
    until("the applications", || a.loaded());
    let all = a.all();
    let with_icons = all.iter().filter(|app| app.icon_path(48).is_some()).count();
    eprintln!("{} applications, {with_icons} with icons", all.len());
    for q in ["fire", "term", "vsc", "files"] {
        let found: Vec<String> = a.search(q).into_iter().take(3).map(|a| a.name).collect();
        eprintln!("{q:?}: {found:?}");
    }
    if let Some(app) = all.first() {
        eprintln!("{}: {:?}", app.name, app.icon_path(48));
    }
}
