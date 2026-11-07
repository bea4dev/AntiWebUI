//! Reads the D-Bus services of the machine this runs on. Not run by default:
//! `cargo test -p anti-webui-wayland-shell --all-features --test dbus_live -- --ignored --nocapture`.

#![cfg(feature = "dbus")]

use std::time::Duration;

#[test]
#[ignore = "reads this machine's services"]
fn what_this_machine_has() {
    #[cfg(feature = "battery")]
    let battery = anti_webui_wayland_shell::battery();
    #[cfg(feature = "power-profiles")]
    let profiles = anti_webui_wayland_shell::power_profiles();
    std::thread::sleep(Duration::from_millis(800));
    #[cfg(feature = "battery")]
    eprintln!(
        "battery: {} {:.0}% {:?} on battery {} empty {:?} full {:?} {:.1} W {}\n  devices {:#?}",
        battery.available(),
        battery.percentage(),
        battery.state(),
        battery.on_battery(),
        battery.time_to_empty(),
        battery.time_to_full(),
        battery.energy_rate(),
        battery.icon_name(),
        battery.devices()
    );
    #[cfg(feature = "power-profiles")]
    eprintln!(
        "profiles: {} {} {:?} {:?}",
        profiles.available(),
        profiles.active(),
        profiles.profiles(),
        profiles.degraded()
    );
}
