//! Glyph rasterization (swash) and a shelf-packed RGBA atlas.

use anti_webui_core::scene::GlyphRun;
use std::collections::HashMap;
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Format, Transform, Vector};

/// Sub-pixel positions per pixel on the x axis.
const SUBPIXEL_STEPS: f32 = 4.0;

/// From this size on, a full atlas may be cleared instead of grown.
const SIZE_TO_STAY_AT: u32 = 2048;
/// How many sets of coordinates count as what is shown for long: those are not what fills it up.
const LASTING_INSTANCES: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    font: u64,
    index: u32,
    glyph: u32,
    /// Font size in 1/64 px.
    size: u32,
    subpixel: u8,
    embolden: bool,
    skew: Option<i32>,
    /// The number of its coordinates: no coordinates are 0.
    instance: u32,
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
    instances: HashMap<Vec<i16>, u32>,
    /// The area the images of each instance take in the atlas.
    areas: Vec<u64>,
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
            instances: HashMap::new(),
            areas: vec![0],
            shelves: Vec::new(),
            size,
            uploads: Vec::new(),
            full: false,
        }
    }

    pub fn clear(&mut self, size: u32) {
        self.entries.clear();
        self.instances.clear();
        self.areas.clear();
        self.areas.push(0);
        self.shelves.clear();
        self.uploads.clear();
        self.size = size;
        self.full = false;
    }

    /// The size to go on with after the atlas got full. What moves along the axes of a font can
    /// make images without end, which staying the same size keeps in check, unless what a frame
    /// needs doesn't fit even after `cleared` (see `docs/design/17-font-axes.md`).
    pub fn size_when_full(&self, cleared: bool, max: u32) -> u32 {
        if !cleared && self.size >= SIZE_TO_STAY_AT && self.mostly_passing() {
            return self.size;
        }
        (self.size * 2).min(max)
    }

    /// Whether most of the atlas is taken by images of coordinates that come and go.
    fn mostly_passing(&self) -> bool {
        let mut areas = self.areas.clone();
        areas.sort_unstable_by(|a, b| b.cmp(a));
        let total: u64 = areas.iter().sum();
        let passing: u64 = areas.iter().skip(LASTING_INSTANCES).sum();
        passing * 2 > total
    }

    fn instance(&mut self, coords: &[i16]) -> u32 {
        if coords.is_empty() {
            return 0;
        }
        if let Some(&i) = self.instances.get(coords) {
            return i;
        }
        let i = self.areas.len() as u32;
        self.areas.push(0);
        self.instances.insert(coords.to_vec(), i);
        i
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
        let instance = self.instance(&run.normalized_coords);
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
                instance,
            };
            let entry = match self.entries.get(&key) {
                Some(e) => *e,
                None => {
                    let e = self.rasterize(run, size, g.id, sub / SUBPIXEL_STEPS)?;
                    if let Some(image) = &e {
                        self.areas[instance as usize] += image.width as u64 * image.height as u64;
                    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use anti_webui_core::prelude::Color;
    use anti_webui_core::scene::{Clip, FontData, Glyph};
    use std::sync::OnceLock;

    static FONT: &[u8] = include_bytes!("../../anti-webui-md3e/fonts/RobotoFlex.ttf");

    /// Letters of Roboto Flex at some coordinates.
    fn run(size: f32, coords: &[i16]) -> GlyphRun {
        static DATA: OnceLock<FontData> = OnceLock::new();
        GlyphRun {
            font: DATA
                .get_or_init(|| FontData::new(FONT.to_vec().into(), 0))
                .clone(),
            size,
            normalized_coords: coords.to_vec(),
            embolden: false,
            skew: None,
            color: Color::BLACK,
            glyphs: (40..60).map(|id| Glyph { id, x: 0.0, y: 0.0 }).collect(),
            clip: Clip::NONE,
        }
    }

    fn place(cache: &mut GlyphCache, run: &GlyphRun) -> Option<usize> {
        let mut out = Vec::new();
        cache.place_run(run, 1, &mut out)?;
        Some(out.len())
    }

    #[test]
    fn the_same_coordinates_use_the_same_images() {
        let mut cache = GlyphCache::new(1024);
        place(&mut cache, &run(20.0, &[0, 256]));
        let made = cache.uploads.len();
        assert!(made > 0);
        place(&mut cache, &run(20.0, &[0, 256]));
        assert_eq!(cache.uploads.len(), made);
        place(&mut cache, &run(20.0, &[0, 512]));
        assert!(cache.uploads.len() > made);
        place(&mut cache, &run(20.0, &[]));
        let others = cache.uploads.len();
        assert!(others > made);
        place(&mut cache, &run(20.0, &[]));
        assert_eq!(cache.uploads.len(), others);
    }

    /// Fills `cache` up with images, a new set of coordinates for every few of them.
    fn fill(cache: &mut GlyphCache, size: f32) {
        for i in 1.. {
            let coords = [0, (i % 60 + 1) * 256, (i / 60) * 256];
            if place(cache, &run(size, &coords)).is_none() {
                assert!(cache.full);
                assert!(cache.mostly_passing());
                return;
            }
        }
    }

    #[test]
    fn an_atlas_full_of_what_goes_by_stays_the_same_size() {
        let mut cache = GlyphCache::new(2048);
        fill(&mut cache, 90.0);
        assert_eq!(cache.size_when_full(false, 8192), 2048);
    }

    #[test]
    fn an_atlas_full_of_what_stays_grows() {
        let mut cache = GlyphCache::new(2048);
        // A lot of glyphs, in just a few sets of coordinates.
        'filling: for size in 40..1000 {
            for coords in [&[][..], &[0, 256][..], &[0, 512][..]] {
                if place(&mut cache, &run(size as f32, coords)).is_none() {
                    break 'filling;
                }
            }
        }
        assert!(cache.full);
        assert_eq!(cache.size_when_full(false, 8192), 4096);
    }

    #[test]
    fn a_small_atlas_grows_whatever_is_in_it() {
        let mut cache = GlyphCache::new(1024);
        fill(&mut cache, 30.0);
        assert_eq!(cache.size_when_full(false, 8192), 2048);
    }

    #[test]
    fn what_does_not_fit_after_clearing_makes_it_grow() {
        let mut cache = GlyphCache::new(2048);
        fill(&mut cache, 90.0);
        assert_eq!(cache.size_when_full(true, 8192), 4096);
        assert_eq!(cache.size_when_full(true, 2048), 2048, "as big as it goes");
    }

    #[test]
    fn clearing_forgets_what_took_the_atlas() {
        let mut cache = GlyphCache::new(2048);
        fill(&mut cache, 90.0);
        cache.clear(2048);
        assert!(!cache.mostly_passing());
        assert!(!cache.full);
        assert!(place(&mut cache, &run(20.0, &[0, 256])).is_some());
    }
}
