//! Cards, list items, dialogs, sheets and the carousel. See `docs/design/20-md3e-components.md`.

mod common;

use anti_webui::core::event::NamedKey;
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    AlertDialog, CardKind, ListItemStyle, alert_dialog, bottom_sheet, carousel, clickable_card,
    dialog, divider, elevated_card, filled_card, list_item, multi_browse_keylines,
    navigation_drawer, outlined_card, segmented_list, side_sheet, styled_list_item,
    vertical_divider,
};
use common::*;

#[test]
fn cards_have_the_colors_and_corners_of_their_kind() {
    let mut ui = start_ui(|| {
        div()
            .child(filled_card().w(100.0).h(50.0))
            .child(elevated_card().w(100.0).h(50.0))
            .child(outlined_card().w(100.0).h(50.0))
    });
    let scene = settle(&mut ui);
    let filled = colored(&scene, C.surface_container_highest)
        .pop()
        .expect("filled");
    assert_eq!(filled.radii, [12.0; 4]);
    assert_eq!(colored(&scene, C.surface_container_low).len(), 1);
    let outlined = colored(&scene, C.surface).pop().expect("outlined");
    assert_eq!(outlined.border_color, C.outline_variant.snapped());
    assert_eq!(outlined.border_widths, [1; 4]);
}

#[test]
fn a_clickable_card_rises_under_the_pointer() {
    let clicks = signal(0);
    let mut ui = start_ui(move || {
        div().child(
            clickable_card(CardKind::Filled)
                .w(100.0)
                .h(50.0)
                .on_click(move |_| clicks.update(|c| *c += 1)),
        )
    });
    let shadows = |scene: &anti_webui::core::scene::Scene| {
        scene
            .primitives
            .iter()
            .filter(|p| matches!(p, anti_webui::core::scene::Primitive::Shadow(_)))
            .count()
    };
    assert_eq!(shadows(&settle(&mut ui)), 0);
    pointer_at(&mut ui, 50.0, 25.0);
    assert_eq!(shadows(&settle(&mut ui)), 2);
    click(&mut ui, 50.0, 25.0);
    assert_eq!(clicks.get(), 1);
}

#[test]
fn dividers_are_thin() {
    let mut ui = start_ui(|| {
        div()
            .w(200.0)
            .child(divider())
            .child(div().flex().h(30.0).child(vertical_divider()))
    });
    let scene = settle(&mut ui);
    let lines = colored(&scene, C.outline_variant);
    let sizes: Vec<_> = lines
        .iter()
        .map(|q| (q.rect.width, q.rect.height))
        .collect();
    assert_eq!(sizes, [(200, 1), (1, 30)]);
}

#[test]
fn a_list_item_is_as_high_as_its_lines() {
    let place = [node_ref(), node_ref(), node_ref()];
    let mut ui = start_ui(move || {
        div()
            .w(300.0)
            .child(list_item("One").node_ref(place[0]))
            .child(
                styled_list_item("Two", ListItemStyle::new().supporting("More")).node_ref(place[1]),
            )
            .child(
                styled_list_item(
                    "Three",
                    ListItemStyle::new().overline("Over").supporting("More"),
                )
                .node_ref(place[2]),
            )
    });
    settle(&mut ui);
    let heights: Vec<_> = place.iter().map(|p| p.rect().unwrap().height).collect();
    assert_eq!(heights, [56, 72, 88]);
}

#[test]
fn a_list_item_changes_shape_with_what_happens_to_it() {
    let on = signal(false);
    let mut ui = start_ui(move || {
        div().w(300.0).child(styled_list_item(
            "Item",
            ListItemStyle::new().selected_when(move || on.get()),
        ))
    });
    let radii = |ui: &mut _| settle(ui);
    let scene = radii(&mut ui);
    assert!(colored(&scene, C.secondary_container).is_empty());
    pointer_at(&mut ui, 100.0, 20.0);
    settle(&mut ui);
    pointer_at(&mut ui, 900.0, 700.0);
    on.set(true);
    let scene = settle(&mut ui);
    let item = colored(&scene, C.secondary_container)
        .pop()
        .expect("selected");
    assert_eq!(item.radii, [16.0; 4]);
}

#[test]
fn a_segmented_list_is_rounded_on_the_outside() {
    let mut ui = start_ui(|| {
        div()
            .w(300.0)
            .child(segmented_list(3, |i, style: ListItemStyle| {
                styled_list_item(format!("{i}"), style.selected_when(|| true)).h(40.0)
            }))
    });
    let scene = settle(&mut ui);
    let items = colored(&scene, C.secondary_container);
    assert_eq!(items.len(), 3);
    assert_eq!(items[1].rect.y - items[0].rect.bottom(), 2);
}

#[test]
fn a_dialog_comes_with_a_scrim_and_goes_by_itself() {
    let open = signal(false);
    let dismissed = signal(0);
    let mut ui = start_ui(move || {
        div().size_full().child(dialog(
            move || open.get(),
            move || {
                dismissed.update(|d| *d += 1);
                open.set(false);
            },
            || div().w(300.0).h(100.0),
        ))
    });
    let scrim = C.scrim.with_alpha(0.32);
    assert!(colored(&settle(&mut ui), scrim).is_empty());
    open.set(true);
    let scene = settle(&mut ui);
    let dark = colored(&scene, scrim).pop().expect("the scrim");
    assert_eq!((dark.rect.width, dark.rect.height), (1000, 800));
    let shown = colored(&scene, C.surface_container_high)
        .pop()
        .expect("the dialog");
    assert_eq!(shown.radii, [28.0; 4]);
    assert_eq!(shown.rect.x + shown.rect.width / 2, 500);
    assert_eq!(shown.rect.y + shown.rect.height / 2, 400);
    // A click inside doesn't close it, a click outside does.
    click(&mut ui, 500.0, 400.0);
    assert_eq!(dismissed.get(), 0);
    click(&mut ui, 10.0, 10.0);
    assert_eq!(dismissed.get(), 1);
    let scene = settle(&mut ui);
    assert!(colored(&scene, scrim).is_empty());
    assert!(colored(&scene, C.surface_container_high).is_empty());
    assert_at_rest(&mut ui);
}

#[test]
fn escape_closes_a_dialog() {
    let open = signal(true);
    let mut ui = start_ui(move || {
        div().size_full().child(dialog(
            move || open.get(),
            move || open.set(false),
            || div().w(300.0).h(100.0),
        ))
    });
    settle(&mut ui);
    key(&mut ui, NamedKey::Escape);
    assert!(!open.get());
}

#[test]
fn tab_stays_in_a_dialog_and_the_focus_comes_back() {
    let open = signal(false);
    let focused = signal(0);
    let item = move |i: i32| {
        div()
            .w(40.0)
            .h(40.0)
            .focusable()
            .on_focus_change(move |on| {
                if on {
                    focused.set(i);
                }
            })
    };
    let mut ui = start_ui(move || {
        div()
            .size_full()
            .child(item(1))
            .child(item(4))
            .child(dialog(
                move || open.get(),
                move || open.set(false),
                move || div().child(item(2)).child(item(3)),
            ))
    });
    key(&mut ui, NamedKey::Tab);
    assert_eq!(focused.get(), 1);
    open.set(true);
    settle(&mut ui);
    for _ in 0..5 {
        key(&mut ui, NamedKey::Tab);
        assert!(![1, 4].contains(&focused.get()), "stayed inside");
    }
    open.set(false);
    settle(&mut ui);
    assert_eq!(focused.get(), 1, "back where it was");
    key(&mut ui, NamedKey::Tab);
    assert_eq!(focused.get(), 4);
}

#[test]
fn an_alert_dialog_has_what_it_was_given_and_its_buttons_work() {
    let open = signal(true);
    let yes = signal(0);
    let mut ui = start_ui(move || {
        div().size_full().child(alert_dialog(
            move || open.get(),
            move || open.set(false),
            AlertDialog::new()
                .title("Title")
                .text("Text")
                .confirm(move || {
                    div()
                        .w(80.0)
                        .h(40.0)
                        .on_click(move |_| yes.update(|y| *y += 1))
                }),
        ))
    });
    let scene = settle(&mut ui);
    let shown = colored(&scene, C.surface_container_high)
        .pop()
        .expect("the dialog");
    assert!(shown.rect.width >= 280);
    // Bottom right, inside of the padding.
    click(
        &mut ui,
        (shown.rect.right() - 24 - 40) as f32,
        (shown.rect.bottom() - 24 - 20) as f32,
    );
    assert_eq!(yes.get(), 1);
}

#[test]
fn a_bottom_sheet_comes_up_and_goes_when_dragged_down() {
    let open = signal(false);
    let mut ui = start_ui(move || {
        div().size_full().child(bottom_sheet(
            move || open.get(),
            move || open.set(false),
            || div().h(200.0),
        ))
    });
    open.set(true);
    let scene = settle(&mut ui);
    let sheet = colored(&scene, C.surface_container_low)
        .pop()
        .expect("the sheet");
    assert_eq!(sheet.rect.width, 640);
    assert_eq!(sheet.rect.x, 180);
    // 48 of handle and 200 of content above the bottom.
    assert_eq!(sheet.rect.y, 800 - 48 - 200);
    assert_eq!(sheet.radii[0], 28.0);
    assert_eq!(sheet.radii[2], 0.0);
    let on_handle = (500.0, (sheet.rect.y + 24) as f32);
    pointer_at(&mut ui, on_handle.0, on_handle.1);
    pointer(&mut ui, on_handle.0, on_handle.1, true);
    pointer_at(&mut ui, on_handle.0, on_handle.1 + 30.0);
    pointer(&mut ui, on_handle.0, on_handle.1 + 30.0, false);
    assert!(open.get() || !open.get());
    open.set(true);
    let scene = settle(&mut ui);
    assert_eq!(
        colored(&scene, C.surface_container_low)
            .pop()
            .unwrap()
            .rect
            .y,
        sheet.rect.y,
        "a little goes back"
    );
    pointer(&mut ui, on_handle.0, on_handle.1, true);
    pointer_at(&mut ui, on_handle.0, on_handle.1 + 120.0);
    pointer(&mut ui, on_handle.0, on_handle.1 + 120.0, false);
    assert!(!open.get(), "far enough closes it");
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.surface_container_low).is_empty());
}

#[test]
fn drawers_and_side_sheets_come_from_their_edges() {
    let left = signal(true);
    let mut ui = start_ui(move || {
        div().size_full().child(navigation_drawer(
            move || left.get(),
            move || left.set(false),
            || div().w(280.0),
        ))
    });
    let scene = settle(&mut ui);
    let sheet = colored(&scene, C.surface_container_low)
        .pop()
        .expect("the drawer");
    assert_eq!(sheet.rect.right(), 280);
    assert_eq!((sheet.rect.y, sheet.rect.height), (0, 800));
    assert_eq!(sheet.radii[1], 16.0);
    assert_eq!(sheet.radii[0], 0.0);
    click(&mut ui, 900.0, 400.0);
    assert!(!left.get());
    let right = signal(true);
    let mut ui = start_ui(move || {
        div().size_full().child(side_sheet(
            move || right.get(),
            move || right.set(false),
            || div().w(300.0),
        ))
    });
    let scene = settle(&mut ui);
    let sheet = colored(&scene, C.surface_container_low)
        .pop()
        .expect("the sheet");
    assert_eq!(sheet.rect.x, 700);
    assert_eq!(sheet.radii[0], 16.0);
}

#[test]
fn a_carousel_fits_its_keylines_exactly() {
    for (main, preferred) in [
        (720.0, 240.0),
        (360.0, 186.0),
        (1000.0, 300.0),
        (500.0, 500.0),
    ] {
        let lines = multi_browse_keylines(main, preferred, 10);
        let total: f32 = lines.sizes.iter().sum::<f32>() + 8.0 * (lines.sizes.len() - 1) as f32;
        assert!((total - main).abs() < 0.01, "{main} {preferred}: {lines:?}");
        assert!(
            (lines.large - preferred).abs() < preferred * 0.5,
            "{lines:?}"
        );
        assert!(
            lines.sizes.windows(2).all(|w| w[0] >= w[1]),
            "getting smaller: {lines:?}"
        );
    }
    let few = multi_browse_keylines(720.0, 240.0, 1);
    assert_eq!(few.sizes, [720.0]);
}

#[test]
fn a_carousel_lays_out_its_items_and_scrolls_by_dragging() {
    let colors = [
        rgb(0x101010),
        rgb(0x202020),
        rgb(0x303030),
        rgb(0x404040),
        rgb(0x505050),
    ];
    let mut ui = start_ui(move || {
        div()
            .w(600.0)
            .child(carousel(5, 200.0, move |i| div().size_full().bg(colors[i])))
    });
    let scene = settle(&mut ui);
    let first = colored(&scene, colors[0]).pop().expect("the first item");
    assert_eq!(first.rect.x, 0);
    pointer_at(&mut ui, 400.0, 100.0);
    pointer(&mut ui, 400.0, 100.0, true);
    pointer_at(&mut ui, 300.0, 100.0);
    run(&mut ui, 16);
    let moved = colored(&run(&mut ui, 16), colors[0]);
    assert!(moved.iter().all(|q| q.rect.x < 0));
    pointer(&mut ui, 300.0, 100.0, false);
    assert_at_rest(&mut ui);
}
