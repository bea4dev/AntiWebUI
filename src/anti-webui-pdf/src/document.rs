//! Drawing pages with `hayro`.

use crate::edits::{self, Edits};
use crate::embed::SubsetCache;
use crate::engine::{Document, Opened, Session};
use crate::links::{PdfOutlineItem, Places};
use crate::marks::{self, Marks};
use crate::plan::{self, Job};
use crate::text::{CollectionMaps, PageText, TextDevice};
use crate::writer::Tail;
use crate::{PdfError, PdfSource, fonts};
use anti_webui_core::geometry::RectI;
use anti_webui_core::images::Bitmap;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_interpret::{Context, InterpreterCache, InterpreterSettings, interpret_page};
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf, PdfData};
use hayro::kurbo::{Affine, Rect};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};
use std::sync::{Arc, OnceLock};

/// How many sizes a worker keeps the glyphs of: those of the pages and those of the thumbnails.
const MOST_SCALES: usize = 2;

/// `Arc<[u8]>` isn't `Sized`, so `hayro` can't take it as it is.
struct Bytes(Arc<[u8]>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// An open document. The workers share it, and each has its own caches.
struct Hayro {
    pdf: Pdf,
    settings: InterpreterSettings,
    maps: CollectionMaps,
    places: Places,
    /// What the document was before it was edited, which edits are always made to. It is `None` if
    /// this is it.
    original: Option<Arc<Hayro>>,
    tail: Option<Tail>,
    marks: OnceLock<Arc<Marks>>,
    subsets: SubsetCache,
}

/// Reads and opens a document. Takes as long as the file takes to read and its index to parse.
pub(crate) fn open(source: &PdfSource, password: Option<&str>) -> Result<Opened, PdfError> {
    let data: PdfData = match source {
        PdfSource::Path(path) => std::fs::read(path)
            .map_err(|e| PdfError::Source(e.to_string()))?
            .into(),
        PdfSource::Bytes(bytes) => Arc::new(Bytes(bytes.clone())).into(),
        #[cfg(test)]
        PdfSource::Fake(fake) => return Ok((fake.doc.clone(), fake.pages.clone())),
    };
    let pdf = Pdf::new_with_password(data, password.unwrap_or("")).map_err(|e| match e {
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) if password.is_some() => {
            PdfError::WrongPassword
        }
        LoadPdfError::Decryption(DecryptionError::PasswordProtected) => PdfError::NeedsPassword,
        LoadPdfError::Decryption(e) => PdfError::Unsupported(format!("{e:?}")),
        LoadPdfError::Invalid => PdfError::Invalid,
    })?;
    let pages: Vec<(f32, f32)> = pdf.pages().iter().map(|p| p.render_dimensions()).collect();
    if pages.is_empty() {
        return Err(PdfError::Invalid);
    }
    let tail = Tail::read(pdf.data().as_ref());
    let doc = Hayro::new(pdf, None, tail);
    Ok((Arc::new(doc), pages))
}

impl Hayro {
    fn new(pdf: Pdf, original: Option<Arc<Hayro>>, tail: Option<Tail>) -> Self {
        Self {
            pdf,
            settings: InterpreterSettings {
                font_resolver: fonts::resolver(),
                ..Default::default()
            },
            maps: CollectionMaps::default(),
            places: Places::default(),
            original,
            tail,
            marks: OnceLock::new(),
            subsets: SubsetCache::default(),
        }
    }

    /// What the document was before it was edited.
    fn first(&self) -> &Hayro {
        self.original.as_deref().unwrap_or(self)
    }

    fn the_marks(&self) -> Arc<Marks> {
        self.first()
            .marks
            .get_or_init(|| Arc::new(marks::read(&self.first().pdf, self.first().tail.as_ref())))
            .clone()
    }

    /// The bytes of the document with edits at the end.
    fn with_edits(&self, edits: &Edits) -> Result<Vec<u8>, String> {
        let first = self.first();
        let tail = first
            .tail
            .as_ref()
            .ok_or("this document is too broken to edit")?;
        if tail.encrypted {
            return Err("an encrypted document can't be edited yet".to_owned());
        }
        let update = edits::build(&first.pdf, tail, &self.the_marks(), edits, &first.subsets)?;
        let data: &[u8] = first.pdf.data().as_ref();
        let mut bytes = Vec::with_capacity(data.len() + update.len());
        bytes.extend_from_slice(data);
        bytes.extend_from_slice(&update);
        Ok(bytes)
    }
}

impl Document for Hayro {
    fn session(&self) -> Box<dyn Session + '_> {
        Box::new(HayroSession {
            doc: self,
            cache: RenderCache::new(),
            scales: Vec::new(),
            text_cache: None,
            settings: RenderSettings::default(),
            context: RenderContext::new(1, 1),
            resources: Resources::default(),
        })
    }

    fn outline(&self) -> Vec<PdfOutlineItem> {
        self.places.outline(&self.pdf)
    }

    fn marks(&self) -> Arc<Marks> {
        self.the_marks()
    }

    fn edited(self: Arc<Self>, edits: &Edits) -> Result<Arc<dyn Document>, String> {
        let marks = self.the_marks();
        // Nothing left to add takes the document as it was, and what the edits took goes.
        if edits.is_empty() && !marks.needs_looks() {
            return Ok(match &self.original {
                Some(first) => first.clone(),
                None => self,
            });
        }
        let bytes = self.with_edits(edits)?;
        let pdf = Pdf::new(bytes).map_err(|e| format!("{e:?}"))?;
        let first = self.original.clone().unwrap_or(self);
        Ok(Arc::new(Hayro::new(pdf, Some(first), None)))
    }

    fn saved(&self, edits: &Edits) -> Result<Vec<u8>, String> {
        if edits.is_empty() {
            return Ok(self.first().pdf.data().as_ref().to_vec());
        }
        self.with_edits(edits)
    }
}

/// What a worker keeps between pages. A `RenderCache` can't go to another thread.
struct HayroSession<'a> {
    doc: &'a Hayro,
    cache: RenderCache<'a>,
    /// The sizes `cache` has seen, in percent. It keeps a picture of every glyph at every size and
    /// never lets go of them, which takes a gigabyte after a few seconds of zooming.
    scales: Vec<u32>,
    /// What reading the text keeps. `hayro` doesn't let it share what drawing has.
    text_cache: Option<InterpreterCache<'a>>,
    settings: RenderSettings,
    context: RenderContext,
    resources: Resources,
}

impl HayroSession<'_> {
    /// The pixels of a picture, on white.
    fn pixels(&mut self, job: Job) -> Vec<u8> {
        let page = &self.doc.pdf.pages()[job.page];
        let (w, h) = page.render_dimensions();
        let size = (w as f64, h as f64);
        let (shown_w, shown_h) = job.turn.size(size);
        let scale = (job.width as f64 / shown_w * 100.0).round() as u32;
        if !self.scales.contains(&scale) {
            // What it has of the fonts goes too, as nothing else can be let go of.
            if self.scales.len() >= MOST_SCALES {
                self.cache = RenderCache::new();
                self.scales.clear();
            }
            self.scales.push(scale);
        }
        // Exactly as many pixels as the box has, so that they meet those of the screen one to one.
        let (x, y) = job.area.map_or((0, 0), |area| (area.x, area.y));
        let transform = Affine::translate((-x as f64, -y as f64))
            * Affine::scale_non_uniform(job.width as f64 / shown_w, job.height as f64 / shown_h)
            * Affine::new(job.turn.matrix(size))
            * page.initial_transform(true).to_kurbo();
        let (width, height) = job.size();
        let (width, height) = (width as u16, height as u16);
        self.context.reset_and_resize(width, height);
        render_into(
            page,
            &self.cache,
            &self.doc.settings,
            &self.settings,
            &mut self.context,
            transform,
        );
        self.context.flush();
        let mut pixmap = Pixmap::new(width, height);
        self.context.render_with(
            &mut pixmap,
            &mut self.resources,
            RasterizerSettings {
                target_init: TargetInit::Clear(WHITE),
                ..Default::default()
            },
        );
        pixmap.take_rgba8(ImageAlphaType::AlphaPremultiplied)
    }
}

impl Session for HayroSession<'_> {
    fn draw(&mut self, job: Job) -> Bitmap {
        let (width, height) = job.size();
        Bitmap::from_premultiplied(width, height, self.pixels(job), true)
    }

    fn cut_out(&mut self, job: Job, boxes: &[RectI]) -> Bitmap {
        let (width, height) = job.size();
        let pixels = plan::cut_out(&self.pixels(job), width, boxes);
        Bitmap::from_premultiplied(width, height, pixels, false)
    }

    fn text(&mut self, page: usize) -> PageText {
        let doc = self.doc;
        let shown = &doc.pdf.pages()[page];
        let (w, h) = shown.render_dimensions();
        let mut context = Context::new(
            shown.initial_transform(true).to_kurbo(),
            Rect::new(0.0, 0.0, w as f64, h as f64),
            self.text_cache.get_or_insert_with(InterpreterCache::new),
            shown.xref(),
            doc.settings.clone(),
        );
        let mut device = TextDevice::new(shown, &doc.maps);
        interpret_page(shown, &mut context, &mut device);
        let mut text = PageText::new(&device.into_glyphs(), (w, h));
        text.links = doc.places.links(&doc.pdf, shown);
        text
    }
}
