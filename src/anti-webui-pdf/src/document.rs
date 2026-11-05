//! Drawing pages with `hayro`.

use crate::engine::{Document, Opened, Session};
use crate::plan::Job;
use crate::{PdfError, PdfSource, fonts};
use anti_webui_core::images::Bitmap;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_interpret::util::TransformExt;
use hayro::hayro_syntax::{DecryptionError, LoadPdfError, Pdf, PdfData};
use hayro::kurbo::Affine;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::vello_cpu::{Pixmap, RasterizerSettings, RenderContext, Resources, TargetInit};
use hayro::{RenderCache, RenderSettings, render_into};
use std::sync::Arc;

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
    let settings = InterpreterSettings {
        font_resolver: fonts::resolver(),
        ..Default::default()
    };
    Ok((Arc::new(Hayro { pdf, settings }), pages))
}

impl Document for Hayro {
    fn session(&self) -> Box<dyn Session + '_> {
        Box::new(HayroSession {
            doc: self,
            cache: RenderCache::new(),
            settings: RenderSettings::default(),
            context: RenderContext::new(1, 1),
            resources: Resources::default(),
        })
    }
}

/// What a worker keeps between pages. A `RenderCache` can't go to another thread.
struct HayroSession<'a> {
    doc: &'a Hayro,
    cache: RenderCache<'a>,
    settings: RenderSettings,
    context: RenderContext,
    resources: Resources,
}

impl Session for HayroSession<'_> {
    fn draw(&mut self, job: Job) -> Bitmap {
        let page = &self.doc.pdf.pages()[job.page];
        let (w, h) = page.render_dimensions();
        // Exactly as many pixels as the box has, so that they meet those of the screen one to one.
        let (x, y) = job.area.map_or((0, 0), |area| (area.x, area.y));
        let transform = Affine::translate((-x as f64, -y as f64))
            * Affine::scale_non_uniform(job.width as f64 / w as f64, job.height as f64 / h as f64)
            * page.initial_transform(true).to_kurbo();
        let (picture_width, picture_height) = job.size();
        let (width, height) = (picture_width as u16, picture_height as u16);
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
        let pixels = pixmap.take_rgba8(ImageAlphaType::AlphaPremultiplied);
        Bitmap::from_premultiplied(picture_width, picture_height, pixels, true)
    }
}
