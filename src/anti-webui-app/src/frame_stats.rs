//! What each frame took, from the start of its work to the screen: `ANTIWEBUI_FRAME_STATS=out.csv`
//! writes a row per frame and prints a summary when the window closes (`-` for no file). See
//! `docs/design/10-frame-pacing.md`.

use crate::pacing::FrameRecord;
use anti_webui_wgpu::Gpu;
use std::fmt::Write as _;

/// How much is kept; about half an hour at 60 Hz.
const MAX_RECORDS: usize = 100_000;

pub(crate) struct FrameStats {
    path: Option<String>,
    records: Vec<FrameRecord>,
    description: Option<String>,
}

impl FrameStats {
    pub fn from_env() -> Option<Self> {
        let path = std::env::var("ANTIWEBUI_FRAME_STATS").ok()?;
        Some(Self {
            path: (path != "-").then_some(path),
            records: Vec::new(),
            description: None,
        })
    }

    pub fn add(&mut self, frame: &FrameRecord) {
        if self.records.len() < MAX_RECORDS {
            self.records.push(frame.clone());
        }
    }

    pub fn describe(&mut self, gpu: &Gpu, path: &str) {
        if self.description.is_none() {
            let info = gpu.adapter.get_info();
            self.description = Some(format!(
                "{} ({:?}, {}, {}), {path}",
                info.name, info.device_type, info.driver, info.driver_info
            ));
        }
    }

    /// Writes out what was recorded.
    pub fn finish(self) {
        let mut summary = String::new();
        if let Some(d) = &self.description {
            let _ = writeln!(summary, "GPU: {d}");
        }
        summary.push_str(&summarize(&self.records));
        eprint!("[anti-webui] frame stats\n{summary}");
        if let Some(path) = &self.path
            && let Err(e) = std::fs::write(path, csv(&self.records))
        {
            eprintln!("[anti-webui] could not write {path}: {e}");
        }
    }
}

fn csv(records: &[FrameRecord]) -> String {
    let opt = |v: Option<i64>| v.map_or(String::new(), |v| v.to_string());
    let mut out = String::from(
        "token,callback,start,ui_done,diff_done,buffer_ready,recorded,submitted,committed,end,\
         gpu_done,presented,refresh,flags,discarded,damage,repaint,pixels,buffers\n",
    );
    for r in records {
        let _ = writeln!(
            out,
            "{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.token,
            opt(r.callback),
            r.start,
            r.ui_done,
            r.diff_done,
            r.buffer_ready,
            r.recorded,
            r.submitted,
            r.committed,
            r.end,
            opt(r.gpu_done),
            opt(r.presented),
            r.refresh,
            r.flags,
            r.discarded as u8,
            r.damage,
            r.repaint,
            r.pixels,
            r.buffers,
        );
    }
    out
}

/// How refreshes got skipped among the frames that started as soon as they could.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Misses {
    /// Frames that started as soon as they could.
    pub continuous: usize,
    /// Refreshes on which those showed nothing new.
    pub skipped: usize,
    /// Frames after which a refresh was skipped, by what took too long: CPU alone...
    pub late_cpu: usize,
    /// ...CPU and GPU together...
    pub late_gpu: usize,
    /// ...or neither, so the time went elsewhere.
    pub late_elsewhere: usize,
}

pub(crate) fn misses(records: &[FrameRecord]) -> Misses {
    let mut m = Misses::default();
    for pair in records.windows(2) {
        let [prev, this] = pair else { continue };
        let Some(skipped) = this.skipped_after(prev) else {
            continue;
        };
        m.continuous += 1;
        if skipped == 0 {
            continue;
        }
        m.skipped += skipped;
        let refresh = this.refresh as i64;
        if this.end - this.start > refresh {
            m.late_cpu += 1;
        } else if this.gpu_done.is_some_and(|g| g - this.start > refresh) {
            m.late_gpu += 1;
        } else {
            m.late_elsewhere += 1;
        }
    }
    m
}

/// The value at quantile `q` of sorted values.
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let i = (sorted.len() as f64 - 1.0) * q;
    sorted[i.round() as usize]
}

pub(crate) fn summarize(records: &[FrameRecord]) -> String {
    let mut out = String::new();
    let shown = records.iter().filter(|r| r.presented.is_some()).count();
    let discarded = records.iter().filter(|r| r.discarded).count();
    let no_fence = records.iter().filter(|r| r.gpu_done.is_none()).count();
    let refresh = records.iter().map(|r| r.refresh).max().unwrap_or(0);
    let pixels = records.iter().map(|r| r.pixels).sum::<i64>().max(1) as f64;
    let _ = writeln!(
        out,
        "{} frames: {shown} shown, {discarded} discarded; {no_fence} without a GPU time; refresh \
         {:.3} ms; damage {:.1}%, repainted {:.1}%; {} buffer(s)",
        records.len(),
        refresh as f64 / 1e6,
        records.iter().map(|r| r.damage).sum::<i64>() as f64 * 100.0 / pixels,
        records.iter().map(|r| r.repaint).sum::<i64>() as f64 * 100.0 / pixels,
        records.iter().map(|r| r.buffers).max().unwrap_or(0),
    );
    let span = |f: &dyn Fn(&FrameRecord) -> Option<i64>| -> Vec<f64> {
        let mut v: Vec<f64> = records
            .iter()
            .filter_map(f)
            .map(|ns| ns as f64 / 1e6)
            .collect();
        v.sort_by(f64::total_cmp);
        v
    };
    let intervals = {
        let mut v: Vec<f64> = records
            .windows(2)
            .filter_map(|p| Some((p[1].presented? - p[0].presented?) as f64 / 1e6))
            .collect();
        v.sort_by(f64::total_cmp);
        v
    };
    // When the compositor says "next" relative to the frame reaching the screen.
    let callback_after_shown = {
        let mut v: Vec<f64> = records
            .windows(2)
            .filter_map(|p| Some((p[1].callback? - p[0].presented?) as f64 / 1e6))
            .collect();
        v.sort_by(f64::total_cmp);
        v
    };
    // How far the time between starts is from the time between the frames on the screen: what
    // animations are off by.
    let step_error = {
        let mut v: Vec<f64> = records
            .windows(2)
            .filter_map(|p| {
                let on_screen = p[1].presented? - p[0].presented?;
                Some((p[1].start - p[0].start - on_screen).abs() as f64 / 1e6)
            })
            .collect();
        v.sort_by(f64::total_cmp);
        v
    };
    let rows: Vec<(&str, Vec<f64>)> = vec![
        ("wait", span(&|r| Some(r.start - r.callback?))),
        ("ui", span(&|r| Some(r.ui_done - r.start))),
        ("diff", span(&|r| Some(r.diff_done - r.ui_done))),
        ("buffer", span(&|r| Some(r.buffer_ready - r.diff_done))),
        ("record", span(&|r| Some(r.recorded - r.buffer_ready))),
        ("submit", span(&|r| Some(r.submitted - r.recorded))),
        ("cpu total", span(&|r| Some(r.end - r.start))),
        ("gpu", span(&|r| Some(r.gpu_done? - r.submitted))),
        ("start-gpu", span(&|r| Some(r.gpu_done? - r.start))),
        (
            "held",
            span(&|r| (r.committed != 0).then(|| r.committed - r.submitted)),
        ),
        ("gpu-screen", span(&|r| Some(r.presented? - r.gpu_done?))),
        ("latency", span(&|r| Some(r.presented? - r.start))),
        ("interval", intervals),
        ("shown-cb", callback_after_shown),
        ("step error", step_error),
    ];
    let _ = writeln!(
        out,
        "{:<11}{:>9}{:>9}{:>9}{:>9}",
        "ms", "p50", "p95", "p99", "max"
    );
    for (name, v) in rows {
        if v.is_empty() {
            continue;
        }
        let _ = writeln!(
            out,
            "{name:<11}{:>9.2}{:>9.2}{:>9.2}{:>9.2}",
            quantile(&v, 0.5),
            quantile(&v, 0.95),
            quantile(&v, 0.99),
            v[v.len() - 1],
        );
    }
    let m = misses(records);
    let _ = writeln!(
        out,
        "continuous frames {}: {} refresh(es) skipped; late by CPU {}, CPU+GPU {}, elsewhere {}",
        m.continuous, m.skipped, m.late_cpu, m.late_gpu, m.late_elsewhere,
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: i64 = 1_000_000;
    const REFRESH: u32 = 16_000_000;

    /// A frame that starts right at its callback, and takes `cpu` and then `gpu`.
    fn frame(start: i64, cpu: i64, gpu: i64, shown: i64) -> FrameRecord {
        FrameRecord {
            callback: Some(start - 100_000),
            start,
            committed: start + cpu,
            end: start + cpu,
            submitted: start + cpu,
            gpu_done: Some(start + cpu + gpu),
            presented: Some(shown),
            refresh: REFRESH,
            ..Default::default()
        }
    }

    #[test]
    fn a_skipped_refresh_is_blamed_on_what_took_too_long() {
        let records = [
            frame(0, 2 * MS, 2 * MS, 16 * MS),
            // CPU alone is over.
            frame(16 * MS, 20 * MS, 2 * MS, 48 * MS),
            // CPU and GPU together are.
            frame(48 * MS, 10 * MS, 10 * MS, 80 * MS),
            // Neither: the compositor was late, and by two refreshes.
            frame(80 * MS, 2 * MS, 2 * MS, 128 * MS),
        ];
        assert_eq!(
            misses(&records),
            Misses {
                continuous: 3,
                skipped: 4,
                late_cpu: 1,
                late_gpu: 1,
                late_elsewhere: 1,
            }
        );
    }

    #[test]
    fn the_summary_lists_what_is_known() {
        let text = summarize(&[frame(0, 2 * MS, 3 * MS, 16 * MS)]);
        assert!(text.contains("1 frames: 1 shown, 0 discarded"), "{text}");
        assert!(text.contains("gpu"), "{text}");
        assert!(!text.contains("interval"), "{text}");
    }
}
