//! Turning XInput 2 into the core's pointer and scroll events (see "入力" in
//! `docs/design/35-platform-x11.md`).

use anti_webui_core::event::{AXIS_MOTION_GAIN, PointerButton, ScrollSource};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use x11rb::protocol::xinput::{self, DeviceClassData, Fp3232, XIDeviceInfo};

/// How long a device that scrolled smoothly is taken to keep doing so.
const SMOOTH_FOR: Duration = Duration::from_millis(300);

pub(crate) fn fp1616(v: i32) -> f64 {
    v as f64 / 65536.0
}

pub(crate) fn fp3232(v: Fp3232) -> f64 {
    v.integral as f64 + v.frac as f64 / 4_294_967_296.0
}

/// A scroll axis of a device: which valuator it is and how much one notch is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Axis {
    valuator: u16,
    increment: f64,
    last: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Device {
    vertical: Option<Axis>,
    horizontal: Option<Axis>,
    /// It says it is a touchpad (libinput's or synaptics' properties).
    touchpad: bool,
    /// When it last moved by less than a notch.
    smooth_until: Option<Instant>,
}

/// One scroll, as the core takes it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Scroll {
    pub dx: f32,
    pub dy: f32,
    pub source: ScrollSource,
}

/// The scroll axes of the devices, by the id of the physical (slave) device.
#[derive(Default)]
pub(crate) struct ScrollDevices {
    devices: HashMap<u16, Device>,
    /// Pixels in one notch of the valuators: 15 for the libinput driver, 10 for XWayland.
    pixels_per_notch: f64,
}

impl ScrollDevices {
    pub fn new(pixels_per_notch: f64) -> Self {
        Self {
            devices: HashMap::new(),
            pixels_per_notch,
        }
    }

    /// Takes in what `XIQueryDevice` says. `touchpad` says which devices are touchpads.
    pub fn set(&mut self, infos: &[XIDeviceInfo], touchpad: impl Fn(u16) -> bool) {
        self.devices.clear();
        for info in infos {
            self.update(info.deviceid, &info.classes, touchpad(info.deviceid));
        }
    }

    /// Takes in the classes of one device, after `XI_DeviceChanged`.
    pub fn update(&mut self, id: u16, classes: &[xinput::DeviceClass], touchpad: bool) {
        let mut device = Device {
            touchpad,
            ..Default::default()
        };
        let mut values = HashMap::new();
        for class in classes {
            match &class.data {
                DeviceClassData::Valuator(v) => {
                    values.insert(v.number, fp3232(v.value));
                }
                DeviceClassData::Scroll(s) => {
                    let axis = Axis {
                        valuator: s.number,
                        increment: fp3232(s.increment),
                        last: None,
                    };
                    if axis.increment == 0.0 {
                        continue;
                    }
                    if s.scroll_type == xinput::ScrollType::VERTICAL {
                        device.vertical = Some(axis);
                    } else {
                        device.horizontal = Some(axis);
                    }
                }
                _ => {}
            }
        }
        for axis in [&mut device.vertical, &mut device.horizontal]
            .into_iter()
            .flatten()
        {
            axis.last = values.get(&axis.valuator).copied();
        }
        if device.vertical.is_some() || device.horizontal.is_some() {
            self.devices.insert(id, device);
        } else {
            self.devices.remove(&id);
        }
    }

    /// Whether the device scrolls through valuators, which makes its buttons 4 to 7 copies.
    pub fn scrolls_smoothly(&self, id: u16) -> bool {
        self.devices.contains_key(&id)
    }

    /// Forgets where the axes were, as the pointer may have moved them elsewhere.
    pub fn forget_positions(&mut self) {
        for d in self.devices.values_mut() {
            for axis in [&mut d.vertical, &mut d.horizontal].into_iter().flatten() {
                axis.last = None;
            }
        }
    }

    /// The scroll in a motion event of the device, if there is one.
    pub fn motion(
        &mut self,
        id: u16,
        mask: &[u32],
        values: &[Fp3232],
        now: Instant,
    ) -> Option<Scroll> {
        let pixels_per_notch = self.pixels_per_notch;
        let device = self.devices.get_mut(&id)?;
        let value_of = |valuator: u16| -> Option<f64> {
            let word = mask.get(valuator as usize / 32)?;
            if word & (1 << (valuator % 32)) == 0 {
                return None;
            }
            // The values come in the order of the bits that are set.
            let before: u32 = mask
                .iter()
                .take(valuator as usize / 32)
                .map(|w| w.count_ones())
                .sum::<u32>()
                + (word & ((1u32 << (valuator % 32)) - 1)).count_ones();
            values.get(before as usize).copied().map(fp3232)
        };
        let mut notches = [0.0f64; 2];
        for (i, axis) in [&mut device.horizontal, &mut device.vertical]
            .into_iter()
            .enumerate()
        {
            let Some(axis) = axis else { continue };
            let Some(value) = value_of(axis.valuator) else {
                continue;
            };
            if let Some(last) = axis.last.replace(value) {
                notches[i] = (value - last) / axis.increment;
            }
        }
        if notches == [0.0, 0.0] {
            return None;
        }
        let whole = notches
            .iter()
            .all(|n| (n - n.round()).abs() < 1e-3 && (*n == 0.0 || n.abs() >= 1.0 - 1e-3));
        if !whole {
            device.smooth_until = Some(now + SMOOTH_FOR);
        }
        let smooth = device.touchpad || device.smooth_until.is_some_and(|t| t > now);
        Some(if smooth {
            let gain = (pixels_per_notch * AXIS_MOTION_GAIN as f64) as f32;
            Scroll {
                dx: notches[0] as f32 * gain,
                dy: notches[1] as f32 * gain,
                source: ScrollSource::Unknown,
            }
        } else {
            Scroll {
                dx: notches[0].round() as f32,
                dy: notches[1].round() as f32,
                source: ScrollSource::Wheel,
            }
        })
    }
}

/// A button of a press or release, or the notches of the old wheel buttons.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Button {
    Pointer(PointerButton),
    Wheel { dx: f32, dy: f32 },
}

pub(crate) fn button(detail: u32) -> Button {
    match detail {
        1 => Button::Pointer(PointerButton::Primary),
        2 => Button::Pointer(PointerButton::Middle),
        3 => Button::Pointer(PointerButton::Secondary),
        4 => Button::Wheel { dx: 0.0, dy: -1.0 },
        5 => Button::Wheel { dx: 0.0, dy: 1.0 },
        6 => Button::Wheel { dx: -1.0, dy: 0.0 },
        7 => Button::Wheel { dx: 1.0, dy: 0.0 },
        // The evdev codes, as on Wayland: BTN_SIDE and BTN_EXTRA.
        8 => Button::Pointer(PointerButton::Other(0x113)),
        9 => Button::Pointer(PointerButton::Other(0x114)),
        b => Button::Pointer(PointerButton::Other(0x110 + b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use x11rb::protocol::xinput::{
        DeviceClass, DeviceClassDataScroll, DeviceClassDataValuator, ScrollFlags, ScrollType,
        ValuatorMode,
    };

    fn fp(v: f64) -> Fp3232 {
        Fp3232 {
            integral: v.floor() as i32,
            frac: ((v - v.floor()) * 4_294_967_296.0) as u32,
        }
    }

    fn classes(increment: f64) -> Vec<DeviceClass> {
        let valuator = |number| DeviceClass {
            len: 0,
            sourceid: 0,
            data: DeviceClassData::Valuator(DeviceClassDataValuator {
                number,
                label: 0,
                min: fp(0.0),
                max: fp(0.0),
                value: fp(0.0),
                resolution: 0,
                mode: ValuatorMode::RELATIVE,
            }),
        };
        let scroll = |number, scroll_type| DeviceClass {
            len: 0,
            sourceid: 0,
            data: DeviceClassData::Scroll(DeviceClassDataScroll {
                number,
                scroll_type,
                flags: ScrollFlags::from(0u32),
                increment: fp(increment),
            }),
        };
        vec![
            valuator(2),
            valuator(3),
            scroll(2, ScrollType::HORIZONTAL),
            scroll(3, ScrollType::VERTICAL),
        ]
    }

    /// A motion event that moves valuators 2 and 3 to `h` and `v`.
    fn motion(devices: &mut ScrollDevices, h: f64, v: f64) -> Option<Scroll> {
        devices.motion(9, &[0b1100], &[fp(h), fp(v)], Instant::now())
    }

    #[test]
    fn whole_notches_are_a_wheel() {
        let mut devices = ScrollDevices::new(10.0);
        devices.update(9, &classes(1.0), false);
        assert!(devices.scrolls_smoothly(9));
        let s = motion(&mut devices, 0.0, 2.0).unwrap();
        assert_eq!((s.dx, s.dy, s.source), (0.0, 2.0, ScrollSource::Wheel));
        let s = motion(&mut devices, -1.0, 2.0).unwrap();
        assert_eq!((s.dx, s.dy), (-1.0, 0.0));
        assert_eq!(motion(&mut devices, -1.0, 2.0), None, "nothing moved");
    }

    #[test]
    fn parts_of_notches_are_pixels_and_stay_so_for_a_while() {
        let mut devices = ScrollDevices::new(10.0);
        devices.update(9, &classes(1.0), false);
        let s = motion(&mut devices, 0.0, 0.25).unwrap();
        assert_eq!(s.source, ScrollSource::Unknown);
        assert_eq!(s.dy, 0.25 * 10.0 * AXIS_MOTION_GAIN);
        let s = motion(&mut devices, 0.0, 1.25).unwrap();
        assert_eq!(
            s.source,
            ScrollSource::Unknown,
            "a whole notch in the middle"
        );
    }

    #[test]
    fn touchpads_are_always_pixels_and_increments_set_the_notch() {
        let mut devices = ScrollDevices::new(15.0);
        devices.update(9, &classes(15.0), true);
        let s = motion(&mut devices, 0.0, 30.0).unwrap();
        assert_eq!(s.source, ScrollSource::Unknown);
        assert_eq!(s.dy, 2.0 * 15.0 * AXIS_MOTION_GAIN);
    }

    #[test]
    fn where_the_axes_were_is_forgotten_on_entering() {
        let mut devices = ScrollDevices::new(10.0);
        devices.update(9, &classes(1.0), false);
        motion(&mut devices, 0.0, 5.0);
        devices.forget_positions();
        assert_eq!(
            motion(&mut devices, 0.0, 9.0),
            None,
            "only sets where it is"
        );
        assert_eq!(motion(&mut devices, 0.0, 10.0).unwrap().dy, 1.0);
    }

    #[test]
    fn devices_without_scroll_axes_use_the_buttons() {
        let mut devices = ScrollDevices::new(10.0);
        devices.update(4, &[], false);
        assert!(!devices.scrolls_smoothly(4));
        assert_eq!(button(5), Button::Wheel { dx: 0.0, dy: 1.0 });
        assert_eq!(button(1), Button::Pointer(PointerButton::Primary));
        assert_eq!(button(3), Button::Pointer(PointerButton::Secondary));
    }
}
