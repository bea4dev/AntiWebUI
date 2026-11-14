//! `gpu_canvas`: a box that the program draws with wgpu, on the GPU and the device that draws the
//! window. See `docs/design/41-gpu-canvas.md`.
//!
//! ```ignore
//! let angle = signal(0.0f32);
//! gpu_canvas::<Spinner>(move || angle.get()).w(320.0).h(200.0).rounded(16.0)
//! ```

pub use anti_webui_core::canvas::GpuCanvas;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};

/// What draws a [`gpu_canvas`].
pub trait GpuPaint: 'static {
    /// What it is drawn from. When what `gpu_canvas` is given returns what is equal to the last,
    /// nothing is drawn again.
    type Props: PartialEq + 'static;

    /// Makes what it draws with, on the device that draws the window: once, and again when that
    /// becomes another GPU.
    fn new(gpu: &GpuInit<'_>) -> Self
    where
        Self: Sized;

    /// Draws into `frame.target`, which holds what was drawn last. Called when the props or the
    /// size of the box change, and on nothing else.
    fn render(&mut self, props: &Self::Props, frame: &mut GpuFrame<'_>);
}

/// What a [`GpuPaint`] is made on.
pub struct GpuInit<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// That of [`GpuFrame::target`]: what is written is what shows, as in the rest of the window.
    pub format: wgpu::TextureFormat,
    /// That of [`GpuFrame::target_srgb`], which takes light as it is and stores it as `format`
    /// stores colors. `None` where views of other formats can't be made, as with OpenGL.
    pub srgb_format: Option<wgpu::TextureFormat>,
}

/// What a [`GpuPaint`] draws into, and with.
pub struct GpuFrame<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    /// Submitted on its own before the window is drawn, so that what goes wrong in it is no harm to
    /// the rest of the window.
    pub encoder: &'a mut wgpu::CommandEncoder,
    /// All of the content box, in [`GpuInit::format`], premultiplied.
    pub target: &'a wgpu::TextureView,
    /// The same texture in [`GpuInit::srgb_format`].
    pub target_srgb: Option<&'a wgpu::TextureView>,
    /// The size of `target` in its pixels.
    pub size: (u32, u32),
    /// How many of its pixels make a logical pixel.
    pub scale: f32,
}

impl GpuFrame<'_> {
    /// A pass over [`target`](Self::target) with nothing but color, cleared to `clear` if given.
    pub fn begin_pass(&mut self, clear: Option<wgpu::Color>) -> wgpu::RenderPass<'_> {
        pass(self.encoder, self.target, clear)
    }

    /// A pass over [`target_srgb`](Self::target_srgb), or over `target` where there is none.
    pub fn begin_srgb_pass(&mut self, clear: Option<wgpu::Color>) -> wgpu::RenderPass<'_> {
        pass(self.encoder, self.target_srgb.unwrap_or(self.target), clear)
    }
}

fn pass<'e>(
    encoder: &'e mut wgpu::CommandEncoder,
    view: &wgpu::TextureView,
    clear: Option<wgpu::Color>,
) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gpu_canvas"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// A box drawn by `T` from what `props` returns, which runs again when the signals it reads change.
/// Like `<canvas>`, it is 300 by 150 when nothing says how big, and what is drawn is as many pixels
/// as the box is on the screen.
pub fn gpu_canvas<T: GpuPaint>(props: impl Fn() -> T::Props + 'static) -> GpuCanvas {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let canvas: Rc<Canvas<T>> = Rc::new(Canvas {
        props: RefCell::new(None),
        made: RefCell::new(None),
        told: Cell::new(false),
    });
    let painter: Rc<dyn Painter> = canvas.clone();
    PAINTERS.with(|p| p.borrow_mut().insert(id, Rc::downgrade(&painter)));
    let slot = Rc::downgrade(&canvas);
    anti_webui_core::canvas::gpu_canvas_from(id, canvas, move || {
        let props = props();
        let Some(canvas) = slot.upgrade() else {
            return false;
        };
        let mut was = canvas.props.borrow_mut();
        if was.as_ref() == Some(&props) {
            return false;
        }
        *was = Some(props);
        true
    })
}

/// What draws each canvas of this thread, by its id: the UI and the renderer are on one thread.
struct Painters {
    of: HashMap<u64, Weak<dyn Painter>>,
    /// How many there can be before those that are gone are looked for.
    limit: usize,
}

impl Painters {
    fn insert(&mut self, id: u64, painter: Weak<dyn Painter>) {
        if self.of.len() >= self.limit {
            self.of.retain(|_, p| p.strong_count() > 0);
            self.limit = (self.of.len() * 2).max(64);
        }
        self.of.insert(id, painter);
    }
}

thread_local! {
    static PAINTERS: RefCell<Painters> = RefCell::new(Painters {
        of: HashMap::new(),
        limit: 64,
    });
}

/// Whether what draws the canvas `id` is still there.
pub(crate) fn alive(id: u64) -> bool {
    PAINTERS.with(|p| p.borrow().of.get(&id).is_some_and(|p| p.strong_count() > 0))
}

/// Where a canvas is drawn this frame.
pub(crate) struct Target<'a> {
    pub view: &'a wgpu::TextureView,
    pub srgb_view: Option<&'a wgpu::TextureView>,
    pub size: (u32, u32),
    pub scale: f32,
}

/// Has the canvas `id` drawn into `target`, and says whether it was.
pub(crate) fn render(
    id: u64,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    target: &Target<'_>,
) -> bool {
    let painter = PAINTERS.with(|p| p.borrow().of.get(&id).and_then(Weak::upgrade));
    painter.is_some_and(|p| p.render(device, queue, target))
}

trait Painter {
    fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, target: &Target<'_>) -> bool;
}

struct Canvas<T: GpuPaint> {
    /// The last of what it is drawn from. Taken out while drawing.
    props: RefCell<Option<T::Props>>,
    /// What draws, and the device it was made on. `None` in it when making it failed.
    made: RefCell<Option<(wgpu::Device, Option<T>)>>,
    /// Whether something went wrong before, which is said only once.
    told: Cell<bool>,
}

impl<T: GpuPaint> Canvas<T> {
    fn tell(&self, what: &str, e: &wgpu::Error) {
        if !self.told.replace(true) {
            log::error!("gpu_canvas<{}> {what}: {e}", std::any::type_name::<T>());
        }
    }
}

impl<T: GpuPaint> Painter for Canvas<T> {
    fn render(&self, device: &wgpu::Device, queue: &wgpu::Queue, target: &Target<'_>) -> bool {
        let mut made = self.made.borrow_mut();
        if made.as_ref().is_none_or(|(d, _)| d != device) {
            let gpu = GpuInit {
                device,
                queue,
                format: CANVAS_FORMAT,
                srgb_format: target.srgb_view.map(|_| SRGB_FORMAT),
            };
            let (painter, error) = guarded(device, || T::new(&gpu));
            if let Some(e) = &error {
                self.tell("couldn't be made", e);
            }
            *made = Some((device.clone(), error.is_none().then_some(painter)));
        }
        let Some((_, Some(painter))) = made.as_mut() else {
            return false;
        };
        // Taken out, so that what is set while drawing waits for the next frame rather than
        // finding it borrowed.
        let Some(props) = self.props.borrow_mut().take() else {
            return false;
        };
        let (commands, error) = guarded(device, || {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("gpu_canvas"),
            });
            painter.render(
                &props,
                &mut GpuFrame {
                    device,
                    queue,
                    encoder: &mut encoder,
                    target: target.view,
                    target_srgb: target.srgb_view,
                    size: target.size,
                    scale: target.scale,
                },
            );
            encoder.finish()
        });
        let mut now = self.props.borrow_mut();
        if now.is_none() {
            *now = Some(props);
        }
        drop(now);
        if let Some(e) = &error {
            self.tell("failed to draw", e);
            return false;
        }
        queue.submit([commands]);
        true
    }
}

/// What a canvas is drawn in.
pub(crate) const CANVAS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
pub(crate) const SRGB_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Runs `f`, catching the errors of what it does on `device` rather than letting them take the
/// program down.
fn guarded<R>(device: &wgpu::Device, f: impl FnOnce() -> R) -> (R, Option<wgpu::Error>) {
    let internal = device.push_error_scope(wgpu::ErrorFilter::Internal);
    let memory = device.push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    let validation = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let out = f();
    let errors = [validation.pop(), memory.pop(), internal.pop()].map(pollster::block_on);
    (out, errors.into_iter().flatten().next())
}
