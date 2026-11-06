//! Measures the Mermaid backend on the diagrams in `MERMAID_DIR` (run with `--ignored`): how long
//! making each SVG takes, how big it is, and how long it takes to become nodes and be laid out. The
//! SVGs go to `MERMAID_OUT` as `<MERMAID_LABEL>-<name>.svg`.

use anti_webui_core::Ui;
use anti_webui_core::event::WindowEvent;
use anti_webui_core::geometry::{Scale, SizeI};
use anti_webui_core::prelude::*;
use anti_webui_core::tree::with_doc;
use anti_webui_markdown::mermaid_svg;
use std::time::{Duration, Instant};

#[test]
#[ignore]
fn measure_the_backend() {
    let (Ok(dir), Ok(out)) = (std::env::var("MERMAID_DIR"), std::env::var("MERMAID_OUT")) else {
        eprintln!("MERMAID_DIR and MERMAID_OUT say where the diagrams are and where SVG goes");
        return;
    };
    let label = std::env::var("MERMAID_LABEL").unwrap_or_else(|_| "backend".into());
    let mut names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "mmd"))
        .collect();
    names.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    // The text system starts once for the thread, which is not the diagrams' doing.
    let _ = svg::measure_text("warm", &Style::default());
    for path in names {
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        let source = std::fs::read_to_string(&path).unwrap();
        let cold = Instant::now();
        let made = mermaid_svg(&source);
        let cold = cold.elapsed();
        let svg = match made {
            Ok(svg) => svg,
            Err(e) => {
                println!("{label} {name}: failed: {}", e.lines().next().unwrap_or(""));
                continue;
            }
        };
        let mut times: Vec<f64> = (0..10)
            .map(|_| {
                let t = Instant::now();
                let _ = mermaid_svg(&source);
                ms(t.elapsed())
            })
            .collect();
        times.sort_by(f64::total_cmp);
        std::fs::write(format!("{out}/{label}-{name}.svg"), &svg).unwrap();
        // As nodes, laid out at the scale of the screen here.
        let t = Instant::now();
        let mut ui2 = {
            let svg = svg.clone();
            Ui::new(move || match svg::parse(svg.as_bytes()) {
                Ok(p) => p.into_view(),
                Err(_) => div().into_view(),
            })
        };
        ui2.handle_event(WindowEvent::Resized {
            size: SizeI::new(1000, 900),
            scale: Scale(216),
        });
        ui2.update();
        let _ = ui2.paint(Instant::now());
        let shown = t.elapsed();
        let nodes = with_doc(|d| d.nodes.len());
        drop(ui2);
        println!(
            "{label} {name}: made {:.2} ms (first {:.2}), {} KB of SVG, shown {:.2} ms, {nodes} nodes",
            times[times.len() / 2],
            ms(cold),
            svg.len() / 1024,
            ms(shown)
        );
    }
}

/// Lists the text nodes `svg::parse` makes of `SVG_FILE` (run with `--ignored`).
#[test]
#[ignore]
fn list_the_text_of_an_svg() {
    let Ok(path) = std::env::var("SVG_FILE") else {
        return;
    };
    let data = std::fs::read(path).unwrap();
    let mut ui = Ui::new(move || svg::parse(&data).unwrap());
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(1000, 900),
        scale: Scale(120),
    });
    ui.update();
    with_doc(|d| {
        for (_, n) in d.nodes.iter() {
            match &n.kind {
                anti_webui_core::tree::NodeKind::Text(t) => {
                    println!("TEXT {:?} at {:?}", t.text, n.abs);
                }
                anti_webui_core::tree::NodeKind::SvgShape(shape) => {
                    let parent_opacity =
                        n.parent.and_then(|p| d.get(p)).map(|p| p.computed.opacity);
                    println!(
                        "SHAPE at {:?} geometry {:?} fill {:?} {} stroke {:?} opacity {} parent {:?} children {}",
                        n.subtree_bounds,
                        shape.geometry.as_ref().map(|g| g.bounds()),
                        n.computed.fill_paint,
                        n.computed.fill_opacity,
                        n.computed.stroke_paint,
                        n.computed.opacity,
                        parent_opacity,
                        n.children.len()
                    );
                }
                _ => {}
            }
        }
    });
}
