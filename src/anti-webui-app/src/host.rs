//! What the runner, the presenter and the pacing ask of a platform backend, whichever it is (see
//! "app の作り替え" in `docs/design/35-platform-x11.md`).

use anti_webui_core::event::{ClipboardKind, DragAction};
use anti_webui_core::geometry::{PointI, RectI, SizeI};
use anti_webui_core::layer::LayerChange;
use anti_webui_core::platform::{
    BufferEvent, BufferId, CursorIcon, DmabufFeedback, DmabufPlaneDesc, DragOffer, ImeState,
    PopupId, PopupParams, Presentation, SurfaceId, SyncPoints, TimelineId,
};
use anti_webui_core::window::ResizeEdge;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use std::os::fd::BorrowedFd;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A backend's context in its callbacks. Each backend has methods of these names; this only lets
/// the code above them be written once.
// What is about dmabufs only Linux's presenter calls.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) trait Host {
    fn request_redraw(&mut self);
    /// For retrying a frame that could not be presented.
    fn request_redraw_after(&mut self, delay: Duration);
    fn set_cursor(&mut self, icon: CursorIcon);
    fn set_ime(&mut self, ime: Option<ImeState>);
    fn set_clipboard(&mut self, kind: ClipboardKind, text: String);
    fn request_paste(&mut self, kind: ClipboardKind);
    /// Something another thread can call to wake up the event loop.
    fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>>;
    fn set_title(&mut self, title: &str);
    fn set_fullscreen(&mut self, on: bool);
    fn start_move(&mut self);
    fn start_resize(&mut self, edge: ResizeEdge);
    fn minimize(&mut self);
    fn set_maximized(&mut self, on: bool);
    fn show_window_menu(&mut self);
    /// For a layer surface; nothing for a window.
    fn change_layer(&mut self, change: LayerChange);
    fn set_layer_content_size(&mut self, size: SizeI);
    fn exit(&mut self);
    fn buffer_size(&self) -> SizeI;

    fn dmabuf_feedback(&self) -> Option<&DmabufFeedback>;
    /// Wraps a dmabuf for the display server. Failure may come later as [`BufferEvent::Failed`].
    fn create_dmabuf_buffer(
        &mut self,
        width: u32,
        height: u32,
        format: u32,
        modifier: u64,
        planes: &[DmabufPlaneDesc<'_>],
    ) -> Option<BufferId>;
    fn destroy_buffer(&mut self, id: BufferId);
    fn take_buffer_events(&mut self) -> Vec<BufferEvent>;
    /// Asks to be told what becomes of the next commit of the surface. `false` if it can't tell.
    fn request_presentation(&mut self, surface: SurfaceId, token: u64) -> bool;
    fn take_presentations(&mut self) -> Vec<Presentation>;
    /// The clock presentation times are in, as a `clockid_t`.
    fn presentation_clock(&self) -> Option<u32>;
    /// Whether what was last presented to the surface hasn't been taken yet.
    fn frame_pending(&mut self, surface: SurfaceId) -> bool;
    fn has_popups(&self) -> bool;
    /// When the display server last said it took a frame of the surface.
    fn last_frame_callback(&mut self, surface: SurfaceId) -> Option<Instant>;
    fn explicit_sync_available(&self) -> bool;
    fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Option<TimelineId>;
    fn destroy_timeline(&mut self, id: TimelineId);
    fn disable_explicit_sync(&mut self);
    /// Shows the buffer, with `damage` (buffer pixels) as what changed.
    fn present_buffer(
        &mut self,
        surface: SurfaceId,
        id: BufferId,
        damage: &[RectI],
        sync: Option<SyncPoints>,
    );
    /// Around something else presenting to the surface, like a GPU surface.
    fn prepare_commit(&mut self, surface: SurfaceId);
    fn committed(&mut self, surface: SurfaceId);
    fn set_opaque(&mut self, surface: SurfaceId, opaque: bool);
    /// Whether the surface was uncovered and has to be drawn again in full.
    fn take_exposed(&mut self, surface: SurfaceId) -> bool;
    /// Raw handles for creating a GPU surface. Valid until the surface closes. `None` where the
    /// platform can't make one, like for a window on a thread of its own on macOS.
    fn raw_handles(&self, surface: SurfaceId) -> Option<(RawDisplayHandle, RawWindowHandle)>;
    /// Wraps an `IOSurface` for showing on a layer.
    #[cfg(target_os = "macos")]
    fn create_iosurface_buffer(
        &mut self,
        surface: &anti_webui_wgpu::iosurface::IOSurfaceRef,
    ) -> Option<BufferId>;

    fn show_popup(&mut self, params: PopupParams);
    fn update_popup(&mut self, params: PopupParams);
    fn close_popup(&mut self, id: PopupId);
    fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId>;
    fn supports_drag(&self) -> bool;
    fn start_drag(&mut self, offer: DragOffer);
    fn cancel_drag(&mut self);
    fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI);
    fn close_drag_icon(&mut self);
    fn accept_drag(&mut self, wanted: Option<(String, DragAction)>);
    fn receive_drop(&mut self, mime: String);
    fn finish_drop(&mut self);
}

/// Implements [`Host`] for a backend's context by calling its methods of the same names.
macro_rules! host_for {
    ($ctx:ty) => {
        impl Host for $ctx {
            fn request_redraw(&mut self) {
                self.request_redraw()
            }
            fn request_redraw_after(&mut self, delay: Duration) {
                self.request_redraw_after(delay)
            }
            fn set_cursor(&mut self, icon: CursorIcon) {
                self.set_cursor(icon)
            }
            fn set_ime(&mut self, ime: Option<ImeState>) {
                self.set_ime(ime)
            }
            fn set_clipboard(&mut self, kind: ClipboardKind, text: String) {
                self.set_clipboard(kind, text)
            }
            fn request_paste(&mut self, kind: ClipboardKind) {
                self.request_paste(kind)
            }
            fn waker(&self) -> Option<Arc<dyn Fn() + Send + Sync>> {
                let waker = self.waker()?;
                Some(Arc::new(waker))
            }
            fn set_title(&mut self, title: &str) {
                self.set_title(title)
            }
            fn set_fullscreen(&mut self, on: bool) {
                self.set_fullscreen(on)
            }
            fn start_move(&mut self) {
                self.start_move()
            }
            fn start_resize(&mut self, edge: ResizeEdge) {
                self.start_resize(edge)
            }
            fn minimize(&mut self) {
                self.minimize()
            }
            fn set_maximized(&mut self, on: bool) {
                self.set_maximized(on)
            }
            fn show_window_menu(&mut self) {
                self.show_window_menu()
            }
            fn change_layer(&mut self, change: LayerChange) {
                self.change_layer(change)
            }
            fn set_layer_content_size(&mut self, size: SizeI) {
                self.set_layer_content_size(size)
            }
            fn exit(&mut self) {
                self.exit()
            }
            fn buffer_size(&self) -> SizeI {
                self.buffer_size()
            }
            fn dmabuf_feedback(&self) -> Option<&DmabufFeedback> {
                self.dmabuf_feedback()
            }
            fn create_dmabuf_buffer(
                &mut self,
                width: u32,
                height: u32,
                format: u32,
                modifier: u64,
                planes: &[DmabufPlaneDesc<'_>],
            ) -> Option<BufferId> {
                self.create_dmabuf_buffer(width, height, format, modifier, planes)
            }
            fn destroy_buffer(&mut self, id: BufferId) {
                self.destroy_buffer(id)
            }
            fn take_buffer_events(&mut self) -> Vec<BufferEvent> {
                self.take_buffer_events()
            }
            fn request_presentation(&mut self, surface: SurfaceId, token: u64) -> bool {
                self.request_presentation(surface, token)
            }
            fn take_presentations(&mut self) -> Vec<Presentation> {
                self.take_presentations()
            }
            fn presentation_clock(&self) -> Option<u32> {
                self.presentation_clock()
            }
            fn frame_pending(&mut self, surface: SurfaceId) -> bool {
                self.frame_pending(surface)
            }
            fn has_popups(&self) -> bool {
                self.has_popups()
            }
            fn last_frame_callback(&mut self, surface: SurfaceId) -> Option<Instant> {
                self.last_frame_callback(surface)
            }
            fn explicit_sync_available(&self) -> bool {
                self.explicit_sync_available()
            }
            fn import_timeline(&mut self, fd: BorrowedFd<'_>) -> Option<TimelineId> {
                self.import_timeline(fd)
            }
            fn destroy_timeline(&mut self, id: TimelineId) {
                self.destroy_timeline(id)
            }
            fn disable_explicit_sync(&mut self) {
                self.disable_explicit_sync()
            }
            fn present_buffer(
                &mut self,
                surface: SurfaceId,
                id: BufferId,
                damage: &[RectI],
                sync: Option<SyncPoints>,
            ) {
                self.present_buffer(surface, id, damage, sync)
            }
            fn prepare_commit(&mut self, surface: SurfaceId) {
                self.prepare_commit(surface)
            }
            fn committed(&mut self, surface: SurfaceId) {
                self.committed(surface)
            }
            fn set_opaque(&mut self, surface: SurfaceId, opaque: bool) {
                self.set_opaque(surface, opaque)
            }
            fn take_exposed(&mut self, surface: SurfaceId) -> bool {
                self.take_exposed(surface)
            }
            fn raw_handles(
                &self,
                surface: SurfaceId,
            ) -> Option<(RawDisplayHandle, RawWindowHandle)> {
                self.raw_handles(surface)
            }
            #[cfg(target_os = "macos")]
            fn create_iosurface_buffer(
                &mut self,
                surface: &anti_webui_wgpu::iosurface::IOSurfaceRef,
            ) -> Option<BufferId> {
                self.create_iosurface_buffer(surface)
            }
            fn show_popup(&mut self, params: PopupParams) {
                self.show_popup(params)
            }
            fn update_popup(&mut self, params: PopupParams) {
                self.update_popup(params)
            }
            fn close_popup(&mut self, id: PopupId) {
                self.close_popup(id)
            }
            fn popups_attached_to(&self, id: PopupId) -> Vec<PopupId> {
                self.popups_attached_to(id)
            }
            fn supports_drag(&self) -> bool {
                self.supports_drag()
            }
            fn start_drag(&mut self, offer: DragOffer) {
                self.start_drag(offer)
            }
            fn cancel_drag(&mut self) {
                self.cancel_drag()
            }
            fn place_drag_icon(&mut self, logical: SizeI, hotspot: PointI) {
                self.place_drag_icon(logical, hotspot)
            }
            fn close_drag_icon(&mut self) {
                self.close_drag_icon()
            }
            fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
                self.accept_drag(wanted)
            }
            fn receive_drop(&mut self, mime: String) {
                self.receive_drop(mime)
            }
            fn finish_drop(&mut self) {
                self.finish_drop()
            }
        }
    };
}

#[cfg(all(target_os = "linux", feature = "wayland"))]
host_for!(anti_webui_wayland::Ctx<'_>);
#[cfg(all(target_os = "linux", feature = "x11"))]
host_for!(anti_webui_x11::Ctx<'_>);
#[cfg(all(target_os = "macos", feature = "macos"))]
host_for!(anti_webui_macos::Ctx<'_>);
