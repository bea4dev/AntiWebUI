//! The scale, from XSETTINGS or `Xft.dpi` (see "スケールとサイズ" in
//! `docs/design/35-platform-x11.md`).

use anti_webui_core::geometry::Scale;

/// A value of XSETTINGS. Colors aren't needed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Setting {
    Int(i32),
    Str(String),
}

/// Reads `_XSETTINGS_SETTINGS`. What can't be read is left out.
pub(crate) fn parse_xsettings(data: &[u8]) -> Vec<(String, Setting)> {
    let mut out = Vec::new();
    let Some(&order) = data.first() else {
        return out;
    };
    let big = order != 0;
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = data.get(at..at + 2)?.try_into().ok()?;
        Some(if big {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = data.get(at..at + 4)?.try_into().ok()?;
        Some(if big {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    };
    let pad = |n: usize| (n + 3) & !3;
    let Some(count) = u32_at(8) else {
        return out;
    };
    let mut at = 12;
    for _ in 0..count {
        let (Some(&kind), Some(len)) = (data.get(at), u16_at(at + 2)) else {
            break;
        };
        let len = len as usize;
        let Some(name) = data.get(at + 4..at + 4 + len) else {
            break;
        };
        let name = String::from_utf8_lossy(name).into_owned();
        // The name, then the serial of its last change.
        at += 4 + pad(len) + 4;
        match kind {
            0 => {
                let Some(v) = u32_at(at) else { break };
                out.push((name, Setting::Int(v as i32)));
                at += 4;
            }
            1 => {
                let Some(len) = u32_at(at) else { break };
                let len = len as usize;
                let Some(s) = data.get(at + 4..at + 4 + len) else {
                    break;
                };
                out.push((name, Setting::Str(String::from_utf8_lossy(s).into_owned())));
                at += 4 + pad(len);
            }
            2 => at += 8,
            _ => break,
        }
    }
    out
}

/// The DPI XSETTINGS gives, if it does.
pub(crate) fn dpi_of_xsettings(settings: &[(String, Setting)]) -> Option<f64> {
    settings
        .iter()
        .find_map(|(name, value)| match (name.as_str(), value) {
            ("Xft/DPI", Setting::Int(v)) if *v > 0 => Some(*v as f64 / 1024.0),
            _ => None,
        })
}

/// The scale for a DPI: 96 is 1.
pub(crate) fn scale_of_dpi(dpi: f64) -> Scale {
    Scale(((dpi / 96.0 * 120.0).round() as u32).max(1))
}

/// `ANTIWEBUI_SCALE=1.5`, for trying scales out.
pub(crate) fn scale_from_env() -> Option<Scale> {
    let v: f64 = std::env::var("ANTIWEBUI_SCALE").ok()?.parse().ok()?;
    (v > 0.0).then(|| Scale(((v * 120.0).round() as u32).max(1)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setting(out: &mut Vec<u8>, kind: u8, name: &str, value: &[u8]) {
        out.extend([kind, 0]);
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(name.as_bytes());
        out.resize(out.len().next_multiple_of(4), 0);
        out.extend(0u32.to_le_bytes());
        out.extend(value);
        out.resize(out.len().next_multiple_of(4), 0);
    }

    #[test]
    fn the_dpi_comes_from_xsettings() {
        let mut data = vec![0, 0, 0, 0];
        data.extend(7u32.to_le_bytes());
        data.extend(3u32.to_le_bytes());
        let mut name = 9u32.to_le_bytes().to_vec();
        name.extend(b"Adwaita\0\0");
        setting(&mut data, 1, "Net/ThemeName", &name);
        setting(&mut data, 2, "Gtk/Color", &[0; 8]);
        setting(&mut data, 0, "Xft/DPI", &(144 * 1024u32).to_le_bytes());
        let settings = parse_xsettings(&data);
        assert_eq!(settings.len(), 2);
        assert_eq!(
            settings[0],
            ("Net/ThemeName".into(), Setting::Str("Adwaita\0\0".into()))
        );
        assert_eq!(dpi_of_xsettings(&settings), Some(144.0));
        assert_eq!(scale_of_dpi(144.0), Scale(180));
    }

    #[test]
    fn what_is_cut_short_gives_what_came_before() {
        let mut data = vec![0, 0, 0, 0];
        data.extend(1u32.to_le_bytes());
        data.extend(2u32.to_le_bytes());
        setting(&mut data, 0, "Xft/DPI", &(192 * 1024u32).to_le_bytes());
        data.extend([0, 0, 40, 0]);
        let settings = parse_xsettings(&data);
        assert_eq!(dpi_of_xsettings(&settings), Some(192.0));
        assert_eq!(scale_of_dpi(192.0), Scale(240));
    }
}
