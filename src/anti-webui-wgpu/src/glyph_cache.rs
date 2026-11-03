//! Glyph rasterization (swash) and a shelf-packed RGBA atlas.

use anti_webui_core::scene::GlyphRun;
use std::collections::HashMap;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Transform, Vector};

/// Sub-pixel positions per pixel on the x axis.
const SUBPIXEL_STEPS: f32 = 4.0;

#[derive(Clone, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    /// Font size in 1/64 px.
    size: u32,
    subpixel: u8,
    embolden: bool,
    skew: Option<i32>,
    coords: Vec<i16>,
}

#[derive(Clone, Copy, Debug)]
pub struct AtlasGlyph {
    /// Atlas pixel rectangle.
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub color: bool,
}

pub struct PlacedGlyph {
    pub rect: [f32; 4],
    pub uv: [f32; 4],
    pub color: bool,
}

struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

pub struct GlyphCache {
    scale: ScaleContext,
    entries: HashMap<GlyphKey, Option<AtlasGlyph>>,
    shelves: Vec<Shelf>,
    pub size: u32,
    /// Pending uploads: (glyph, RGBA pixels).
    pub uploads: Vec<(AtlasGlyph, Vec<u8>)>,
    pub full: bool,
}

impl GlyphCache {
    pub fn new(size: u32) -> Self {
        Self {
            scale: ScaleContext::new(),
            entries: HashMap::new(),
            shelves: Vec::new(),
            size,
            uploads: Vec::new(),
            full: false,
        }
    }

    pub fn clear(&mut self, size: u32) {
        self.entries.clear();
        self.shelves.clear();
        self.uploads.clear();
        self.size = size;
        self.full = false;
    }

    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        // One pixel of padding avoids bleeding between neighbours when sampling.
        let (pw, ph) = (w + 1, h + 1);
        if pw > self.size || ph > self.size {
            return None;
        }
        let mut best: Option<usize> = None;
        for (i, s) in self.shelves.iter().enumerate() {
            if s.height >= ph
                && s.x + pw <= self.size
                && s.height <= ph + ph / 2 + 2
                && best.is_none_or(|b| self.shelves[b].height > s.height)
            {
                best = Some(i);
            }
        }
        if let Some(i) = best {
            let s = &mut self.shelves[i];
            let pos = (s.x, s.y);
            s.x += pw;
            return Some(pos);
        }
        let y = self.shelves.last().map(|s| s.y + s.height).unwrap_or(0);
        if y + ph > self.size {
            return None;
        }
        self.shelves.push(Shelf {
            y,
            height: ph,
            x: pw,
        });
        Some((0, y))
    }

    /// Places every glyph of a run drawn `k` times bigger, rasterizing missing ones. Returns `None`
    /// if the atlas is full.
    pub fn place_run(&mut self, run: &GlyphRun, k: i32, out: &mut Vec<PlacedGlyph>) -> Option<()> {
        let font_id = run.font.data.id();
        let k = k as f32;
        let size = run.size * k;
        let size_key = (size * 64.0).round() as u32;
        let skew = run.skew.map(|s| (s * 100.0).round() as i32);
        for g in &run.glyphs {
            let (gx, gy) = (g.x * k, g.y * k);
            let fx = gx.floor();
            let mut sub = ((gx - fx) * SUBPIXEL_STEPS).round();
            let mut base_x = fx;
            if sub >= SUBPIXEL_STEPS {
                sub = 0.0;
                base_x += 1.0;
            }
            let key = GlyphKey {
                font: font_id,
                index: run.font.index,
                glyph: g.id,
                size: size_key,
                subpixel: sub as u8,
                embolden: run.embolden,
                skew,
                coords: run.normalized_coords.clone(),
            };
            let entry = match self.entries.get(&key) {
                Some(e) => *e,
                None => {
                    let e = self.rasterize(run, size, g.id, sub / SUBPIXEL_STEPS)?;
                    self.entries.insert(key, e);
                    e
                }
            };
            let Some(e) = entry else { continue };
            let s = self.size as f32;
            out.push(PlacedGlyph {
                rect: [
                    base_x + e.left as f32,
                    gy.round() - e.top as f32,
                    e.width as f32,
                    e.height as f32,
                ],
                uv: [
                    e.x as f32 / s,
                    e.y as f32 / s,
                    (e.x + e.width) as f32 / s,
                    (e.y + e.height) as f32 / s,
                ],
                color: e.color,
            });
        }
        Some(())
    }

    /// Rasterizes a glyph. `Some(None)` means the glyph is empty; `None` means the atlas is full.
    fn rasterize(
        &mut self,
        run: &GlyphRun,
        size: f32,
        glyph: u32,
        offset_x: f32,
    ) -> Option<Option<AtlasGlyph>> {
        let data = run.font.data.data();
        let Some(font) = FontRef::from_index(data, run.font.index as usize) else {
            return Some(None);
        };
        let mut scaler = self
            .scale
            .builder(font)
            .size(size)
            .hint(false)
            .normalized_coords(run.normalized_coords.iter().copied())
            .build();
        let sources = [
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ];
        let mut render = Render::new(&sources);
        render
            .format(Format::Alpha)
            .offset(Vector::new(offset_x, 0.0));
        if run.embolden {
            render.embolden(size / 24.0);
        }
        if let Some(skew) = run.skew {
            render.transform(Some(Transform::skew(
                swash::zeno::Angle::from_degrees(skew),
                swash::zeno::Angle::ZERO,
            )));
        }
        let Some(image) = render.render(&mut scaler, glyph as u16) else {
            return Some(None);
        };
        let (w, h) = (image.placement.width, image.placement.height);
        if w == 0 || h == 0 {
            return Some(None);
        }
        let Some((x, y)) = self.allocate(w, h) else {
            self.full = true;
            return None;
        };
        let (pixels, color) = match image.content {
            Content::Mask => (
                image.data.iter().flat_map(|&a| [a, a, a, a]).collect(),
                false,
            ),
            Content::SubpixelMask => (
                image
                    .data
                    .chunks_exact(4)
                    .flat_map(|p| {
                        let a = ((p[0] as u32 + p[1] as u32 + p[2] as u32) / 3) as u8;
                        [a, a, a, a]
                    })
                    .collect(),
                false,
            ),
            Content::Color => {
                // swash produces straight alpha; the shader expects premultiplied.
                let px = image
                    .data
                    .chunks_exact(4)
                    .flat_map(|p| {
                        let a = p[3] as u32;
                        [
                            (p[0] as u32 * a / 255) as u8,
                            (p[1] as u32 * a / 255) as u8,
                            (p[2] as u32 * a / 255) as u8,
                            p[3],
                        ]
                    })
                    .collect();
                (px, true)
            }
        };
        let g = AtlasGlyph {
            x,
            y,
            width: w,
            height: h,
            left: image.placement.left,
            top: image.placement.top,
            color,
        };
        self.uploads.push((g, pixels));
        Some(Some(g))
    }
}
