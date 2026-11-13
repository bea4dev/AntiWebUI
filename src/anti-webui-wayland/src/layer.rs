//! Layer surfaces of a desktop shell (`zwlr_layer_shell_v1`): bars, wallpapers, launchers. See
//! `docs/design/37-wayland-shell.md`.

use crate::state::State;
use anti_webui_core::layer::{
    Anchor, InputRegion, Keyboard, Layer, LayerChange, LayerSize, LayerSpec,
};
use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

/// `keyboard_interactivity` can be `on_demand` from this version.
const ON_DEMAND_VERSION: u32 = 4;
/// `set_layer` came in this version.
const SET_LAYER_VERSION: u32 = 2;
/// `set_exclusive_edge` came in this version.
const EXCLUSIVE_EDGE_VERSION: u32 = 5;

pub(crate) struct LayerRole {
    pub object: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    pub spec: LayerSpec,
    /// The exclusive zone last asked for.
    zone: i32,
    /// The size asked for along the axes that follow what is in it.
    content: (i32, i32),
}

/// The outputs a layer surface could go on, with their connector names (`wl_output` v4). On a
/// queue of their own, as they are needed before the window's state is made.
pub(crate) struct Outputs {
    /// Keeps the objects; what it is told later is left there.
    _queue: EventQueue<Names>,
    names: Names,
}

pub(crate) struct Names(Vec<(wl_output::WlOutput, Option<String>)>);

impl Outputs {
    pub(crate) fn bind(conn: &Connection, globals: &GlobalList) -> Result<Self, crate::Error> {
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let found: Vec<(u32, u32)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == wl_output::WlOutput::interface().name)
                .map(|g| (g.name, g.version))
                .collect()
        });
        let mut names = Names(
            found
                .into_iter()
                .map(|(name, version)| {
                    let output = globals.registry().bind(name, version.min(4), &qh, ());
                    (output, None)
                })
                .collect(),
        );
        queue.roundtrip(&mut names).map_err(crate::err)?;
        Ok(Self {
            _queue: queue,
            names,
        })
    }

    pub(crate) fn find(&self, name: &str) -> Option<&wl_output::WlOutput> {
        self.names
            .0
            .iter()
            .find(|(_, n)| n.as_deref() == Some(name))
            .map(|(o, _)| o)
    }

    pub(crate) fn release(self) {
        for (o, _) in self.names.0 {
            if o.version() >= 3 {
                o.release();
            }
        }
    }
}

impl Dispatch<wl_output::WlOutput, ()> for Names {
    fn event(
        names: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some((_, n)) = names.0.iter_mut().find(|(o, _)| o == output)
        {
            *n = Some(name);
        }
    }
}

fn layer(l: Layer) -> zwlr_layer_shell_v1::Layer {
    match l {
        Layer::Background => zwlr_layer_shell_v1::Layer::Background,
        Layer::Bottom => zwlr_layer_shell_v1::Layer::Bottom,
        Layer::Top => zwlr_layer_shell_v1::Layer::Top,
        Layer::Overlay => zwlr_layer_shell_v1::Layer::Overlay,
    }
}

fn anchor(e: Anchor) -> zwlr_layer_surface_v1::Anchor {
    zwlr_layer_surface_v1::Anchor::from_bits_truncate(e.bits() as u32)
}

impl LayerRole {
    /// Makes the surface a layer surface and asks for everything before its first commit.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn create(
        shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        surface: &wl_surface::WlSurface,
        output: Option<&wl_output::WlOutput>,
        spec: LayerSpec,
        size: (i32, i32),
        qh: &QueueHandle<State>,
    ) -> Self {
        let object = shell.get_layer_surface(
            surface,
            output,
            layer(spec.layer),
            spec.namespace.clone(),
            qh,
            (),
        );
        let mut role = Self {
            object,
            spec,
            zone: 0,
            content: size,
        };
        role.send_size();
        role.object.set_anchor(anchor(role.spec.anchor));
        let m = role.spec.margin;
        role.object.set_margin(m.top, m.right, m.bottom, m.left);
        role.send_keyboard();
        role.send_exclusive_edge();
        role.send_zone(size);
        role
    }

    /// What is asked for along an axis: 0 lets the compositor stretch it.
    fn along(size: LayerSize, content: i32) -> u32 {
        match size {
            LayerSize::Fixed(n) => n.max(1) as u32,
            LayerSize::Stretch => 0,
            LayerSize::Content => content.max(1) as u32,
        }
    }

    fn send_size(&self) {
        self.object.set_size(
            Self::along(self.spec.width, self.content.0),
            Self::along(self.spec.height, self.content.1),
        );
    }

    fn send_keyboard(&self) {
        use zwlr_layer_surface_v1::KeyboardInteractivity as K;
        let k = match self.spec.keyboard {
            Keyboard::None => K::None,
            Keyboard::Exclusive => K::Exclusive,
            Keyboard::OnDemand if self.object.version() >= ON_DEMAND_VERSION => K::OnDemand,
            Keyboard::OnDemand => {
                eprintln!(
                    "[anti-webui] the compositor's layer shell can't give the keyboard on demand"
                );
                K::None
            }
        };
        self.object.set_keyboard_interactivity(k);
    }

    fn send_exclusive_edge(&self) {
        if let Some(edge) = self.spec.exclusive_edge
            && self.object.version() >= EXCLUSIVE_EDGE_VERSION
        {
            self.object.set_exclusive_edge(anchor(edge));
        }
    }

    /// The exclusive zone for the size the surface has, if it changed.
    fn send_zone(&mut self, size: (i32, i32)) {
        let zone = self.spec.exclusive_zone(size.0, size.1);
        if zone != self.zone {
            self.zone = zone;
            self.object.set_exclusive_zone(zone);
        }
    }

    /// The size the surface has from a `configure`: what the compositor says where it says it,
    /// otherwise what was asked.
    fn configured_size(&self, width: u32, height: u32, current: (i32, i32)) -> (i32, i32) {
        let pick = |given: u32, size: LayerSize, content: i32, current: i32| {
            if given > 0 {
                given as i32
            } else {
                match size {
                    LayerSize::Fixed(n) => n.max(1),
                    LayerSize::Content => content.max(1),
                    LayerSize::Stretch => current.max(1),
                }
            }
        };
        (
            pick(width, self.spec.width, self.content.0, current.0),
            pick(height, self.spec.height, self.content.1, current.1),
        )
    }
}

impl State {
    pub(crate) fn layer(&mut self) -> Option<&mut LayerRole> {
        match &mut self.role {
            crate::state::SurfaceRole::Layer(l) => Some(l),
            _ => None,
        }
    }

    /// Changes the layer surface, which applies with the commit this makes.
    pub(crate) fn change_layer(&mut self, change: LayerChange) {
        let size = self.logical_size;
        let Some(role) = self.layer() else {
            return;
        };
        role.spec.apply(&change);
        match &change {
            LayerChange::Size(..) => role.send_size(),
            LayerChange::Geometry { anchor: edges, .. } => {
                role.send_size();
                role.object.set_anchor(anchor(*edges));
            }
            LayerChange::Anchor(a) => role.object.set_anchor(anchor(*a)),
            LayerChange::Margin(m) => role.object.set_margin(m.top, m.right, m.bottom, m.left),
            LayerChange::Exclusive(_) => {}
            LayerChange::Keyboard(_) => role.send_keyboard(),
            LayerChange::Layer(l) => {
                if role.object.version() >= SET_LAYER_VERSION {
                    role.object.set_layer(layer(*l));
                }
            }
            LayerChange::InputRegion(_) => {}
        }
        role.send_zone(size);
        if matches!(change, LayerChange::InputRegion(_)) {
            self.apply_input_region();
        }
        self.surface.commit();
    }

    /// Asks for the size of what is in it along the axes that follow it.
    pub(crate) fn set_layer_content_size(&mut self, width: i32, height: i32) {
        let Some(role) = self.layer() else {
            return;
        };
        if role.content == (width, height) {
            return;
        }
        role.content = (width, height);
        role.send_size();
        self.surface.commit();
    }

    /// Where the surface takes the pointer.
    pub(crate) fn apply_input_region(&mut self) {
        let Some(region) = self.layer().map(|r| r.spec.input_region.clone()) else {
            return;
        };
        match region {
            InputRegion::Full => self.surface.set_input_region(None),
            InputRegion::Empty | InputRegion::Rects(_) => {
                let r = self.compositor.create_region(&self.qh, ());
                if let InputRegion::Rects(rects) = region {
                    for rect in rects {
                        r.add(rect.x, rect.y, rect.width, rect.height);
                    }
                }
                self.surface.set_input_region(Some(&r));
                r.destroy();
            }
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for State {
    fn event(
        state: &mut Self,
        surface: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                surface.ack_configure(serial);
                let current = state.logical_size;
                let Some(role) = state.layer() else {
                    return;
                };
                let size = role.configured_size(width, height, current);
                // Applies with the next commit, which the frame for this size makes.
                role.send_zone(size);
                state.logical_size = size;
                state.configured = true;
                state.update_size();
                state.redraw_requested = true;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                // Its output went away, or the compositor wants it gone: nothing to ask first.
                state.closed = true;
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
