//! Drag and drop with other programs: the view is a dragging destination for what comes in, and
//! the source of the drags it hands to AppKit (see "ドラッグ＆ドロップ" in
//! `docs/design/37-platform-macos.md`). Drags of ours over our own windows come in the same way.

use crate::frames::Sendable;
use crate::keyboard::modifiers_of;
use crate::state::State;
use crate::view::View;
use anti_webui_core::event::{DragAction, DragActions, DragResult, WindowEvent};
use anti_webui_core::geometry::{PointF, PointI, SizeI};
use anti_webui_core::platform::DragOffer;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{AnyThread, DefinedClass};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageRep, NSDeviceRGBColorSpace, NSDragOperation, NSDraggingInfo,
    NSDraggingItem, NSEvent, NSImage, NSPasteboard, NSPasteboardItem, NSPasteboardType,
};
use objc2_core_foundation::CFRetained;
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString, NSURL};
use objc2_io_surface::{IOSurfaceLockOptions, IOSurfaceRef};

const TEXT: &str = "public.utf8-plain-text";
const FILE_URL: &str = "public.file-url";
const TEXT_MIMES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
const FILES_MIME: &str = "text/uri-list";

/// The pasteboard type of a MIME type. Files go one per item, as `public.file-url`; what has no
/// type of the system's goes by its MIME type, which our windows understand.
fn type_of(mime: &str) -> &str {
    match mime {
        m if TEXT_MIMES.contains(&m) => TEXT,
        FILES_MIME => FILE_URL,
        "text/html" => "public.html",
        "image/png" => "public.png",
        m => m,
    }
}

/// The MIME types of what a pasteboard has, as the core knows them.
fn mimes_of(types: &[String]) -> Vec<String> {
    let mut mimes: Vec<String> = Vec::new();
    let mut add = |m: &str| {
        if !mimes.iter().any(|x| x == m) {
            mimes.push(m.to_owned());
        }
    };
    for t in types {
        match t.as_str() {
            TEXT => {
                add(TEXT_MIMES[0]);
                add(TEXT_MIMES[1]);
            }
            FILE_URL => add(FILES_MIME),
            "public.html" => add("text/html"),
            "public.png" => add("image/png"),
            m if m.contains('/') => add(m),
            _ => {}
        }
    }
    mimes
}

fn ns(s: &str) -> Retained<NSPasteboardType> {
    NSString::from_str(s)
}

/// What is on a pasteboard in the form of a MIME type.
fn read(board: &NSPasteboard, mime: &str) -> Vec<u8> {
    let items = board
        .pasteboardItems()
        .map(|a| a.to_vec())
        .unwrap_or_default();
    if mime == FILES_MIME {
        // `text/uri-list` of the files, with what Finder gives as file references made paths.
        let mut list = String::new();
        for item in &items {
            let Some(s) = item.stringForType(&ns(FILE_URL)) else {
                continue;
            };
            let url = NSURL::URLWithString(&s)
                .and_then(|u| u.filePathURL())
                .and_then(|u| u.absoluteString())
                .map_or_else(|| s.to_string(), |u| u.to_string());
            list.push_str(&url);
            list.push_str("\r\n");
        }
        return list.into_bytes();
    }
    let t = ns(type_of(mime));
    if type_of(mime) == TEXT {
        return board
            .stringForType(&t)
            .map(|s| s.to_string().into_bytes())
            .unwrap_or_default();
    }
    board
        .dataForType(&t)
        .map(|d| d.to_vec())
        .unwrap_or_default()
}

/// A drag coming in, from another program or from us.
pub(crate) struct Incoming {
    mimes: Vec<String>,
    /// What the core said it would take.
    accepted: Option<(String, DragAction)>,
    /// What may be done with it, as the source says.
    mask: NSDragOperation,
    board: Retained<NSPasteboard>,
    /// Where it was and with what modifiers, last told.
    last: Option<(PointF, anti_webui_core::event::Modifiers)>,
    dropped: bool,
}

/// A drag of ours, which AppKit runs.
pub(crate) struct Outgoing {
    actions: DragActions,
    /// The core ended it; what AppKit says at the end is no news.
    cancelled: bool,
}

#[derive(Default)]
pub(crate) struct Dnd {
    pub incoming: Option<Incoming>,
    pub outgoing: Option<Outgoing>,
}

impl Dnd {
    /// Whether a drag of ours is going on, during which AppKit's loop runs instead of ours.
    pub fn dragging(&self) -> bool {
        self.outgoing.is_some()
    }
}

fn operation_of(action: DragAction, mask: NSDragOperation) -> NSDragOperation {
    match action {
        DragAction::Move if mask.contains(NSDragOperation::Move) => NSDragOperation::Move,
        DragAction::Move if mask.contains(NSDragOperation::Generic) => NSDragOperation::Generic,
        DragAction::Copy if mask.contains(NSDragOperation::Copy) => NSDragOperation::Copy,
        _ => NSDragOperation::None,
    }
}

/// What the view does as a dragging destination.
impl View {
    fn drag_position(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> PointF {
        self.position_in_window(info.draggingLocation())
    }

    pub(crate) fn drag_entered(
        &self,
        info: &ProtocolObject<dyn NSDraggingInfo>,
    ) -> NSDragOperation {
        let shared = &self.ivars().shared;
        let board = info.draggingPasteboard();
        let types: Vec<String> = board
            .types()
            .map(|a| a.iter().map(|t| t.to_string()).collect())
            .unwrap_or_default();
        let mimes = mimes_of(&types);
        let position = self.drag_position(info);
        let modifiers = modifiers_of(NSEvent::modifierFlags_class());
        shared.dnd.borrow_mut().incoming = Some(Incoming {
            mimes: mimes.clone(),
            accepted: None,
            mask: info.draggingSourceOperationMask(),
            board,
            last: Some((position, modifiers)),
            dropped: false,
        });
        shared.push(WindowEvent::DragEntered {
            position,
            mimes,
            modifiers,
        });
        NSDragOperation::None
    }

    pub(crate) fn drag_updated(
        &self,
        info: &ProtocolObject<dyn NSDraggingInfo>,
    ) -> NSDragOperation {
        let shared = &self.ivars().shared;
        let position = self.drag_position(info);
        let modifiers = modifiers_of(NSEvent::modifierFlags_class());
        let mut dnd = shared.dnd.borrow_mut();
        let Some(inc) = &mut dnd.incoming else {
            return NSDragOperation::None;
        };
        inc.mask = info.draggingSourceOperationMask();
        let op = inc
            .accepted
            .as_ref()
            .map_or(NSDragOperation::None, |(_, a)| operation_of(*a, inc.mask));
        let moved = inc.last.replace((position, modifiers)) != Some((position, modifiers));
        drop(dnd);
        if moved {
            shared.push(WindowEvent::DragMoved {
                position,
                modifiers,
            });
        }
        // A drag of ours: AppKit's loop runs, and the core answers from ours.
        if shared.dnd.borrow().dragging() {
            crate::app::run_pump();
        }
        op
    }

    pub(crate) fn drag_exited(&self) {
        let shared = &self.ivars().shared;
        let gone = shared.dnd.borrow_mut().incoming.take();
        if gone.is_some_and(|i| !i.dropped) {
            shared.push(WindowEvent::DragLeft);
        }
    }

    /// The drop: taken if the core said what it would take. That is read at once, while the
    /// pasteboard surely has it, and handed over when the core asks.
    pub(crate) fn drag_performed(&self, info: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
        let shared = &self.ivars().shared;
        let position = self.drag_position(info);
        let mut dnd = shared.dnd.borrow_mut();
        let Some(inc) = &mut dnd.incoming else {
            return false;
        };
        let Some((_, action)) = inc.accepted.clone() else {
            return false;
        };
        inc.dropped = true;
        drop(dnd);
        shared.push(WindowEvent::DragDropped { position, action });
        true
    }
}

/// What the view does as the source of a drag of ours.
impl View {
    pub(crate) fn drag_source_mask(&self) -> NSDragOperation {
        let dnd = self.ivars().shared.dnd.borrow();
        match dnd.outgoing.as_ref().map(|o| o.actions) {
            Some(DragActions::Move) => NSDragOperation::Move | NSDragOperation::Generic,
            Some(DragActions::Copy) => NSDragOperation::Copy,
            Some(DragActions::Both) => {
                NSDragOperation::Copy | NSDragOperation::Move | NSDragOperation::Generic
            }
            None => NSDragOperation::None,
        }
    }

    pub(crate) fn drag_ended(&self, operation: NSDragOperation) {
        let shared = &self.ivars().shared;
        let Some(out) = shared.dnd.borrow_mut().outgoing.take() else {
            return;
        };
        if out.cancelled {
            return;
        }
        let result = if operation.contains(NSDragOperation::Copy) {
            DragResult::Copied
        } else if operation
            .intersects(NSDragOperation::Move | NSDragOperation::Generic | NSDragOperation::Delete)
        {
            DragResult::Moved
        } else {
            DragResult::Cancelled
        };
        shared.push(WindowEvent::DragSourceEnded(result));
    }
}

/// A picture of an `IOSurface` of ours, which AppKit shows under the pointer in a drag.
fn image_of(surface: &IOSurfaceRef, logical: SizeI) -> Option<Retained<NSImage>> {
    let (w, h) = (surface.width(), surface.height());
    // SAFETY: AppKit makes the pixels itself, as many as the arguments say.
    let rep = unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            w as isize,
            h as isize,
            8,
            4,
            true,
            false,
            NSDeviceRGBColorSpace,
            // B, G, R, A in memory and premultiplied, as we draw.
            NSBitmapFormat::AlphaFirst | NSBitmapFormat::ThirtyTwoBitLittleEndian,
            (w * 4) as isize,
            32,
        )
    }?;
    let to = rep.bitmapData();
    if to.is_null() {
        return None;
    }
    // SAFETY: locked for reading, which waits for the GPU's writes; rows are copied within both.
    unsafe {
        if surface.lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut()) != 0 {
            return None;
        }
        let from = surface.base_address().as_ptr() as *const u8;
        let stride = surface.bytes_per_row();
        for row in 0..h {
            std::ptr::copy_nonoverlapping(from.add(row * stride), to.add(row * w * 4), w * 4);
        }
        surface.unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
    }
    let size = NSSize::new(logical.width.max(1) as f64, logical.height.max(1) as f64);
    let image = NSImage::initWithSize(NSImage::alloc(), size);
    image.addRepresentation(&rep);
    Some(image)
}

/// The pasteboard items of what is dragged: one per file, the first with the rest of the formats.
fn items_of(offer: &DragOffer) -> Vec<Retained<NSPasteboardItem>> {
    let files: Vec<String> = offer
        .items
        .iter()
        .filter(|(m, _)| m == FILES_MIME)
        .flat_map(|(_, bytes)| {
            String::from_utf8_lossy(bytes)
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .collect();
    let mut items: Vec<Retained<NSPasteboardItem>> = Vec::new();
    for url in &files {
        let item = NSPasteboardItem::new();
        item.setString_forType(&NSString::from_str(url), &ns(FILE_URL));
        items.push(item);
    }
    if items.is_empty() {
        items.push(NSPasteboardItem::new());
    }
    let first = &items[0];
    let mut done: Vec<&str> = Vec::new();
    for (mime, bytes) in &offer.items {
        let t = type_of(mime);
        if mime == FILES_MIME || done.contains(&t) {
            continue;
        }
        done.push(t);
        if t == TEXT {
            let s = String::from_utf8_lossy(bytes);
            first.setString_forType(&NSString::from_str(&s), &ns(t));
        } else {
            first.setData_forType(&NSData::with_bytes(bytes), &ns(t));
        }
    }
    items
}

impl State {
    /// Hands a drag to AppKit, from the last press or move of the mouse. Its picture is `icon`,
    /// of `place` (the logical size, and where the pointer is on it).
    pub fn begin_drag(
        &mut self,
        offer: DragOffer,
        place: Option<(SizeI, PointI)>,
        icon: Option<Sendable<CFRetained<IOSurfaceRef>>>,
    ) {
        let shared = &self.shared;
        let event = shared.last_mouse.borrow().clone();
        let Some(event) = event.filter(|_| NSEvent::pressedMouseButtons() & 1 != 0) else {
            shared.push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
            return;
        };
        let (logical, hotspot) = place.unwrap_or((SizeI::new(1, 1), PointI::new(0, 0)));
        let image = icon
            .as_ref()
            .and_then(|i| image_of(i, logical))
            .unwrap_or_else(|| NSImage::initWithSize(NSImage::alloc(), NSSize::new(1.0, 1.0)));
        let at = self
            .view
            .convertPoint_fromView(event.locationInWindow(), None);
        let frame = NSRect::new(
            NSPoint::new(at.x - hotspot.x as f64, at.y - hotspot.y as f64),
            NSSize::new(logical.width.max(1) as f64, logical.height.max(1) as f64),
        );
        let actions = offer.actions;
        let items: Vec<Retained<NSDraggingItem>> = items_of(&offer)
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let d = NSDraggingItem::initWithPasteboardWriter(
                    NSDraggingItem::alloc(),
                    ProtocolObject::from_ref(&*item),
                );
                let contents: Option<&AnyObject> = (i == 0).then_some(&image);
                unsafe { d.setDraggingFrame_contents(frame, contents) };
                d
            })
            .collect();
        shared.dnd.borrow_mut().outgoing = Some(Outgoing {
            actions,
            cancelled: false,
        });
        let _ = self.view.beginDraggingSessionWithItems_event_source(
            &NSArray::from_retained_slice(&items),
            &event,
            ProtocolObject::from_ref(&*self.view),
        );
    }

    /// The core ended a drag of ours. AppKit's goes on until the button is let go.
    pub fn cancel_drag(&mut self) {
        let mut dnd = self.shared.dnd.borrow_mut();
        let Some(out) = &mut dnd.outgoing else {
            return;
        };
        if !std::mem::replace(&mut out.cancelled, true) {
            drop(dnd);
            self.shared
                .push(WindowEvent::DragSourceEnded(DragResult::Cancelled));
        }
    }

    pub fn accept_drag(&mut self, wanted: Option<(String, DragAction)>) {
        if let Some(inc) = &mut self.shared.dnd.borrow_mut().incoming {
            inc.accepted = wanted.filter(|(m, _)| inc.mimes.contains(m));
        }
    }

    pub fn receive_drop(&mut self, mime: String) {
        let bytes = match &self.shared.dnd.borrow().incoming {
            Some(inc) if inc.dropped && inc.mimes.contains(&mime) => read(&inc.board, &mime),
            _ => Vec::new(),
        };
        self.shared.dnd.borrow_mut().incoming = None;
        self.shared.push(WindowEvent::DropData { mime, bytes });
    }

    pub fn finish_drop(&mut self) {
        self.shared.dnd.borrow_mut().incoming = None;
    }

    /// What the view takes from other programs.
    pub(crate) fn register_drags(&self) {
        let types = [TEXT, FILE_URL, "public.html", "public.png"].map(ns);
        self.view
            .registerForDraggedTypes(&NSArray::from_retained_slice(&types));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_types_and_pasteboard_types() {
        assert_eq!(type_of("text/plain;charset=utf-8"), TEXT);
        assert_eq!(type_of("text/plain"), TEXT);
        assert_eq!(type_of("text/uri-list"), FILE_URL);
        assert_eq!(type_of("application/x-thing"), "application/x-thing");
        let types = [TEXT, FILE_URL, "com.apple.thing", "application/x-thing"]
            .map(String::from)
            .to_vec();
        assert_eq!(
            mimes_of(&types),
            [
                "text/plain;charset=utf-8",
                "text/plain",
                "text/uri-list",
                "application/x-thing"
            ]
        );
    }

    #[test]
    fn operations_follow_what_the_source_allows() {
        let both = NSDragOperation::Copy | NSDragOperation::Move;
        assert_eq!(operation_of(DragAction::Move, both), NSDragOperation::Move);
        assert_eq!(operation_of(DragAction::Copy, both), NSDragOperation::Copy);
        assert_eq!(
            operation_of(DragAction::Move, NSDragOperation::Copy),
            NSDragOperation::None
        );
        assert_eq!(
            operation_of(DragAction::Move, NSDragOperation::Generic),
            NSDragOperation::Generic
        );
    }
}
