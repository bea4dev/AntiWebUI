//! The performance overlay: what frames take and how they are paced, at the top right of the
//! window. It is primitives added to the window's scene, not nodes, so it has no part in the app's
//! layout or events. See `docs/design/30-performance-overlay.md`.

use crate::geometry::{PointI, RectI, Scale};
use crate::scene::{ChunkKey, Clip, Glyph, GlyphRun, Primitive, Quad, Scene};
use crate::text::{ShapeStyle, TextLayout, with_text};
use anti_webui_style::{Color, ComputedStyle, FontFamily};
use parley::PositionedLayoutItem;
use std::collections::VecDeque;

/// How many frames the graphs show.
const SHOWN: usize = 90;
/// How many frames the numbers are the average of.
const AVERAGED: usize = 30;
/// A longer time between frames shown is a pause, which says nothing of how fast frames come.
const PAUSE_MS: f32 = 100.0;
/// The chunk of the overlay, which no node has.
const CHUNK: ChunkKey = ChunkKey {
    node: u64::MAX,
    part: 0,
};

/// Logical pixels.
const MARGIN: f32 = 8.0;
const WIDTH: f32 = 240.0;
/// How many letters of the monospace font fit across.
const LETTERS: usize = 34;
const PADDING: f32 = 6.0;
const FONT_SIZE: f32 = 11.0;
const LINE: f32 = 14.0;
const GRAPH: f32 = 28.0;
const GAP: f32 = 4.0;

const BACKGROUND: Color = Color::from_rgba(0x101418d0);
const TEXT: Color = Color::from_rgb(0xf0f3f6);
const FAINT: Color = Color::from_rgba(0xf0f3f640);
const GOOD: Color = Color::from_rgb(0x3fb950);
const LATE: Color = Color::from_rgb(0xf85149);
const CPU: Color = Color::from_rgb(0x58a6ff);
const GPU: Color = Color::from_rgb(0xf0883e);

/// As many letters of a line as fit in the panel.
fn fit(line: String) -> String {
    if line.chars().count() <= LETTERS {
        return line;
    }
    let mut short: String = line.chars().take(LETTERS - 1).collect();
    short.push('…');
    short
}

/// What a frame took, in milliseconds.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSample {
    /// Since the frame shown before it, if that one was shown too.
    pub interval: Option<f32>,
    /// From the start of its work to handing it to the compositor.
    pub cpu: f32,
    /// The part of `cpu` that was the UI: styles, layout and the scene.
    pub ui: f32,
    /// What the GPU spent on it, if that is known.
    pub gpu: Option<f32>,
    /// From the start of its work to the screen, if the compositor said.
    pub latency: Option<f32>,
    /// How often the screen refreshes, if known.
    pub refresh: Option<f32>,
}

/// The frames and the state of the pacing, and how they were last drawn.
#[derive(Default)]
pub struct PerformanceOverlay {
    frames: VecDeque<FrameSample>,
    /// What renders ahead, as words: `Auto: on`, `Disable`.
    advance: String,
    /// What draws: the GPU and how frames reach the compositor, and the CPU, a line each.
    devices: Vec<String>,
    /// The lines of text last shaped, kept while they stay the same.
    shaped: Vec<(String, TextLayout)>,
}

impl PerformanceOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, frame: FrameSample) {
        if self.frames.len() == SHOWN {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    /// The frames it has, oldest first.
    pub fn frames(&self) -> impl Iterator<Item = &FrameSample> {
        self.frames.iter()
    }

    /// What draws, a line each: the GPU, how frames reach the compositor, the CPU. Lines too
    /// long for the panel are cut short.
    pub fn set_devices(&mut self, lines: Vec<String>) {
        self.devices = lines;
    }

    /// How rendering ahead is set and what it is doing.
    pub fn set_advance(&mut self, advance: impl Into<String>) {
        self.advance = advance.into();
    }

    /// The lines of numbers, from the latest frames.
    fn lines(&self) -> Vec<String> {
        let recent = || self.frames.iter().rev().take(AVERAGED);
        let mean = |v: Vec<f32>| (!v.is_empty()).then(|| v.iter().sum::<f32>() / v.len() as f32);
        let interval = mean(
            recent()
                .filter_map(|f| f.interval)
                .filter(|i| *i < PAUSE_MS)
                .collect(),
        );
        let cpu = mean(recent().map(|f| f.cpu).collect());
        let ui = mean(recent().map(|f| f.ui).collect());
        let gpu = mean(recent().filter_map(|f| f.gpu).collect());
        let latency = mean(recent().filter_map(|f| f.latency).collect());
        let ms = |v: Option<f32>| v.map_or("    -".to_owned(), |v| format!("{v:5.1}"));
        vec![
            match interval {
                Some(i) => format!("{:4.0} fps {i:6.1} ms", 1000.0 / i.max(0.01)),
                None => "   - fps".to_owned(),
            },
            format!("CPU {} ms (UI {:.1})", ms(cpu), ui.unwrap_or(0.0)),
            format!("GPU {} ms", ms(gpu)),
            format!("latency {} ms", ms(latency)),
            format!("Advance {}", self.advance),
        ]
        .into_iter()
        .chain(self.devices.iter().cloned().map(fit))
        .collect()
    }

    /// Adds the overlay at the top right of `scene`, over everything.
    pub fn paint(&mut self, scene: &mut Scene, scale: Scale) {
        let px = |v: f32| scale.px(v);
        let lines = self.lines();
        let width = px(WIDTH);
        let inner = width - 2 * px(PADDING);
        let height = 2 * px(PADDING) + lines.len() as i32 * px(LINE) + 2 * (px(GAP) + px(GRAPH));
        let left = scene.width as i32 - px(MARGIN) - width;
        let top = px(MARGIN);
        scene.begin_chunk(CHUNK);
        scene.primitives.push(Primitive::Quad(Quad {
            rect: RectI::new(left, top, width, height),
            background: BACKGROUND,
            border_widths: [0; 4],
            border_color: Color::TRANSPARENT,
            radii: [scale.pxf(4.0); 4],
            clip: Clip::NONE,
        }));
        let (x, mut y) = (left + px(PADDING), top + px(PADDING));
        self.text(scene, &lines, PointI::new(x, y), scale);
        y += lines.len() as i32 * px(LINE) + px(GAP);

        // The scale of both graphs: twice a refresh, or two refreshes at 60 Hz if it isn't known.
        let refresh = self.frames.iter().rev().find_map(|f| f.refresh);
        let top_ms = refresh.unwrap_or(1000.0 / 60.0) * 2.0;
        let graph = px(GRAPH);
        let bar = |ms: f32| ((ms / top_ms).clamp(0.0, 1.0) * graph as f32).round() as i32;
        let slot = inner as f32 / SHOWN as f32;
        let first = SHOWN - self.frames.len();
        let column = |i: usize| {
            let a = x + ((first + i) as f32 * slot).round() as i32;
            let b = x + ((first + i + 1) as f32 * slot).round() as i32;
            (a, (b - a - 1).max(1))
        };
        let refresh_line = |scene: &mut Scene, y: i32| {
            if let Some(r) = refresh {
                scene.fill(
                    RectI::new(x, y + graph - bar(r), inner, 1),
                    FAINT,
                    Clip::NONE,
                );
            }
        };
        // How far apart the frames came.
        for (i, f) in self.frames.iter().enumerate() {
            let Some(interval) = f.interval else { continue };
            let late = f.refresh.is_some_and(|r| interval > r * 1.5);
            let h = bar(interval).max(1);
            let (cx, w) = column(i);
            let color = if late { LATE } else { GOOD };
            scene.fill(RectI::new(cx, y + graph - h, w, h), color, Clip::NONE);
        }
        refresh_line(scene, y);
        y += graph + px(GAP);
        // What CPU and GPU took, side by side.
        for (i, f) in self.frames.iter().enumerate() {
            let (cx, w) = column(i);
            let half = (w / 2).max(1);
            let h = bar(f.cpu).max(1);
            scene.fill(RectI::new(cx, y + graph - h, half, h), CPU, Clip::NONE);
            if let Some(gpu) = f.gpu {
                let h = bar(gpu).max(1);
                scene.fill(
                    RectI::new(cx + half, y + graph - h, (w - half).max(1), h),
                    GPU,
                    Clip::NONE,
                );
            }
        }
        refresh_line(scene, y);
    }

    /// The lines of text, one under another from `at`, shaped again only where they changed.
    fn text(&mut self, scene: &mut Scene, lines: &[String], at: PointI, scale: Scale) {
        let style = ShapeStyle::from_computed(
            &ComputedStyle {
                font_family: FontFamily::from("monospace"),
                font_size: FONT_SIZE,
                ..ComputedStyle::default()
            },
            scale,
        );
        self.shaped.resize_with(lines.len(), Default::default);
        with_text(|ts| {
            for (i, line) in lines.iter().enumerate() {
                let (kept, layout) = &mut self.shaped[i];
                if kept != line {
                    *kept = line.clone();
                    // A new version makes it shape again.
                    *layout = TextLayout::default();
                    layout.shape(ts, line, 1, &style);
                    layout.break_lines(None);
                }
            }
        });
        for (i, (_, layout)) in self.shaped.iter().enumerate() {
            let oy = (at.y + i as i32 * scale.px(LINE)) as f32;
            for l in layout.layout.lines() {
                for item in l.items() {
                    let PositionedLayoutItem::GlyphRun(run) = item else {
                        continue;
                    };
                    let glyphs: Vec<Glyph> = run
                        .positioned_glyphs()
                        .map(|g| Glyph {
                            id: g.id,
                            x: at.x as f32 + g.x,
                            y: (oy + g.y).round(),
                        })
                        .collect();
                    let r = run.run();
                    let synthesis = r.synthesis();
                    scene.primitives.push(Primitive::Glyphs(GlyphRun {
                        font: r.font().clone(),
                        size: r.font_size(),
                        normalized_coords: crate::paint::rounded_coords(
                            r.normalized_coords().iter().map(|c| c.to_bits()),
                        ),
                        embolden: synthesis.embolden(),
                        skew: synthesis.skew(),
                        color: TEXT,
                        glyphs,
                        clip: Clip::NONE,
                    }));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(interval: f32, cpu: f32, gpu: Option<f32>) -> FrameSample {
        FrameSample {
            interval: Some(interval),
            cpu,
            ui: cpu / 2.0,
            gpu,
            latency: Some(20.0),
            refresh: Some(1000.0 / 60.0),
        }
    }

    #[test]
    fn the_numbers_are_averages_without_pauses() {
        let mut o = PerformanceOverlay::new();
        o.set_advance("Auto: on");
        for _ in 0..10 {
            o.push(frame(1000.0 / 60.0, 3.0, Some(1.0)));
        }
        // A pause says nothing of how fast frames come.
        o.push(frame(500.0, 3.0, None));
        let lines = o.lines();
        assert!(lines[0].starts_with("  60 fps"), "{lines:?}");
        assert!(lines[1].contains("3.0 ms (UI 1.5)"), "{lines:?}");
        assert!(lines[2].contains("1.0 ms"), "{lines:?}");
        assert_eq!(lines[4], "Advance Auto: on");
    }

    #[test]
    fn it_names_what_draws_in_as_much_as_fits() {
        let mut o = PerformanceOverlay::new();
        o.push(frame(16.0, 4.0, Some(2.0)));
        o.set_devices(vec![
            "GPU Radeon 780M".into(),
            "    radv, explicit sync, and a lot more than fits".into(),
        ]);
        let lines = o.lines();
        assert_eq!(lines.len(), 7);
        assert_eq!(lines[5], "GPU Radeon 780M");
        assert_eq!(lines[6].chars().count(), LETTERS);
        assert!(lines[6].ends_with('…'));
    }

    #[test]
    fn it_keeps_only_what_the_graphs_show() {
        let mut o = PerformanceOverlay::new();
        for _ in 0..200 {
            o.push(frame(8.0, 1.0, None));
        }
        assert_eq!(o.frames.len(), SHOWN);
        assert_eq!(o.lines()[2], "GPU     - ms");
    }

    #[test]
    fn it_paints_over_everything_at_the_top_right() {
        let mut o = PerformanceOverlay::new();
        o.push(frame(16.0, 4.0, Some(2.0)));
        let mut scene = Scene {
            width: 800,
            height: 600,
            ..Default::default()
        };
        o.paint(&mut scene, Scale(120));
        assert_eq!(scene.chunks.last().unwrap().key, CHUNK);
        let Some(Primitive::Quad(panel)) = scene.primitives.first() else {
            panic!("the panel comes first");
        };
        assert_eq!(panel.rect.right(), 800 - 8);
        assert_eq!(panel.rect.y, 8);
        assert!(
            scene
                .primitives
                .iter()
                .any(|p| matches!(p, Primitive::Glyphs(_)))
        );
    }
}
