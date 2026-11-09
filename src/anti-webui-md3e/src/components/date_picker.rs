//! Date pickers. Sizes, colors and timing are those of Jetpack Compose. See
//! `docs/design/20-md3e-components.md`.

use super::card::divider;
use super::icon::sized_icon;
use super::modal::dialog;
use super::surface::{Ink, When, focus_ring};
use crate::color::color_scheme;
use crate::icons;
use crate::motion::{default_effects, default_spatial, fast_spatial};
use crate::typography::TextStyle;
use anti_webui::prelude::*;
use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CELL: f32 = 48.0;
const DAY: f32 = 40.0;
const PICKER_WIDTH: f32 = 360.0;
const GRID_PAD: f32 = 12.0;
const MONTH_NAMES: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const WEEKDAYS: [&str; 7] = ["S", "M", "T", "W", "T", "F", "S"];

/// A day of the calendar.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Date {
    pub year: i32,
    /// From 1 to 12.
    pub month: u32,
    /// From 1.
    pub day: u32,
}

impl Date {
    pub const fn new(year: i32, month: u32, day: u32) -> Self {
        Self { year, month, day }
    }

    /// Today, in UTC.
    pub fn today() -> Self {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()) as i64;
        from_days(seconds.div_euclid(86_400))
    }

    /// From Sunday as 0 to Saturday as 6.
    pub fn weekday(self) -> u32 {
        (to_days(self) + 4).rem_euclid(7) as u32
    }
}

/// Days since 1970-01-01. The algorithms of Howard Hinnant's `chrono`.
fn to_days(date: Date) -> i64 {
    let y = date.year as i64 - (date.month <= 2) as i64;
    let era = y.div_euclid(400);
    let year_of_era = y.rem_euclid(400);
    let month = date.month as i64;
    let day_of_year =
        (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + date.day as i64 - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn from_days(days: i64) -> Date {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = year_of_era + era * 400 + (month <= 2) as i64;
    Date::new(year as i32, month, day)
}

fn days_in_month(year: i32, month: u32) -> u32 {
    let (next_year, next) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    (to_days(Date::new(next_year, next, 1)) - to_days(Date::new(year, month, 1))) as u32
}

/// A month as one number, to count in.
fn index(year: i32, month: u32) -> i32 {
    year * 12 + month as i32 - 1
}

fn from_index(i: i32) -> (i32, u32) {
    (i.div_euclid(12), i.rem_euclid(12) as u32 + 1)
}

/// How a date picker looks and which dates it takes.
#[derive(Clone)]
pub struct DatePickerStyle {
    title: String,
    header: bool,
    years: std::ops::RangeInclusive<i32>,
    allowed: Option<Rc<dyn Fn(Date) -> bool>>,
    cascade: Option<Duration>,
}

impl Default for DatePickerStyle {
    fn default() -> Self {
        Self {
            title: "Select date".to_string(),
            header: true,
            years: 1900..=2100,
            allowed: None,
            cascade: None,
        }
    }
}

impl DatePickerStyle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Without the title and the chosen date above the month, like Compose's `title = null` and
    /// `headline = null`: a calendar that shows the month, for one that is always open.
    pub fn no_header(mut self) -> Self {
        self.header = false;
        self
    }

    pub fn years(mut self, years: std::ops::RangeInclusive<i32>) -> Self {
        self.years = years;
        self
    }

    /// Which dates can be chosen.
    pub fn allowed(mut self, f: impl Fn(Date) -> bool + 'static) -> Self {
        self.allowed = Some(Rc::new(f));
        self
    }

    /// The days of the week and of the month come in one after another when it first shows,
    /// `step` apart, along the diagonals from the top left: for a calendar that comes in with what
    /// it is in. The months it goes to after that don't.
    pub fn cascade(mut self, step: Duration) -> Self {
        self.cascade = Some(step);
        self
    }
}

/// `node` coming in, growing as it fades in, `order` steps after it is first laid out. Not
/// before: a timer set when it is made could be over before a new window is on the screen.
fn cascading(node: Div, order: u32, step: Duration) -> Div {
    let go = signal(false);
    let shown = follow(move || if go.get() { 1.0f32 } else { 0.0 }, fast_spatial());
    let started = Cell::new(false);
    node.on_resized(move |_, _| {
        if !started.replace(true) {
            set_timeout(step * order, move || {
                if go.is_alive() {
                    go.set(true);
                }
            });
        }
    })
    .dyn_style(move |s| {
        let t = shown.get();
        s.opacity(t.clamp(0.0, 1.0)).scale(0.6 + 0.4 * t)
    })
}

fn day_cell(
    date: Date,
    selected: Rc<dyn Fn() -> Option<Date>>,
    on_select: Rc<dyn Fn(Date)>,
    today: Date,
    allowed: bool,
) -> Div {
    let is_today = date == today;
    let on: When = Rc::new(move || selected() == Some(date));
    let off: When = Rc::new(move || !allowed);
    let (bg_on, text_on, ring_on, layer_on) = (on.clone(), on.clone(), on.clone(), on);
    let background = follow(
        move || {
            if bg_on() {
                let c = color_scheme();
                if allowed {
                    c.primary
                } else {
                    c.primary.with_alpha(0.38)
                }
            } else {
                Color::TRANSPARENT
            }
        },
        default_effects(),
    );
    let content = follow(
        move || {
            let c = color_scheme();
            let base = if text_on() {
                c.on_primary
            } else if is_today {
                c.primary
            } else {
                c.on_surface
            };
            if allowed { base } else { base.with_alpha(0.38) }
        },
        default_effects(),
    );
    let ink = Ink::new_when(
        move |c| {
            if layer_on() {
                c.on_primary
            } else {
                c.on_surface
            }
        },
        off.clone(),
    );
    let focused = ink.focused;
    let disabled_off = off;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(CELL)
        .h(CELL)
        .child(
            ink.paint(
                ink.listen(
                    TextStyle::BODY_LARGE.apply(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(DAY)
                            .h(DAY)
                            .rounded(DAY / 2.0)
                            .select_none()
                            .focusable()
                            .cursor(if allowed {
                                CursorIcon::Pointer
                            } else {
                                CursorIcon::Default
                            })
                            .dyn_style(move |s| {
                                let c = color_scheme();
                                let s = s.bg(background.get()).text_color(content.get());
                                let s = if is_today && !ring_on() {
                                    s.border(1.0).border_color(c.primary)
                                } else {
                                    s
                                };
                                focus_ring(s, focused.get(), &c)
                            })
                            .on_click(move |_| on_select(date))
                            .child(text(date.day.to_string())),
                    ),
                ),
            )
            .disabled(move || disabled_off()),
        )
}

fn month_grid(
    month_index: i32,
    selected: Rc<dyn Fn() -> Option<Date>>,
    on_select: Rc<dyn Fn(Date)>,
    today: Date,
    allowed: Option<Rc<dyn Fn(Date) -> bool>>,
    cascade: Option<Duration>,
) -> Div {
    let (year, month) = from_index(month_index);
    let first = Date::new(year, month, 1).weekday() as usize;
    let days = days_in_month(year, month) as usize;
    let mut rows = Vec::new();
    for row in 0..6 {
        let cells: Vec<Div> = (0..7)
            .map(|column| {
                let n = row * 7 + column;
                if n < first || n >= first + days {
                    return div().w(CELL).h(CELL).flex_none();
                }
                let date = Date::new(year, month, (n - first + 1) as u32);
                let ok = allowed.as_ref().is_none_or(|f| f(date));
                let cell = day_cell(date, selected.clone(), on_select.clone(), today, ok);
                match cascade {
                    // After the days of the week.
                    Some(step) => cascading(cell, (row + 1 + column) as u32, step),
                    None => cell,
                }
            })
            .collect();
        rows.push(
            div()
                .flex()
                .flex_row()
                .justify_between()
                .flex_none()
                .children(cells),
        );
    }
    div()
        .flex()
        .flex_col()
        .h(CELL * 6.0)
        .flex_none()
        .children(rows)
}

fn plain_icon_button(symbol: &'static crate::icons::Symbol, f: impl Fn() + 'static) -> Div {
    let ink = Ink::new(|c| c.on_surface_variant, || false);
    let focused = ink.focused;
    ink.paint(
        ink.listen(
            div()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .w(40.0)
                .h(40.0)
                .rounded(20.0)
                .select_none()
                .focusable()
                .cursor(CursorIcon::Pointer)
                .dyn_style(move |s| focus_ring(s, focused.get(), &color_scheme()))
                .on_click(move |_| f())
                .child(sized_icon(symbol, 24.0)),
        ),
    )
}

/// A date picker that shows `selected`; `on_select` gets what it should become.
pub fn date_picker(
    selected: impl Fn() -> Option<Date> + 'static,
    on_select: impl Fn(Date) + 'static,
    style: DatePickerStyle,
) -> Div {
    crate::ensure_ready();
    let selected: Rc<dyn Fn() -> Option<Date>> = Rc::new(selected);
    let on_select: Rc<dyn Fn(Date)> = Rc::new(on_select);
    let DatePickerStyle {
        title,
        header: with_header,
        years,
        allowed,
        cascade,
    } = style;
    let today = Date::today();
    let start = untrack(|| selected()).unwrap_or(today);
    let month = signal(index(start.year, start.month));
    // Where the strip of months is, in months from the first: small numbers stay precise.
    let base = month.get_untracked();
    let position = follow(
        move || (month.get() - base) as f32,
        Spring::from_ratio(1500.0, 1.0),
    );
    let year_list = signal(false);
    let list_height = follow(
        move || {
            if year_list.get() {
                CELL * 7.0 - 1.0
            } else {
                0.0f32
            }
        },
        default_effects(),
    );
    let turn = follow(
        move || if year_list.get() { 180.0f32 } else { 0.0 },
        default_spatial(),
    );
    let inner_width = PICKER_WIDTH - GRID_PAD * 2.0;

    let shown = {
        let selected = selected.clone();
        move || match selected() {
            Some(d) => format!(
                "{} {}, {}",
                &MONTH_NAMES[d.month as usize - 1][..3],
                d.day,
                d.year
            ),
            None => "Selected date".to_string(),
        }
    };
    let mut header = div()
        .flex()
        .flex_col()
        .flex_none()
        .min_h(120.0)
        .justify_between()
        .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
        .child(TextStyle::LABEL_LARGE.apply(div().pl(24.0).pr(12.0).pt(16.0).child(text(title))))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .pl(24.0)
                .pr(12.0)
                .pb(12.0)
                .child(TextStyle::HEADLINE_LARGE.apply(div().child(text(shown)))),
        );
    let mut rule = divider();
    if !with_header {
        header = div();
        rule = div();
    }
    let nav = div()
        .flex()
        .flex_row()
        .flex_none()
        .items_center()
        .justify_between()
        .h(56.0)
        .px(12.0)
        .dyn_style(|s| s.text_color(color_scheme().on_surface_variant))
        .child(
            TextStyle::LABEL_LARGE.apply(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(4.0)
                    .h(40.0)
                    .px(12.0)
                    .cursor(CursorIcon::Pointer)
                    .select_none()
                    .on_click(move |_| year_list.update(|v| *v = !*v))
                    .child(text(move || {
                        let (year, m) = from_index(month.get());
                        format!("{} {}", MONTH_NAMES[m as usize - 1], year)
                    }))
                    .child(
                        sized_icon(icons::ARROW_DROP_DOWN, 18.0)
                            .dyn_style(move |s| s.rotate(turn.get())),
                    ),
            ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .dyn_style(move |s| {
                    if year_list.get() {
                        s.opacity(0.0).pointer_events_none()
                    } else {
                        s
                    }
                })
                .child(plain_icon_button(icons::CHEVRON_LEFT, move || {
                    month.update(|m| *m -= 1)
                }))
                .child(plain_icon_button(icons::CHEVRON_RIGHT, move || {
                    month.update(|m| *m += 1)
                })),
        );
    let week = div()
        .flex()
        .flex_row()
        .flex_none()
        .justify_between()
        .h(CELL)
        .px(GRID_PAD)
        .children(
            WEEKDAYS
                .iter()
                .enumerate()
                .map(|(column, day)| {
                    let cell = TextStyle::BODY_LARGE.apply(
                        div()
                            .flex()
                            .items_center()
                            .justify_center()
                            .w(CELL)
                            .h(CELL)
                            .flex_none()
                            .dyn_style(|s| s.text_color(color_scheme().on_surface))
                            .child(text(day.to_string())),
                    );
                    match cascade {
                        Some(step) => cascading(cell, column as u32, step),
                        None => cell,
                    }
                })
                .collect::<Vec<_>>(),
        );
    let visible = move || {
        let p = position.get();
        let (low, high) = (base + p.floor() as i32, base + p.ceil() as i32);
        let wanted = month.get();
        let mut months = vec![low];
        if high != low {
            months.push(high);
        }
        if !months.contains(&wanted) {
            months.push(wanted);
        }
        months.sort_unstable();
        months
    };
    let (cell_selected, cell_select, cell_allowed) =
        (selected.clone(), on_select.clone(), allowed.clone());
    // Only the month it shows first comes in one day after another.
    let first_cascade = Cell::new(cascade);
    let strip = for_each(
        visible,
        |m| *m,
        move |m| {
            month_grid(
                m,
                cell_selected.clone(),
                cell_select.clone(),
                today,
                cell_allowed.clone(),
                first_cascade.take(),
            )
            .absolute()
            .top(0.0)
            .w(inner_width)
            .dyn_style(move |s| s.left(((m - base) as f32 - position.get()) * inner_width))
        },
    );
    let chosen_year = move || from_index(month.get()).0;
    let current_year = today.year;
    let (year_selected, list_years) = (selected.clone(), years);
    let year_rows = {
        let years: Vec<i32> = list_years.collect();
        let refs: Vec<(i32, NodeRef)> = years.iter().map(|y| (*y, node_ref())).collect();
        let rows: Vec<Div> = years
            .chunks(3)
            .enumerate()
            .map(|(row, chunk)| {
                div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .justify_between()
                    .px(24.0)
                    .children(
                        chunk
                            .iter()
                            .enumerate()
                            .map(|(column, &y)| {
                                let on_year: When = {
                                    let selected = year_selected.clone();
                                    Rc::new(move || {
                                        selected().map_or(y == chosen_year(), |d| d.year == y)
                                    })
                                };
                                year_cell(
                                    y,
                                    on_year,
                                    y == current_year,
                                    refs[row * 3 + column].1,
                                    move || {
                                        month.update(|m| *m = index(y, from_index(*m).1));
                                        year_list.set(false);
                                    },
                                )
                            })
                            .collect::<Vec<_>>(),
                    )
            })
            .collect();
        effect(move || {
            if year_list.get() {
                let y = untrack(chosen_year);
                if let Some((_, r)) = refs.iter().find(|(year, _)| *year == y) {
                    r.scroll_into_view();
                }
            }
        });
        rows
    };
    let list = div()
        .absolute()
        .left(0.0)
        .right(0.0)
        .top(0.0)
        .overflow_y_scroll()
        .flex()
        .flex_col()
        .gap(16.0)
        .dyn_style(move |s| {
            let h = list_height.get().max(0.0);
            let s = s.h(h).bg(color_scheme().surface_container_high);
            if h <= 0.0 {
                s.opacity(0.0).pointer_events_none()
            } else {
                s
            }
        })
        .children(year_rows);
    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(PICKER_WIDTH)
        .select_none()
        .dyn_style(|s| s.bg(color_scheme().surface_container_high))
        .child(header)
        .child(rule)
        .child(nav)
        .child(
            div()
                .relative()
                .flex()
                .flex_col()
                .flex_none()
                .h(CELL * 7.0)
                .overflow_hidden()
                .child(week)
                .child(
                    div()
                        .relative()
                        .mx(GRID_PAD)
                        .h(CELL * 6.0)
                        .overflow_hidden()
                        .child(strip),
                )
                .child(list),
        )
}

fn year_cell(year: i32, on: When, is_current: bool, node: NodeRef, f: impl Fn() + 'static) -> Div {
    let (bg_on, text_on, ring_on, layer_on) = (on.clone(), on.clone(), on.clone(), on);
    let background = follow(
        move || {
            if bg_on() {
                color_scheme().primary
            } else {
                Color::TRANSPARENT
            }
        },
        default_effects(),
    );
    let content = follow(
        move || {
            let c = color_scheme();
            if text_on() {
                c.on_primary
            } else if is_current {
                c.primary
            } else {
                c.on_surface_variant
            }
        },
        default_effects(),
    );
    let ink = Ink::new(
        move |c| {
            if layer_on() {
                c.on_primary
            } else {
                c.on_surface_variant
            }
        },
        || false,
    );
    ink.paint(
        ink.listen(
            TextStyle::BODY_LARGE.apply(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_center()
                    .w(72.0)
                    .h(36.0)
                    .rounded(18.0)
                    .focusable()
                    .node_ref(node)
                    .cursor(CursorIcon::Pointer)
                    .dyn_style(move |s| {
                        let c = color_scheme();
                        let s = s.bg(background.get()).text_color(content.get());
                        if is_current && !ring_on() {
                            s.border(1.0).border_color(c.primary)
                        } else {
                            s
                        }
                    })
                    .on_click(move |_| f())
                    .child(text(year.to_string())),
            ),
        ),
    )
}

/// A date picker in a dialog. Buttons are made each time it opens.
pub fn date_picker_dialog(
    open: impl Fn() -> bool + 'static,
    on_dismiss: impl Fn() + 'static,
    selected: impl Fn() -> Option<Date> + 'static,
    on_select: impl Fn(Date) + 'static,
    style: DatePickerStyle,
    buttons: impl Fn() -> Vec<Div> + 'static,
) -> Show {
    let selected: Rc<dyn Fn() -> Option<Date>> = Rc::new(selected);
    let on_select: Rc<dyn Fn(Date)> = Rc::new(on_select);
    dialog(open, on_dismiss, move || {
        let (selected, on_select) = (selected.clone(), on_select.clone());
        div()
            .flex()
            .flex_col()
            .w(PICKER_WIDTH)
            .overflow_hidden()
            .rounded(28.0)
            .child(date_picker(
                move || selected(),
                move |d| on_select(d),
                style.clone(),
            ))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(8.0)
                    .pb(8.0)
                    .pr(6.0)
                    .children(buttons()),
            )
    })
}
