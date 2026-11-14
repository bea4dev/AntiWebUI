//! What core knows of a box that the program draws on the GPU, `gpu_canvas`: a number to find what
//! draws it by, and how many times what it shows has changed. What draws it is up to the renderer.
//! See `docs/design/41-gpu-canvas.md`.

use crate::drag::{DragData, DropEvent, GrabEvent};
use crate::event::{DragResult, KeyboardEvent, PinchEvent, PointerEvent, ScrollEvent};
use crate::geometry::PointF;
use crate::reactive::effect;
use crate::shader::{IntoImage, IntoPaint};
use crate::tree::{Node, NodeKind, with_doc};
use crate::view::{ElementConfig, IntoView, NodeRef, View, also, also_bool};
use anti_webui_style::{Sheet, Style, Styled};
use std::any::Any;
use std::rc::Rc;

/// What a canvas is, when nothing says how big: as `<canvas>`.
const NATURAL: (f32, f32) = (300.0, 150.0);

#[doc(hidden)]
pub struct CanvasData {
    /// What the renderer finds what draws it by.
    pub id: u64,
    /// One more each time what it shows changes.
    pub version: u64,
    /// What draws it, for as long as the node is there.
    _keep: Rc<dyn Any>,
}

impl CanvasData {
    pub fn natural(&self) -> (f32, f32) {
        NATURAL
    }
}

/// A box whose pixels the program draws on the GPU. Make one with `anti_webui::gpu::gpu_canvas`.
pub struct GpuCanvas {
    cfg: ElementConfig,
    data: CanvasData,
    update: Box<dyn Fn() -> bool>,
    children: Vec<View>,
}

/// A canvas drawn by what the renderer finds by `id`, which `keep` keeps. `update` runs again when
/// the signals it reads change, and says whether what the canvas shows changed.
#[doc(hidden)]
pub fn gpu_canvas_from(
    id: u64,
    keep: Rc<dyn Any>,
    update: impl Fn() -> bool + 'static,
) -> GpuCanvas {
    GpuCanvas {
        cfg: ElementConfig::default(),
        data: CanvasData {
            id,
            version: 0,
            _keep: keep,
        },
        update: Box::new(update),
        children: Vec::new(),
    }
}

impl Styled for GpuCanvas {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl GpuCanvas {
    crate::view::element_methods!();
    crate::view::box_paint_methods!();

    /// Lays something over the canvas, in its content box. It has no say in the size of the box.
    pub fn child(mut self, child: impl IntoView) -> Self {
        self.children.push(child.into_view());
        self
    }

    pub fn children<V: IntoView>(mut self, children: impl IntoIterator<Item = V>) -> Self {
        self.children
            .extend(children.into_iter().map(IntoView::into_view));
        self
    }
}

impl IntoView for GpuCanvas {
    fn into_view(self) -> View {
        let id = self
            .cfg
            .build(Node::new(NodeKind::Canvas(Box::new(self.data))));
        let children: Vec<_> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        let update = self.update;
        effect(move || {
            if !update() {
                return;
            }
            with_doc(|d| {
                let Some(NodeKind::Canvas(c)) = d.get_mut(id).map(|n| &mut n.kind) else {
                    return;
                };
                c.version += 1;
                // Only the box changes, as when a shader's parameters do.
                d.mark_shader_of(id);
            });
        });
        View(id)
    }
}
