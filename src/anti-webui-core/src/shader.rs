//! Shaders written in WGSL that paint inside of a box. See `docs/design/14-shaders.md`.

use anti_webui_style::Color;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex};

/// The type of a shader parameter in WGSL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
pub struct ParamField {
    pub name: &'static str,
    pub ty: ParamType,
}

/// Lays out values the way WGSL does in a uniform buffer.
#[derive(Default)]
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
pub fn preloads_from(start: usize) -> Vec<&'static ShaderSource> {
    let list = PRELOADS.lock().expect("nothing panics while holding it");
    list.get(start..).map(<[_]>::to_vec).unwrap_or_default()
}

/// A shader with its parameters: what to paint with.
#[derive(Clone, Debug)]
pub struct ShaderPaint {
    source: &'static ShaderSource,
    params: Arc<[u8]>,
}

impl ShaderPaint {
    pub fn source(&self) -> &'static ShaderSource {
        self.source
    }

    pub fn params(&self) -> &[u8] {
        &self.params
    }
}

impl PartialEq for ShaderPaint {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.source, other.source)
            && (Arc::ptr_eq(&self.params, &other.params) || self.params == other.params)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
