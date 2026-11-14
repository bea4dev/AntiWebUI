//! The ramps a monitor's colors go through on their way to the screen
//! (`zwlr_gamma_control_manager_v1`). A client that holds an output's ramps decides what every
//! framebuffer value of it comes out as, e.g. to warm a screen up for the evening. A compositor
//! lets one client of an output have them, so the ones asked for are held onto until they are
//! given back.

use crate::conn::{State, link, output_object};
use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::os::unix::io::AsFd;
use std::sync::{Arc, Mutex, OnceLock};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_wlr::gamma_control::v1::client::{
    zwlr_gamma_control_manager_v1 as manager, zwlr_gamma_control_v1 as control,
};

/// An output's gamma ramps. The same one comes back for an output until
/// [`release`](Gamma::release) or the compositor fails it, and after a failure the next
/// [`gamma`] asks for them again. Cheap to clone.
#[derive(Clone, Debug)]
pub struct Gamma(Arc<Inner>);

#[derive(Debug)]
struct Inner {
    /// The control of the ramps; gone once given back or failed, both of which leave it inert.
    object: Mutex<Option<control::ZwlrGammaControlV1>>,
    /// How many entries a ramp has; the compositor says so when it makes the control.
    size: SharedSignal<u32>,
    failed: SharedSignal<bool>,
}

impl Drop for Inner {
    fn drop(&mut self) {
        if let (Some(o), Some(link)) = (self.object.lock().ok().and_then(|mut o| o.take()), link())
        {
            o.destroy();
            link.flush();
        }
    }
}

static MANAGER: OnceLock<Option<manager::ZwlrGammaControlManagerV1>> = OnceLock::new();

/// The ramps that are held, by the name of the output they are of.
static HELD: Mutex<Vec<(String, Gamma)>> = Mutex::new(Vec::new());

/// An output's gamma ramps, asking the compositor for them if it has none yet. `None` when the
/// compositor doesn't do gamma, or there is no such output.
pub fn gamma(output: &str) -> Option<Gamma> {
    if let Ok(held) = HELD.lock() {
        if let Some((_, gamma)) = held.iter().find(|(name, _)| name == output) {
            return Some(gamma.clone());
        }
    }
    let manager = MANAGER.get_or_init(|| link()?.bind(1, ())).as_ref()?;
    let screen = output_object(output)?;
    let link = link()?;
    let gamma = Gamma(Arc::new(Inner {
        object: Mutex::new(None),
        size: shared_signal(0),
        failed: shared_signal(false),
    }));
    let object = manager.get_gamma_control(&screen, &link.qh, gamma.0.clone());
    *gamma.0.object.lock().ok()? = Some(object);
    link.flush();
    // How big the ramps are, before anyone has to ask.
    link.sync();
    if !gamma.0.failed.get() {
        if let Ok(mut held) = HELD.lock() {
            held.retain(|(name, _)| name != output);
            held.push((output.to_owned(), gamma.clone()));
        }
    }
    Some(gamma)
}

impl Gamma {
    /// Whether the ramps can be set: the compositor has said how big they are and hasn't failed
    /// the output.
    pub fn available(&self) -> bool {
        !self.0.failed.get() && self.0.size.get() > 0
    }

    /// How many entries each of the three ramps has to have.
    pub fn size(&self) -> usize {
        self.0.size.get() as usize
    }

    /// Puts the three ramps in the way of the screen: what a framebuffer value of an entry's
    /// place comes out as, as 16 bits, red then green then blue. All three have to be
    /// [`size`](Gamma::size) long, and a given-back or failed output takes nothing. Whether
    /// anything was sent.
    pub fn set(&self, red: &[u16], green: &[u16], blue: &[u16]) -> bool {
        let size = self.0.size.get() as usize;
        if size == 0
            || [red, green, blue].iter().any(|r| r.len() != size)
            || self.0.failed.get()
        {
            return false;
        }
        let (Some(link), Some(object)) = (
            link(),
            self.0.object.lock().ok().and_then(|o| o.clone()),
        ) else {
            return false;
        };
        // The ramps go over as a file of u16s in the order of the machine, the three of them
        // one after another: the protocol says the file's length and no more.
        let mut file = Vec::with_capacity(size * 6);
        for ramp in [red, green, blue] {
            file.extend(ramp.iter().flat_map(|v| v.to_ne_bytes()));
        }
        let Ok((read, written)) = rustix::pipe::pipe() else {
            return false;
        };
        let mut at = 0;
        while at < file.len() {
            match rustix::io::write(&written, &file[at..]) {
                Ok(n) => at += n,
                Err(_) => return false,
            }
        }
        // What the file holds is what there is: after this, reading it ends.
        drop(written);
        object.set_gamma(read.as_fd());
        link.flush();
        true
    }

    /// Gives the ramps back, and the compositor goes back to its own. The next [`gamma`] for
    /// the output asks for them again.
    pub fn release(&self) {
        if let Some(o) = self.0.object.lock().ok().and_then(|mut o| o.take()) {
            o.destroy();
            if let Some(link) = link() {
                link.flush();
            }
        }
        if let Ok(mut held) = HELD.lock() {
            held.retain(|(_, g)| !Arc::ptr_eq(&g.0, &self.0));
        }
    }
}

impl Dispatch<control::ZwlrGammaControlV1, Arc<Inner>> for State {
    fn event(
        _: &mut Self,
        _: &control::ZwlrGammaControlV1,
        event: control::Event,
        data: &Arc<Inner>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            control::Event::GammaSize { size } => data.size.set(size),
            control::Event::Failed => {
                data.failed.set(true);
                // A failed one is of no use: it goes, and the next gamma() asks again.
                if let Some(o) = data.object.lock().ok().and_then(|mut o| o.take()) {
                    o.destroy();
                    if let Some(link) = link() {
                        link.flush();
                    }
                }
                if let Ok(mut held) = HELD.lock() {
                    held.retain(|(_, g)| !Arc::ptr_eq(&g.0, data));
                }
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(State: ignore manager::ZwlrGammaControlManagerV1);
