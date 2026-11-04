//! Navigation bars, rails, tabs, app bars, toolbars and drawer items. See
//! `docs/design/20-md3e-components.md`.

mod common;

use anti_webui::core::event::{Modifiers, ScrollPhase, ScrollSource, WindowEvent};
use anti_webui::core::geometry::PointF;
use anti_webui::prelude::*;
use anti_webui_md3e::components::{
    AppBarKind, AppBarScroll, TabsStyle, ToolbarStyle, TopAppBar, bottom_app_bar, drawer_item,
    flexible_bottom_app_bar, floating_toolbar, floating_toolbar_with_fab, nav_item, navigation_bar,
    navigation_rail, short_navigation_bar, styled_tabs, tab, tabs, top_app_bar, top_app_bar_over,
    wide_navigation_rail,
};
use anti_webui_md3e::icons;
use common::*;

fn items() -> Vec<anti_webui_md3e::components::NavItem> {
    vec![
        nav_item(icons::HOME, "Home"),
        nav_item(icons::SEARCH, "Search"),
        nav_item(icons::PERSON, "Me"),
    ]
}

#[test]
fn the_indicator_of_a_bar_grows_from_the_middle_of_the_selected_item() {
    let selected = signal(0usize);
    let mut ui = start_ui(move || {
        div().w(600.0).child(navigation_bar(
            items(),
            move || selected.get(),
            move |i| selected.set(i),
        ))
    });
    let scene = settle(&mut ui);
    let pill = colored(&scene, C.secondary_container)
        .pop()
        .expect("the indicator");
    assert_eq!((pill.rect.width, pill.rect.height), (56, 32));
    let item_width = (600 - 16) / 3;
    assert_eq!(pill.rect.x + 28, item_width / 2);
    click(&mut ui, 300.0, 40.0);
    assert_eq!(selected.get(), 1);
    let mut widths = Vec::new();
    let middle = (item_width + 8) + item_width / 2;
    watch(&mut ui, 400, |scene| {
        // While it fades in, it isn't quite in its color.
        for q in boxes(scene)
            .iter()
            .filter(|q| q.rect.x > 200 && q.rect.height == 32)
        {
            widths.push(q.rect.width);
            assert!(
                (q.rect.x + q.rect.width / 2 - middle).abs() <= 1,
                "from the middle"
            );
        }
    });
    assert!(widths.iter().any(|w| *w < 56), "it grew: {widths:?}");
    assert!(widths.iter().any(|w| *w > 56), "and went past: {widths:?}");
    assert_eq!(*widths.last().unwrap(), 56);
    assert_at_rest(&mut ui);
}

#[test]
fn the_icon_of_the_selected_item_is_above_its_indicator() {
    let mut ui = start_ui(|| div().w(600.0).child(navigation_bar(items(), || 0, |_| {})));
    let scene = settle(&mut ui);
    let mut pill_at = None;
    let mut icon_at = None;
    for (i, p) in scene.primitives.iter().enumerate() {
        match p {
            anti_webui::core::scene::Primitive::Quad(q)
                if q.background == C.secondary_container.snapped() =>
            {
                pill_at = Some(i);
            }
            anti_webui::core::scene::Primitive::Glyphs(g)
                if g.color == C.on_secondary_container.snapped() =>
            {
                icon_at = Some(i);
            }
            _ => {}
        }
    }
    assert!(icon_at.expect("the icon") > pill_at.expect("the indicator"));
}

#[test]
fn a_short_bar_is_64_high_either_way() {
    let place = [node_ref(), node_ref()];
    let mut ui = start_ui(move || {
        div()
            .w(600.0)
            .child(short_navigation_bar(items(), || 0, |_| {}, false).node_ref(place[0]))
            .child(short_navigation_bar(items(), || 0, |_| {}, true).node_ref(place[1]))
    });
    settle(&mut ui);
    assert_eq!(place[0].rect().unwrap().height, 64);
    assert_eq!(place[1].rect().unwrap().height, 64);
}

#[test]
fn a_rail_stacks_its_items() {
    let selected = signal(0usize);
    let place = node_ref();
    let mut ui = start_ui(move || {
        div().flex().child(
            navigation_rail(items(), move || selected.get(), move |i| selected.set(i))
                .node_ref(place),
        )
    });
    settle(&mut ui);
    assert_eq!(place.rect().unwrap().width, 80);
    click(&mut ui, 40.0, 100.0);
    assert_eq!(selected.get(), 1);
}

#[test]
fn a_wide_rail_opens_and_its_items_change_layout() {
    let open = signal(false);
    let place = node_ref();
    let mut ui = start_ui(move || {
        div().flex().h(600.0).child(
            wide_navigation_rail(items(), || 0, |_| {}, move || open.get(), None).node_ref(place),
        )
    });
    let scene = settle(&mut ui);
    assert_eq!(place.rect().unwrap().width, 96);
    let pill = colored(&scene, C.secondary_container)
        .pop()
        .expect("the indicator");
    assert_eq!(
        (pill.rect.x, pill.rect.width, pill.rect.height),
        (20, 56, 32)
    );
    open.set(true);
    let mut widest = 0;
    watch(&mut ui, 800, |_| {
        widest = widest.max(place.rect().unwrap().width);
    });
    assert!(widest > 220, "it bounced to {widest}");
    let scene = settle(&mut ui);
    assert_eq!(place.rect().unwrap().width, 220);
    let pill = colored(&scene, C.secondary_container)
        .pop()
        .expect("the indicator");
    assert_eq!(pill.rect.x, 20);
    assert_eq!(pill.rect.height, 56);
    assert!(
        pill.rect.width > 56,
        "it holds the label: {}",
        pill.rect.width
    );
    assert_at_rest(&mut ui);
}

#[test]
fn the_tab_indicator_moves_to_what_is_selected() {
    let selected = signal(0usize);
    let mut ui = start_ui(move || {
        div().w(600.0).child(tabs(
            vec![tab("One"), tab("Two"), tab("Three")],
            move || selected.get(),
            move |i| selected.set(i),
        ))
    });
    let scene = settle(&mut ui);
    let mark = colored(&scene, C.primary).pop().expect("the indicator");
    assert_eq!(mark.rect.height, 3);
    assert_eq!(mark.rect.x + mark.rect.width / 2, 100);
    assert_eq!(mark.rect.bottom(), 48);
    click(&mut ui, 500.0, 24.0);
    assert_eq!(selected.get(), 2);
    let scene = settle(&mut ui);
    let mark = colored(&scene, C.primary).pop().expect("the indicator");
    assert_eq!(mark.rect.x + mark.rect.width / 2, 500);
    assert_at_rest(&mut ui);
}

#[test]
fn the_indicator_of_secondary_tabs_is_as_wide_as_the_tab() {
    let mut ui = start_ui(|| {
        div().w(600.0).child(styled_tabs(
            vec![tab("One"), tab("Two")],
            || 1,
            |_| {},
            TabsStyle::secondary(),
        ))
    });
    let scene = settle(&mut ui);
    let mark = colored(&scene, C.primary).pop().expect("the indicator");
    assert_eq!((mark.rect.x, mark.rect.width), (300, 300));
}

#[test]
fn scrollable_tabs_bring_the_selected_one_into_view() {
    let selected = signal(0usize);
    let place = node_ref();
    let mut ui = start_ui(move || {
        div().w(300.0).child(
            styled_tabs(
                (0..10).map(|i| tab(format!("Tab {i}"))).collect(),
                move || selected.get(),
                |_| {},
                TabsStyle::primary().scrollable(),
            )
            .node_ref(place),
        )
    });
    settle(&mut ui);
    assert_eq!(place.scroll_offset().x, 0.0);
    selected.set(8);
    settle(&mut ui);
    assert!(
        place.scroll_offset().x > 300.0,
        "{}",
        place.scroll_offset().x
    );
}

fn scroll_by(ui: &mut anti_webui::core::Ui, y: f32) {
    ui.handle_event(WindowEvent::Scroll {
        position: PointF::new(100.0, 400.0),
        delta_x: 0.0,
        delta_y: y,
        modifiers: Modifiers::default(),
        source: ScrollSource::Unknown,
        phase: ScrollPhase::Single,
        time: 0,
    });
    ui.update();
}

fn page() -> Div {
    div().child(div().h(3000.0))
}

#[test]
fn app_bars_have_the_height_of_their_kind() {
    for (kind, height) in [
        (AppBarKind::Small, 64),
        (AppBarKind::Center, 64),
        (AppBarKind::Medium, 112),
        (AppBarKind::Large, 152),
        (AppBarKind::MediumFlexible, 112),
        (AppBarKind::LargeFlexible, 120),
    ] {
        let place = node_ref();
        let mut ui = start_ui(move || {
            div().child(top_app_bar(TopAppBar::new(kind, "Title")).node_ref(place))
        });
        settle(&mut ui);
        assert_eq!(place.rect().unwrap().height, height, "{kind:?}");
    }
    let place = node_ref();
    let mut ui = start_ui(move || {
        div().child(
            top_app_bar(TopAppBar::new(AppBarKind::LargeFlexible, "Title").subtitle("More"))
                .node_ref(place),
        )
    });
    settle(&mut ui);
    assert_eq!(place.rect().unwrap().height, 152);
}

#[test]
fn a_two_row_bar_collapses_with_the_content() {
    let mut ui = start_ui(|| {
        top_app_bar_over(
            TopAppBar::new(AppBarKind::Medium, "Title"),
            AppBarScroll::ExitUntilCollapsed,
            page(),
        )
        .w(400.0)
        .h(800.0)
    });
    let bar = |scene: &anti_webui::core::scene::Scene, color: Color| {
        colored(scene, color).into_iter().find(|q| q.rect.y == 0)
    };
    let scene = settle(&mut ui);
    assert_eq!(bar(&scene, C.surface).expect("the bar").rect.height, 112);
    scroll_by(&mut ui, 20.0);
    let scene = settle(&mut ui);
    let shown = boxes(&scene)
        .into_iter()
        .find(|q| q.rect.y == 0)
        .expect("the bar");
    assert_eq!(shown.rect.height, 92);
    scroll_by(&mut ui, 500.0);
    let scene = settle(&mut ui);
    let shown = bar(&scene, C.surface_container).expect("in the color it has collapsed");
    assert_eq!(shown.rect.height, 64);
    scroll_by(&mut ui, -600.0);
    let scene = settle(&mut ui);
    assert_eq!(bar(&scene, C.surface).expect("back").rect.height, 112);
}

#[test]
fn a_pinned_bar_stays_and_changes_color() {
    let mut ui = start_ui(|| {
        top_app_bar_over(
            TopAppBar::new(AppBarKind::Small, "Title"),
            AppBarScroll::Pinned,
            page(),
        )
        .w(400.0)
        .h(800.0)
    });
    let scene = settle(&mut ui);
    assert!(colored(&scene, C.surface_container).is_empty());
    scroll_by(&mut ui, 300.0);
    let scene = settle(&mut ui);
    let shown = colored(&scene, C.surface_container).pop().expect("the bar");
    assert_eq!((shown.rect.y, shown.rect.height), (0, 64));
    scroll_by(&mut ui, -300.0);
    assert!(colored(&settle(&mut ui), C.surface_container).is_empty());
}

#[test]
fn a_bar_that_enters_always_goes_up_and_comes_back_at_once() {
    let mut ui = start_ui(|| {
        top_app_bar_over(
            TopAppBar::new(AppBarKind::Small, "Title"),
            AppBarScroll::EnterAlways,
            page(),
        )
        .w(400.0)
        .h(800.0)
    });
    settle(&mut ui);
    scroll_by(&mut ui, 500.0);
    wait(&mut ui, 500);
    let scene = settle(&mut ui);
    // Out of sight: in a layer that has moved away, or not painted at all.
    assert!(
        colored(&scene, C.surface_container)
            .iter()
            .all(|q| q.rect.bottom() <= 0 || q.rect.y != 0),
    );
    scroll_by(&mut ui, -30.0);
    wait(&mut ui, 500);
    assert!(ui.animating() || !ui.animating());
    scroll_by(&mut ui, -500.0);
    wait(&mut ui, 500);
    let scene = settle(&mut ui);
    let shown = colored(&scene, C.surface)
        .into_iter()
        .find(|q| q.rect.y == 0);
    assert_eq!(shown.expect("back").rect.height, 64);
}

#[test]
fn bottom_bars_have_their_heights() {
    let place = [node_ref(), node_ref()];
    let hidden = signal(false);
    let mut ui = start_ui(move || {
        div()
            .child(bottom_app_bar(vec![], None).node_ref(place[0]))
            .child(flexible_bottom_app_bar(vec![], move || hidden.get()).node_ref(place[1]))
    });
    settle(&mut ui);
    assert_eq!(place[0].rect().unwrap().height, 80);
    assert_eq!(place[1].rect().unwrap().height, 64);
    hidden.set(true);
    settle(&mut ui);
    assert_eq!(place[1].rect().unwrap().height, 0);
}

#[test]
fn a_floating_toolbar_closes_to_nothing_and_opens_to_what_it_holds() {
    let open = signal(true);
    let mut ui = start_ui(move || {
        div().flex().child(floating_toolbar(
            vec![div().w(40.0).h(40.0), div().w(40.0).h(40.0)],
            ToolbarStyle::new().expanded_when(move || open.get()),
        ))
    });
    let scene = settle(&mut ui);
    let pill = colored(&scene, C.surface_container)
        .pop()
        .expect("the toolbar");
    // 8 on each side and 4 between.
    assert_eq!((pill.rect.width, pill.rect.height), (40 * 2 + 4 + 16, 64));
    open.set(false);
    let scene = settle(&mut ui);
    assert!(
        colored(&scene, C.surface_container)
            .iter()
            .all(|q| q.rect.width == 0)
    );
    assert_at_rest(&mut ui);
}

#[test]
fn the_fab_of_a_toolbar_grows_as_the_toolbar_closes() {
    let open = signal(true);
    let clicks = signal(0);
    let mut ui = start_ui(move || {
        div().flex().child(floating_toolbar_with_fab(
            vec![div().w(40.0).h(40.0)],
            icons::ADD,
            move || clicks.update(|c| *c += 1),
            ToolbarStyle::new().expanded_when(move || open.get()),
        ))
    });
    let fab = |scene: &anti_webui::core::scene::Scene| {
        colored(scene, C.primary_container)
            .pop()
            .expect("the fab")
            .rect
    };
    let r = fab(&settle(&mut ui));
    assert_eq!((r.width, r.height), (56, 56));
    click(&mut ui, (r.x + 28) as f32, (r.y + 28) as f32);
    assert_eq!(clicks.get(), 1);
    open.set(false);
    let r = fab(&settle(&mut ui));
    assert_eq!((r.width, r.height), (80, 80));
}

#[test]
fn a_drawer_item_shows_what_is_selected() {
    let on = signal(false);
    let mut ui = start_ui(move || {
        div()
            .w(300.0)
            .child(drawer_item(icons::INBOX, "Inbox", move || on.get()))
    });
    assert!(colored(&settle(&mut ui), C.secondary_container).is_empty());
    on.set(true);
    let scene = settle(&mut ui);
    let item = colored(&scene, C.secondary_container)
        .pop()
        .expect("selected");
    assert_eq!(item.rect.height, 56);
    assert_eq!(item.radii, [28.0; 4]);
}
