//! The components. Each returns an ordinary builder that can be styled further.

mod app_bar;
mod badge;
mod button;
mod button_group;
mod card;
mod carousel;
mod checkbox;
mod chip;
mod date_picker;
mod fab;
mod fab_menu;
mod icon;
mod icon_button;
mod image;
mod list;
mod loading;
mod menu;
mod modal;
mod navigation;
mod progress;
mod search;
mod slider;
mod snackbar;
mod split_button;
mod surface;
mod switch;
mod tabs;
mod text_field;
mod time_picker;
mod toolbar;
mod tooltip;

pub use app_bar::{
    AppBarKind, AppBarScroll, TopAppBar, bottom_app_bar, flexible_bottom_app_bar, top_app_bar,
    top_app_bar_over,
};
pub use badge::{badge, badged, text_badge};
pub use button::{
    ButtonKind, ButtonSize, ButtonStyle, button, elevated_button, outlined_button, styled_button,
    styled_toggle_button, text_button, toggle_button, tonal_button,
};
pub use button_group::{button_group, connected_button_group};
pub use card::{
    CardKind, clickable_card, divider, elevated_card, filled_card, outlined_card,
    styled_clickable_card, vertical_divider,
};
pub use carousel::{Keylines, carousel, multi_browse_keylines};
pub use checkbox::{
    CheckState, CheckboxStyle, RadioStyle, checkbox, radio_button, styled_checkbox,
    styled_radio_button, tri_state_checkbox,
};
pub use chip::{
    ChipStyle, assist_chip, filter_chip, input_chip, selectable_input_chip, styled_assist_chip,
    styled_filter_chip, styled_suggestion_chip, suggestion_chip,
};
pub use date_picker::{Date, DatePickerStyle, date_picker, date_picker_dialog};
pub use fab::{FabColor, FabSize, FabStyle, extended_fab, fab, styled_extended_fab, styled_fab};
pub use fab_menu::{FabMenuItem, fab_menu, fab_menu_item};
pub use icon::{icon, sized_icon};
pub use icon_button::{
    IconButtonKind, IconButtonStyle, IconButtonWidth, filled_icon_button, icon_button,
    icon_toggle_button, outlined_icon_button, styled_icon_button, tonal_icon_button,
};
pub use image::{avatar, card_media, carousel_image, image, initials_avatar};
pub use list::{ListItemStyle, list_item, segmented_list, styled_list_item};
pub use loading::loading_indicator;
pub use menu::{MenuItem, MenuStyle, menu, menu_item, styled_menu};
pub use modal::{AlertDialog, alert_dialog, bottom_sheet, dialog, navigation_drawer, side_sheet};
pub use navigation::{
    NavItem, drawer_item, nav_icon, nav_item, navigation_bar, navigation_rail,
    short_navigation_bar, wide_navigation_rail,
};
pub use progress::{
    circular_progress, circular_wavy_progress, indeterminate_circular_progress,
    indeterminate_circular_wavy_progress, indeterminate_linear_progress,
    indeterminate_wavy_progress, linear_progress, wavy_progress,
};
pub use search::{SearchStyle, search_bar};
pub use slider::{SliderStyle, range_slider, slider, styled_slider};
pub use snackbar::{Snackbar, Snackbars, snackbars};
pub use split_button::{SplitButtonStyle, split_button};
pub use switch::{SwitchStyle, styled_switch, switch};
pub use tabs::{Tab, TabsStyle, icon_tab, styled_tabs, tab, tabs};
pub use text_field::{TextFieldStyle, dropdown_field, outlined_text_field, text_field};
pub use time_picker::{TimePickerStyle, time_picker, time_picker_dialog};
pub use toolbar::{ToolbarStyle, floating_toolbar, floating_toolbar_with_fab};
pub use tooltip::{RichTooltip, rich_tooltip, tooltip};

/// Compiles the shaders of the components, so that the first press doesn't have to wait for that.
pub(crate) fn preload_shaders() {
    surface::preload_shaders();
    loading::preload_shaders();
    progress::preload_shaders();
    checkbox::preload_shaders();
    slider::preload_shaders();
    text_field::preload_shaders();
    time_picker::preload_shaders();
}

/// The shaders of the components, for checking them without a GPU.
#[doc(hidden)]
pub fn check_shaders() -> Result<(), String> {
    surface::check_shader()?;
    loading::check_shader()?;
    progress::check_shader()?;
    checkbox::check_shader()?;
    slider::check_shader()?;
    text_field::check_shader()?;
    time_picker::check_shader()
}
