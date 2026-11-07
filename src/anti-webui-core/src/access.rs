//! What assistive technology, like a screen reader, needs: what each node is, what it is called
//! and what state it is in, the tree of that, when it changes, and a way to act on it. AntiWebUI
//! doesn't talk to the platform's accessibility service itself: a bridge (to AT-SPI through
//! AccessKit, say) reads [`snapshot`], follows [`watch`] and calls [`perform`], so that only the
//! programs that want it pay for it. See `docs/design/34-accessibility.md`.

use crate::geometry::RectF;
use crate::tree::{Document, NodeId, NodeKind, with_doc};
use slotmap::{Key, KeyData};
use std::cell::RefCell;
use std::rc::Rc;

/// What a node is, in the terms of ARIA and of the platforms' accessibility services.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Role {
    /// Nothing in particular: what holds other things.
    #[default]
    Generic,
    /// A group of related controls.
    Group,
    /// Text that is only read.
    Text,
    Heading,
    Paragraph,
    Link,
    Image,
    Button,
    /// A button that stays pressed: it has [`Checked`].
    ToggleButton,
    Checkbox,
    Radio,
    RadioGroup,
    Switch,
    Slider,
    ProgressBar,
    TextInput,
    MultilineTextInput,
    /// What shows a value and opens a list to choose it from: a pull-down list.
    ComboBox,
    ListBox,
    /// An option of a list box or of a combo box.
    ListOption,
    Menu,
    MenuItem,
    /// A menu item that is on or off: it has [`Checked`].
    MenuItemCheckbox,
    TabList,
    Tab,
    TabPanel,
    Tree,
    TreeItem,
    Table,
    Row,
    Cell,
    ColumnHeader,
    Dialog,
    AlertDialog,
    Tooltip,
    /// A message that is told when it comes, without the focus moving: a toast.
    Status,
    Toolbar,
    Separator,
    Navigation,
}

impl Role {
    /// Whether what it is called comes from the text in it when it isn't given (ARIA's "name from
    /// content").
    pub fn named_from_content(self) -> bool {
        matches!(
            self,
            Role::Button
                | Role::ToggleButton
                | Role::Checkbox
                | Role::Radio
                | Role::Switch
                | Role::Heading
                | Role::Link
                | Role::ListOption
                | Role::MenuItem
                | Role::MenuItemCheckbox
                | Role::Tab
                | Role::TreeItem
                | Role::Cell
                | Role::ColumnHeader
                | Role::Tooltip
        )
    }
}

/// Whether something that can be checked is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Checked {
    False,
    True,
    /// Partly: some of what it stands for is.
    Mixed,
}

impl From<bool> for Checked {
    fn from(on: bool) -> Self {
        if on { Checked::True } else { Checked::False }
    }
}

/// How a change to what a node says is told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Live {
    #[default]
    Off,
    /// When what is being said is done.
    Polite,
    /// At once.
    Assertive,
}

/// A value in a range, as of a slider or a progress bar.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AccessValue {
    pub now: f64,
    pub min: f64,
    pub max: f64,
    /// How far one step goes, if it goes in steps.
    pub step: Option<f64>,
}

/// What a node says about itself. Built with `.access(|a| a.role(..).label(..))` on the node; what
/// is not said is worked out, like the label of a button from its text, being disabled or having
/// the focus.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Access {
    pub role: Option<Role>,
    pub label: Option<String>,
    pub description: Option<String>,
    pub checked: Option<Checked>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub value: Option<AccessValue>,
    /// What the value is, said in words, like "40%" or the chosen option of a pull-down list.
    pub value_text: Option<String>,
    /// The level of a heading or of a row of a tree, from 1.
    pub level: Option<u32>,
    pub live: Option<Live>,
    pub multiselectable: bool,
    /// Left out, with all that is in it, as what is only there to look at.
    pub hidden: bool,
}

impl Access {
    pub fn role(mut self, role: Role) -> Self {
        self.role = Some(role);
        self
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    pub fn description(mut self, text: impl Into<String>) -> Self {
        self.description = Some(text.into());
        self
    }

    pub fn checked(mut self, checked: impl Into<Checked>) -> Self {
        self.checked = Some(checked.into());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = Some(selected);
        self
    }

    pub fn expanded(mut self, expanded: bool) -> Self {
        self.expanded = Some(expanded);
        self
    }

    pub fn value(mut self, now: f64, min: f64, max: f64) -> Self {
        self.value = Some(AccessValue {
            now,
            min,
            max,
            step: self.value.and_then(|v| v.step),
        });
        self
    }

    pub fn step(mut self, step: f64) -> Self {
        let mut v = self.value.unwrap_or_default();
        v.step = Some(step);
        self.value = Some(v);
        self
    }

    pub fn value_text(mut self, text: impl Into<String>) -> Self {
        self.value_text = Some(text.into());
        self
    }

    pub fn level(mut self, level: u32) -> Self {
        self.level = Some(level);
        self
    }

    pub fn live(mut self, live: Live) -> Self {
        self.live = Some(live);
        self
    }

    pub fn multiselectable(mut self) -> Self {
        self.multiselectable = true;
        self
    }

    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }
}

pub(crate) type AccessFn = Rc<dyn Fn(Access) -> Access>;

/// A node as assistive technology sees it.
#[derive(Clone, Debug, PartialEq)]
pub struct AccessNode {
    /// What it is known by in [`AccessTree`] and to [`perform`]. It stays the same for as long as
    /// the node exists.
    pub id: u64,
    pub role: Role,
    pub label: Option<String>,
    pub description: Option<String>,
    pub checked: Option<Checked>,
    pub selected: Option<bool>,
    pub expanded: Option<bool>,
    pub value: Option<AccessValue>,
    /// The value in words; for something that is typed in, what is in it.
    pub value_text: Option<String>,
    pub level: Option<u32>,
    pub live: Live,
    pub multiselectable: bool,
    pub focusable: bool,
    pub disabled: bool,
    /// The border box in logical pixels, in the window or in the popup it is in.
    pub bounds: RectF,
    pub children: Vec<u64>,
}

/// The tree assistive technology sees. Nodes that say nothing (a box for the layout) are left out
/// and what is in them goes to their parent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AccessTree {
    pub root: u64,
    pub focus: Option<u64>,
    /// In the order of the document: a parent before its children.
    pub nodes: Vec<AccessNode>,
}

impl AccessTree {
    pub fn get(&self, id: u64) -> Option<&AccessNode> {
        self.nodes.iter().find(|n| n.id == id)
    }

    /// The first node with this role and label, for tests and for bridges that look things up.
    pub fn find(&self, role: Role, label: &str) -> Option<&AccessNode> {
        self.nodes
            .iter()
            .find(|n| n.role == role && n.label.as_deref() == Some(label))
    }
}

/// What assistive technology asks for.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub enum AccessAction {
    Focus,
    /// Presses it, as a click does.
    Click,
    /// Makes the value bigger by a step, as the Up key does.
    Increment,
    /// Makes the value smaller by a step, as the Down key does.
    Decrement,
    ScrollIntoView,
}

pub(crate) fn to_id(id: NodeId) -> u64 {
    id.data().as_ffi()
}

pub(crate) fn from_id(id: u64) -> NodeId {
    KeyData::from_ffi(id).into()
}

/// The tree as it is now, with the layout of the last update.
pub fn snapshot() -> AccessTree {
    with_doc(|d| build(d))
}

/// Asks for `action` on node `id` of an [`AccessTree`]. It happens in the next update, on the UI
/// thread; from another thread, send this through a [`ui_channel`](crate::channel::ui_channel).
/// Returns false when there is no such node.
pub fn perform(id: u64, action: AccessAction) -> bool {
    with_doc(|d| {
        let node = from_id(id);
        if d.get(node).is_none() {
            return false;
        }
        d.access_actions.push((node, action));
        d.mark_paint();
        true
    })
}

type Watcher = Rc<dyn Fn(&AccessTree)>;

thread_local! {
    static WATCHERS: RefCell<Vec<(u64, Watcher)>> = const { RefCell::new(Vec::new()) };
    static LAST: RefCell<Option<AccessTree>> = const { RefCell::new(None) };
    static NEXT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// What each window has of its own here; see `crate::window`.
#[derive(Default)]
pub(crate) struct Locals {
    watchers: Vec<(u64, Watcher)>,
    last: Option<AccessTree>,
}

/// Trades what the window being worked on has for `other`.
pub(crate) fn swap_locals(other: &mut Locals) {
    WATCHERS.with(|w| std::mem::swap(&mut *w.borrow_mut(), &mut other.watchers));
    LAST.with(|l| std::mem::swap(&mut *l.borrow_mut(), &mut other.last));
}

/// What [`watch`] gives back: dropping it doesn't stop the watching; [`stop`](Self::stop) does.
#[derive(Clone, Copy, Debug)]
pub struct AccessWatch {
    id: u64,
    window: u64,
}

impl AccessWatch {
    pub fn stop(self) {
        crate::window::within(self.window, || {
            WATCHERS.with(|w| w.borrow_mut().retain(|(id, _)| *id != self.id));
            if WATCHERS.with(|w| w.borrow().is_empty()) {
                LAST.with(|l| l.borrow_mut().take());
            }
        });
    }
}

/// Calls `f` with the tree now, and after each update that changed it, on the UI thread. Nothing
/// of this is worked out while nobody watches.
pub fn watch(f: impl Fn(&AccessTree) + 'static) -> AccessWatch {
    let id = NEXT.with(|n| n.replace(n.get() + 1));
    let tree = snapshot();
    f(&tree);
    LAST.with(|l| *l.borrow_mut() = Some(tree));
    WATCHERS.with(|w| w.borrow_mut().push((id, Rc::new(f))));
    with_doc(|d| d.access_stale = true);
    AccessWatch {
        id,
        window: crate::window::active_id(),
    }
}

/// Tells the watchers, if there are any and the tree changed. Called at the end of an update.
pub(crate) fn tell_watchers() {
    if WATCHERS.with(|w| w.borrow().is_empty()) {
        return;
    }
    if !with_doc(|d| std::mem::take(&mut d.access_stale)) {
        return;
    }
    let tree = snapshot();
    let changed = LAST.with(|l| l.borrow().as_ref() != Some(&tree));
    if !changed {
        return;
    }
    let watchers: Vec<_> = WATCHERS.with(|w| w.borrow().iter().map(|(_, f)| f.clone()).collect());
    for f in watchers {
        f(&tree);
    }
    LAST.with(|l| *l.borrow_mut() = Some(tree));
}

fn build(d: &Document) -> AccessTree {
    let mut nodes = Vec::new();
    let Some(root) = d.root else {
        return AccessTree::default();
    };
    let root_children = collect(d, root, &mut nodes);
    let root_id = to_id(root);
    let bounds = rect_of(d, root);
    nodes.insert(
        0,
        AccessNode {
            id: root_id,
            role: Role::Generic,
            label: None,
            description: None,
            checked: None,
            selected: None,
            expanded: None,
            value: None,
            value_text: None,
            level: None,
            live: Live::Off,
            multiselectable: false,
            focusable: false,
            disabled: false,
            bounds,
            children: root_children,
        },
    );
    AccessTree {
        root: root_id,
        focus: d.state.focused.map(to_id),
        nodes,
    }
}

fn rect_of(d: &Document, id: NodeId) -> RectF {
    let l = |v: i32| d.scale.to_logical(v as f32);
    d.window_rect(id)
        .map(|r| RectF::new(l(r.x), l(r.y), l(r.width), l(r.height)))
        .unwrap_or_default()
}

/// Adds what is under `id` that says something, and gives the ids of what goes right under it.
fn collect(d: &Document, id: NodeId, out: &mut Vec<AccessNode>) -> Vec<u64> {
    let Some(n) = d.get(id) else {
        return Vec::new();
    };
    let mut ids = Vec::new();
    for &child in &n.children {
        match make(d, child, out) {
            Made::Node(node) => ids.push(node),
            Made::Through => ids.extend(collect(d, child, out)),
            Made::Hidden => {}
        }
    }
    ids
}

enum Made {
    Node(u64),
    /// It says nothing: what is in it goes to its parent.
    Through,
    /// Neither it nor anything in it is there.
    Hidden,
}

/// Adds `id` if it says something, with what is under it.
fn make(d: &Document, id: NodeId, out: &mut Vec<AccessNode>) -> Made {
    let Some(n) = d.get(id) else {
        return Made::Hidden;
    };
    let access = n.access.as_deref().cloned().unwrap_or_default();
    if !n.visible || access.hidden {
        return Made::Hidden;
    }
    let (role, value_text) = match &n.kind {
        NodeKind::Text(t) => (
            access
                .role
                .or_else(|| (!t.text.trim().is_empty()).then_some(Role::Text)),
            None,
        ),
        NodeKind::Edit(e) => (
            Some(access.role.unwrap_or(if e.multiline {
                Role::MultilineTextInput
            } else {
                Role::TextInput
            })),
            Some(e.text.clone()),
        ),
        NodeKind::Editor(e) => (
            Some(access.role.unwrap_or(Role::MultilineTextInput)),
            Some(e.text.to_string()),
        ),
        NodeKind::Image(_) if access.label.is_some() => {
            (Some(access.role.unwrap_or(Role::Image)), None)
        }
        _ => (access.role, None),
    };
    let speaks = role.is_some() || access.label.is_some() || n.focusable;
    if !speaks {
        return Made::Through;
    }
    let role = role.unwrap_or(Role::Generic);
    let at = out.len();
    out.push(AccessNode {
        id: to_id(id),
        role,
        label: None,
        description: access.description.clone(),
        checked: access.checked,
        selected: access.selected,
        expanded: access.expanded,
        value: access.value,
        value_text: access.value_text.clone().or(value_text),
        level: access.level,
        live: access.live.unwrap_or(if role == Role::Status {
            Live::Polite
        } else {
            Live::Off
        }),
        multiselectable: access.multiselectable,
        focusable: n.focusable,
        disabled: n.is_disabled(),
        bounds: rect_of(d, id),
        children: Vec::new(),
    });
    let children = collect(d, id, out);
    let label = access.label.clone().or_else(|| match &n.kind {
        NodeKind::Text(t) => Some(t.text.clone()),
        _ if role.named_from_content() => {
            let mut s = String::new();
            text_in(d, id, &mut s);
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            (!s.is_empty()).then_some(s)
        }
        _ => None,
    });
    out[at].label = label;
    out[at].children = children;
    Made::Node(to_id(id))
}

/// The text of what is under `id`, in order, for a name that comes from it.
fn text_in(d: &Document, id: NodeId, s: &mut String) {
    let Some(n) = d.get(id) else {
        return;
    };
    for &child in &n.children {
        let Some(c) = d.get(child) else {
            continue;
        };
        if !c.visible || c.access.as_ref().is_some_and(|a| a.hidden) {
            continue;
        }
        if let NodeKind::Text(t) = &c.kind {
            s.push(' ');
            s.push_str(&t.text);
        }
        text_in(d, child, s);
    }
}
