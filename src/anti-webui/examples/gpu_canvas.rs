//! A cube drawn with wgpu in a box of the UI (see `docs/design/41-gpu-canvas.md`): drag it to turn
//! it. It is drawn with 4× MSAA and a depth buffer, lit in linear light through the sRGB view, on a
//! transparent background over the box's own, with text laid over it. While it stands still nothing
//! is drawn; while it turns only its box is (`ANTIWEBUI_PERF_OVERLAY=1` shows it). What draws the
//! window, and so the cube, is shown with `gpu_info`.
//!
//! `cargo run --release -p anti-webui --example gpu_canvas`

use anti_webui::gpu::{GpuFrame, GpuInit, GpuPaint, gpu_canvas, wgpu};
use anti_webui::prelude::*;
use std::time::Instant;
use wgpu::util::DeviceExt;

/// What the cube is drawn from.
#[derive(Clone, Copy, PartialEq)]
struct Pose {
    yaw: f32,
    pitch: f32,
    /// In sRGB, as colors are written.
    color: [f32; 3],
}

struct Cube {
    pipeline: wgpu::RenderPipeline,
    vertices: wgpu::Buffer,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    format: wgpu::TextureFormat,
    /// Light has to be made into color by the shader: there is no sRGB view, as on OpenGL.
    encode: bool,
    /// The multisampled color and the depth, and the size they were made for.
    targets: Option<((u32, u32), wgpu::TextureView, wgpu::TextureView)>,
}

const SAMPLES: u32 = 4;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth24Plus;

const SHADER: &str = r#"
struct Uniforms {
    mvp: mat4x4<f32>,
    model: mat4x4<f32>,
    // `a` is 1 where light has to be made into color here.
    color: vec4<f32>,
}
@group(0) @binding(0) var<uniform> u: Uniforms;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) normal: vec3<f32>,
}

@vertex
fn vs(@location(0) position: vec3<f32>, @location(1) normal: vec3<f32>) -> Out {
    var out: Out;
    out.position = u.mvp * vec4<f32>(position, 1.0);
    out.normal = (u.model * vec4<f32>(normal, 0.0)).xyz;
    return out;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    // A light from above and to the right, and a weaker one from where it is seen.
    let key = max(dot(n, normalize(vec3<f32>(0.4, 0.8, 0.6))), 0.0);
    let fill = max(n.z, 0.0);
    var c = u.color.rgb * (0.06 + 0.75 * key + 0.3 * fill);
    if (u.color.a > 0.5) {
        c = pow(c, vec3<f32>(1.0 / 2.2));
    }
    return vec4<f32>(c, 1.0);
}
"#;

impl GpuPaint for Cube {
    type Props = Pose;

    fn new(gpu: &GpuInit<'_>) -> Self {
        // Lit in linear light, stored as the window stores colors.
        let format = gpu.srgb_format.unwrap_or(gpu.format);
        let device = gpu.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cube"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cube"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cube"),
            layout: Some(
                &device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("cube"),
                    bind_group_layouts: &[Some(&layout)],
                    immediate_size: 0,
                }),
            ),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: SAMPLES,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cube"),
            contents: &bytes(&cube()),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cube"),
            size: 36 * 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cube"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        Cube {
            pipeline,
            vertices,
            uniforms,
            bind,
            format,
            encode: gpu.srgb_format.is_none(),
            targets: None,
        }
    }

    fn render(&mut self, pose: &Pose, frame: &mut GpuFrame<'_>) {
        if self.targets.as_ref().is_none_or(|t| t.0 != frame.size) {
            let texture = |format, label| {
                frame
                    .device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some(label),
                        size: wgpu::Extent3d {
                            width: frame.size.0,
                            height: frame.size.1,
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: SAMPLES,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            };
            self.targets = Some((
                frame.size,
                texture(self.format, "cube color"),
                texture(DEPTH, "cube depth"),
            ));
        }
        let (_, color, depth) = self.targets.as_ref().expect("just made");

        let aspect = frame.size.0 as f32 / frame.size.1.max(1) as f32;
        // Turned about its own vertical axis, then tilted toward the eye, as by an orbiting camera.
        let model = mul(rotate_x(pose.pitch), rotate_y(pose.yaw));
        let view = translate(0.0, 0.0, -6.0);
        let mvp = mul(perspective(0.8, aspect, 0.1, 100.0), mul(view, model));
        // What is lit is light, not the color as written.
        let [r, g, b] = pose.color.map(|c| c.powf(2.2));
        let mut uniforms = Vec::with_capacity(36);
        uniforms.extend(mvp);
        uniforms.extend(model);
        uniforms.extend([r, g, b, if self.encode { 1.0 } else { 0.0 }]);
        frame
            .queue
            .write_buffer(&self.uniforms, 0, &bytes(&uniforms));

        let resolve = frame.target_srgb.unwrap_or(frame.target);
        let mut pass = frame
            .encoder
            .begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("cube"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: color,
                    depth_slice: None,
                    resolve_target: Some(resolve),
                    ops: wgpu::Operations {
                        // What is behind the box shows around the cube.
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..36, 0..1);
    }
}

/// The positions and normals of the 36 corners of the triangles of a cube 2 across.
fn cube() -> Vec<f32> {
    let mut out = Vec::with_capacity(36 * 6);
    for axis in 0..3 {
        for sign in [1.0f32, -1.0] {
            let along = |i: usize, v: f32| {
                let mut p = [0.0f32; 3];
                p[(axis + i) % 3] = v;
                p
            };
            let n = along(0, sign);
            let corner = |a: f32, b: f32| {
                let (u, w) = (along(1, a), along(2, b));
                [n[0] + u[0] + w[0], n[1] + u[1] + w[1], n[2] + u[2] + w[2]]
            };
            for (a, b) in [
                (-1., -1.),
                (1., -1.),
                (1., 1.),
                (-1., -1.),
                (1., 1.),
                (-1., 1.),
            ] {
                out.extend(corner(a, b));
                out.extend(n);
            }
        }
    }
    out
}

fn bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_ne_bytes()).collect()
}

/// 4×4 matrices by columns, as WGSL has them.
type Mat = [f32; 16];

fn mul(a: Mat, b: Mat) -> Mat {
    std::array::from_fn(|i| {
        let (c, r) = (i / 4, i % 4);
        (0..4).map(|k| a[k * 4 + r] * b[c * 4 + k]).sum()
    })
}

fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat {
    let f = 1.0 / (fov_y / 2.0).tan();
    let d = far / (near - far);
    [
        f / aspect,
        0.0,
        0.0,
        0.0, //
        0.0,
        f,
        0.0,
        0.0, //
        0.0,
        0.0,
        d,
        -1.0, //
        0.0,
        0.0,
        near * d,
        0.0,
    ]
}

fn translate(x: f32, y: f32, z: f32) -> Mat {
    [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, x, y, z, 1.0,
    ]
}

fn rotate_x(a: f32) -> Mat {
    let (s, c) = a.sin_cos();
    [
        1.0, 0.0, 0.0, 0.0, 0.0, c, s, 0.0, 0.0, -s, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn rotate_y(a: f32) -> Mat {
    let (s, c) = a.sin_cos();
    [
        c, 0.0, -s, 0.0, 0.0, 1.0, 0.0, 0.0, s, 0.0, c, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

/// Turns the cube on for as long as `on` says, by the time between frames.
fn spin(yaw: Signal<f32>, on: Signal<bool>, last: Option<Instant>) {
    request_frame(move |now| {
        if !on.get_untracked() {
            return;
        }
        if let Some(last) = last {
            yaw.update(|y| *y += now.saturating_duration_since(last).as_secs_f32() * 0.8);
        }
        spin(yaw, on, Some(now));
    });
}

const COLORS: [(&str, [f32; 3]); 3] = [
    ("Blue", [0.25, 0.5, 0.95]),
    ("Orange", [0.95, 0.55, 0.2]),
    ("Green", [0.3, 0.8, 0.45]),
];

fn main() {
    App::new()
        .title("gpu_canvas")
        .size(900, 600)
        .run(|| {
            let yaw = signal(0.6f32);
            let pitch = signal(0.4f32);
            let color = signal(COLORS[0].1);
            let auto = signal(true);
            spin(yaw, auto, None);

            let canvas = gpu_canvas::<Cube>(move || Pose {
                yaw: yaw.get(),
                pitch: pitch.get(),
                color: color.get(),
            })
            // A canvas keeps 2:1 like `<canvas>`, which would make it twice as wide as it is high
            // and keep it from shrinking, as in CSS.
            .flex_1()
            .min_w(0.0)
            .h(pct(100.0))
            .rounded(16.0)
            .bg(rgb(0x1e2430))
            .shadow_md()
            .cursor(CursorIcon::Grab)
            .on_grab(move |e| {
                auto.set(false);
                yaw.update(|y| *y += e.delta.x * 0.01);
                pitch.update(|p| *p = (*p + e.delta.y * 0.01).clamp(-1.5, 1.5));
            })
            .child(
                div()
                    .p(12.0)
                    .text_color(rgb(0xe8ecf4))
                    .child(text("Drag to turn it"))
                    // What draws the window, and so the cube: `GpuBackend::Gl` if it fell back.
                    .child(text(move || {
                        gpu_info().map(|g| g.to_string()).unwrap_or_default()
                    }))
                    .child(text(move || {
                        format!(
                            "yaw {:.0}°  pitch {:.0}°",
                            yaw.get().to_degrees(),
                            pitch.get().to_degrees()
                        )
                    })),
            );

            let side = div()
                .w(200.0)
                .flex()
                .flex_col()
                .gap(8.0)
                .child(text("Color"))
                .children(COLORS.map(|(name, c)| button(name).on_click(move |_| color.set(c))))
                .child(
                    button(move || if auto.get() { "Stop" } else { "Spin" }).on_click(move |_| {
                        let on = !auto.get_untracked();
                        auto.set(on);
                        if on {
                            spin(yaw, auto, None);
                        }
                    }),
                );

            div()
                .h(pct(100.0))
                .p(16.0)
                .gap(16.0)
                .flex()
                .flex_row()
                .bg(rgb(0xf4f5f7))
                .child(canvas)
                .child(side)
        })
        .unwrap();
}
