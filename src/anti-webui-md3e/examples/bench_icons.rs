//! What icons cost at run time: `--icons` different ones, of which `--moving` keep moving on `FILL`.
//!
//! ```text
//! ANTIWEBUI_FRAME_STATS=out.csv cargo run --release -p anti-webui-md3e --example bench_icons -- \
//!     --seconds 8 --icons 100 --moving 20
//! ```
//!
//! With nothing moving, no frame is asked for and nothing closes the window, so whoever runs it
//! ends it.

use anti_webui::core::platform::PlatformRequest;
use anti_webui::core::tree::with_doc;
use anti_webui::prelude::*;
use anti_webui_md3e::components::icon;
use anti_webui_md3e::icons;
use anti_webui_md3e::icons::Symbol;
use std::time::Instant;

const ICONS: [&Symbol; 100] = [
    icons::BIA,
    icons::BATTERY_3_BAR,
    icons::LANDSCAPE_2,
    icons::FORWARD_30,
    icons::PERSONAL_PLACES,
    icons::NOTES,
    icons::CLOCK_LOADER_80,
    icons::MERGE_TYPE,
    icons::HOME_STORAGE,
    icons::CALL,
    icons::HDR_PLUS,
    icons::VOLUME_OFF,
    icons::PANORAMA_WIDE_ANGLE,
    icons::MOPED_PACKAGE,
    icons::HEALTH_CROSS,
    icons::ACCESSIBILITY,
    icons::RECEIPT_LONG,
    icons::WATER_LOSS,
    icons::WORKSPACE_PREMIUM,
    icons::FIBER_PIN,
    icons::ESCALATOR_WARNING,
    icons::BOTTOM_RIGHT_CLICK,
    icons::SAVINGS,
    icons::CARDS,
    icons::BLUR_MEDIUM,
    icons::DEVICES,
    icons::CONTACT_EMERGENCY,
    icons::FOLDER_MATCH,
    icons::MOVE_SELECTION_UP,
    icons::RESTORE_FROM_TRASH,
    icons::UNGROUP,
    icons::ANDROID_WIFI_3_BAR_LOCK,
    icons::NOT_STARTED,
    icons::LOCAL_CAR_WASH,
    icons::DOWNLOADING,
    icons::NEARBY,
    icons::DOMAIN_DISABLED,
    icons::LIBRARY_ADD,
    icons::SPATIAL_AUDIO,
    icons::ADD_TRIANGLE,
    icons::TAB_GROUP,
    icons::TILE_LARGE,
    icons::BOOK_5,
    icons::SPEED,
    icons::PERM_CAMERA_MIC,
    icons::SWIPE_RIGHT,
    icons::HIDE_SOURCE,
    icons::DIRECTIONS_ALT_OFF,
    icons::PADEL,
    icons::DETECTOR_STATUS,
    icons::GARAGE_DOOR,
    icons::MONITORING,
    icons::SYNC_SAVED_LOCALLY,
    icons::ZONE_PERSON_ALERT,
    icons::POINT_SCAN,
    icons::POKER_CHIP,
    icons::TRANSITION_FADE,
    icons::FRAGRANCE,
    icons::TABLET_MAC,
    icons::ON_DEVICE_TRAINING,
    icons::HORIZONTAL_ALIGN_RIGHT,
    icons::WORKSPACES,
    icons::VIEW_LIST,
    icons::VERTICAL_SPLIT,
    icons::FOLDER_CHECK_2,
    icons::WOMAN,
    icons::FLIP_CAMERA_ANDROID,
    icons::EDGESENSOR_HIGH,
    icons::VALVE,
    icons::NEST_THERMOSTAT_ZIRCONIUM_EU,
    icons::CAR_DEFROST_LOW_RIGHT,
    icons::WINDSHIELD_DEFROST_REAR,
    icons::DIAMOND_SHINE,
    icons::DATASET_LINKED,
    icons::SHIELD_WITH_HEART,
    icons::CHAT_APPS_SCRIPT,
    icons::SMOKE_FREE,
    icons::HALLWAY,
    icons::ROUTER,
    icons::UNIVERSAL_LOCAL,
    icons::CHECK_IN_OUT,
    icons::STICKER,
    icons::GLOBE,
    icons::ADMIN_MEDS,
    icons::WIFI_CALLING_1,
    icons::ENCRYPTED_ADD_CIRCLE,
    icons::SHOPPING_BAG_SPEED,
    icons::MORE_DOWN,
    icons::TIBIA_ALT,
    icons::FLAG_CIRCLE,
    icons::BLUETOOTH_CONNECTED,
    icons::FRAME_PERSON_MIC,
    icons::ADAPTIVE_AUDIO_MIC,
    icons::TAPAS,
    icons::PAN_TOOL,
    icons::AUTO_DELETE,
    icons::UNLICENSE,
    icons::STOP_CIRCLE,
    icons::SHUTTER_SPEED_ADD,
    icons::_1X_MOBILEDATA,
];

#[derive(Clone, Copy)]
struct Load {
    seconds: f64,
    icons: usize,
    moving: usize,
}

fn parse_args() -> Load {
    let mut load = Load {
        seconds: 8.0,
        icons: 100,
        moving: 0,
    };
    let mut args = std::env::args().skip(1);
    while let Some(name) = args.next() {
        let value: f64 = args
            .next()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("{name} needs a number"));
        match name.as_str() {
            "--seconds" => load.seconds = value,
            "--icons" => load.icons = value as usize,
            "--moving" => load.moving = value as usize,
            other => panic!("unknown option {other}"),
        }
    }
    load
}

fn tick(load: Load, started: Option<Instant>, time: Signal<f32>) {
    request_frame(move |now| {
        let started = started.unwrap_or(now);
        let t = now.saturating_duration_since(started).as_secs_f64();
        if t >= load.seconds {
            with_doc(|d| d.request(PlatformRequest::Close));
            return;
        }
        time.set(t as f32);
        tick(load, Some(started), time);
    });
}

fn main() {
    let load = parse_args();
    App::new()
        .title("bench_icons")
        .size(900, 720)
        .run(move || {
            let time = signal(0.0_f32);
            if load.moving > 0 {
                tick(load, None, time);
            }
            let item = move |i: usize| {
                let moved = icon(ICONS[i]);
                if i < load.moving {
                    moved.dyn_style(move |s| {
                        s.font_axis(b"FILL", 0.5 + 0.5 * (time.get() * 3.0).sin())
                    })
                } else {
                    moved
                }
            };
            div()
                .flex()
                .flex_col()
                .gap(12.0)
                .p(12.0)
                .size_full()
                .bg(rgb(0xfafafa))
                .text_color(rgb(0x1a1c1e))
                .children((0..load.icons.div_ceil(20)).map(move |r| {
                    div()
                        .flex()
                        .flex_row()
                        .gap(12.0)
                        .children((r * 20..((r + 1) * 20).min(load.icons)).map(item))
                }))
        })
        .unwrap();
}
