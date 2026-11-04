//! Incremental updates must give exactly what recomputing everything gives, and culling must
//! never lose anything visible.

use anti_webui_core::Ui;
use anti_webui_core::damage;
use anti_webui_core::event::*;
use anti_webui_core::geometry::{PointF, RectI, Scale, SizeI};
use anti_webui_core::layout::child_clip;
use anti_webui_core::paint::paint_everything;
use anti_webui_core::prelude::*;
use anti_webui_core::scene::{Primitive, Scene};
use anti_webui_core::text::selection;
use anti_webui_core::tree::{Document, NodeId, NodeKind, with_doc};
use std::time::Instant;

/// What every later stage depends on, for every node.
#[derive(PartialEq, Debug)]
struct NodeState {
    id: NodeId,
    computed: anti_webui_style::ComputedStyle,
    rect: RectI,
    abs: RectI,
    clip: RectI,
    clip_radii: [f32; 4],
    scroll_root: bool,
    space: Option<NodeId>,
    transformed: bool,
    layer_clip: anti_webui_core::scene::Clip,
    outer_space: Option<NodeId>,
    doc_order: u32,
    visible: bool,
}

fn snapshot() -> Vec<(NodeState, RectI)> {
    with_doc(|d| {
        d.preorder()
            .into_iter()
            .map(|id| {
                let n = d.get(id).unwrap();
                (
                    NodeState {
                        id,
                        computed: n.computed.clone(),
                        rect: n.rect,
                        abs: n.abs,
                        clip: n.clip,
                        clip_radii: n.clip_radii,
                        scroll_root: n.scroll_root,
                        space: n.space,
                        transformed: n.transformed,
                        layer_clip: if n.transformed {
                            n.layer_clip
                        } else {
                            anti_webui_core::scene::Clip::NONE
                        },
                        outer_space: n.transformed.then_some(n.outer_space).flatten(),
                        doc_order: n.doc_order,
                        visible: n.visible,
                    },
                    n.subtree_bounds,
                )
            })
            .collect()
    })
}

/// The scene with nothing culled.
fn paint_all(now: Instant) -> Scene {
    with_doc(|d| paint_everything(d, now))
}

/// What the scene draws inside `view`, in order.
fn in_view(prims: &[Primitive], view: RectI) -> Vec<&Primitive> {
    prims
        .iter()
        .filter(|p| damage::primitive_bounds(p).intersects(&view))
        .collect()
}

/// Panics unless the scenes draw the same things, in the same order, where they can be seen.
fn assert_same_in_view(all: &Scene, culled: &Scene, what: &str) {
    let screen = RectI::new(0, 0, all.width as i32, all.height as i32);
    same_primitives_in_view(&all.primitives, &culled.primitives, screen, what);
}

fn same_primitives_in_view(all: &[Primitive], culled: &[Primitive], view: RectI, what: &str) {
    let (a, b) = (in_view(all, view), in_view(culled, view));
    assert_eq!(a.len(), b.len(), "{what}: primitives lost by culling");
    for (a, b) in a.iter().zip(&b) {
        let (Primitive::Layer(a), Primitive::Layer(b)) = (a, b) else {
            assert_eq!(a, b, "{what}: primitive changed by culling");
            continue;
        };
        assert!(a.same_placement(b), "{what}: layer {a:?} vs {b:?}");
        let inner = a
            .matrix()
            .inverse()
            .unwrap()
            .bounds_of(a.bounds_on_parent().intersect(&view));
        same_primitives_in_view(&a.content.primitives, &b.content.primitives, inner, what);
    }
}

/// The paint order worked out from the styles alone, without the lists the painter uses.
fn paint_order(d: &Document) -> Vec<NodeId> {
    fn stacked(d: &Document, id: NodeId) -> bool {
        let n = d.get(id).unwrap();
        !matches!(n.kind, NodeKind::Fragment)
            && (n.computed.z_index.is_some() || n.computed.position == Position::Absolute)
    }
    fn context(d: &Document, id: NodeId) -> bool {
        let n = d.get(id).unwrap();
        !matches!(n.kind, NodeKind::Fragment)
            && (d.root == Some(id)
                || n.computed.z_index.is_some()
                || n.computed.opacity < 1.0
                || n.computed.has_transform()
                || n.computed.overflow_x == Overflow::Scroll
                || n.computed.overflow_y == Overflow::Scroll)
    }
    fn gather(d: &Document, id: NodeId, out: &mut Vec<NodeId>) {
        for &c in &d.get(id).unwrap().children {
            if stacked(d, c) {
                out.push(c);
            }
            if !context(d, c) {
                gather(d, c, out);
            }
        }
    }
    fn flow(d: &Document, id: NodeId, out: &mut Vec<NodeId>) {
        for &c in &d.get(id).unwrap().children {
            if !stacked(d, c) {
                paint(d, c, out);
            }
        }
    }
    fn paint(d: &Document, id: NodeId, out: &mut Vec<NodeId>) {
        out.push(id);
        if !context(d, id) {
            flow(d, id, out);
            return;
        }
        let mut lifted = Vec::new();
        gather(d, id, &mut lifted);
        lifted.sort_by_key(|n| d.get(*n).unwrap().computed.z_index.unwrap_or(0));
        let z = |n: &NodeId| d.get(*n).unwrap().computed.z_index.unwrap_or(0);
        for n in lifted.iter().filter(|n| z(n) < 0) {
            paint(d, *n, out);
        }
        flow(d, id, out);
        for n in lifted.iter().filter(|n| z(n) >= 0) {
            paint(d, *n, out);
        }
    }
    let mut out = Vec::new();
    paint(d, d.root.unwrap(), &mut out);
    out
}

/// Whether what `id` paints can be seen at `p`, going through every space it is in.
fn shows_at(d: &Document, id: NodeId, p: PointF) -> bool {
    let n = d.get(id).unwrap();
    let Some(q) = d.from_window(n, p) else {
        return false;
    };
    if !n.abs.contains(q) || !n.clip.contains(q) {
        return false;
    }
    let mut space = n.space;
    while let Some(r) = space.map(|s| d.get(s).unwrap()) {
        if r.is_popup() {
            break;
        }
        let next = if r.transformed {
            r.outer_space
        } else {
            r.space
        };
        let Some(q) = d.from_window_space(next, p) else {
            return false;
        };
        let clip = if r.transformed {
            r.layer_clip.rect
        } else {
            child_clip(r, d.scale, r.clip_shape()).rect
        };
        if !clip.contains(q) {
            return false;
        }
        space = next;
    }
    true
}

/// What hit testing should find, by looking at every node: the last one in paint order that
/// isn't clipped away at `p`.
fn hit_by_brute_force(p: PointF) -> Option<NodeId> {
    with_doc(|d| {
        paint_order(d).into_iter().rev().find(|id| {
            let n = d.get(*id).unwrap();
            n.visible
                && n.computed.pointer_events
                && !matches!(n.kind, NodeKind::Fragment)
                && shows_at(d, *id, p)
        })
    })
}

/// What selection should find, by looking at every node: the nearest visible selectable text.
fn nearest_text_by_brute_force(p: PointF) -> Option<NodeId> {
    with_doc(|d| {
        let mut best: Option<((f32, f32), NodeId)> = None;
        for id in selection::selectable_text_nodes(d) {
            let vis = d.visible_window_rect(id).unwrap();
            if vis.is_empty() {
                continue;
            }
            let (dx, dy) = vis.axis_distance(p);
            if best.is_none_or(|(k, _)| (dy, dx) < k) {
                best = Some(((dy, dx), id));
            }
        }
        best.map(|(_, id)| id)
    })
}

/// Hit testing and selection skip what can't be seen; they must find what looking at everything
/// finds.
fn check_finding(p: PointF, pressed: bool, what: &str) {
    if !pressed {
        let hovered = with_doc(|d| d.state.hovered.first().copied());
        assert_eq!(hovered, hit_by_brute_force(p), "{what}: hit test at {p:?}");
    }
    let found = with_doc(|d| selection::hit_text_point(d, p).map(|t| t.node));
    assert_eq!(
        found,
        nearest_text_by_brute_force(p),
        "{what}: nearest text to {p:?}"
    );
}

fn check(ui: &mut Ui, now: Instant, what: &str) {
    ui.update();
    let mut incremental_scene = ui.paint(now);
    if with_doc(|d| d.settling) {
        ui.update();
        incremental_scene = ui.paint(now);
        assert!(!with_doc(|d| d.settling), "{what}: one frame must do");
    }
    assert_same_in_view(&paint_all(now), &incremental_scene, what);

    let incremental = snapshot();
    with_doc(|d| {
        d.mark_style_all();
        d.invalidate_all_layout();
        d.stacking_dirty = true;
    });
    ui.update();
    let full = snapshot();
    assert_eq!(incremental.len(), full.len(), "{what}: node count");
    for ((a, a_bounds), (b, b_bounds)) in incremental.iter().zip(&full) {
        assert!(
            a == b,
            "{what}: stale node {:?}: rect {:?}/{:?} abs {:?}/{:?} clip {:?}/{:?} radii {:?}/{:?} root {}/{} space {:?}/{:?} order {}/{} visible {}/{} computed same {}",
            a.id,
            a.rect,
            b.rect,
            a.abs,
            b.abs,
            a.clip,
            b.clip,
            a.clip_radii,
            b.clip_radii,
            a.scroll_root,
            b.scroll_root,
            a.space,
            b.space,
            a.doc_order,
            b.doc_order,
            a.visible,
            b.visible,
            a.computed == b.computed,
        );
        // Bounds may be too big after growing, never too small. Nothing is inside of empty ones.
        assert!(
            b_bounds.is_empty() || a_bounds.intersect(b_bounds) == *b_bounds,
            "{what}: bounds {a_bounds:?} vs {b_bounds:?}"
        );
    }
    let full_scene = ui.paint(now);
    let d = damage::diff(&incremental_scene, &full_scene);
    assert!(
        d.is_empty(),
        "{what}: the scene differs from a full update: {d:?}"
    );
}

/// A small deterministic generator, so failures reproduce.
struct Rng(u64);

impl Rng {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % n
    }
}

struct Doc {
    color: Signal<u32>,
    size: Signal<f32>,
    gap: Signal<f32>,
    label: Signal<String>,
    items: Signal<Vec<u32>>,
    shown: Signal<bool>,
    hidden: Signal<bool>,
    glow: Signal<bool>,
    /// What shaders paint some of the rows with.
    painted: Signal<u32>,
    corner: Signal<f32>,
    fills: Signal<bool>,
    box_corner: Signal<f32>,
    /// `z_index` of some rows; 0 for none.
    row_z: Signal<i32>,
    /// Overlapping stacked boxes, in the scroller and over everything.
    overlays: Signal<bool>,
    faded: Signal<bool>,
    /// How some rows and boxes are transformed, and the scroller with all of that in it.
    moved: Signal<u32>,
    moved_scroller: Signal<u32>,
    outer: NodeRef,
    inner: NodeRef,
    field: NodeRef,
}

struct Painted(f32);

impl ShaderParams for Painted {
    const FIELDS: &'static [anti_webui_core::shader::ParamField] =
        &[anti_webui_core::shader::ParamField {
            name: "value",
            ty: anti_webui_core::shader::ParamType::F32,
        }];

    fn write(&self, out: &mut anti_webui_core::shader::ParamWriter) {
        anti_webui_core::shader::ParamValue::write(&self.0, out);
    }
}

static PAINT: Shader<Painted> = Shader::new("paint", "");

/// Only what keeps boxes as boxes: what the hit test finds is then easy to work out.
fn transformed(st: Style, how: u32) -> Style {
    match how {
        1 => st.translate(7.0, 3.0),
        2 => st.scale(2.0).transform_origin(0.0, 0.0),
        3 => st.translate(-4.5, 10.25).opacity(0.5),
        4 => st.scale_x(0.5).translate_y(20.0),
        _ => st,
    }
}

fn view(s: &Doc) -> impl IntoView + use<> {
    let (color, size, gap, label, items) = (s.color, s.size, s.gap, s.label, s.items);
    let (shown, hidden, glow, corner, fills) = (s.shown, s.hidden, s.glow, s.corner, s.fills);
    let painted = s.painted;
    let (outer, inner, field) = (s.outer, s.inner, s.field);
    let box_corner = s.box_corner;
    let (row_z, overlays, faded) = (s.row_z, s.overlays, s.faded);
    let (moved, moved_scroller) = (s.moved, s.moved_scroller);
    // Clips without scrolling; its child's clip follows the corners without any layout.
    let rounded_box = move || {
        div()
            .h(30.0)
            .w(120.0)
            .overflow_clip()
            .dyn_style(move |st| st.rounded(box_corner.get()))
            .dyn_style(move |st| transformed(st, moved.get()))
            .child(
                div()
                    .h(30.0)
                    .bg(rgb(0xddddff))
                    .child(text("in a rounded box")),
            )
    };
    div()
        .h(pct(100.0))
        .flex()
        .flex_col()
        .gap(4.0)
        .dyn_style(move |st| st.text_color(rgb(color.get())).text_size(size.get()))
        .child(div().h(24.0).child(text(move || label.get())))
        .child(rounded_box())
        .child(
            div()
                .flex_grow(1.0)
                .h(px(0.0))
                .node_ref(outer)
                // A border makes the box bigger than what the content is clipped to.
                .border(3.0)
                .overflow_scroll()
                .dyn_style(move |st| st.rounded(corner.get()))
                .dyn_style(move |st| transformed(st, moved_scroller.get()))
                .child(
                    div()
                        .w(900.0)
                        .flex()
                        .flex_col()
                        .dyn_style(move |st| st.gap(gap.get()))
                        .child(
                            text_input()
                                .w(200.0)
                                .node_ref(field)
                                .value(signal("edit".to_string())),
                        )
                        .child(
                            div()
                                .h(90.0)
                                .w(200.0)
                                .node_ref(inner)
                                .overflow_y_scroll()
                                .children((0..12).map(|i| {
                                    div()
                                        .h(24.0)
                                        .hover(|st| st.bg(rgb(0xddeeff)))
                                        .child(text(format!("inner {i}")))
                                })),
                        )
                        .child(
                            show(
                                move || shown.get(),
                                || div().h(50.0).bg(rgb(0xffeeee)).child(text("shown")),
                            )
                            .enter(|st| st.opacity(0.0).translate_y(10.0))
                            .exit(|st| st.opacity(0.0).scale(0.5))
                            .transition(Transition::ms(100)),
                        )
                        .child(
                            div()
                                .h(40.0)
                                .dyn_style(move |st| if hidden.get() { st.hidden() } else { st })
                                .bg(rgb(0xeeffee))
                                .child(text("may hide")),
                        )
                        .child(
                            for_each(
                                move || items.get(),
                                |i| *i,
                                move |i| {
                                    div()
                                        .h(30.0)
                                        .m(2.0)
                                        .bg(rgb(0xf4f4f4))
                                        .hover(|st| st.bg(rgb(0xccccff)).rounded(6.0))
                                        .active(|st| st.bg(rgb(0xaaaaff)))
                                        // Some of what changes on the way, every now and then.
                                        .transition(Transition::ms(if i % 2 == 0 {
                                            100
                                        } else {
                                            0
                                        }))
                                        .fill(move || {
                                            let on = i.is_multiple_of(3) && painted.get() % 2 == 1;
                                            on.then(|| PAINT.params(Painted(painted.get() as f32)))
                                        })
                                        .overlay(move || {
                                            let on = i.is_multiple_of(4)
                                                && !painted.get().is_multiple_of(3);
                                            on.then(|| PAINT.params(Painted(painted.get() as f32)))
                                        })
                                        .filter(move || {
                                            let v = painted.get();
                                            (i.is_multiple_of(5) && v % 3 == 1)
                                                .then(|| Image::content().blur(1.0 + v as f32))
                                        })
                                        .dyn_style(move |st| {
                                            let v = painted.get();
                                            let st = if i % 7 == 1 && v.is_multiple_of(2) {
                                                st.blur(v as f32).backdrop_blur(2.0 + v as f32)
                                            } else {
                                                st
                                            };
                                            if glow.get() && i % 3 == 0 {
                                                st.shadow_md()
                                            } else {
                                                st
                                            }
                                        })
                                        .dyn_style(move |st| {
                                            if row_z.get() != 0 && i % 2 == 0 {
                                                st.z_index(row_z.get() * (i as i32 % 5 - 2))
                                            } else {
                                                st
                                            }
                                        })
                                        .dyn_style(move |st| {
                                            if i % 3 == 2 {
                                                transformed(st, moved.get())
                                            } else {
                                                st
                                            }
                                        })
                                        .dyn_style(move |st| {
                                            if faded.get() && i % 4 == 1 {
                                                st.opacity(0.5)
                                            } else {
                                                st
                                            }
                                        })
                                        .child(text(format!("row {i}")))
                                },
                            )
                            .enter(|st| st.opacity(0.0).h(0.0))
                            .exit(|st| st.opacity(0.0).translate_x(10.0))
                            .transition(Transition::ms(100))
                            .moves(Transition::ms(100)),
                        )
                        .child(rounded_box())
                        .child(show(
                            move || overlays.get(),
                            || {
                                div()
                                    .absolute()
                                    .top(40.0)
                                    .left(30.0)
                                    .w(150.0)
                                    .h(120.0)
                                    .bg(rgb(0xccddee))
                                    .child(text("in the content"))
                                    .child(
                                        div()
                                            .absolute()
                                            .top(10.0)
                                            .left(100.0)
                                            .w(120.0)
                                            .h(40.0)
                                            .z_index(1)
                                            .bg(rgb(0xeeddcc))
                                            .shadow_md(),
                                    )
                            },
                        ))
                        .child(
                            div().dyn_style(move |st| st.h(if fills.get() { 3000.0 } else { 1.0 })),
                        ),
                ),
        )
        .child(show(
            move || overlays.get(),
            || {
                div()
                    .absolute()
                    .top(10.0)
                    .right(10.0)
                    .w(160.0)
                    .h(90.0)
                    .z_index(2)
                    .rounded(8.0)
                    .bg(rgb(0xffffff))
                    .shadow_md()
                    .hover(|st| st.bg(rgb(0xeeeeff)))
                    .child(text("over everything"))
            },
        ))
        // Stays where it is when the list above changes, but comes later in document order.
        .child(div().h(20.0).child(text("footer")))
}

#[test]
fn random_changes_match_a_full_update() {
    // Each thread has its own document.
    let threads: Vec<_> = (1..=8)
        .map(|seed| std::thread::spawn(move || random_changes(seed)))
        .collect();
    for t in threads {
        t.join().unwrap();
    }
}

fn random_changes(seed: u64) {
    let s = Doc {
        color: signal(0x202020),
        size: signal(14.0),
        gap: signal(0.0),
        label: signal("label".to_string()),
        items: signal((0..30).collect()),
        shown: signal(true),
        hidden: signal(false),
        glow: signal(false),
        painted: signal(0),
        corner: signal(0.0),
        fills: signal(true),
        box_corner: signal(0.0),
        row_z: signal(0),
        overlays: signal(false),
        faded: signal(false),
        moved: signal(0),
        moved_scroller: signal(0),
        outer: node_ref(),
        inner: node_ref(),
        field: node_ref(),
    };
    let mut ui = Ui::new(|| view(&s));
    let mut size = SizeI::new(500, 400);
    ui.handle_event(WindowEvent::Resized {
        size,
        scale: Scale(150),
    });
    ui.handle_event(WindowEvent::Focused(true));
    let now = Instant::now();
    check(&mut ui, now, "start");

    let m = Modifiers::default();
    let mut rng = Rng(seed);
    let mut next_item = 1000;
    let mut down = false;
    for step in 0..400 {
        let p = PointF::new(
            rng.next(size.width as u32) as f32,
            rng.next(size.height as u32) as f32,
        );
        let what = rng.next(26);
        // Like a real pointer, it gets to a place before doing anything there.
        ui.handle_event(WindowEvent::PointerMoved {
            position: p,
            modifiers: m,
        });
        match what {
            0..=2 => {}
            3 => {
                for pressed in [true, false] {
                    ui.handle_event(WindowEvent::PointerButton {
                        position: p,
                        button: PointerButton::Primary,
                        pressed,
                        modifiers: m,
                        time: step * 1000,
                    });
                }
                down = false;
            }
            4 => {
                down = rng.next(2) == 0;
                ui.handle_event(WindowEvent::PointerButton {
                    position: p,
                    button: PointerButton::Primary,
                    pressed: down,
                    modifiers: m,
                    time: step * 1000,
                });
            }
            5..=7 => ui.handle_event(WindowEvent::Scroll {
                position: p,
                delta_x: rng.next(300) as f32 - 150.0,
                delta_y: rng.next(900) as f32 - 450.0,
                modifiers: m,
                source: ScrollSource::Unknown,
                phase: ScrollPhase::Single,
                time: 0,
            }),
            20 => {
                // Fingers leaving, then some frames of inertia that the other changes interrupt.
                let dy = rng.next(120) as f32 - 60.0;
                for (i, phase) in [
                    ScrollPhase::Began,
                    ScrollPhase::Moved,
                    ScrollPhase::Moved,
                    ScrollPhase::Lifted { fling: true },
                ]
                .into_iter()
                .enumerate()
                {
                    ui.handle_event(WindowEvent::Scroll {
                        position: p,
                        delta_x: 0.0,
                        delta_y: dy,
                        modifiers: m,
                        source: ScrollSource::Touchpad,
                        phase,
                        time: step * 1000 + i as u32 * 10,
                    });
                }
                check(&mut ui, now, "flick");
                for i in 1..=rng.next(5) {
                    ui.animate(Instant::now() + std::time::Duration::from_millis(i as u64 * 60));
                    check(&mut ui, now, "inertia");
                }
            }
            8 => s.color.set(rng.next(0xffffff)),
            9 => s.size.set(10.0 + rng.next(10) as f32),
            10 => s.gap.set(rng.next(8) as f32),
            11 => s.label.set(format!("label {}", rng.next(100000))),
            12 => {
                let mut items = s.items.get();
                match rng.next(3) {
                    0 if items.len() > 1 => {
                        let i = rng.next(items.len() as u32) as usize;
                        items.remove(i);
                    }
                    1 => {
                        let i = rng.next(items.len() as u32 + 1) as usize;
                        items.insert(i, next_item);
                        next_item += 1;
                    }
                    _ => {
                        let i = rng.next(items.len() as u32) as usize;
                        items.swap(0, i);
                    }
                }
                s.items.set(items);
            }
            13 => s.shown.update(|v| *v = !*v),
            14 => s.hidden.update(|v| *v = !*v),
            15 => {
                s.glow.update(|v| *v = !*v);
                // Goes through the values rather than taking from `rng`, which would change every later step.
                s.painted.update(|v| *v = (*v + 1) % 6);
            }
            16 => s.corner.set(rng.next(20) as f32),
            17 => s.fills.update(|v| *v = !*v),
            18 => s.box_corner.set(rng.next(16) as f32),
            21 => s.row_z.set(rng.next(4) as i32 - 1),
            22 => s.overlays.update(|v| *v = !*v),
            23 => s.faded.update(|v| *v = !*v),
            24 => s.moved.set(rng.next(5)),
            25 => s.moved_scroller.set(rng.next(5)),
            _ => {
                size = SizeI::new(300 + rng.next(400) as i32, 200 + rng.next(400) as i32);
                let scale = Scale(120 + rng.next(120));
                ui.handle_event(WindowEvent::Resized { size, scale });
            }
        }
        let what = format!("seed {seed} step {step} (change {what})");
        ui.update();
        // Look from somewhere else too, now that things may have moved.
        check_finding(p, down, &what);
        let q = PointF::new(
            rng.next(size.width as u32) as f32,
            rng.next(size.height as u32) as f32,
        );
        ui.handle_event(WindowEvent::PointerMoved {
            position: q,
            modifiers: m,
        });
        ui.update();
        check_finding(q, down, &what);
        check(&mut ui, now, &what);
    }
}

/// Things that paint outside of their parents must survive culling as the view moves.
#[test]
fn culling_keeps_what_sticks_out() {
    let view_ref = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .overflow_scroll()
            .node_ref(view_ref)
            .children((0..60).map(|i| {
                let row = div().h(30.0).w(100.0);
                let row = if i % 7 == 0 {
                    // A long way out of its zero-height parent, sideways and downwards.
                    row.child(
                        div().h(0.0).child(
                            div()
                                .ml(700.0)
                                .mt(500.0)
                                .w(60.0)
                                .h(60.0)
                                .bg(rgb(0xff0000))
                                .shadow_md(),
                        ),
                    )
                } else {
                    row
                };
                row.shadow_md().child(text(
                    "a line of text much wider than the row it is in, on and on and on",
                ))
            }))
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale(120),
    });
    let now = Instant::now();
    let m = Modifiers::default();
    for step in 0..60 {
        ui.handle_event(WindowEvent::Scroll {
            position: PointF::new(100.0, 100.0),
            delta_x: if step % 5 == 0 { 40.0 } else { 0.0 },
            delta_y: 70.0,
            modifiers: m,
            source: ScrollSource::Unknown,
            phase: ScrollPhase::Single,
            time: 0,
        });
        ui.update();
        let scene = ui.paint(now);
        assert_same_in_view(&paint_all(now), &scene, &format!("step {step}"));
    }
}

/// Layout places a fragment's children directly, so they can move while the fragment itself stays
/// exactly where it was.
#[test]
fn fragment_children_follow_a_growing_sibling() {
    let height = signal(0.0f32);
    let row = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .h(100.0)
            .w(150.0)
            .flex()
            .flex_col()
            .child(div().dyn_style(move |st| st.h(height.get())))
            .child(for_each(
                || vec![1],
                |i| *i,
                move |_: i32| div().h(20.0).node_ref(row),
            ))
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(500, 400),
        scale: Scale(120),
    });
    ui.update();
    let before = row.rect().unwrap();
    height.set(30.0);
    ui.update();
    assert_eq!(row.rect().unwrap().y, before.y + 30);
}

/// Things just outside the window that paint into it must survive culling too; there is no
/// margin here, unlike in a layer.
#[test]
fn culling_at_the_window_edge() {
    let mut ui = Ui::new(|| {
        div()
            .h(pct(100.0))
            // Below the window, with a shadow reaching up into it.
            .child(
                div()
                    .absolute()
                    .top(305.0)
                    .left(10.0)
                    .w(100.0)
                    .h(50.0)
                    .bg(rgb(0xff0000))
                    .shadow(BoxShadow {
                        offset_x: 0.0,
                        offset_y: 0.0,
                        blur: 40.0,
                        spread: 0.0,
                        color: Color::BLACK,
                        inset: false,
                    }),
            )
            // Past the right edge, with an outline reaching back.
            .child(
                div()
                    .absolute()
                    .top(10.0)
                    .left(405.0)
                    .w(50.0)
                    .h(50.0)
                    .outline(12.0)
                    .outline_color(Color::BLACK),
            )
            // Left of the window, with a line of text much wider than its box.
            .child(
                div()
                    .absolute()
                    .top(100.0)
                    .left(-300.0)
                    .w(100.0)
                    .h(30.0)
                    .whitespace_nowrap()
                    .child(text(
                        "this line of text is a lot wider than the box it sits in, so that \
                         it comes out of the box and reaches into the window from the left",
                    )),
            )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(400, 300),
        scale: Scale(120),
    });
    ui.update();
    let now = Instant::now();
    let scene = ui.paint(now);
    let visible = in_view(&scene.primitives, RectI::new(0, 0, 400, 300)).len();
    assert!(visible >= 3, "only {visible} of them reach the window");
    assert_same_in_view(&paint_all(now), &scene, "window edge");
}

/// Everything facing the outside is in window coordinates, wherever the content has scrolled to.
#[test]
fn scrolled_content_keeps_window_coordinates_outside() {
    let view_ref = node_ref();
    let button = node_ref();
    let field = node_ref();
    let above = node_ref();
    let clicks = signal(0);
    let down_at = signal((0.0f32, 0.0f32));
    let value = signal("hello world".to_string());
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .flex()
            .flex_col()
            .child(
                div()
                    .h(50.0)
                    .node_ref(above)
                    .child(text("above the scroller")),
            )
            .child(
                div()
                    .flex_grow(1.0)
                    .h(px(0.0))
                    .overflow_y_scroll()
                    .node_ref(view_ref)
                    .child(div().h(800.0))
                    .child(
                        div()
                            .w(120.0)
                            .h(40.0)
                            .node_ref(button)
                            .on_click(move |_| clicks.update(|c| *c += 1))
                            .on_pointer_down(move |e| down_at.set((e.local.x, e.local.y))),
                    )
                    .child(text_input().w(300.0).node_ref(field).value(value))
                    .child(div().h(800.0)),
            )
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(500, 400),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let view_rect = view_ref.rect().unwrap();
    let before = button.rect().unwrap();
    assert_eq!(before.y, view_rect.y + 800);

    // Scroll so that the button is at a known place, without any position pass.
    let m = Modifiers::default();
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(100.0, 200.0),
        delta_x: 0.0,
        delta_y: 700.0,
        modifiers: m,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let r = button.rect().unwrap();
    assert_eq!(
        r.y,
        before.y - 700,
        "NodeRef::rect is in window coordinates"
    );

    let click = |ui: &mut Ui, p: PointF, time: u32| {
        for pressed in [true, false] {
            ui.handle_event(WindowEvent::PointerButton {
                position: p,
                button: PointerButton::Primary,
                pressed,
                modifiers: m,
                time,
            });
        }
        ui.update();
    };
    click(&mut ui, PointF::new(r.x as f32 + 10.0, r.y as f32 + 7.0), 0);
    assert_eq!(clicks.get(), 1, "hit testing follows the scroll offset");
    assert_eq!(down_at.get(), (10.0, 7.0), "handlers get local coordinates");

    // The place the button had before scrolling is now empty.
    let old = PointF::new(before.x as f32 + 10.0, 300.0);
    click(&mut ui, old, 1000);
    assert_eq!(clicks.get(), 1);

    // A click in a text input lands at the right offset, and the IME hears window coordinates.
    let f = field.rect().unwrap();
    let _ = ui.take_requests();
    click(
        &mut ui,
        PointF::new(f.x as f32 + 1.0, f.y as f32 + 5.0),
        2000,
    );
    let ime = ui
        .take_requests()
        .into_iter()
        .rev()
        .find_map(|r| match r {
            anti_webui_core::platform::PlatformRequest::SetIme(Some(s)) => Some(s),
            _ => None,
        })
        .expect("the input asked for the IME");
    assert!(
        f.intersects(&ime.cursor_rect),
        "{:?} should be inside {f:?}",
        ime.cursor_rect
    );
    assert_eq!(ime.cursor, 0, "clicked at the start of the text");

    // Scrolling moves the caret on screen, so the IME is told again.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(100.0, 200.0),
        delta_x: 0.0,
        delta_y: 30.0,
        modifiers: m,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let moved = ui
        .take_requests()
        .into_iter()
        .rev()
        .find_map(|r| match r {
            anti_webui_core::platform::PlatformRequest::SetIme(Some(s)) => Some(s),
            _ => None,
        })
        .expect("the IME hears about the scrolled caret");
    assert_eq!(moved.cursor_rect.y, ime.cursor_rect.y - 30);

    // Content scrolled out of the scroller can't be hit through what is above it.
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(100.0, 200.0),
        delta_x: 0.0,
        delta_y: 700.0,
        modifiers: m,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let a = above.rect().unwrap();
    let hidden = button.rect().unwrap();
    assert!(
        hidden.bottom() < view_rect.y,
        "{hidden:?} went above the view"
    );
    assert!(hidden.y < a.bottom());
    let at = PointF::new(hidden.x as f32 + 5.0, a.y as f32 + 5.0);
    if hidden.y <= a.y + 5 && hidden.bottom() > a.y + 5 {
        click(&mut ui, at, 3000);
        assert_eq!(clicks.get(), 1, "clipped content was hit");
    }

    // The scrollbar thumb drags with window coordinates, and past the end it clamps.
    let x = view_rect.right() as f32 - 3.0;
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, 200.0),
        modifiers: m,
    });
    let scroll = || with_doc(|d| d.get(view_ref.get().unwrap()).unwrap().scroll.y);
    let at_start = scroll();
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, view_rect.y as f32 + 200.0),
        button: PointerButton::Primary,
        pressed: true,
        modifiers: m,
        time: 4000,
    });
    ui.handle_event(WindowEvent::PointerMoved {
        position: PointF::new(x, view_rect.y as f32 + 170.0),
        modifiers: m,
    });
    ui.update();
    assert!(
        scroll() < at_start,
        "{} should be below {at_start}",
        scroll()
    );
    ui.handle_event(WindowEvent::PointerButton {
        position: PointF::new(x, view_rect.y as f32 + 170.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: m,
        time: 4100,
    });
}

/// Selection works across the scroller, and copies what was selected.
#[test]
fn selecting_scrolled_text() {
    let view_ref = node_ref();
    let a = node_ref();
    let b = node_ref();
    let mut ui = Ui::new(move || {
        div()
            .h(pct(100.0))
            .overflow_y_scroll()
            .node_ref(view_ref)
            .child(div().h(600.0))
            .child(div().node_ref(a).child(text("first selectable line")))
            .child(div().node_ref(b).child(text("second selectable line")))
            .child(div().h(600.0))
    });
    ui.handle_event(WindowEvent::Resized {
        size: SizeI::new(500, 300),
        scale: Scale(120),
    });
    ui.handle_event(WindowEvent::Focused(true));
    ui.update();
    let m = Modifiers::default();
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(100.0, 100.0),
        delta_x: 0.0,
        delta_y: 550.0,
        modifiers: m,
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
    let (ra, rb) = (a.rect().unwrap(), b.rect().unwrap());
    let start = PointF::new(ra.x as f32 + 1.0, ra.y as f32 + 5.0);
    let end = PointF::new(ra.x as f32 + 400.0, rb.y as f32 + 5.0);
    ui.handle_event(WindowEvent::PointerButton {
        position: start,
        button: PointerButton::Primary,
        pressed: true,
        modifiers: m,
        time: 0,
    });
    ui.handle_event(WindowEvent::PointerMoved {
        position: end,
        modifiers: m,
    });
    ui.handle_event(WindowEvent::PointerButton {
        position: end,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: m,
        time: 50,
    });
    ui.update();
    let selected = with_doc(|d| {
        anti_webui_core::text::selection::selected_text(d, &d.state.selection.unwrap())
    });
    assert_eq!(selected, "first selectable line\nsecond selectable line");
}
