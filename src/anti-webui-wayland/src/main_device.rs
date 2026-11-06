//! Which GPU the compositor uses, asked on a short connection of its own before anything else
//! starts. See "読み込む Vulkan ドライバ" in `docs/design/06-platform-wayland.md`.

use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_dmabuf_feedback_v1, zwp_linux_dmabuf_v1,
};

#[derive(Default)]
struct Asking {
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    main_device: Option<u64>,
    done: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for Asking {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
            && interface == "zwp_linux_dmabuf_v1"
            && version >= 4
        {
            state.dmabuf = Some(registry.bind(name, 4, qh, ()));
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for Asking {
    fn event(
        state: &mut Self,
        _: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_linux_dmabuf_feedback_v1::Event;
        match event {
            Event::MainDevice { device } => {
                state.main_device = device
                    .get(..8)
                    .map(|b| u64::from_ne_bytes(b.try_into().unwrap()));
            }
            Event::Done => state.done = true,
            _ => {}
        }
    }
}

delegate_noop!(Asking: ignore zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1);

/// The `dev_t` of the GPU the compositor uses (its main device), from `zwp_linux_dmabuf_v1`
/// version 4. `None` without a compositor that says.
pub fn compositor_main_device() -> Option<u64> {
    let conn = Connection::connect_to_env().ok()?;
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let _registry = conn.display().get_registry(&qh, ());
    let mut state = Asking::default();
    queue.roundtrip(&mut state).ok()?;
    let feedback = state.dmabuf.as_ref()?.get_default_feedback(&qh, ());
    // The feedback comes in one go, ending with `done`.
    for _ in 0..4 {
        if state.done {
            break;
        }
        queue.roundtrip(&mut state).ok()?;
    }
    feedback.destroy();
    if let Some(d) = &state.dmabuf {
        d.destroy();
    }
    let _ = conn.flush();
    state.main_device
}
