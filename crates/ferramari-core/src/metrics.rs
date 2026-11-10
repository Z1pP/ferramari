//! Lightweight process-tree metrics for the Performance page.
//!
//! The Linux sampler reads `/proc`; there is no dependency on a system
//! monitor, UPower or `nvidia-smi`. A sample covers Ferramari plus its mpv
//! children.
//!
//! Every number is a [`Metric`], so a missing value carries *why* it is
//! missing. Showing `0%` for a platform that cannot measure CPU at all, or for
//! the very first sample, makes an unmeasurable thing look like an idle one —
//! which is the opposite of what the Eco work is trying to prove.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

/// One metric reading, or the reason there is none.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Metric<T> {
    /// Measured this round.
    Value(T),
    /// This platform, driver or session has no way to measure it. Permanent
    /// until the code gains a sampler for it.
    Unsupported,
    /// Measurable here, but this round produced nothing: the process tree is
    /// gone, or the interface did not answer.
    Unavailable,
    /// A rate that needs two samples; the first one has no interval yet.
    Pending,
}

impl<T> Metric<T> {
    pub fn value(self) -> Option<T> {
        match self {
            Metric::Value(value) => Some(value),
            _ => None,
        }
    }

    pub fn is_value(&self) -> bool {
        matches!(self, Metric::Value(_))
    }

    /// The value when present, otherwise `fallback`. Used by call sites that
    /// only need a number for display formatting.
    pub fn unwrap_or(self, fallback: T) -> T {
        self.value().unwrap_or(fallback)
    }
}

/// A single resource reading for the whole process tree.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// CPU used by the process tree since the previous sample.
    pub cpu: Metric<f64>,
    pub memory: Metric<u64>,
    /// Total physical memory, so the UI can show a share rather than a raw
    /// RSS number. Read from `/proc/meminfo` (`MemTotal`).
    pub memory_total: Metric<u64>,
    pub threads: Metric<u32>,
    pub processes: Metric<u32>,
    /// GPU utilisation, only available on drivers that expose
    /// `gpu_busy_percent` (currently AMDGPU).
    pub gpu: Metric<f64>,
}

impl Sample {
    /// A sample for a platform with no process sampler at all, so the page can
    /// say so instead of drawing zeroes.
    pub fn unsupported() -> Self {
        Self {
            cpu: Metric::Unsupported,
            memory: Metric::Unsupported,
            memory_total: Metric::Unsupported,
            threads: Metric::Unsupported,
            processes: Metric::Unsupported,
            gpu: Metric::Unsupported,
        }
    }

    /// True when nothing in this sample is a measurement, which is what makes
    /// the UI show a single explanation rather than five dashes.
    pub fn is_empty(&self) -> bool {
        !self.cpu.is_value()
            && !self.memory.is_value()
            && !self.threads.is_value()
            && !self.processes.is_value()
            && !self.gpu.is_value()
    }
}

#[derive(Debug, Default)]
pub struct Monitor {
    previous: Option<(Instant, u64)>,
}

impl Monitor {
    pub fn sample(&mut self) -> Sample {
        self.sample_of(std::process::id())
    }

    /// Sample a specific process tree. The UI needs this to report the
    /// server's footprint (mpv children included) instead of its own.
    pub fn sample_of(&mut self, root: u32) -> Sample {
        let gpu = gpu_percent(Path::new("/sys/class/drm"));
        let memory_total = mem_total(Path::new("/proc/meminfo"));
        let Some(tree) = read_tree(root) else {
            // Either this platform has no sampler, or the tree is gone. The
            // distinction belongs to the sampler, so ask it directly.
            return Sample {
                cpu: Metric::Unsupported,
                memory: Metric::Unsupported,
                memory_total,
                threads: Metric::Unsupported,
                processes: Metric::Unsupported,
                gpu,
            };
        };
        if tree.processes == 0 {
            // The sampler works here but this pid is not in the table any
            // more: the server exited or was never reachable.
            self.previous = None;
            return Sample {
                cpu: Metric::Unavailable,
                memory: Metric::Unavailable,
                memory_total,
                threads: Metric::Unavailable,
                processes: Metric::Unavailable,
                gpu,
            };
        }

        let now = Instant::now();
        let cpu = match self.previous {
            Some((previous_at, previous_ticks)) => {
                let elapsed = now.duration_since(previous_at).as_secs_f64();
                if elapsed > 0.0 {
                    let ticks = tree.ticks.saturating_sub(previous_ticks);
                    Metric::Value(ticks as f64 / clock_ticks() as f64 / elapsed * 100.0)
                } else {
                    // Two reads within the same clock tick: there is no
                    // interval to divide by, and `0%` would be a guess.
                    Metric::Pending
                }
            }
            None => Metric::Pending,
        };
        self.previous = Some((now, tree.ticks));

        Sample {
            cpu,
            memory: Metric::Value(tree.rss_bytes),
            memory_total,
            threads: Metric::Value(tree.threads),
            processes: Metric::Value(tree.processes),
            gpu,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct Tree {
    ticks: u64,
    rss_bytes: u64,
    threads: u32,
    processes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stat {
    ppid: u32,
    ticks: u64,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Status {
    rss_kb: u64,
    threads: u32,
}

/// Walk the process tree below `root`. `None` means this platform has no
/// process table the sampler understands; an empty [`Tree`] means the tree
/// was read and the root was not in it.
#[cfg(target_os = "linux")]
fn read_tree(root: u32) -> Option<Tree> {
    let entries = fs::read_dir("/proc").ok()?;
    let mut stats: HashMap<u32, Stat> = HashMap::new();
    let mut statuses: HashMap<u32, Status> = HashMap::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        let Some(stat) = fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|text| parse_stat(&text))
        else {
            continue;
        };
        let status = fs::read_to_string(entry.path().join("status"))
            .ok()
            .map(|text| parse_status(&text))
            .unwrap_or_default();
        stats.insert(pid, stat);
        statuses.insert(pid, status);
    }

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, stat) in &stats {
        children.entry(stat.ppid).or_default().push(*pid);
    }
    let mut tree = Tree::default();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        let Some(stat) = stats.get(&pid) else {
            continue;
        };
        let status = statuses.get(&pid).copied().unwrap_or_default();
        tree.ticks = tree.ticks.saturating_add(stat.ticks);
        tree.rss_bytes = tree
            .rss_bytes
            .saturating_add(status.rss_kb.saturating_mul(1024));
        tree.threads = tree.threads.saturating_add(status.threads.max(1));
        tree.processes = tree.processes.saturating_add(1);
        if let Some(children) = children.get(&pid) {
            stack.extend(children);
        }
    }
    Some(tree)
}

/// Platforms without a `/proc` table have no sampler yet. Windows needs
/// `GetProcessTimes`/PSAPI and macOS `proc_pid_rusage`; until one of those is
/// wired up the metrics page must report that rather than zeroes.
#[cfg(not(target_os = "linux"))]
fn read_tree(_root: u32) -> Option<Tree> {
    None
}

/// Parse `/proc/<pid>/stat`. The command name can contain spaces and `)`, so
/// split after the last `)` instead of using `split_whitespace` on the whole
/// line.
fn parse_stat(text: &str) -> Option<Stat> {
    let close = text.rfind(')')?;
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    // After comm: state(3), ppid(4), … utime(14), stime(15), …
    let ppid = fields.get(1)?.parse().ok()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    Some(Stat {
        ppid,
        ticks: utime.saturating_add(stime),
    })
}

fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            status.rss_kb = value
                .split_whitespace()
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0);
        } else if let Some(value) = line.strip_prefix("Threads:") {
            status.threads = value.trim().parse().unwrap_or(0);
        }
    }
    status
}

#[cfg(unix)]
fn clock_ticks() -> u64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as u64
    } else {
        100
    }
}

#[cfg(not(unix))]
fn clock_ticks() -> u64 {
    // Only reached by code paths that already reported Unsupported.
    100
}

/// Busiest GPU, or the reason nothing is known.
///
/// `gpu_busy_percent` is an AMDGPU-specific sysfs file; i915, nouveau and
/// NVIDIA do not provide it, and neither does any non-Linux platform. That is
/// a missing measurement, not a free one, so it reports `Unsupported`.
fn gpu_percent(root: &Path) -> Metric<f64> {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return Metric::Unsupported,
    };
    let mut busiest: Option<f64> = None;
    for entry in entries.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("card") {
            continue;
        }
        let Ok(text) = fs::read_to_string(entry.path().join("device/gpu_busy_percent")) else {
            continue;
        };
        match text.trim().parse::<f64>() {
            Ok(value) => busiest = Some(busiest.map_or(value, |current| current.max(value))),
            Err(_) => return Metric::Unavailable,
        }
    }
    busiest.map_or(Metric::Unsupported, Metric::Value)
}

/// Total physical memory in bytes, from `MemTotal` in `/proc/meminfo`. The
/// Performance page shows Ferramari's share of it, which needs the whole.
fn mem_total(path: &Path) -> Metric<u64> {
    let Ok(text) = fs::read_to_string(path) else {
        return Metric::Unsupported;
    };
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            return rest
                .split_whitespace()
                .next()
                .and_then(|kib| kib.parse::<u64>().ok())
                .map(|kib| kib.saturating_mul(1024))
                .map_or(Metric::Unavailable, Metric::Value);
        }
    }
    Metric::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stat_parser_handles_spaces_and_parentheses_in_the_command() {
        // comm = "a) b c", then state S, ppid 42, nine fields, utime 100,
        // stime 50.
        let mut fields = vec!["S", "42"];
        fields.extend(std::iter::repeat_n("0", 9));
        fields.extend(["100", "50", "0", "0", "20", "0", "1", "0", "0"]);
        let line = format!("123 (a) b c) {}", fields.join(" "));
        let stat = parse_stat(&line).unwrap();
        assert_eq!(stat.ppid, 42);
        assert_eq!(stat.ticks, 150);
    }

    #[test]
    fn status_parser_reads_rss_and_threads() {
        let status = parse_status("Name:\tferramari\nVmRSS:\t  12345 kB\nThreads:\t7\n");
        assert_eq!(status.rss_kb, 12345);
        assert_eq!(status.threads, 7);
    }

    #[test]
    fn mem_total_reads_kibibytes_as_bytes() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ferramari-meminfo-{unique}"));
        fs::write(&path, "MemTotal:       16384000 kB\nMemFree: 1 kB\n").unwrap();
        assert_eq!(mem_total(&path), Metric::Value(16_384_000 * 1024));
        fs::write(&path, "MemFree: 1 kB\n").unwrap();
        assert_eq!(mem_total(&path), Metric::Unavailable);
        // A file that is not there is a platform with no `/proc/meminfo`.
        let _ = std::fs::remove_file(&path);
        assert_eq!(mem_total(&path), Metric::Unsupported);
    }

    #[test]
    fn the_busiest_card_wins_and_a_missing_driver_is_unsupported() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("ferramari-gpu-{}-{unique}", std::process::id()));

        // No DRM directory at all: nothing can be known.
        assert_eq!(gpu_percent(&root), Metric::Unsupported);

        let first = root.join("card0/device");
        let second = root.join("card1/device");
        fs::create_dir_all(&first).unwrap();
        fs::create_dir_all(&second).unwrap();
        // A card without the file (i915, nouveau, NVIDIA) does not make the
        // metric supported.
        assert_eq!(gpu_percent(&root), Metric::Unsupported);

        fs::write(first.join("gpu_busy_percent"), "37\n").unwrap();
        assert_eq!(gpu_percent(&root), Metric::Value(37.0));
        fs::write(second.join("gpu_busy_percent"), "12\n").unwrap();
        assert_eq!(gpu_percent(&root), Metric::Value(37.0));
        fs::write(second.join("gpu_busy_percent"), "91\n").unwrap();
        assert_eq!(gpu_percent(&root), Metric::Value(91.0));

        fs::write(second.join("gpu_busy_percent"), "not a number\n").unwrap();
        assert_eq!(gpu_percent(&root), Metric::Unavailable);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn a_gpu_directory_that_is_a_file_is_unsupported_not_a_panic() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("ferramari-gpu-file-{}", unique));
        fs::write(&path, "x").unwrap();
        assert_eq!(gpu_percent(&path), Metric::Unsupported);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn the_first_sample_has_no_rate_to_report() {
        let mut monitor = Monitor::default();
        let sample = monitor.sample();
        assert_eq!(
            sample.cpu,
            Metric::Pending,
            "a rate needs two samples; reporting 0% would claim an idle process"
        );
        assert!(sample.memory.is_value());
        assert!(sample.processes.is_value());
        assert!(sample.threads.is_value());
    }

    #[test]
    fn current_process_tree_contains_this_test() {
        let mut monitor = Monitor::default();
        monitor.sample();
        let sample = monitor.sample();
        if !sample.cpu.is_value() {
            // A second read inside one clock tick is legitimately Pending.
            assert_eq!(sample.cpu, Metric::Pending);
        }
        assert!(sample.processes.unwrap_or(0) >= 1);
        assert!(sample.threads.unwrap_or(0) >= 1);
        assert!(sample.memory.unwrap_or(0) > 0);
        assert!(!sample.is_empty());
    }

    #[test]
    fn a_dead_process_tree_is_unavailable_not_zero() {
        // Sample a pid that cannot exist here. The tree read succeeds and
        // finds nothing, which is "gone", not "unsupported".
        let mut monitor = Monitor::default();
        let sample = monitor.sample_of(u32::MAX);
        if cfg!(target_os = "linux") {
            assert_eq!(sample.processes, Metric::Unavailable);
            assert_eq!(sample.cpu, Metric::Unavailable);
            assert_eq!(sample.memory, Metric::Unavailable);
            assert_eq!(sample.threads, Metric::Unavailable);
        } else {
            assert_eq!(sample.processes, Metric::Unsupported);
        }
        // A dead process tree says nothing about the GPU: the driver reading
        // is taken independently and must not be dragged into the failure.
        assert!(!matches!(sample.gpu, Metric::Pending));
    }

    #[test]
    fn a_gone_server_resets_the_cpu_interval() {
        // Otherwise the next live sample would divide by the elapsed time
        // across the gap and report a meaningless spike.
        let mut monitor = Monitor::default();
        monitor.sample();
        let _ = monitor.sample_of(u32::MAX);
        assert_eq!(monitor.sample().cpu, Metric::Pending);
    }

    #[test]
    fn an_unsupported_sample_reports_no_measurement_anywhere() {
        let sample = Sample::unsupported();
        assert!(sample.is_empty());
        assert_eq!(sample.cpu, Metric::Unsupported);
        assert_eq!(sample.memory, Metric::Unsupported);
        assert_eq!(sample.threads, Metric::Unsupported);
        assert_eq!(sample.processes, Metric::Unsupported);
        assert_eq!(sample.gpu, Metric::Unsupported);
    }

    #[test]
    fn metrics_expose_their_value_only_when_measured() {
        assert_eq!(Metric::Value(12).value(), Some(12));
        assert_eq!(Metric::Unsupported::<u32>.value(), None);
        assert_eq!(Metric::Pending::<u32>.value(), None);
        assert_eq!(Metric::Unavailable::<u32>.value(), None);
        assert_eq!(Metric::Value(3.5).unwrap_or(0.0), 3.5);
        assert_eq!(Metric::Pending::<f64>.unwrap_or(0.0), 0.0);
        assert!(Metric::Value(1).is_value());
        assert!(!Metric::Pending::<u32>.is_value());
    }
}
