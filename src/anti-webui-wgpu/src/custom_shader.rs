//! Compiling and drawing shaders written by users. See `docs/design/14-shaders.md`.

use anti_webui_core::shader::{Shader, ShaderParams, ShaderSource};
use bytemuck::{Pod, Zeroable};

const PRELUDE: &str = include_str!("custom_prelude.wgsl");
const ENTRY: &str = include_str!("custom_entry.wgsl");

/// Parameters are bound in pieces of this size: what every backend can bind at once.
pub(crate) const CHUNK: usize = 16 * 1024;

/// One box painted by a shader.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShaderInstance {
    pub area: [f32; 4],
    pub rect: [f32; 4],
    pub rect_radii: [f32; 4],
    pub clip: [f32; 4],
    pub clip_radii: [f32; 4],
    pub radii: [f32; 4],
    pub border: [i32; 4],
    pub padding: [i32; 4],
    pub size: [i32; 2],
    pub misc: [f32; 3],
    pub index: u32,
}

/// How many parameters fit in a chunk.
fn per_chunk(params_size: usize) -> usize {
    CHUNK / params_size
}

/// The WGSL declaring the parameters.
fn params_decl(source: &ShaderSource) -> String {
    let mut out = String::from("struct Params {\n");
    if source.fields.is_empty() {
        out.push_str("    @align(16) aw_nothing: f32,\n");
    }
    for (i, field) in source.fields.iter().enumerate() {
        // The struct is an element of a uniform array, which takes an alignment of 16.
        let align = if i == 0 { "@align(16) " } else { "" };
        out.push_str(&format!(
            "    {align}{}: {},\n",
            field.name,
            field.ty.wgsl()
        ));
    }
    out.push_str("}\n");
    out.push_str(&format!(
        "@group(1) @binding(0) var<uniform> aw_params: array<Params, {}>;\n",
        per_chunk(source.params_size())
    ));
    out.push_str("var<private> params: Params;\n\n");
    out
}

/// The whole module, and how many lines come before the user's code.
fn compose(source: &ShaderSource) -> (String, usize) {
    let head = format!("{PRELUDE}{}", params_decl(source));
    let lines = head.matches('\n').count();
    (format!("{head}{}\n{ENTRY}", source.code), lines)
}

/// Checks the code of a shader without a GPU. The error says where in the user's code it is.
pub(crate) fn validate(source: &ShaderSource) -> Result<String, String> {
    if source.params_size() > CHUNK {
        return Err(format!(
            "shader `{}`: its parameters take {} bytes, and at most {CHUNK} fit",
            source.name,
            source.params_size(),
        ));
    }
    let (code, lines) = compose(source);
    let module = naga::front::wgsl::parse_str(&code).map_err(|e| {
        let line = e.location(&code).map(|l| l.line_number as usize);
        report(source, line, lines, e.message())
    })?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .map_err(|e| {
        let line = e
            .spans()
            .next()
            .map(|(span, _)| span.location(&code).line_number as usize);
        let mut message = e.as_inner().to_string();
        let mut cause = std::error::Error::source(e.as_inner());
        while let Some(c) = cause {
            message.push_str(": ");
            message.push_str(&c.to_string());
            cause = c.source();
        }
        report(source, line, lines, &message)
    })?;
    Ok(code)
}

fn report(source: &ShaderSource, line: Option<usize>, before: usize, message: &str) -> String {
    let code_lines = source.code.lines().count();
    match line
        .and_then(|l| l.checked_sub(before))
        .filter(|l| (1..=code_lines).contains(l))
    {
        Some(l) => format!(
            "shader `{}`, line {l}: {message}\n  {l} | {}",
            source.name,
            source.code.lines().nth(l - 1).unwrap_or_default().trim(),
        ),
        // It doesn't point into the user's code: most likely `paint` is missing or has the wrong
        // signature, which shows where the entry point calls it.
        None => format!("shader `{}`: {message}", source.name),
    }
}

/// Checks a shader without a GPU, for unit tests.
pub fn check_shader<P: ShaderParams>(shader: &'static Shader<P>) -> Result<(), String> {
    validate(shader.source()).map(drop)
}

/// A shader as the renderer keeps it.
pub(crate) struct CompiledShader {
    /// `None` if it doesn't compile: it paints nothing.
    pub pipeline: Option<wgpu::RenderPipeline>,
    pub stride: usize,
    /// The parameters of this frame.
    pub data: Vec<u8>,
    pub count: usize,
    pub buffer: Option<wgpu::Buffer>,
    pub bind: Option<wgpu::BindGroup>,
}

impl CompiledShader {
    /// Adds the parameters of an instance. Returns which chunk they are in and at which index.
    pub fn push(&mut self, params: &[u8]) -> (u32, u32) {
        let per_chunk = per_chunk(self.stride);
        let chunk = self.count / per_chunk;
        let index = self.count % per_chunk;
        let at = chunk * CHUNK + index * self.stride;
        self.data.resize(at + self.stride, 0);
        self.data[at..at + params.len()].copy_from_slice(params);
        self.count += 1;
        (chunk as u32, index as u32)
    }

    /// Hands the parameters of this frame to the GPU.
    pub fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
    ) {
        if self.data.is_empty() {
            return;
        }
        // Every chunk is bound whole, even the last one.
        let needed = self.data.len().next_multiple_of(CHUNK) as u64;
        if self.buffer.as_ref().is_none_or(|b| b.size() < needed) {
            let size = needed.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("shader parameters"),
                size,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.bind = Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shader parameters"),
                layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(CHUNK as u64),
                    }),
                }],
            }));
            self.buffer = Some(buffer);
        }
        if let Some(buffer) = &self.buffer {
            queue.write_buffer(buffer, 0, &self.data);
        }
    }
}

/// Compiles a shader. What goes wrong is reported once, and the shader then paints nothing.
pub(crate) fn compile(
    device: &wgpu::Device,
    source: &ShaderSource,
    format: wgpu::TextureFormat,
    globals: &wgpu::BindGroupLayout,
    params: &wgpu::BindGroupLayout,
    inputs: &wgpu::BindGroupLayout,
    cache: Option<&wgpu::PipelineCache>,
) -> CompiledShader {
    let pipeline = match validate(source) {
        Ok(code) => create_pipeline(
            device, source, &code, format, globals, params, inputs, cache,
        ),
        Err(e) => {
            eprintln!("[anti-webui] {e}");
            None
        }
    };
    CompiledShader {
        pipeline,
        stride: source.params_size().min(CHUNK),
        data: Vec::new(),
        count: 0,
        buffer: None,
        bind: None,
    }
}

#[allow(clippy::too_many_arguments)]
fn create_pipeline(
    device: &wgpu::Device,
    source: &ShaderSource,
    code: &str,
    format: wgpu::TextureFormat,
    globals: &wgpu::BindGroupLayout,
    params: &wgpu::BindGroupLayout,
    inputs: &wgpu::BindGroupLayout,
    cache: Option<&wgpu::PipelineCache>,
) -> Option<wgpu::RenderPipeline> {
    let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(source.name),
        source: wgpu::ShaderSource::Wgsl(code.into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(source.name),
        bind_group_layouts: &[Some(globals), Some(params), Some(inputs)],
        immediate_size: 0,
    });
    let attributes = wgpu::vertex_attr_array![
        0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4,
        5 => Float32x4, 6 => Sint32x4, 7 => Sint32x4, 8 => Sint32x2, 9 => Float32x3, 10 => Uint32
    ];
    let blend = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::One,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(source.name),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("aw_vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<ShaderInstance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &attributes,
            })],
        },
        primitive: wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("aw_fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState {
                    color: blend,
                    alpha: blend,
                }),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache,
    });
    if let Some(e) = pollster::block_on(scope.pop()) {
        eprintln!("[anti-webui] shader `{}`: {e}", source.name);
        return None;
    }
    Some(pipeline)
}

/// The layout of the parameters, which a shader binds a chunk of at a time.
pub(crate) fn params_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("shader parameters"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: None,
            },
            count: None,
        }],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    static GOOD: Shader<()> = Shader::new(
        "good",
        "fn paint(f: Frag) -> vec4<f32> {
    return vec4<f32>(f32(aw_box.size.x), aw_shape_distance(f.px), aw_padding_rect().x, 1.0);
}
",
    );

    static BROKEN: Shader<()> = Shader::new(
        "broken",
        "fn paint(f: Frag) -> vec4<f32> {

    return vec4<f32>(nothing_like_this, 0.0, 0.0, 1.0);
}
",
    );

    static WRONG_TYPE: Shader<()> = Shader::new(
        "wrong_type",
        "fn paint(f: Frag) -> vec4<f32> {
    let x: i32 = f.px.x;
    return vec4<f32>(0.0);
}
",
    );

    static NO_PAINT: Shader<()> = Shader::new("no_paint", "fn other() {}\n");

    #[test]
    fn a_good_shader_passes() {
        check_shader(&GOOD).unwrap();
    }

    #[test]
    fn an_error_says_the_line_in_the_users_code() {
        let e = check_shader(&BROKEN).unwrap_err();
        assert!(e.contains("shader `broken`, line 3"), "{e}");
        assert!(e.contains("nothing_like_this"), "{e}");
    }

    #[test]
    fn a_type_error_says_what_it_can() {
        let e = check_shader(&WRONG_TYPE).unwrap_err();
        assert!(e.contains("shader `wrong_type`"), "{e}");
    }

    #[test]
    fn a_missing_paint_is_an_error_too() {
        let e = check_shader(&NO_PAINT).unwrap_err();
        assert!(e.contains("shader `no_paint`"), "{e}");
    }

    #[test]
    fn the_declaration_comes_from_the_fields() {
        use anti_webui_core::shader::{ParamField, ParamType};
        let source = ShaderSource {
            name: "x",
            code: "",
            fields: &[
                ParamField {
                    name: "time",
                    ty: ParamType::F32,
                },
                ParamField {
                    name: "waves",
                    ty: ParamType::Vec4Array(3),
                },
            ],
        };
        let decl = params_decl(&source);
        assert!(decl.contains("@align(16) time: f32"), "{decl}");
        assert!(decl.contains("waves: array<vec4<f32>, 3>"), "{decl}");
        // 16 for the time and 48 for the waves.
        assert!(
            decl.contains(&format!("array<Params, {}>", CHUNK / 64)),
            "{decl}"
        );
    }

    #[test]
    fn parameters_too_big_for_a_chunk_are_an_error() {
        static BIG: ShaderSource = ShaderSource {
            name: "big",
            code: "",
            fields: &[anti_webui_core::shader::ParamField {
                name: "a",
                ty: anti_webui_core::shader::ParamType::Vec4Array(2000),
            }],
        };
        assert!(validate(&BIG).unwrap_err().contains("at most"));
    }

    #[test]
    fn chunks_start_where_they_can_be_bound() {
        let mut shader = CompiledShader {
            pipeline: None,
            stride: 6000,
            data: Vec::new(),
            count: 0,
            buffer: None,
            bind: None,
        };
        let params = [7u8; 6000];
        assert_eq!(shader.push(&params), (0, 0));
        assert_eq!(shader.push(&params), (0, 1));
        // A third wouldn't fit in 16 KiB.
        assert_eq!(shader.push(&params), (1, 0));
        assert_eq!(shader.data[CHUNK], 7);
        assert_eq!(shader.data[12000], 0);
    }
}
