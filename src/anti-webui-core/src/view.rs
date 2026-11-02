//! Builders that create nodes and wire them to the reactive system.
//!
//! A builder only collects configuration; nodes are created in [`IntoView::into_view`], inside
//! the current reactive owner, and removed again when that owner is disposed.

use crate::event::{KeyboardEvent, PointerEvent, ScrollEvent};
use crate::geometry::RectI;
use crate::platform::ImePurpose;
use crate::reactive::{
    Owner, Signal, create_child_owner, effect, memo, on_cleanup, signal, untrack,
};
use crate::text::TextLayout;
use crate::text::edit::{EditData, TextCallback};
use crate::tree::{Listeners, Node, NodeId, NodeKind, TextData, with_doc};
use anti_webui_style::{Style, Styled};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::rc::Rc;

/// The root node of a built piece of UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct View(pub NodeId);

pub trait IntoView {
    fn into_view(self) -> View;
}

impl IntoView for View {
    fn into_view(self) -> View {
        self
    }
}

/// Inserts a node and removes it when the current owner is disposed.
fn create_node(node: Node) -> NodeId {
    let id = with_doc(|d| d.create(node));
    on_cleanup(move || with_doc(|d| d.remove(id)));
    id
}

/// A handle to a node that can be attached with `.node_ref()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NodeRef(Signal<Option<NodeId>>);

pub fn node_ref() -> NodeRef {
    NodeRef(signal(None))
}

impl NodeRef {
    pub fn get(&self) -> Option<NodeId> {
        self.0.get()
    }

    /// The border box in window physical pixels, as of the last layout.
    pub fn rect(&self) -> Option<RectI> {
        let id = self.0.get_untracked()?;
        with_doc(|d| d.window_rect(id))
    }

    pub fn focus(&self) {
        if let Some(id) = self.0.get_untracked() {
            with_doc(|d| d.pending_focus = Some(Some(id)));
        }
    }

    pub fn blur(&self) {
        with_doc(|d| d.pending_focus = Some(None));
    }
}

type StyleFn = Box<dyn Fn(Style) -> Style>;

/// Keeps the node's dynamic style layers in sync with their closures.
fn bind_dynamic_styles(id: NodeId, layers: Vec<StyleFn>) {
    with_doc(|d| {
        if let Some(n) = d.get_mut(id) {
            n.style.dynamic = vec![Style::default(); layers.len()];
        }
    });
    for (slot, f) in layers.into_iter().enumerate() {
        effect(move || {
            let style = f(Style::default());
            with_doc(|d| {
                if let Some(n) = d.get_mut(id)
                    && n.style.dynamic[slot] != style
                {
                    n.style.dynamic[slot] = style;
                    d.mark_style_of(id);
                }
            });
        });
    }
}

/// Common element configuration shared by `div` and `text_input`.
#[derive(Default)]
struct ElementConfig {
    style: Style,
    hover: Option<Style>,
    focus: Option<Style>,
    active: Option<Style>,
    dynamic: Vec<StyleFn>,
    listeners: Listeners,
    focusable: bool,
    disabled: Option<Box<dyn Fn() -> bool>>,
    node_ref: Option<NodeRef>,
}

impl ElementConfig {
    fn build(self, mut node: Node) -> NodeId {
        node.style.base = self.style;
        node.style.hover = self.hover;
        node.style.focus = self.focus;
        node.style.active = self.active;
        node.listeners = self.listeners;
        node.focusable |= self.focusable;
        let id = create_node(node);
        bind_dynamic_styles(id, self.dynamic);
        if let Some(disabled) = self.disabled {
            effect(move || {
                let v = disabled();
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id) {
                        n.disabled = v;
                    }
                    d.mark_style_of(id);
                });
            });
        }
        if let Some(r) = self.node_ref {
            r.0.set(Some(id));
        }
        id
    }
}

macro_rules! element_methods {
    () => {
        /// Style applied while the pointer is over the node.
        pub fn hover(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.hover = Some(f(self.cfg.hover.take().unwrap_or_default()));
            self
        }

        /// Style applied while the node has keyboard focus.
        pub fn focused(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.focus = Some(f(self.cfg.focus.take().unwrap_or_default()));
            self
        }

        /// Style applied while the node (or a descendant) is pressed.
        pub fn active(mut self, f: impl FnOnce(Style) -> Style) -> Self {
            self.cfg.active = Some(f(self.cfg.active.take().unwrap_or_default()));
            self
        }

        /// A reactive style layer, recomputed whenever the signals it reads change.
        pub fn dyn_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
            self.cfg.dynamic.push(Box::new(f));
            self
        }

        pub fn on_click(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.click = Some(Rc::new(f));
            self
        }

        pub fn on_pointer_down(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.pointer_down = Some(Rc::new(f));
            self
        }

        pub fn on_pointer_up(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.pointer_up = Some(Rc::new(f));
            self
        }

        pub fn on_pointer_move(mut self, f: impl Fn(&PointerEvent) + 'static) -> Self {
            self.cfg.listeners.pointer_move = Some(Rc::new(f));
            self
        }

        pub fn on_hover(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.listeners.hover = Some(Rc::new(f));
            self
        }

        pub fn on_key_down(mut self, f: impl Fn(&KeyboardEvent) + 'static) -> Self {
            self.cfg.listeners.key_down = Some(Rc::new(f));
            self
        }

        pub fn on_key_up(mut self, f: impl Fn(&KeyboardEvent) + 'static) -> Self {
            self.cfg.listeners.key_up = Some(Rc::new(f));
            self
        }

        pub fn on_scroll(mut self, f: impl Fn(&ScrollEvent) + 'static) -> Self {
            self.cfg.listeners.scroll = Some(Rc::new(f));
            self
        }

        pub fn on_focus_change(mut self, f: impl Fn(bool) + 'static) -> Self {
            self.cfg.listeners.focus = Some(Rc::new(f));
            self
        }

        /// Makes the node reachable with Tab and focusable by clicking.
        pub fn focusable(mut self) -> Self {
            self.cfg.focusable = true;
            self
        }

        /// Disables pointer and keyboard handlers while `f` returns true.
        pub fn disabled(mut self, f: impl Fn() -> bool + 'static) -> Self {
            self.cfg.disabled = Some(Box::new(f));
            self
        }

        pub fn node_ref(mut self, r: NodeRef) -> Self {
            self.cfg.node_ref = Some(r);
            self
        }
    };
}

/// A generic box element.
#[derive(Default)]
pub struct Div {
    cfg: ElementConfig,
    children: Vec<View>,
}

pub fn div() -> Div {
    Div::default()
}

impl Styled for Div {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl Div {
    element_methods!();

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

impl IntoView for Div {
    fn into_view(self) -> View {
        let id = self.cfg.build(Node::new(NodeKind::Element));
        let children: Vec<NodeId> = self.children.into_iter().map(|v| v.0).collect();
        with_doc(|d| d.set_children(id, children));
        View(id)
    }
}

/// The source of a text node's content.
pub enum TextSource {
    Static(String),
    Dynamic(Box<dyn Fn() -> String>),
}

pub trait IntoTextSource {
    fn into_text_source(self) -> TextSource;
}

impl IntoTextSource for &str {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self.to_owned())
    }
}

impl IntoTextSource for String {
    fn into_text_source(self) -> TextSource {
        TextSource::Static(self)
    }
}

impl<F, S> IntoTextSource for F
where
    F: Fn() -> S + 'static,
    S: Display,
{
    fn into_text_source(self) -> TextSource {
        TextSource::Dynamic(Box::new(move || self().to_string()))
    }
}

/// A run of text. Selectable by default.
pub struct Text {
    source: TextSource,
    style: Style,
    dynamic: Vec<StyleFn>,
}

/// Creates a text node. Pass a closure for reactive text: `text(move || count.get())`.
pub fn text(source: impl IntoTextSource) -> Text {
    Text {
        source: source.into_text_source(),
        style: Style::default(),
        dynamic: Vec::new(),
    }
}

impl Text {
    /// A reactive style layer, recomputed whenever the signals it reads change.
    pub fn dyn_style(mut self, f: impl Fn(Style) -> Style + 'static) -> Self {
        self.dynamic.push(Box::new(f));
        self
    }
}

impl Styled for Text {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.style
    }
}

impl IntoView for Text {
    fn into_view(self) -> View {
        let initial = match &self.source {
            TextSource::Static(s) => s.clone(),
            TextSource::Dynamic(_) => String::new(),
        };
        let mut node = Node::new(NodeKind::Text(TextData {
            text: initial,
            version: 0,
            layout: TextLayout::default(),
        }));
        node.style.base = self.style;
        let id = create_node(node);
        bind_dynamic_styles(id, self.dynamic);
        if let TextSource::Dynamic(f) = self.source {
            effect(move || {
                let s = f();
                with_doc(|d| {
                    if let Some(n) = d.get_mut(id)
                        && let NodeKind::Text(t) = &mut n.kind
                        && t.text != s
                    {
                        t.text = s;
                        t.version += 1;
                        d.invalidate_layout(id);
                    }
                });
            });
        }
        View(id)
    }
}

impl IntoView for &str {
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl IntoView for String {
    fn into_view(self) -> View {
        text(self).into_view()
    }
}

impl<V: IntoView> IntoView for Option<V> {
    fn into_view(self) -> View {
        match self {
            Some(v) => v.into_view(),
            None => fragment(Vec::<View>::new()),
        }
    }
}

impl<V: IntoView> IntoView for Vec<V> {
    fn into_view(self) -> View {
        fragment(self)
    }
}

/// Groups views without generating a box.
pub fn fragment<V: IntoView>(children: impl IntoIterator<Item = V>) -> View {
    let id = create_node(Node::new(NodeKind::Fragment));
    let children: Vec<NodeId> = children.into_iter().map(|v| v.into_view().0).collect();
    with_doc(|d| d.set_children(id, children));
    View(id)
}

/// Rebuilds its content whenever the signals read by `f` change.
pub fn dyn_view<V: IntoView>(f: impl Fn() -> V + 'static) -> View {
    let id = create_node(Node::new(NodeKind::Fragment));
    effect(move || {
        // Nodes created here are owned by this effect and disposed before the next run.
        let child = f().into_view();
        with_doc(|d| d.set_children(id, vec![child.0]));
    });
    View(id)
}

/// Shows `view` while `when` returns true. `view` is only rebuilt when `when` flips.
pub fn show<V: IntoView>(
    when: impl Fn() -> bool + 'static,
    view: impl Fn() -> V + 'static,
) -> View {
    let cond = memo(when);
    dyn_view(move || {
        if cond.get() {
            Some(untrack(&view))
        } else {
            None
        }
    })
}

/// A keyed list. Items whose key survives an update keep their nodes; only new items are built.
pub fn for_each<T, K, V>(
    items: impl Fn() -> Vec<T> + 'static,
    key: impl Fn(&T) -> K + 'static,
    view: impl Fn(T) -> V + 'static,
) -> View
where
    T: 'static,
    K: Eq + Hash + Clone + 'static,
    V: IntoView,
{
    let id = create_node(Node::new(NodeKind::Fragment));
    let outer = Owner::current();
    type Rows<K> = Rc<RefCell<HashMap<K, (NodeId, Option<Owner>)>>>;
    let rows: Rows<K> = Rc::new(RefCell::new(HashMap::new()));
    on_cleanup({
        let rows = rows.clone();
        move || {
            for (_, (_, owner)) in rows.borrow_mut().drain() {
                if let Some(o) = owner {
                    o.dispose();
                }
            }
        }
    });
    effect(move || {
        let list = items();
        let mut old = std::mem::take(&mut *rows.borrow_mut());
        let mut next = HashMap::with_capacity(list.len());
        let mut order = Vec::with_capacity(list.len());
        for item in list {
            let k = key(&item);
            if next.contains_key(&k) {
                continue;
            }
            let entry = old.remove(&k).unwrap_or_else(|| {
                let build = || create_child_owner(|| view(item).into_view());
                let (v, owner) = match outer {
                    Some(o) => o.with(build),
                    None => untrack(build),
                };
                (v.0, Some(owner))
            });
            order.push(entry.0);
            next.insert(k, entry);
        }
        for (_, (_, owner)) in old {
            if let Some(o) = owner {
                o.dispose();
            }
        }
        *rows.borrow_mut() = next;
        with_doc(|d| d.set_children(id, order));
    });
    View(id)
}

/// A single- or multi-line text field.
pub struct TextInput {
    cfg: ElementConfig,
    multiline: bool,
    rows: u32,
    placeholder: String,
    value: Option<Signal<String>>,
    on_input: Option<TextCallback>,
    on_submit: Option<TextCallback>,
    purpose: ImePurpose,
    read_only: bool,
}

pub fn text_input() -> TextInput {
    TextInput {
        cfg: ElementConfig::default(),
        multiline: false,
        rows: 1,
        placeholder: String::new(),
        value: None,
        on_input: None,
        on_submit: None,
        purpose: ImePurpose::Normal,
        read_only: false,
    }
}

pub fn text_area() -> TextInput {
    TextInput {
        multiline: true,
        rows: 3,
        ..text_input()
    }
}

impl Styled for TextInput {
    fn style_mut(&mut self) -> &mut Style {
        &mut self.cfg.style
    }
}

impl TextInput {
    element_methods!();

    /// Two-way binds the text to a signal.
    pub fn value(mut self, value: Signal<String>) -> Self {
        self.value = Some(value);
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    /// Minimum number of visible lines for multi-line inputs.
    pub fn rows(mut self, rows: u32) -> Self {
        self.rows = rows;
        self
    }

    pub fn on_input(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_input = Some(Rc::new(f));
        self
    }

    /// Called on Enter in single-line inputs (Ctrl+Enter in multi-line ones).
    pub fn on_submit(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.on_submit = Some(Rc::new(f));
        self
    }

    pub fn purpose(mut self, purpose: ImePurpose) -> Self {
        self.purpose = purpose;
        self
    }

    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }
}

impl IntoView for TextInput {
    fn into_view(self) -> View {
        let mut data = EditData::new(self.multiline);
        data.rows = self.rows;
        data.placeholder = self.placeholder;
        data.value = self.value;
        data.on_input = self.on_input;
        data.on_submit = self.on_submit;
        data.purpose = self.purpose;
        data.read_only = self.read_only;
        let mut node = Node::new(NodeKind::Edit(Box::new(data)));
        node.focusable = true;
        let id = self.cfg.build(node);
        if let Some(value) = self.value {
            effect(move || {
                let v = value.get();
                with_doc(|d| {
                    if let Some(e) = d.get_mut(id).and_then(|n| n.edit_mut())
                        && e.text != v
                    {
                        e.set_text(v);
                        d.invalidate_layout(id);
                    }
                });
            });
        }
        View(id)
    }
}
