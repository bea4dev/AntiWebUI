//! How long reading the text of a PDF takes, without a window: `bench_pdf_text <file.pdf> [--dump
//! out.txt] [--no-copy] [--search word]...`. It copies all of it, which goes through all of the
//! pages, and then searches from the first page, which goes through those it hasn't been through.
//! What it prints is one line of JSON for each. See `docs/design/26-pdf.md`.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::platform::PlatformRequest;
use anti_webui_core::prelude::*;
use anti_webui_pdf::{PdfState, PdfViewer, pdf_view, pdf_viewer};
use std::rc::Rc;
use std::time::{Duration, Instant};

const FRAME: Duration = Duration::from_micros(8333);

fn memory_kib(field: &str) -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    status
        .lines()
        .find(|l| l.starts_with(field))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0)
}

/// Goes on frame by frame until `done`, and says how long it took and the longest frame.
fn run(ui: &mut Ui, mut done: impl FnMut(&mut Ui) -> bool) -> (f64, f64) {
    let start = Instant::now();
    let mut longest = 0.0f64;
    loop {
        let frame = Instant::now();
        ui.animate(frame);
        ui.update();
        let _ = ui.paint(frame);
        let took = frame.elapsed();
        longest = longest.max(took.as_secs_f64() * 1e3);
        if done(ui) {
            return (start.elapsed().as_secs_f64() * 1e3, longest);
        }
        std::thread::sleep(FRAME.saturating_sub(took));
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let path = args[1].clone();
    let value = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let searches: Vec<String> = args
        .windows(2)
        .filter(|w| w[0] == "--search")
        .map(|w| w[1].clone())
        .collect();

    let handed: Rc<std::cell::Cell<Option<PdfViewer>>> = Rc::default();
    let mut ui = {
        let handed = handed.clone();
        Ui::new(move || {
            let viewer = pdf_viewer();
            handed.set(Some(viewer));
            pdf_view(path).viewer(viewer).w_full().h_full()
        })
    };
    let viewer = handed.get().unwrap();
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(2340, 1616),
        scale: Scale(216),
    });
    run(&mut ui, |_| untrack(|| viewer.state()) == PdfState::Ready);
    // Let the pictures that show come first, as they would.
    run(&mut ui, |ui| {
        ui.wait_for_images(Duration::from_secs(60));
        true
    });
    let before = memory_kib("VmRSS");

    if !args.iter().any(|a| a == "--no-copy") {
        viewer.select_all();
        viewer.copy();
        let mut text = None;
        let (ms, longest) = run(&mut ui, |ui| {
            text = ui.take_requests().into_iter().find_map(|r| match r {
                PlatformRequest::SetClipboard { text, .. } => Some(text),
                _ => None,
            });
            text.is_some()
        });
        let text = text.unwrap();
        println!(
            "{{\"what\":\"copy all\",\"ms\":{ms:.1},\"longest_frame_ms\":{longest:.2},\"chars\":{},\"rss_kib\":{}}}",
            text.chars().count(),
            memory_kib("VmRSS") as i64 - before as i64,
        );
        if let Some(out) = value("--dump") {
            std::fs::write(out, &text).unwrap();
        }
        viewer.clear_selection();
    }

    for query in searches {
        for round in ["first", "again"] {
            viewer.search(query.clone());
            let mut first_ms = None;
            let start = Instant::now();
            let (ms, longest) = run(&mut ui, |_| {
                if first_ms.is_none() && untrack(|| viewer.search_count()) > 0 {
                    first_ms = Some(start.elapsed().as_secs_f64() * 1e3);
                }
                untrack(|| viewer.search_done())
            });
            println!(
                "{{\"what\":\"search {round}\",\"query\":{query:?},\"ms\":{ms:.1},\"first_hit_ms\":{:.1},\"longest_frame_ms\":{longest:.2},\"hits\":{},\"rss_kib\":{}}}",
                first_ms.unwrap_or(0.0),
                untrack(|| viewer.search_count()),
                memory_kib("VmRSS") as i64 - before as i64,
            );
            viewer.search("");
            run(&mut ui, |_| true);
        }
    }
}
