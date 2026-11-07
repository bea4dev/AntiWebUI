//! How the computer is doing: CPU, memory, temperatures, disks and network speed, read from `/proc`
//! and `/sys` on a thread nobody sees, as often as the handles alive ask and not at all without
//! any.

use anti_webui_core::shared::{SharedSignal, shared_signal};
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Memory, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    /// What programs could have without swapping.
    pub available: u64,
    /// `total - available`.
    pub used: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

/// A temperature sensor.
#[derive(Clone, Debug, PartialEq)]
pub struct Temperature {
    /// What the driver calls itself, like `k10temp` or `nvme`.
    pub sensor: String,
    /// What the sensor is of, like `Tctl` or `Composite`, or the sensor's name without one.
    pub label: String,
    pub celsius: f64,
}

/// A mounted filesystem, in bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Disk {
    pub mount: String,
    pub device: String,
    pub total: u64,
    /// What a program that isn't root can still write.
    pub available: u64,
}

/// How fast the network is going, over every interface but loopback.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NetSpeed {
    pub rx_bytes_per_sec: f64,
    pub tx_bytes_per_sec: f64,
    /// Since the interfaces came up.
    pub rx_total: u64,
    pub tx_total: u64,
}

/// What the computer is doing. Cheap to clone; reading tracks, on any thread. While any handle is
/// alive, the readings are taken as often as the most eager of them asked.
#[derive(Clone, Debug)]
pub struct System(Arc<Handle>);

#[derive(Debug)]
struct Handle {
    id: u64,
    shared: &'static Shared,
}

impl Drop for Handle {
    fn drop(&mut self) {
        let mut wanted = self.shared.wanted.lock().expect("no panics in here");
        wanted.retain(|(id, _)| *id != self.id);
        self.shared.wake.notify_all();
    }
}

#[derive(Debug)]
struct Shared {
    cpu: SharedSignal<f64>,
    cpus: SharedSignal<Vec<f64>>,
    memory: SharedSignal<Memory>,
    temperatures: SharedSignal<Vec<Temperature>>,
    cpu_temperature: SharedSignal<Option<f64>>,
    disks: SharedSignal<Vec<Disk>>,
    network: SharedSignal<NetSpeed>,
    load: SharedSignal<(f64, f64, f64)>,
    uptime: SharedSignal<Duration>,
    /// The handles alive and how often each wants readings.
    wanted: Mutex<Vec<(u64, Duration)>>,
    wake: Condvar,
}

static SHARED: OnceLock<Shared> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);

/// Readings once a second.
pub fn system() -> System {
    system_every(Duration::from_secs(1))
}

/// Readings every `interval`, or more often if another handle asks for that.
pub fn system_every(interval: Duration) -> System {
    let shared = SHARED.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("anti-webui system".into())
            .spawn(sample);
        Shared {
            cpu: shared_signal(0.0),
            cpus: shared_signal(Vec::new()),
            memory: shared_signal(Memory::default()),
            temperatures: shared_signal(Vec::new()),
            cpu_temperature: shared_signal(None),
            disks: shared_signal(Vec::new()),
            network: shared_signal(NetSpeed::default()),
            load: shared_signal((0.0, 0.0, 0.0)),
            uptime: shared_signal(Duration::ZERO),
            wanted: Mutex::new(Vec::new()),
            wake: Condvar::new(),
        }
    });
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    let interval = interval.max(Duration::from_millis(50));
    shared
        .wanted
        .lock()
        .expect("no panics in here")
        .push((id, interval));
    shared.wake.notify_all();
    System(Arc::new(Handle { id, shared }))
}

impl System {
    /// How busy all the CPUs are together, from 0 to 1, since the reading before.
    pub fn cpu(&self) -> f64 {
        self.0.shared.cpu.get()
    }

    /// How busy each CPU is, from 0 to 1.
    pub fn cpus(&self) -> Vec<f64> {
        self.0.shared.cpus.get()
    }

    pub fn memory(&self) -> Memory {
        self.0.shared.memory.get()
    }

    /// Every temperature sensor there is.
    pub fn temperatures(&self) -> Vec<Temperature> {
        self.0.shared.temperatures.get()
    }

    /// The CPU's temperature, from the sensor that best says it.
    pub fn cpu_temperature(&self) -> Option<f64> {
        self.0.shared.cpu_temperature.get()
    }

    /// The real filesystems mounted, each device once.
    pub fn disks(&self) -> Vec<Disk> {
        self.0.shared.disks.get()
    }

    pub fn network(&self) -> NetSpeed {
        self.0.shared.network.get()
    }

    /// The load average over 1, 5 and 15 minutes.
    pub fn load(&self) -> (f64, f64, f64) {
        self.0.shared.load.get()
    }

    /// How long since the computer started.
    pub fn uptime(&self) -> Duration {
        self.0.shared.uptime.get()
    }
}

fn put<T: PartialEq + Send + 'static>(signal: &SharedSignal<T>, value: T) {
    if signal.with_untracked(|v| *v != value) {
        signal.set(value);
    }
}

/// What the last reading was, to tell rates by.
#[derive(Default)]
struct Last {
    cpus: Vec<CpuTimes>,
    net: Option<(Instant, u64, u64)>,
}

/// The sampling thread: waits while nobody wants readings.
fn sample() {
    let shared = loop {
        if let Some(s) = SHARED.get() {
            break s;
        }
        std::thread::sleep(Duration::from_millis(1));
    };
    let mut last = Last::default();
    let mut wanted = shared.wanted.lock().expect("no panics in here");
    loop {
        let Some(interval) = wanted.iter().map(|(_, i)| *i).min() else {
            // Rates from long ago would say little.
            last = Last::default();
            wanted = shared.wake.wait(wanted).expect("no panics in here");
            continue;
        };
        drop(wanted);
        read(shared, &mut last);
        let until = Instant::now() + interval;
        wanted = shared.wanted.lock().expect("no panics in here");
        // Woken early when handles come or go, to take up their interval.
        loop {
            let now = Instant::now();
            if now >= until || wanted.is_empty() {
                break;
            }
            let shortest = wanted.iter().map(|(_, i)| *i).min().unwrap_or(interval);
            if shortest < interval {
                break;
            }
            wanted = shared
                .wake
                .wait_timeout(wanted, until - now)
                .expect("no panics in here")
                .0;
        }
    }
}

fn read_file(path: impl AsRef<Path>) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn read(shared: &Shared, last: &mut Last) {
    if let Some(stat) = read_file("/proc/stat") {
        let now = parse_stat(&stat);
        if let Some((total, each)) = now.split_first() {
            let busy = |i: usize, t: &CpuTimes| busy(last.cpus.get(i), t);
            put(&shared.cpu, busy(0, total));
            put(
                &shared.cpus,
                each.iter()
                    .enumerate()
                    .map(|(i, t)| busy(i + 1, t))
                    .collect(),
            );
        }
        last.cpus = now;
    }
    if let Some(meminfo) = read_file("/proc/meminfo") {
        put(&shared.memory, parse_meminfo(&meminfo));
    }
    let temperatures = read_temperatures();
    put(&shared.cpu_temperature, cpu_temperature(&temperatures));
    put(&shared.temperatures, temperatures);
    if let Some(mounts) = read_file("/proc/mounts") {
        put(&shared.disks, disks(&mounts));
    }
    if let Some(dev) = read_file("/proc/net/dev") {
        let (rx, tx) = parse_net_dev(&dev);
        let now = Instant::now();
        let rate = |before: u64, after: u64, secs: f64| {
            if secs > 0.0 && after >= before {
                (after - before) as f64 / secs
            } else {
                0.0
            }
        };
        let speed = match last.net {
            Some((then, rx0, tx0)) => {
                let secs = now.duration_since(then).as_secs_f64();
                NetSpeed {
                    rx_bytes_per_sec: rate(rx0, rx, secs),
                    tx_bytes_per_sec: rate(tx0, tx, secs),
                    rx_total: rx,
                    tx_total: tx,
                }
            }
            None => NetSpeed {
                rx_total: rx,
                tx_total: tx,
                ..Default::default()
            },
        };
        last.net = Some((now, rx, tx));
        put(&shared.network, speed);
    }
    if let Some(load) = read_file("/proc/loadavg").and_then(|l| parse_loadavg(&l)) {
        put(&shared.load, load);
    }
    if let Some(uptime) = read_file("/proc/uptime").and_then(|u| parse_uptime(&u)) {
        put(&shared.uptime, uptime);
    }
}

/// Jiffies a CPU spent, busy and idle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CpuTimes {
    total: u64,
    idle: u64,
}

/// The total line first, then each CPU's.
fn parse_stat(stat: &str) -> Vec<CpuTimes> {
    stat.lines()
        .filter(|l| l.starts_with("cpu"))
        .map(|l| {
            let n: Vec<u64> = l
                .split_whitespace()
                .skip(1)
                .filter_map(|x| x.parse().ok())
                .collect();
            // user nice system idle iowait irq softirq steal; guests are in user already.
            let total = n.iter().take(8).sum();
            let idle = n.get(3).copied().unwrap_or(0) + n.get(4).copied().unwrap_or(0);
            CpuTimes { total, idle }
        })
        .collect()
}

/// From 0 to 1; since boot when there is no reading before.
fn busy(before: Option<&CpuTimes>, now: &CpuTimes) -> f64 {
    let before = before.copied().unwrap_or_default();
    let total = now.total.saturating_sub(before.total);
    let idle = now.idle.saturating_sub(before.idle);
    if total == 0 {
        return 0.0;
    }
    (1.0 - idle as f64 / total as f64).clamp(0.0, 1.0)
}

fn parse_meminfo(meminfo: &str) -> Memory {
    let kb = |key: &str| {
        meminfo
            .lines()
            .find_map(|l| l.strip_prefix(key)?.strip_prefix(':'))
            .and_then(|v| v.split_whitespace().next()?.parse::<u64>().ok())
            .unwrap_or(0)
            * 1024
    };
    let (total, available) = (kb("MemTotal"), kb("MemAvailable"));
    let (swap_total, swap_free) = (kb("SwapTotal"), kb("SwapFree"));
    Memory {
        total,
        available,
        used: total.saturating_sub(available),
        swap_total,
        swap_used: swap_total.saturating_sub(swap_free),
    }
}

fn read_temperatures() -> Vec<Temperature> {
    let mut all = Vec::new();
    let mut hwmons: Vec<_> = std::fs::read_dir("/sys/class/hwmon")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    hwmons.sort();
    for dir in hwmons {
        let sensor = read_file(dir.join("name"))
            .map(|n| n.trim().to_owned())
            .unwrap_or_default();
        let mut inputs: Vec<_> = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                let n = name.strip_prefix("temp")?.strip_suffix("_input")?;
                Some((n.parse::<u32>().ok()?, name))
            })
            .collect();
        inputs.sort();
        for (n, input) in inputs {
            let Some(milli) =
                read_file(dir.join(&input)).and_then(|v| v.trim().parse::<i64>().ok())
            else {
                continue;
            };
            let label = read_file(dir.join(format!("temp{n}_label")))
                .map(|l| l.trim().to_owned())
                .unwrap_or_else(|| sensor.clone());
            all.push(Temperature {
                sensor: sensor.clone(),
                label,
                celsius: milli as f64 / 1000.0,
            });
        }
    }
    if !all.is_empty() {
        return all;
    }
    // No hwmon: the thermal zones.
    let mut zones: Vec<_> = std::fs::read_dir("/sys/class/thermal")
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("thermal_zone"))
        })
        .collect();
    zones.sort();
    for zone in zones {
        let kind = read_file(zone.join("type")).map(|t| t.trim().to_owned());
        let milli = read_file(zone.join("temp")).and_then(|t| t.trim().parse::<i64>().ok());
        if let (Some(kind), Some(milli)) = (kind, milli) {
            all.push(Temperature {
                sensor: kind.clone(),
                label: kind,
                celsius: milli as f64 / 1000.0,
            });
        }
    }
    all
}

/// The sensor that says the CPU's temperature best: the package's or die's, then a thermal zone
/// for it.
fn cpu_temperature(all: &[Temperature]) -> Option<f64> {
    let find = |sensor: &str, labels: &[&str]| {
        all.iter()
            .find(|t| t.sensor == sensor && labels.iter().any(|l| t.label.starts_with(l)))
            .map(|t| t.celsius)
    };
    find("k10temp", &["Tctl", "Tdie"])
        .or_else(|| find("zenpower", &["Tdie", "Tctl"]))
        .or_else(|| find("coretemp", &["Package"]))
        .or_else(|| find("cpu_thermal", &["cpu_thermal"]))
        .or_else(|| find("x86_pkg_temp", &["x86_pkg_temp"]))
        .or_else(|| find("acpitz", &["acpitz"]))
}

/// What `/proc/mounts` escapes, like `\040` for a space.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\'
            && i + 3 < b.len()
            && let Ok(digits) = std::str::from_utf8(&b[i + 1..i + 4])
            && let Ok(v) = u8::from_str_radix(digits, 8)
        {
            out.push(v);
            i += 4;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The filesystems on disks.
const REAL: [&str; 11] = [
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "vfat", "exfat", "ntfs", "ntfs3", "zfs",
];

/// Each device's first mount of a real filesystem, from `/proc/mounts`.
fn mounts(text: &str) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    text.lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (device, mount, kind) = (f.next()?, f.next()?, f.next()?);
            (REAL.contains(&kind) && seen.insert(device.to_owned()))
                .then(|| (unescape_mount(device), unescape_mount(mount)))
        })
        .collect()
}

fn disks(text: &str) -> Vec<Disk> {
    mounts(text)
        .into_iter()
        .filter_map(|(device, mount)| {
            let path = std::ffi::CString::new(mount.clone()).ok()?;
            // SAFETY: a valid path and a zeroed struct for it to fill.
            let mut s: libc::statvfs = unsafe { std::mem::zeroed() };
            if unsafe { libc::statvfs(path.as_ptr(), &mut s) } != 0 {
                return None;
            }
            let block = s.f_frsize as u64;
            Some(Disk {
                mount,
                device,
                total: s.f_blocks as u64 * block,
                available: s.f_bavail as u64 * block,
            })
        })
        .collect()
}

/// Bytes received and sent by every interface but loopback.
fn parse_net_dev(text: &str) -> (u64, u64) {
    text.lines()
        .skip(2)
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            if name.trim() == "lo" {
                return None;
            }
            let n: Vec<u64> = rest
                .split_whitespace()
                .filter_map(|x| x.parse().ok())
                .collect();
            Some((*n.first()?, *n.get(8)?))
        })
        .fold((0, 0), |(rx, tx), (r, t)| (rx + r, tx + t))
}

fn parse_loadavg(text: &str) -> Option<(f64, f64, f64)> {
    let mut f = text.split_whitespace().map(|x| x.parse::<f64>().ok());
    Some((f.next()??, f.next()??, f.next()??))
}

fn parse_uptime(text: &str) -> Option<Duration> {
    let secs: f64 = text.split_whitespace().next()?.parse().ok()?;
    Some(Duration::from_secs_f64(secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_time_becomes_how_busy() {
        let a =
            parse_stat("cpu  100 0 100 700 100 0 0 0 0 0\ncpu0 50 0 50 350 50 0 0 0 0 0\nintr 1\n");
        let b = parse_stat("cpu  150 0 150 750 150 0 0 0 0 0\ncpu0 100 0 50 400 50 0 0 0 0 0\n");
        assert_eq!(a.len(), 2);
        assert_eq!(
            a[0],
            CpuTimes {
                total: 1000,
                idle: 800
            }
        );
        // 200 more, 100 of them idle.
        assert_eq!(busy(Some(&a[0]), &b[0]), 0.5);
        assert_eq!(busy(Some(&a[1]), &b[1]), 0.5);
        assert!((busy(None, &a[0]) - 0.2).abs() < 1e-9);
    }

    #[test]
    fn meminfo_is_read_in_bytes() {
        let m = parse_meminfo(
            "MemTotal:       1000 kB\nMemFree:  10 kB\nMemAvailable:    600 kB\nSwapTotal: 200 kB\nSwapFree: 50 kB\n",
        );
        assert_eq!(
            m,
            Memory {
                total: 1_024_000,
                available: 614_400,
                used: 409_600,
                swap_total: 204_800,
                swap_used: 153_600,
            }
        );
    }

    #[test]
    fn only_real_filesystems_count_each_device_once() {
        let m = mounts(
            "proc /proc proc rw 0 0\n/dev/nvme0n1p2 / btrfs rw,subvol=/@ 0 0\ntmpfs /tmp tmpfs rw 0 0\n/dev/nvme0n1p2 /home btrfs rw,subvol=/@home 0 0\n/dev/sda1 /mnt/my\\040disk vfat rw 0 0\n",
        );
        assert_eq!(
            m,
            [
                ("/dev/nvme0n1p2".to_owned(), "/".to_owned()),
                ("/dev/sda1".to_owned(), "/mnt/my disk".to_owned()),
            ]
        );
    }

    #[test]
    fn network_counters_leave_out_loopback() {
        let text = "Inter-|   Receive |  Transmit\n face |bytes packets|bytes\n    lo:     240       4    0    0    0     0          0         0      240       4    0    0    0     0       0          0\n wlan0: 1000  4    0    0    0     0          0         0 2000  4    0    0    0     0       0          0\n  eth0: 10  4    0    0    0     0          0         0 20  4    0    0    0     0       0          0\n";
        assert_eq!(parse_net_dev(text), (1010, 2020));
    }

    #[test]
    fn load_uptime_and_the_cpu_sensor() {
        assert_eq!(
            parse_loadavg("0.50 0.25 0.10 1/100 1234\n"),
            Some((0.5, 0.25, 0.1))
        );
        assert_eq!(
            parse_uptime("12.5 40.0\n"),
            Some(Duration::from_millis(12_500))
        );
        let t = |sensor: &str, label: &str, celsius| Temperature {
            sensor: sensor.into(),
            label: label.into(),
            celsius,
        };
        let all = [
            t("nvme", "Composite", 40.0),
            t("acpitz", "acpitz", 45.0),
            t("k10temp", "Tctl", 55.0),
        ];
        assert_eq!(cpu_temperature(&all), Some(55.0));
        assert_eq!(cpu_temperature(&all[..2]), Some(45.0));
        assert_eq!(cpu_temperature(&all[..1]), None);
    }
}
