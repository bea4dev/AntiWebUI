//! Shaders written in WGSL that paint inside of a box. See `docs/design/14-shaders.md`.

use anti_webui_style::Color;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

/// The type of a shader parameter in WGSL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub enum ParamType {
    F32,
    I32,
    U32,
    Vec2,
    Vec3,
    Vec4,
    /// `array<vec4<f32>, N>`.
    Vec4Array(usize),
}

impl ParamType {
    /// The alignment and the size in a WGSL uniform buffer.
    pub const fn layout(self) -> (usize, usize) {
        match self {
            ParamType::F32 | ParamType::I32 | ParamType::U32 => (4, 4),
            ParamType::Vec2 => (8, 8),
            ParamType::Vec3 => (16, 12),
            ParamType::Vec4 => (16, 16),
            ParamType::Vec4Array(n) => (16, 16 * n),
        }
    }

    pub fn wgsl(self) -> String {
        match self {
            ParamType::F32 => "f32".into(),
            ParamType::I32 => "i32".into(),
            ParamType::U32 => "u32".into(),
            ParamType::Vec2 => "vec2<f32>".into(),
            ParamType::Vec3 => "vec3<f32>".into(),
            ParamType::Vec4 => "vec4<f32>".into(),
            ParamType::Vec4Array(n) => format!("array<vec4<f32>, {n}>"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[doc(hidden)]
pub struct ParamField {
    pub name: &'static str,
    pub ty: ParamType,
}

/// Lays out values the way WGSL does in a uniform buffer.
#[derive(Default)]
#[doc(hidden)]
pub struct ParamWriter {
    bytes: Vec<u8>,
}

impl ParamWriter {
    /// Adds a value of `ty` made of `values`, which is what it takes for the field's type.
    pub fn put(&mut self, ty: ParamType, values: &[[u8; 4]]) {
        let (align, size) = ty.layout();
        debug_assert_eq!(values.len() * 4, size);
        self.bytes
            .resize(self.bytes.len().next_multiple_of(align), 0);
        self.bytes.extend(values.iter().flatten());
    }

    /// The bytes of the whole struct, whose size is always a multiple of 16: it is an element of
    /// a uniform array.
    fn finish(mut self) -> Vec<u8> {
        self.bytes
            .resize(self.bytes.len().next_multiple_of(16).max(16), 0);
        self.bytes
    }
}

/// What can be a field of [`ShaderParams`].
#[doc(hidden)]
pub trait ParamValue {
    const TYPE: ParamType;
    fn write(&self, out: &mut ParamWriter);
}

impl ParamValue for f32 {
    const TYPE: ParamType = ParamType::F32;
    fn write(&self, out: &mut ParamWriter) {
        out.put(Self::TYPE, &[self.to_ne_bytes()]);
    }
}

impl ParamValue for i32 {
    const TYPE: ParamType = ParamType::I32;
    fn write(&self, out: &mut ParamWriter) {
        out.put(Self::TYPE, &[self.to_ne_bytes()]);
    }
}

impl ParamValue for u32 {
    const TYPE: ParamType = ParamType::U32;
    fn write(&self, out: &mut ParamWriter) {
        out.put(Self::TYPE, &[self.to_ne_bytes()]);
    }
}

impl ParamValue for [f32; 2] {
    const TYPE: ParamType = ParamType::Vec2;
    fn write(&self, out: &mut ParamWriter) {
        out.put(Self::TYPE, &self.map(f32::to_ne_bytes));
    }
}

impl ParamValue for [f32; 3] {
    const TYPE: ParamType = ParamType::Vec3;
    fn write(&self, out: &mut ParamWriter) {
        out.put(Self::TYPE, &self.map(f32::to_ne_bytes));
    }
}

impl ParamValue for [f32; 4] {
    const TYPE: ParamType = ParamType::Vec4;
    fn write(&self, out: &mut ParamWriter) {
        out.put(Self::TYPE, &self.map(f32::to_ne_bytes));
    }
}

/// Premultiplied, like everything the shaders output.
impl ParamValue for Color {
    const TYPE: ParamType = ParamType::Vec4;
    fn write(&self, out: &mut ParamWriter) {
        self.premultiplied().write(out);
    }
}

impl<const N: usize> ParamValue for [[f32; 4]; N] {
    const TYPE: ParamType = ParamType::Vec4Array(N);
    fn write(&self, out: &mut ParamWriter) {
        let values: Vec<[u8; 4]> = self.iter().flatten().map(|v| v.to_ne_bytes()).collect();
        out.put(Self::TYPE, &values);
    }
}

/// The parameters of a shader, which it reads as `params`. Use `#[derive(ShaderParams)]`.
pub trait ShaderParams: 'static {
    const FIELDS: &'static [ParamField];
    fn write(&self, out: &mut ParamWriter);
}

/// A shader that takes no parameters.
impl ShaderParams for () {
    const FIELDS: &'static [ParamField] = &[];
    fn write(&self, _: &mut ParamWriter) {}
}

/// What the renderer needs to compile a shader.
#[derive(Debug)]
#[doc(hidden)]
pub struct ShaderSource {
    /// Shown in error messages.
    pub name: &'static str,
    /// WGSL with `fn paint(f: Frag) -> vec4<f32>`.
    pub code: &'static str,
    pub fields: &'static [ParamField],
}

impl ShaderSource {
    /// The size of the parameters, which is what the WGSL struct takes.
    pub fn params_size(&self) -> usize {
        let end = self.fields.iter().fold(0, |at: usize, f| {
            let (align, size) = f.ty.layout();
            at.next_multiple_of(align) + size
        });
        end.next_multiple_of(16).max(16)
    }
}

/// A shader. It lives in a `static`, which identifies it, and needs no registering anywhere:
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// static RED: Shader<()> = Shader::new("red", "fn paint(f: Frag) -> vec4<f32> {
///     return vec4<f32>(1.0, 0.0, 0.0, 1.0);
/// }");
///
/// let view = div().w(100.0).h(100.0).fill(|| RED.params(()));
/// ```
pub struct Shader<P> {
    source: ShaderSource,
    params: PhantomData<fn(P)>,
}

impl<P: ShaderParams> Shader<P> {
    pub const fn new(name: &'static str, code: &'static str) -> Self {
        Self {
            source: ShaderSource {
                name,
                code,
                fields: P::FIELDS,
            },
            params: PhantomData,
        }
    }

    pub fn source(&'static self) -> &'static ShaderSource {
        &self.source
    }

    /// This shader with these parameters, for [`fill`](crate::view::Div::fill) and
    /// [`overlay`](crate::view::Div::overlay).
    pub fn params(&'static self, params: P) -> ShaderPaint {
        let mut out = ParamWriter::default();
        params.write(&mut out);
        ShaderPaint {
            source: &self.source,
            params: out.finish().into(),
            inputs: Vec::new(),
            reach: 0.0,
        }
    }

    /// Compiles it before the next frame rather than when it is first shown, which could make
    /// that frame late.
    pub fn preload(&'static self) {
        PRELOADS
            .lock()
            .expect("nothing panics while holding it")
            .push(&self.source);
    }
}

static PRELOADS: Mutex<Vec<&'static ShaderSource>> = Mutex::new(Vec::new());

/// The shaders asked to be compiled ahead, from `start` on.
#[doc(hidden)]
pub fn preloads_from(start: usize) -> Vec<&'static ShaderSource> {
    let list = PRELOADS.lock().expect("nothing panics while holding it");
    list.get(start..).map(<[_]>::to_vec).unwrap_or_default()
}

/// A shader with its parameters: what to paint with.
#[derive(Clone, Debug)]
pub struct ShaderPaint {
    source: &'static ShaderSource,
    params: Arc<[u8]>,
    inputs: Vec<Image>,
    reach: f32,
}

impl ShaderPaint {
    pub fn source(&self) -> &'static ShaderSource {
        self.source
    }

    pub fn params(&self) -> &[u8] {
        &self.params
    }

    /// What the shader reads as `read0` to `read3`. More than four are ignored.
    pub fn inputs(mut self, inputs: impl IntoIterator<Item = Image>) -> Self {
        self.inputs = inputs.into_iter().take(MAX_INPUTS).collect();
        self
    }

    /// How far from a pixel the shader reads its inputs, in logical pixels. What it reads further
    /// away gives the color at the edge of that range. In a `filter`, it is also how far the
    /// result goes past what it is made from.
    pub fn reach(mut self, logical: f32) -> Self {
        self.reach = logical.max(0.0);
        self
    }

    pub fn input_images(&self) -> &[Image] {
        &self.inputs
    }

    /// [`ShaderPaint::reach`] in physical pixels.
    pub fn reach_px(&self, scale: f32) -> i32 {
        (self.reach * scale).ceil() as i32
    }
}

impl PartialEq for ShaderPaint {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.source, other.source)
            && (Arc::ptr_eq(&self.params, &other.params) || self.params == other.params)
            && self.reach == other.reach
            && self.inputs == other.inputs
    }
}

/// How many inputs a shader can read.
#[doc(hidden)]
pub const MAX_INPUTS: usize = 4;

/// The standard deviation, in texels, of a blur that goes down this many times and back.
const BLUR_SIGMAS: [f32; 8] = [1.68, 3.76, 7.71, 15.52, 31.08, 62.19, 124.4, 248.8];

/// How a blur of standard deviation `sigma` in texels is done: how many times to go down, and how
/// much of the last step to use. Zero times means computing the gaussian: going down blurs more
/// than that already. See `docs/design/16-filters.md`.
#[doc(hidden)]
pub fn blur_levels(sigma: f32) -> (u32, f32) {
    if sigma <= BLUR_SIGMAS[0] {
        return (0, 1.0);
    }
    let sigma = sigma.min(BLUR_SIGMAS[BLUR_SIGMAS.len() - 1]);
    for i in 1..BLUR_SIGMAS.len() {
        let (below, above) = (BLUR_SIGMAS[i - 1], BLUR_SIGMAS[i]);
        if sigma <= above {
            let mix = (sigma * sigma - below * below) / (above * above - below * below);
            return (i as u32 + 1, mix);
        }
    }
    (BLUR_SIGMAS.len() as u32, 1.0)
}

/// How far a blur reads: what lies further away has too little weight to see.
#[doc(hidden)]
pub fn blur_reach(sigma: f32) -> i32 {
    (sigma.min(BLUR_SIGMAS[BLUR_SIGMAS.len() - 1]) * 3.0).ceil() as i32
}

/// How much more than [`blur_reach`] is read to keep to the grid the blur goes down on. Holds at
/// any number of texels per pixel, unlike the grid itself.
fn blur_grid_margin(sigma: f32) -> i32 {
    (sigma * 2.2).ceil() as i32 + 2
}

/// A picture made of what a node paints or has behind it, blurs and shaders. See
/// `docs/design/16-filters.md`.
#[derive(Clone, Debug)]
pub struct Image(Arc<ImageNode>);

#[derive(Debug, PartialEq)]
pub enum ImageNode {
    /// What the node and its children paint.
    Content,
    /// What is behind the node.
    Backdrop,
    /// `radius` is the standard deviation in logical pixels.
    Blur {
        input: Image,
        radius: f32,
    },
    Shader(ShaderPaint),
}

impl PartialEq for Image {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || self.0 == other.0
    }
}

impl Image {
    /// What the node and its children paint, for [`filter`](crate::view::Div::filter).
    pub fn content() -> Image {
        Image(Arc::new(ImageNode::Content))
    }

    /// What is behind the node, for [`fill`](crate::view::Div::fill) and
    /// [`overlay`](crate::view::Div::overlay).
    pub fn backdrop() -> Image {
        Image(Arc::new(ImageNode::Backdrop))
    }

    /// Blurs it. `radius` is the standard deviation in logical pixels, like CSS `blur()`.
    pub fn blur(self, radius: f32) -> Image {
        if radius <= 0.0 {
            return self;
        }
        Image(Arc::new(ImageNode::Blur {
            input: self,
            radius,
        }))
    }

    /// Passes it through a shader, which reads it as `read0`.
    pub fn through<P: ShaderParams>(self, shader: &'static Shader<P>, params: P) -> Image {
        shader.params(params).inputs([self]).into()
    }

    /// See [`ShaderPaint::reach`]. Only means something right after a shader.
    pub fn reach(self, logical: f32) -> Image {
        match &*self.0 {
            ImageNode::Shader(paint) => paint.clone().reach(logical).into(),
            _ => self,
        }
    }

    pub fn node(&self) -> &ImageNode {
        &self.0
    }

    /// The same for what shares the result, and so is computed once.
    pub fn id(&self) -> usize {
        Arc::as_ptr(&self.0) as usize
    }

    /// Whether what is behind the node decides how it looks.
    pub fn reads_backdrop(&self) -> bool {
        match &*self.0 {
            ImageNode::Content => false,
            ImageNode::Backdrop => true,
            ImageNode::Blur { input, .. } => input.reads_backdrop(),
            ImageNode::Shader(paint) => paint.inputs.iter().any(Image::reads_backdrop),
        }
    }

    /// How far from a pixel its value depends on what it is made from, in physical pixels.
    pub fn reach_px(&self, scale: f32) -> i32 {
        match &*self.0 {
            ImageNode::Content | ImageNode::Backdrop => 0,
            ImageNode::Blur { input, radius } => input.reach_px(scale) + blur_reach(radius * scale),
            ImageNode::Shader(paint) => paint.reach_px(scale) + paint.inputs_reach_px(scale),
        }
    }

    /// How far around a pixel it reads, which is more than what it depends on: everything up to
    /// here is drawn again when it changes.
    pub fn read_px(&self, scale: f32) -> i32 {
        match &*self.0 {
            ImageNode::Content | ImageNode::Backdrop => 0,
            ImageNode::Blur { input, radius } => {
                let sigma = radius * scale;
                input.read_px(scale) + blur_reach(sigma) + blur_grid_margin(sigma)
            }
            ImageNode::Shader(paint) => paint.reach_px(scale) + paint.inputs_read_px(scale),
        }
    }
}

impl ShaderPaint {
    pub fn reads_backdrop(&self) -> bool {
        self.inputs.iter().any(Image::reads_backdrop)
    }

    /// The furthest any input depends on what it is made from.
    pub fn inputs_reach_px(&self, scale: f32) -> i32 {
        self.inputs
            .iter()
            .map(|i| i.reach_px(scale))
            .max()
            .unwrap_or(0)
    }

    /// The furthest any input reads.
    pub fn inputs_read_px(&self, scale: f32) -> i32 {
        self.inputs
            .iter()
            .map(|i| i.read_px(scale))
            .max()
            .unwrap_or(0)
    }
}

impl From<ShaderPaint> for Image {
    fn from(paint: ShaderPaint) -> Image {
        Image(Arc::new(ImageNode::Shader(paint)))
    }
}

/// Shows an image as it is, for one that doesn't end in a shader.
static SHOW: Shader<()> = Shader::new(
    "aw_show",
    "fn paint(f: Frag) -> vec4<f32> { return read0(f.px); }",
);

/// What [`fill`](crate::view::Div::fill) and [`overlay`](crate::view::Div::overlay) can paint with.
pub trait IntoPaint {
    fn into_paint(self) -> Option<ShaderPaint>;
}

impl IntoPaint for ShaderPaint {
    fn into_paint(self) -> Option<ShaderPaint> {
        Some(self)
    }
}

impl IntoPaint for Image {
    fn into_paint(self) -> Option<ShaderPaint> {
        Some(match &*self.0 {
            ImageNode::Shader(paint) => paint.clone(),
            _ => SHOW.params(()).inputs([self]),
        })
    }
}

impl<T: IntoPaint> IntoPaint for Option<T> {
    fn into_paint(self) -> Option<ShaderPaint> {
        self.and_then(IntoPaint::into_paint)
    }
}

/// What [`filter`](crate::view::Div::filter) can apply.
pub trait IntoImage {
    fn into_image(self) -> Option<Image>;
}

impl IntoImage for Image {
    fn into_image(self) -> Option<Image> {
        Some(self)
    }
}

impl IntoImage for ShaderPaint {
    fn into_image(self) -> Option<Image> {
        Some(self.into())
    }
}

impl<T: IntoImage> IntoImage for Option<T> {
    fn into_image(self) -> Option<Image> {
        self.and_then(IntoImage::into_image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blur_changes_continuously_with_its_radius() {
        let at = |sigma: f32| {
            let (levels, mix) = blur_levels(sigma);
            assert!((0..=8).contains(&levels));
            assert!((0.0..=1.0).contains(&mix), "{sigma}: {mix}");
            (levels, mix)
        };
        for &edge in &BLUR_SIGMAS[1..7] {
            let (a, mix_a) = at(edge - 0.001);
            let (b, mix_b) = at(edge + 0.001);
            assert_eq!(a + 1, b, "{edge}");
            assert!(mix_a > 0.999 && mix_b < 0.001, "{edge}: {mix_a} {mix_b}");
        }
        assert_eq!(at(1.0), (0, 1.0));
        assert_eq!(at(BLUR_SIGMAS[0] + 0.001).0, 2);
        assert!(at(BLUR_SIGMAS[0] + 0.001).1 < 0.001);
        assert_eq!(at(10_000.0), (8, 1.0));
    }

    #[test]
    fn what_is_read_covers_the_grid_at_any_resolution() {
        for tenths in 1..4000 {
            let sigma = tenths as f32 * 0.1;
            for k in 1..=4 {
                let (levels, _) = blur_levels(sigma * k as f32);
                // Up to one whole cell of the grid more, in pixels of the scene.
                let grid = (1_i32 << levels) as f32 / k as f32;
                let reach = blur_reach(sigma * k as f32) as f32 / k as f32;
                let read = (blur_reach(sigma) + blur_grid_margin(sigma)) as f32;
                assert!(
                    reach + grid <= read + 1.0,
                    "{sigma} {k}: {reach} {grid} {read}"
                );
            }
        }
    }

    struct Mixed {
        time: f32,
        size: [f32; 2],
        tint: [f32; 3],
        count: i32,
        color: Color,
        waves: [[f32; 4]; 2],
        last: f32,
    }

    impl ShaderParams for Mixed {
        const FIELDS: &'static [ParamField] = &[
            ParamField {
                name: "time",
                ty: f32::TYPE,
            },
            ParamField {
                name: "size",
                ty: <[f32; 2]>::TYPE,
            },
            ParamField {
                name: "tint",
                ty: <[f32; 3]>::TYPE,
            },
            ParamField {
                name: "count",
                ty: i32::TYPE,
            },
            ParamField {
                name: "color",
                ty: Color::TYPE,
            },
            ParamField {
                name: "waves",
                ty: <[[f32; 4]; 2]>::TYPE,
            },
            ParamField {
                name: "last",
                ty: f32::TYPE,
            },
        ];

        fn write(&self, out: &mut ParamWriter) {
            self.time.write(out);
            self.size.write(out);
            self.tint.write(out);
            self.count.write(out);
            self.color.write(out);
            self.waves.write(out);
            self.last.write(out);
        }
    }

    static MIXED: Shader<Mixed> = Shader::new("mixed", "");
    static NOTHING: Shader<()> = Shader::new("nothing", "");

    fn at(bytes: &[u8], offset: usize) -> f32 {
        f32::from_ne_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn values_land_where_wgsl_looks_for_them() {
        let paint = MIXED.params(Mixed {
            time: 1.0,
            size: [2.0, 3.0],
            tint: [4.0, 5.0, 6.0],
            count: 7,
            color: Color::from_rgb(0xffffff),
            waves: [[8.0, 0.0, 0.0, 0.0], [9.0, 0.0, 0.0, 0.0]],
            last: 10.0,
        });
        let b = paint.params();
        // vec2 goes to a multiple of 8, vec3 and vec4 to a multiple of 16; the i32 fits right
        // after the vec3.
        assert_eq!(at(b, 0), 1.0);
        assert_eq!(at(b, 8), 2.0);
        assert_eq!(at(b, 12), 3.0);
        assert_eq!(at(b, 16), 4.0);
        assert_eq!(at(b, 24), 6.0);
        assert_eq!(i32::from_ne_bytes(b[28..32].try_into().unwrap()), 7);
        assert_eq!(at(b, 32), 1.0);
        assert_eq!(at(b, 48), 8.0);
        assert_eq!(at(b, 64), 9.0);
        assert_eq!(at(b, 80), 10.0);
        assert_eq!(b.len(), 96);
        assert_eq!(MIXED.source().params_size(), b.len());
    }

    #[test]
    fn a_shader_without_parameters_still_has_a_struct() {
        assert_eq!(NOTHING.params(()).params().len(), 16);
        assert_eq!(NOTHING.source().params_size(), 16);
    }

    #[test]
    fn only_the_same_shader_with_the_same_values_is_the_same() {
        static OTHER: Shader<()> = Shader::new("other", "");
        assert_eq!(NOTHING.params(()), NOTHING.params(()));
        assert_ne!(NOTHING.params(()), OTHER.params(()));
    }
}
