//! Diagrams with `merman`, which follows mermaid.js closely, with labels measured by our fonts.

use anti_webui_core::prelude::*;
use merman::svg::{
    DeterministicTextMeasurer, MeasurementProfileId, TextMeasurementPolicy, TextMeasurementProfile,
    TextMeasurementProfileIdentity, TextStyle,
};
use merman::{OperationControl, RenderOutput, RenderRequest, Renderer, SvgEnvironment, SvgRequest};
use std::cell::OnceCell;

thread_local! {
    static RENDERER: OnceCell<(Renderer, SvgRequest)> = const { OnceCell::new() };
}

/// How wide a label is with our fonts. It runs on the thread the diagram is made on, which has a
/// text system of its own.
fn width(text: &str, style: &TextStyle) -> f64 {
    let mut s = Style::default().text_size(style.font_size as f32);
    if let Some(family) = &style.font_family {
        s = s.font_family(FontFamily::from(family.as_str()));
    }
    match style.font_weight.as_deref() {
        Some("bold" | "bolder") => s = s.bold(),
        Some(w) => {
            if let Ok(w) = w.parse::<u16>() {
                s = s.font_weight(FontWeight(w));
            }
        }
        None => {}
    }
    if style.font_style.as_deref() == Some("italic") {
        s = s.italic();
    }
    svg::measure_text(text, &s).0 as f64
}

pub(super) fn setup() {
    RENDERER.with(|r| {
        r.get_or_init(|| {
            let measurer = DeterministicTextMeasurer::default().with_width_callback(width);
            let id = TextMeasurementProfileIdentity::new(
                MeasurementProfileId::new("anti-webui").expect("a good id"),
                "1",
            )
            .expect("a good identity");
            let policy = TextMeasurementPolicy::uniform(TextMeasurementProfile::new(id, measurer));
            let request = SvgRequest {
                environment: SvgEnvironment::deterministic().with_text_measurement_policy(policy),
                ..Default::default()
            };
            (Renderer::new(), request)
        });
    });
}

pub(super) fn render(source: &str) -> Result<String, String> {
    setup();
    RENDERER.with(|r| {
        let (renderer, request) = r.get().expect("it was set up");
        match renderer.render(RenderRequest::svg(
            source,
            OperationControl::new(),
            request.clone(),
        )) {
            Ok(RenderOutput::Svg(Some(svg))) => Ok(svg.svg().to_string()),
            Ok(_) => Err("no picture".to_owned()),
            Err(e) => Err(e.to_string()),
        }
    })
}
