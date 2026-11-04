//! Pictures: what they are decoded into, and keeping them while they are of use. See
//! `docs/design/22-images.md`.

mod decode;
mod worker;

use crate::settings::with_settings;
use crate::tree::{Document, NodeId, NodeKind};
use anti_webui_style::ImageSource;
use decode::planned_size;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use worker::{Done, Job, Outcome, Reply, Task, submit};

/// How much to hand to the GPU in a frame: more waits for the next one, so that many pictures
/// arriving at once don't stop the UI.
const BYTES_PER_FRAME: usize = 64 << 20;
/// How many sizes of a picture drawn from a description to keep, when nothing shows them.
const VECTOR_SIZES: usize = 8;

/// How big a picture is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Info {
    /// In pixels, as it shows: EXIF has been taken into account.
    pub pixels: (u32, u32),
    /// In logical pixels.
    pub natural: (f32, f32),
    /// Drawn from a description, at any size.
    pub vector: bool,
}

/// What a decoder makes.
pub(crate) struct Decoded {
    pub size: (u32, u32),
    pub opaque: bool,
    /// Each half the size of the last, down to a pixel. Premultiplied. Empty for animations.
    pub levels: Vec<Vec<u8>>,
    /// The frames of an animation, and how long each shows.
    pub frames: Vec<(Vec<u8>, Duration)>,
    /// How many times an animation goes through, or `None` for forever.
    pub loops: Option<u32>,
}

/// Pixels at some size, for the renderer. A picture drawn smaller than this reads the smaller
/// levels. It is shared with the scenes that show it, which is what keeps it from being thrown away.
pub struct Bitmap {
    id: u64,
    width: u32,
    height: u32,
    opaque: bool,
    bytes: usize,
    /// Let go of once the GPU has them.
    levels: Mutex<Option<Vec<Vec<u8>>>>,
    frames: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bitmap({}: {}x{})", self.id, self.width, self.height)
    }
}

static NEXT_BITMAP: AtomicU64 = AtomicU64::new(1);

impl Bitmap {
    fn new(d: Decoded) -> (Self, Vec<Duration>, Option<u32>) {
        let delays = d.frames.iter().map(|f| f.1).collect();
        let frames: Vec<Vec<u8>> = d.frames.into_iter().map(|f| f.0).collect();
        let bytes = d.levels.iter().chain(&frames).map(Vec::len).sum();
        let bitmap = Self {
            id: NEXT_BITMAP.fetch_add(1, Ordering::Relaxed),
            width: d.size.0,
            height: d.size.1,
            opaque: d.opaque,
            bytes,
            levels: Mutex::new(Some(d.levels)),
            frames,
        };
        (bitmap, delays, d.loops)
    }

    /// Identifies the pixels.
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// Every pixel is opaque.
    pub fn is_opaque(&self) -> bool {
        self.opaque
    }

    /// How many levels the texture needs.
    pub fn level_count(&self) -> u32 {
        if self.frames.is_empty() {
            32 - self.width.max(self.height).leading_zeros()
        } else {
            1
        }
    }

    /// A frame of an animation, or `None` for a still.
    pub fn frame(&self, frame: u32) -> Option<&[u8]> {
        if self.frames.is_empty() {
            return None;
        }
        Some(&self.frames[frame as usize % self.frames.len()])
    }

    /// Calls `f` with each level of a still and its size, largest first. Returns false if they
    /// have been let go of.
    pub fn with_levels(&self, mut f: impl FnMut(u32, (u32, u32), &[u8])) -> bool {
        let levels = self.levels.lock().expect("no panics in here");
        let Some(levels) = levels.as_ref() else {
            return false;
        };
        for (i, level) in levels.iter().enumerate() {
            let i = i as u32;
            f(
                i,
                ((self.width >> i).max(1), (self.height >> i).max(1)),
                level,
            );
        }
        true
    }

    /// Lets go of the levels of a still: nothing else in this process is going to need them.
    pub fn release_levels(&self) {
        *self.levels.lock().expect("no panics in here") = None;
    }
}

/// A picture to show in a frame.
pub(crate) struct Showing {
    pub bitmap: Arc<Bitmap>,
    pub frame: u32,
}

struct Playback {
    delays: Vec<Duration>,
    frame: u32,
    next: Instant,
    plays_left: Option<u32>,
    /// Nothing that shows it is in view.
    paused: bool,
}

#[derive(Default)]
struct Entry {
    info: Option<Info>,
    failed: Option<String>,
    probing: bool,
    /// What has been decoded: a size of a vector picture each, or one otherwise.
    held: Vec<(Arc<Bitmap>, u64)>,
    /// What is being decoded, and how to call it off.
    flight: Option<((u32, u32), Arc<AtomicBool>)>,
    /// The size to decode at when the flight is over.
    next: Option<(u32, u32)>,
    /// The `img` nodes it belongs to.
    nodes: Vec<NodeId>,
    /// The nodes that have painted it.
    painters: HashSet<NodeId>,
    playback: Option<Playback>,
}

impl Entry {
    fn best(&self) -> Option<&Arc<Bitmap>> {
        self.held
            .iter()
            .map(|h| &h.0)
            .max_by_key(|b| b.width * b.height)
    }
}

/// Pictures in use and those kept in case they are of use again. It lives in the document and is
/// only touched on the UI thread.
pub struct ImageStore {
    entries: HashMap<ImageSource, Entry>,
    reply: Arc<Reply>,
    /// Counts frames, to tell what was used last.
    clock: u64,
    /// What everything in `held` takes.
    bytes: usize,
    /// Animations, of which there are usually few.
    animated: Vec<ImageSource>,
    /// Decoded nodes to tell about it. Told outside of painting.
    decoded_nodes: Vec<NodeId>,
    loaded_nodes: Vec<NodeId>,
    in_flight: usize,
}

impl Default for ImageStore {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            reply: Arc::new(Reply::default()),
            clock: 0,
            bytes: 0,
            animated: Vec::new(),
            decoded_nodes: Vec::new(),
            loaded_nodes: Vec::new(),
            in_flight: 0,
        }
    }
}

impl ImageStore {
    fn start(&mut self, source: &ImageSource, task: Task, cancelled: Arc<AtomicBool>) {
        self.in_flight += 1;
        submit(Job {
            source: source.clone(),
            task,
            cancelled,
            reply: self.reply.clone(),
        });
    }

    fn probed(&mut self, source: &ImageSource) -> &mut Entry {
        if !self.entries.contains_key(source) {
            self.start(source, Task::Probe, Arc::default());
            self.entries.insert(
                source.clone(),
                Entry {
                    probing: true,
                    ..Entry::default()
                },
            );
        }
        self.entries.get_mut(source).expect("just made sure")
    }

    /// How big the picture of a background is, as far as that is known. Starts finding out, and
    /// `node` will paint again when it is.
    pub(crate) fn info(&mut self, node: NodeId, source: &ImageSource) -> Option<Info> {
        let entry = self.probed(source);
        entry.painters.insert(node);
        entry.info
    }

    /// `node` is an `img` that shows this now. Returns its size if that is known already.
    pub(crate) fn acquire(&mut self, node: NodeId, source: &ImageSource) -> Option<Info> {
        let entry = self.probed(source);
        entry.nodes.push(node);
        entry.info
    }

    /// `node` no longer shows this.
    pub(crate) fn release(&mut self, node: NodeId, source: &ImageSource) {
        let Some(entry) = self.entries.get_mut(source) else {
            return;
        };
        entry.nodes.retain(|n| *n != node);
        entry.painters.remove(&node);
        if !entry.nodes.is_empty() || !entry.painters.is_empty() {
            return;
        }
        if let Some((_, cancelled)) = entry.flight.take() {
            cancelled.store(true, Ordering::Relaxed);
            // It won't say so if it hasn't started, and its answer is of no use if it has.
            self.in_flight -= 1;
        }
        entry.next = None;
        if entry.held.is_empty() && !entry.probing {
            self.entries.remove(source);
            self.animated.retain(|s| s != source);
        }
    }

    /// The most detailed pixels there are of a picture.
    pub(crate) fn best(&self, source: &ImageSource) -> Option<Arc<Bitmap>> {
        self.entries.get(source)?.best().cloned()
    }

    /// `node` paints this at `wanted` physical pixels. Returns what there is to show now, and
    /// starts decoding if that isn't good enough.
    pub(crate) fn want(
        &mut self,
        node: NodeId,
        source: &ImageSource,
        wanted: (u32, u32),
        pixelated: bool,
    ) -> Option<Showing> {
        let clock = self.clock;
        let entry = self.probed(source);
        entry.painters.insert(node);
        let info = entry.info?;
        let target = planned_size(&info, wanted, pixelated);
        let good = |b: &Bitmap| {
            if info.vector {
                (b.width, b.height) == target
            } else {
                b.width >= target.0 && b.height >= target.1
            }
        };
        let found = entry.held.iter().position(|h| good(&h.0));
        if let Some(i) = found {
            entry.held[i].1 = clock;
        } else if entry.failed.is_none() {
            let start = match &entry.flight {
                None => true,
                Some((flying, _)) => {
                    if *flying != target
                        && (info.vector || flying.0 < target.0 || flying.1 < target.1)
                    {
                        entry.next = Some(target);
                    }
                    false
                }
            };
            if start {
                let cancelled = Arc::new(AtomicBool::new(false));
                entry.flight = Some((target, cancelled.clone()));
                self.start(source, Task::Decode { target }, cancelled);
            }
        }
        let entry = self.entries.get_mut(source).expect("it was there");
        let bitmap = match found {
            Some(i) => entry.held[i].0.clone(),
            None => entry.best()?.clone(),
        };
        Some(Showing {
            frame: entry.playback.as_ref().map_or(0, |p| p.frame),
            bitmap,
        })
    }

    /// How many pictures it knows, how many sets of pixels it has, and how many bytes they take, for
    /// tests.
    #[doc(hidden)]
    pub fn counts(&self) -> (usize, usize, usize) {
        (
            self.entries.len(),
            self.entries.values().map(|e| e.held.len()).sum(),
            self.bytes,
        )
    }

    pub(crate) fn tell_decoded(&mut self, node: NodeId) {
        self.decoded_nodes.push(node);
    }

    /// Whether something is yet to arrive.
    pub(crate) fn busy(&self) -> bool {
        self.in_flight > 0
    }

    /// Whether anyone is waiting to be told something.
    pub(crate) fn has_news(&self) -> bool {
        !self.decoded_nodes.is_empty() || !self.loaded_nodes.is_empty()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub(crate) fn set_waker(&self, waker: Arc<dyn Fn() + Send + Sync>) {
        self.reply.set_waker(waker);
    }

    pub(crate) fn wait(&self, timeout: Duration) {
        self.reply.wait(timeout);
    }

    pub(crate) fn next_frame(&self) -> Option<Instant> {
        self.animated
            .iter()
            .filter_map(|s| self.entries.get(s)?.playback.as_ref())
            .filter(|p| !p.paused && p.plays_left != Some(0))
            .map(|p| p.next)
            .min()
    }

    /// A frame has been painted: throws away what has gone over the budget.
    pub(crate) fn end_frame(&mut self) {
        self.clock += 1;
        let budget = with_settings(|s| s.images.cache_bytes);
        if self.bytes <= budget {
            return;
        }
        // In the cache and nowhere else: not in any scene, so nothing shows it.
        let mut idle: Vec<(u64, ImageSource, u64, usize)> = self
            .entries
            .iter()
            .flat_map(|(source, entry)| {
                entry
                    .held
                    .iter()
                    .filter(|h| Arc::strong_count(&h.0) == 1)
                    .map(|h| (h.1, source.clone(), h.0.id, h.0.bytes))
            })
            .collect();
        idle.sort_by_key(|i| i.0);
        for (_, source, id, bytes) in idle {
            if self.bytes <= budget {
                break;
            }
            self.bytes -= bytes;
            let entry = self.entries.get_mut(&source).expect("it was there");
            entry.held.retain(|h| h.0.id != id);
            if entry.held.is_empty() {
                entry.playback = None;
                self.animated.retain(|s| *s != source);
            }
            if entry.held.is_empty() && entry.nodes.is_empty() && entry.flight.is_none() {
                self.entries.remove(&source);
            }
        }
    }

    /// A new size has been decoded: one size is enough of what can be scaled down, and what is
    /// drawn from a description keeps the sizes that were used last.
    fn take_held_over(&mut self, source: &ImageSource, keep: u64) {
        let Some(entry) = self.entries.get_mut(source) else {
            return;
        };
        let vector = entry.info.is_some_and(|i| i.vector);
        entry.held.sort_by_key(|h| std::cmp::Reverse(h.1));
        let mut kept = 0;
        let bytes = &mut self.bytes;
        entry.held.retain(|h| {
            let keeps = h.0.id == keep
                || Arc::strong_count(&h.0) > 1 && vector
                || (vector && kept < VECTOR_SIZES);
            kept += usize::from(keeps);
            if !keeps {
                *bytes -= h.0.bytes;
            }
            keeps
        });
    }
}

/// What an `img` is told about, outside of any borrow of the document.
pub(crate) enum Tell {
    Load(Rc<dyn Fn(f32, f32)>, f32, f32),
    Decoded(Rc<dyn Fn()>),
    Error(Rc<dyn Fn(&str)>, String),
}

impl Tell {
    pub(crate) fn run(self) {
        match self {
            Tell::Load(f, w, h) => f(w, h),
            Tell::Decoded(f) => f(),
            Tell::Error(f, message) => f(&message),
        }
    }
}

/// Takes in what has arrived, as much as should go to the GPU in one frame.
pub(crate) fn poll(doc: &mut Document) -> Vec<Tell> {
    let mut tells = Vec::new();
    let mut arrived = doc.images.get_mut().reply.take();
    let mut sent = 0;
    let mut held_back = Vec::new();
    for done in arrived.drain(..) {
        let Done {
            source,
            target,
            outcome,
        } = done;
        if let Outcome::Decoded(d) = &outcome {
            let size = d.levels.iter().map(Vec::len).sum::<usize>()
                + d.frames.first().map_or(0, |f| f.0.len());
            if sent > 0 && sent + size > BYTES_PER_FRAME {
                held_back.push(Done {
                    source,
                    target,
                    outcome,
                });
                continue;
            }
            sent += size;
        }
        take_in(doc, source, target, outcome, &mut tells);
    }
    let store = doc.images.get_mut();
    if !held_back.is_empty() {
        store.reply.put_back(held_back);
        doc.dirty.paint = true;
    }
    tell_nodes(doc, &mut tells);
    tells
}

fn take_in(
    doc: &mut Document,
    source: ImageSource,
    target: Option<(u32, u32)>,
    outcome: Outcome,
    tells: &mut Vec<Tell>,
) {
    let store = doc.images.get_mut();
    let Some(entry) = store.entries.get_mut(&source) else {
        if target.is_none() {
            store.in_flight -= 1;
        }
        return;
    };
    // What arrives after it was called off has been counted already.
    let is_current = match target {
        None => true,
        Some(t) => entry.flight.as_ref().is_some_and(|f| f.0 == t),
    };
    if !is_current {
        return;
    }
    store.in_flight -= 1;
    match target {
        None => entry.probing = false,
        Some(_) => entry.flight = None,
    }
    let mut touched: Vec<NodeId> = entry.painters.iter().copied().collect();
    touched.extend(entry.nodes.iter().copied());
    match outcome {
        Outcome::Info(info) => {
            entry.info = Some(info);
            let nodes = entry.nodes.clone();
            for id in nodes {
                if let Some(n) = doc.nodes.get_mut(id)
                    && let NodeKind::Image(data) = &mut n.kind
                    && data.source.as_ref() == Some(&source)
                {
                    data.natural = Some(info.natural);
                    if let Some(f) = &data.on_load {
                        let d = data.density;
                        tells.push(Tell::Load(
                            f.clone(),
                            info.natural.0 / d,
                            info.natural.1 / d,
                        ));
                    }
                    doc.invalidate_layout(id);
                }
            }
        }
        Outcome::Failed(message) => {
            entry.failed = Some(message.clone());
            let nodes = entry.nodes.clone();
            for id in nodes {
                if let Some(n) = doc.nodes.get_mut(id)
                    && let NodeKind::Image(data) = &mut n.kind
                    && data.source.as_ref() == Some(&source)
                {
                    data.fallback = None;
                    let had_size = data.natural.take().is_some();
                    if let Some(f) = &data.on_error {
                        tells.push(Tell::Error(f.clone(), message.clone()));
                    }
                    if had_size {
                        doc.invalidate_layout(id);
                    }
                }
            }
        }
        Outcome::Decoded(decoded) => {
            let (bitmap, delays, loops) = Bitmap::new(decoded);
            let bitmap = Arc::new(bitmap);
            let id = bitmap.id;
            if !delays.is_empty() {
                entry.playback = Some(Playback {
                    next: Instant::now() + delays[0],
                    delays,
                    frame: 0,
                    plays_left: loops,
                    paused: false,
                });
                if !store.animated.contains(&source) {
                    store.animated.push(source.clone());
                }
            }
            let entry = store.entries.get_mut(&source).expect("it was there");
            store.bytes += bitmap.bytes;
            entry.held.push((bitmap, store.clock));
            let nodes = entry.nodes.clone();
            store.take_held_over(&source, id);
            // What was shown until this came can go.
            for node in nodes {
                if let Some(NodeKind::Image(data)) = doc.nodes.get_mut(node).map(|n| &mut n.kind) {
                    data.fallback = None;
                }
            }
        }
    }
    let store = doc.images.get_mut();
    let entry = store.entries.get_mut(&source).expect("it was there");
    // Someone has asked for another size in the meantime.
    if let Some(next) = entry.next.take()
        && entry.flight.is_none()
    {
        let cancelled = Arc::new(AtomicBool::new(false));
        entry.flight = Some((next, cancelled.clone()));
        store.start(&source, Task::Decode { target: next }, cancelled);
    }
    for id in touched {
        doc.mark_shader_of(id);
    }
}

fn tell_nodes(doc: &mut Document, tells: &mut Vec<Tell>) {
    let store = doc.images.get_mut();
    let decoded = std::mem::take(&mut store.decoded_nodes);
    let loaded = std::mem::take(&mut store.loaded_nodes);
    for id in decoded {
        if let Some(n) = doc.nodes.get(id)
            && let NodeKind::Image(data) = &n.kind
            && let Some(f) = &data.on_decoded
        {
            tells.push(Tell::Decoded(f.clone()));
        }
    }
    for id in loaded {
        if let Some(n) = doc.nodes.get(id)
            && let NodeKind::Image(data) = &n.kind
            && let (Some(f), Some(natural)) = (&data.on_load, data.natural)
        {
            let d = data.density;
            tells.push(Tell::Load(f.clone(), natural.0 / d, natural.1 / d));
        }
    }
}

/// Moves animations on to what shows at `now`, those that are in view. Those coming into view go on
/// from where they were. Without `step`, only finds out which are in view.
pub(crate) fn advance(doc: &mut Document, now: Instant, step: bool) {
    let animated = doc.images.get_mut().animated.clone();
    let viewport = crate::geometry::RectI::new(0, 0, doc.viewport.width, doc.viewport.height);
    for source in animated {
        let Some(entry) = doc.images.get_mut().entries.get_mut(&source) else {
            continue;
        };
        let painters: Vec<NodeId> = entry.painters.iter().copied().collect();
        // A node that took another picture in the meantime has no part in this any more.
        let painters: Vec<NodeId> = painters
            .into_iter()
            .filter(|&id| {
                doc.nodes.get(id).is_some_and(|n| {
                    n.image()
                        .is_some_and(|d| d.source.as_ref() == Some(&source))
                        || n.computed.background_image.as_ref() == Some(&source)
                })
            })
            .collect();
        let shown = painters.iter().any(|&id| {
            doc.nodes.get(id).is_some_and(|n| n.visible)
                && doc
                    .visible_window_rect(id)
                    .is_some_and(|r| r.intersects(&viewport))
        });
        let entry = doc
            .images
            .get_mut()
            .entries
            .get_mut(&source)
            .expect("just found");
        entry.painters = painters.iter().copied().collect();
        let Some(p) = &mut entry.playback else {
            continue;
        };
        if !shown {
            p.paused = true;
            continue;
        }
        if std::mem::take(&mut p.paused) {
            p.next = now + p.delays[p.frame as usize];
            continue;
        }
        if !step || now < p.next || p.plays_left == Some(0) {
            continue;
        }
        let last = p.frame as usize + 1 == p.delays.len();
        if last {
            if let Some(left) = &mut p.plays_left {
                *left -= 1;
                if *left == 0 {
                    continue;
                }
            }
            p.frame = 0;
        } else {
            p.frame += 1;
        }
        p.next = now + p.delays[p.frame as usize];
        for id in painters {
            doc.mark_shader_of(id);
        }
    }
}

/// Queues telling a node that its picture is known already.
pub(crate) fn tell_loaded(store: &mut ImageStore, node: NodeId) {
    store.loaded_nodes.push(node);
}
