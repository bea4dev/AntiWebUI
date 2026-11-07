//! More than one window: which one a thread is working on, what each keeps, and how windows are
//! opened, spawned on threads of their own and closed (see `docs/design/36-multi-window.md`).
//!
//! What belongs to a window (the document, timers, frame callbacks, what assistive technology
//! watches, the window's own signals) lives in thread-locals for the window being worked on. The
//! other windows of the thread keep theirs here, and switching swaps them in. A thread with one
//! window never switches.

use crate::channel::{UiSender, ui_channel};
use crate::platform::PlatformRequest;
use crate::reactive::{Signal, batch, create_root, on_cleanup, signal};
use crate::settings::Settings;
use crate::tree::{Document, with_doc};
use crate::view::{IntoView, View};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;

/// A window, whichever thread it is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(u64);

impl WindowId {
    #[doc(hidden)]
    pub fn get(self) -> u64 {
        self.0
    }

    pub(crate) fn from_raw(id: u64) -> Self {
        Self(id)
    }
}

static NEXT_WINDOW: AtomicU64 = AtomicU64::new(1);

fn allocate() -> u64 {
    NEXT_WINDOW.fetch_add(1, Ordering::Relaxed)
}

/// What a window asks the system for its frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DecorationMode {
    /// The system's frame where there is one (Wayland's xdg-decoration, an X window manager), and
    /// the window's own otherwise.
    #[default]
    Auto,
    /// The system's frame. A Wayland compositor may still say no.
    Server,
    /// The window draws its own, with [`title_bar`](https://docs.rs/anti-webui-widgets) or
    /// anything else.
    Client,
}

/// Who draws the frame of the window, as it turned out.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decorations {
    /// The system: its title bar and buttons are there.
    #[default]
    Server,
    /// The window: without something of its own there is no title bar.
    Client,
}

/// An edge or a corner to resize the window from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

/// How a window opens.
#[derive(Clone, Debug)]
pub struct WindowOptions {
    pub title: String,
    /// The size in logical pixels.
    pub width: i32,
    pub height: i32,
    /// Keeps input from the window that opened it until it closes. A modal window is a dialog.
    pub modal: bool,
    /// The system is told it belongs to the window that opened it: it stays above it, and window
    /// managers frame it as a dialog, often with a close button only.
    pub dialog: bool,
    pub decorations: DecorationMode,
    /// Doesn't close with the window that opened it.
    pub top_level: bool,
    /// Opens on all of the screen.
    pub fullscreen: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowOptions {
    pub fn new() -> Self {
        Self {
            title: String::new(),
            width: 640,
            height: 480,
            modal: false,
            dialog: false,
            decorations: DecorationMode::Auto,
            top_level: false,
            fullscreen: false,
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// The size in logical pixels.
    pub fn size(mut self, width: i32, height: i32) -> Self {
        self.width = width;
        self.height = height;
        self
    }

    /// Keeps input from the window that opened it until this one closes. It is a
    /// [`dialog`](Self::dialog) too.
    pub fn modal(mut self, modal: bool) -> Self {
        self.modal = modal;
        self
    }

    /// Tells the system the window belongs to the one that opened it: it stays above it, and window
    /// managers frame it as a dialog, often with a close button only. Without it the window is like
    /// any other, with all of its buttons, as the windows a browser opens.
    pub fn dialog(mut self, dialog: bool) -> Self {
        self.dialog = dialog;
        self
    }

    pub fn decorations(mut self, mode: DecorationMode) -> Self {
        self.decorations = mode;
        self
    }

    /// Doesn't close with the window that opened it, only with the program. It can't be a
    /// [`dialog`](Self::dialog) then.
    pub fn top_level(mut self) -> Self {
        self.top_level = true;
        self
    }

    pub fn fullscreen(mut self, on: bool) -> Self {
        self.fullscreen = on;
        self
    }
}

/// What a [`WindowHandle`] asks of the window, on whichever thread it is.
#[derive(Clone, Debug, PartialEq)]
#[doc(hidden)]
pub enum Command {
    Close,
    /// Goes through [`on_close_request`] first.
    RequestClose,
    SetTitle(String),
    /// A modal window it opened came (`true`) or went (`false`).
    ModalChild(bool),
}

enum Route {
    /// The window hasn't opened yet.
    Waiting(Vec<Command>),
    Thread {
        thread: ThreadId,
        /// For other threads; `None` on a thread that never started a UI loop, like a test's.
        sender: Option<UiSender<(u64, Command)>>,
    },
}

struct Link {
    id: u64,
    open: AtomicBool,
    route: Mutex<Route>,
}

/// A window: it can be used from any thread, and outlives the window.
#[derive(Clone)]
pub struct WindowHandle(Arc<Link>);

impl std::fmt::Debug for WindowHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("WindowHandle").field(&self.0.id).finish()
    }
}

impl PartialEq for WindowHandle {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for WindowHandle {}

impl WindowHandle {
    fn waiting(id: u64) -> Self {
        Self(Arc::new(Link {
            id,
            open: AtomicBool::new(true),
            route: Mutex::new(Route::Waiting(Vec::new())),
        }))
    }

    pub fn id(&self) -> WindowId {
        WindowId(self.0.id)
    }

    /// Whether the window is still there. It may close right after.
    pub fn is_open(&self) -> bool {
        self.0.open.load(Ordering::Acquire)
    }

    /// Closes the window without asking [`on_close_request`], along with the windows it opened.
    pub fn close(&self) {
        self.send(Command::Close);
    }

    /// Closes the window as its close button does: [`on_close_request`] can keep it open.
    pub fn request_close(&self) {
        self.send(Command::RequestClose);
    }

    pub fn set_title(&self, title: impl Into<String>) {
        self.send(Command::SetTitle(title.into()));
    }

    pub(crate) fn send(&self, command: Command) {
        if !self.is_open() {
            return;
        }
        let mut route = self.0.route.lock().expect("no panics in here");
        match &mut *route {
            Route::Waiting(commands) => commands.push(command),
            Route::Thread { thread, sender } => {
                if *thread == std::thread::current().id() {
                    drop(route);
                    PENDING.with(|p| p.borrow_mut().commands.push((self.id(), command)));
                } else if let Some(sender) = sender {
                    sender.send((self.0.id, command));
                }
            }
        }
    }

    /// The window opened on this thread: commands come here from now on.
    fn attach(&self) {
        let sender = THREAD_ROUTE.with(|r| r.borrow().clone());
        let thread = std::thread::current().id();
        let mut route = self.0.route.lock().expect("no panics in here");
        let old = std::mem::replace(&mut *route, Route::Thread { thread, sender });
        drop(route);
        if let Route::Waiting(commands) = old {
            PENDING.with(|p| {
                let mut p = p.borrow_mut();
                p.commands
                    .extend(commands.into_iter().map(|c| (self.id(), c)));
            });
        }
    }
}

/// Whether the platform's close button, Alt+F4 and the like may close the window. See
/// [`on_close_request`].
#[derive(Default)]
pub struct CloseRequest {
    prevented: Cell<bool>,
}

impl CloseRequest {
    /// Keeps the window open.
    pub fn prevent(&self) {
        self.prevented.set(true);
    }

    pub fn is_prevented(&self) -> bool {
        self.prevented.get()
    }
}

type CloseHandler = Rc<dyn Fn(&CloseRequest)>;

/// What the platform says of the window, and the signals that follow it.
#[derive(Clone, Copy, Default)]
struct Shown {
    decorations: Decorations,
    maximized: bool,
    active: bool,
}

type ShownSignals = (Signal<Decorations>, Signal<bool>, Signal<bool>);

/// What each window keeps of its own, besides the document.
#[derive(Default)]
pub(crate) struct Info {
    link: Option<WindowHandle>,
    parent: Option<WindowHandle>,
    /// The windows it opened that close with it.
    children: Vec<WindowHandle>,
    modal: bool,
    /// How many modal windows it opened are open, which keep input from it.
    pub(crate) blocked: u32,
    close_handlers: Vec<(u64, CloseHandler)>,
    next_handler: u64,
    shown: Shown,
    signals: Option<ShownSignals>,
}

/// What the windows not being worked on keep.
#[derive(Default)]
struct Parked {
    doc: Document,
    animation: crate::animation::Locals,
    access: crate::access::Locals,
    view: crate::view::Locals,
    info: Info,
}

impl Parked {
    /// Trades what this holds for what the thread-locals hold.
    fn swap(&mut self) {
        crate::tree::swap_doc(&mut self.doc);
        crate::animation::swap_locals(&mut self.animation);
        crate::access::swap_locals(&mut self.access);
        crate::view::swap_locals(&mut self.view);
        INFO.with(|i| std::mem::swap(&mut *i.borrow_mut(), &mut self.info));
    }
}

/// A window to open on this thread, which the app does after the event it was asked in.
#[doc(hidden)]
pub struct OpenRequest {
    pub handle: WindowHandle,
    pub options: WindowOptions,
    pub parent: Option<WindowHandle>,
    pub(crate) root: Box<dyn FnOnce() -> View>,
}

/// A window to open on a thread of its own.
#[doc(hidden)]
pub struct SpawnRequest {
    pub handle: WindowHandle,
    pub options: WindowOptions,
    pub parent: Option<WindowHandle>,
    root: Box<dyn FnOnce() -> View + Send>,
    settings: Settings,
}

impl SpawnRequest {
    /// On the new thread: sets it up like the one that asked, and gives what to open there.
    pub fn start(self) -> OpenRequest {
        crate::settings::adopt(self.settings);
        let root = self.root;
        OpenRequest {
            handle: self.handle,
            options: self.options,
            parent: self.parent,
            root: Box::new(root),
        }
    }
}

#[derive(Default)]
struct Pending {
    opens: Vec<OpenRequest>,
    spawns: Vec<SpawnRequest>,
    commands: Vec<(WindowId, Command)>,
}

thread_local! {
    /// The window being worked on; 0 until something asks.
    static ACTIVE: Cell<u64> = const { Cell::new(0) };
    static PARKED: RefCell<HashMap<u64, Parked>> = RefCell::default();
    /// The windows that have a `Ui`.
    static OWNED: RefCell<HashSet<u64>> = RefCell::default();
    static INFO: RefCell<Info> = RefCell::default();
    static PENDING: RefCell<Pending> = RefCell::default();
    /// How other threads reach the windows of this one.
    static THREAD_ROUTE: RefCell<Option<UiSender<(u64, Command)>>> = const { RefCell::new(None) };
}

/// The window being worked on.
pub(crate) fn active_id() -> u64 {
    ACTIVE.with(|a| {
        if a.get() == 0 {
            a.set(allocate());
        }
        a.get()
    })
}

/// Makes `id` the window being worked on. 0 leaves it as it is. Never while one of the window's
/// thread-locals is borrowed.
pub(crate) fn activate(id: u64) {
    let current = ACTIVE.with(Cell::get);
    if id == 0 || id == current {
        return;
    }
    let mut parked = PARKED
        .with(|p| p.borrow_mut().remove(&id))
        .unwrap_or_default();
    parked.swap();
    // What a closed window left is let go; see `forget_active`.
    if current != 0 {
        PARKED.with(|p| p.borrow_mut().insert(current, parked));
    }
    ACTIVE.with(|a| a.set(id));
}

/// Switches to a window until dropped.
pub(crate) struct Enter(u64);

impl Enter {
    pub(crate) fn new(id: u64) -> Self {
        let previous = ACTIVE.with(Cell::get);
        activate(id);
        Self(previous)
    }
}

impl Drop for Enter {
    fn drop(&mut self) {
        activate(self.0);
    }
}

/// Runs `f` in window `id`, and comes back.
pub(crate) fn within<R>(id: u64, f: impl FnOnce() -> R) -> R {
    let _enter = Enter::new(id);
    f()
}

/// Runs `f` in a window of this thread, for tests and for what drives a `Ui` from outside.
#[doc(hidden)]
pub fn in_window<R>(window: WindowId, f: impl FnOnce() -> R) -> R {
    within(window.0, f)
}

/// What a new `Ui` works in: the window being worked on if no other `Ui` has it, which is the only
/// window of a program with one, otherwise a new one.
pub(crate) fn claim_for_ui() -> u64 {
    let id = active_id();
    if OWNED.with(|o| o.borrow().contains(&id)) {
        let id = allocate();
        activate(id);
        OWNED.with(|o| o.borrow_mut().insert(id));
        id
    } else {
        OWNED.with(|o| o.borrow_mut().insert(id));
        id
    }
}

/// A `Ui` for a window from [`open_window`] or [`spawn_window`]: always a new one.
pub(crate) fn claim_new(handle: &WindowHandle, parent: Option<WindowHandle>, modal: bool) -> u64 {
    let id = handle.0.id;
    activate(id);
    OWNED.with(|o| o.borrow_mut().insert(id));
    handle.attach();
    INFO.with(|i| {
        let mut i = i.borrow_mut();
        i.link = Some(handle.clone());
        i.modal = modal;
        i.parent = parent.clone();
    });
    if modal && let Some(parent) = parent {
        parent.send(Command::ModalChild(true));
    }
    id
}

/// The `Ui` of the window is dropped without closing it: another may take the window.
pub(crate) fn release(id: u64) {
    OWNED.with(|o| o.borrow_mut().remove(&id));
}

/// After a window closed: what it kept goes, and nothing is being worked on. Its owner must have
/// been disposed already.
pub(crate) fn close_active() {
    let id = ACTIVE.with(Cell::get);
    let info = INFO.with(|i| std::mem::take(&mut *i.borrow_mut()));
    if let Some(link) = &info.link {
        link.0.open.store(false, Ordering::Release);
    }
    for child in &info.children {
        child.close();
    }
    if info.modal
        && let Some(parent) = &info.parent
    {
        parent.send(Command::ModalChild(false));
    }
    let mut fresh = Parked::default();
    fresh.swap();
    drop(fresh);
    OWNED.with(|o| o.borrow_mut().remove(&id));
    ACTIVE.with(|a| a.set(0));
}

/// Gives `Ui` what was asked of its window through a handle that is about the window itself.
pub(crate) fn blocked() -> bool {
    INFO.with(|i| i.borrow().blocked > 0)
}

pub(crate) fn modal_child(opened: bool) {
    INFO.with(|i| {
        let mut i = i.borrow_mut();
        i.blocked = if opened {
            i.blocked + 1
        } else {
            i.blocked.saturating_sub(1)
        };
    });
}

/// Runs the handlers of [`on_close_request`]; whether none kept the window open.
pub(crate) fn may_close() -> bool {
    let handlers: Vec<CloseHandler> = INFO.with(|i| {
        i.borrow()
            .close_handlers
            .iter()
            .map(|(_, h)| h.clone())
            .collect()
    });
    let request = CloseRequest::default();
    batch(|| {
        for handler in handlers {
            handler(&request);
        }
    });
    !request.is_prevented()
}

fn shown_signals() -> ShownSignals {
    if let Some(s) = INFO.with(|i| i.borrow().signals) {
        return s;
    }
    let shown = INFO.with(|i| i.borrow().shown);
    // Under a root of its own: it outlives whatever asks for it first.
    let (signals, _) = create_root(|| {
        (
            signal(shown.decorations),
            signal(shown.maximized),
            signal(shown.active),
        )
    });
    INFO.with(|i| i.borrow_mut().signals = Some(signals));
    signals
}

/// The platform told something of the window. Not while the document is borrowed.
pub(crate) fn tell_decorations(decorations: Decorations) {
    INFO.with(|i| i.borrow_mut().shown.decorations = decorations);
    if let Some((s, _, _)) = INFO.with(|i| i.borrow().signals) {
        s.set_if_changed(decorations);
    }
}

pub(crate) fn tell_maximized(on: bool) {
    INFO.with(|i| i.borrow_mut().shown.maximized = on);
    if let Some((_, s, _)) = INFO.with(|i| i.borrow().signals) {
        s.set_if_changed(on);
    }
}

pub(crate) fn tell_active(on: bool) {
    INFO.with(|i| i.borrow_mut().shown.active = on);
    if let Some((_, _, s)) = INFO.with(|i| i.borrow().signals) {
        s.set_if_changed(on);
    }
}

// --- What the app takes ------------------------------------------------------------------------

/// Starts the UI loop of a thread: other threads can reach its windows, and it hears of settings
/// changed elsewhere. `spawned` is for a thread from [`spawn_window`], which also gets the fonts
/// registered so far.
#[doc(hidden)]
pub fn start_ui_thread(spawned: bool) {
    let sender = ui_channel(|(id, command): (u64, Command)| {
        PENDING.with(|p| p.borrow_mut().commands.push((WindowId(id), command)));
    });
    THREAD_ROUTE.with(|r| *r.borrow_mut() = Some(sender));
    crate::settings::join_broadcast();
    if spawned {
        crate::text::register_shared_fonts();
    }
}

/// Windows asked for with [`open_window`] since the last time.
#[doc(hidden)]
pub fn take_opens() -> Vec<OpenRequest> {
    PENDING.with(|p| std::mem::take(&mut p.borrow_mut().opens))
}

/// Windows asked for with [`spawn_window`] since the last time.
#[doc(hidden)]
pub fn take_spawns() -> Vec<SpawnRequest> {
    PENDING.with(|p| std::mem::take(&mut p.borrow_mut().spawns))
}

/// What handles asked of the windows of this thread since the last time.
#[doc(hidden)]
pub fn take_commands() -> Vec<(WindowId, Command)> {
    PENDING.with(|p| std::mem::take(&mut p.borrow_mut().commands))
}

/// Whether something is waiting for [`take_opens`], [`take_spawns`] or [`take_commands`].
#[doc(hidden)]
pub fn has_pending() -> bool {
    PENDING.with(|p| {
        let p = p.borrow();
        !p.opens.is_empty() || !p.spawns.is_empty() || !p.commands.is_empty()
    })
}

// --- What programs use -------------------------------------------------------------------------

/// Opens a window on this thread. `root` can use the signals of the window that opens it. Unless
/// [`WindowOptions::top_level`], the window closes with the one that opened it; with
/// [`WindowOptions::dialog`] it also stays above it. Cleanups in `root` run when it closes.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// let count = signal(0);
/// open_window(WindowOptions::new().title("Count"), move || {
///     text(move || count.get().to_string())
/// });
/// ```
pub fn open_window<V: IntoView>(
    options: WindowOptions,
    root: impl FnOnce() -> V + 'static,
) -> WindowHandle {
    let handle = WindowHandle::waiting(allocate());
    let parent = (!options.top_level).then(current_window);
    if parent.is_some() {
        INFO.with(|i| i.borrow_mut().children.push(handle.clone()));
    }
    PENDING.with(|p| {
        p.borrow_mut().opens.push(OpenRequest {
            handle: handle.clone(),
            options,
            parent,
            root: Box::new(move || root().into_view()),
        })
    });
    handle
}

/// Opens a window on a thread of its own, so that what it does never holds up the others. It
/// can't share signals: use [`shared_signal`](crate::shared::shared_signal) or
/// [`ui_channel`](crate::channel::ui_channel). Settings and registered fonts go along.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// let progress = shared_signal(0.0f32);
/// let shown = progress.clone();
/// spawn_window(WindowOptions::new().title("Worker"), move || {
///     text(move || format!("{:.0}%", shown.get() * 100.0))
/// });
/// ```
pub fn spawn_window<V: IntoView>(
    options: WindowOptions,
    root: impl FnOnce() -> V + Send + 'static,
) -> WindowHandle {
    let handle = WindowHandle::waiting(allocate());
    let parent = (!options.top_level).then(current_window);
    if parent.is_some() {
        INFO.with(|i| i.borrow_mut().children.push(handle.clone()));
    }
    PENDING.with(|p| {
        p.borrow_mut().spawns.push(SpawnRequest {
            handle: handle.clone(),
            options,
            parent,
            root: Box::new(move || root().into_view()),
            settings: crate::settings::settings_untracked(),
        })
    });
    handle
}

/// The window this runs in.
pub fn current_window() -> WindowHandle {
    if let Some(link) = INFO.with(|i| i.borrow().link.clone()) {
        return link;
    }
    // A window the app didn't open with a handle, like the first one, or a test's.
    let handle = WindowHandle::waiting(active_id());
    handle.attach();
    INFO.with(|i| i.borrow_mut().link = Some(handle.clone()));
    handle
}

/// Closes the window without asking [`on_close_request`], along with the windows it opened. The
/// program ends with its first window.
pub fn close_window() {
    with_doc(|d| d.request(PlatformRequest::Close));
}

/// Closes the window as its close button does: [`on_close_request`] can keep it open.
pub fn request_close_window() {
    if blocked() {
        return;
    }
    if may_close() {
        close_window();
    }
}

pub fn set_window_title(title: impl Into<String>) {
    let title = title.into();
    with_doc(|d| d.request(PlatformRequest::SetTitle(title)));
}

/// Runs `f` when something asks to close the window: its close button, whether the system's or a
/// [`title_bar`](https://docs.rs/anti-webui-widgets)'s, Alt+F4, the window manager, or
/// [`request_close_window`]. [`CloseRequest::prevent`] keeps it open. Stops with the owner it was
/// set up in.
///
/// ```no_run
/// use anti_webui_core::prelude::*;
///
/// let unsaved = signal(true);
/// on_close_request(move |request| {
///     if unsaved.get_untracked() {
///         request.prevent();
///     }
/// });
/// ```
pub fn on_close_request(f: impl Fn(&CloseRequest) + 'static) {
    let id = INFO.with(|i| {
        let mut i = i.borrow_mut();
        i.next_handler += 1;
        let id = i.next_handler;
        i.close_handlers.push((id, Rc::new(f)));
        id
    });
    on_cleanup(move || {
        INFO.with(|i| {
            i.borrow_mut()
                .close_handlers
                .retain(|(other, _)| *other != id)
        });
    });
}

/// Who draws the frame of the window. Reading it tracks it.
pub fn window_decorations() -> Decorations {
    shown_signals().0.get()
}

/// Whether the window fills the screen but for panels. Reading it tracks it.
pub fn window_maximized() -> bool {
    shown_signals().1.get()
}

/// Whether the window has the keyboard. Reading it tracks it.
pub fn window_active() -> bool {
    shown_signals().2.get()
}

/// Lets the system move the window with the pointer. Call while a button is down, as in
/// `on_pointer_down`.
pub fn start_window_move() {
    with_doc(|d| d.request(PlatformRequest::StartMove));
}

/// Lets the system resize the window with the pointer from `edge`. Call while a button is down.
pub fn start_window_resize(edge: ResizeEdge) {
    with_doc(|d| d.request(PlatformRequest::StartResize(edge)));
}

pub fn minimize_window() {
    with_doc(|d| d.request(PlatformRequest::Minimize));
}

/// [`window_maximized`] says when it has happened.
pub fn set_window_maximized(on: bool) {
    with_doc(|d| d.request(PlatformRequest::SetMaximized(on)));
}

pub fn toggle_window_maximized() {
    let on = INFO.with(|i| i.borrow().shown.maximized);
    set_window_maximized(!on);
}

/// Shows the system's menu for the window where the pointer is, if it has one.
pub fn show_window_menu() {
    with_doc(|d| d.request(PlatformRequest::ShowWindowMenu));
}
