//! Another notification daemon already has the name. Run with `--features notifications`.
#![cfg(feature = "notifications")]

mod common;

use anti_webui_wayland_shell::{ServiceError, notification_server};

#[test]
fn a_name_already_taken_is_said_so() {
    let _bus = common::private_bus();
    let other = common::connect(Some("org.freedesktop.Notifications"));
    assert_eq!(notification_server().err(), Some(ServiceError::NameTaken));
    // Once the other one goes, asking again takes it.
    drop(other);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(notification_server().is_ok());
}
