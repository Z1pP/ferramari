//! Lightweight process-tree metrics for the Performance page.
//!
//! Everything is read from `/proc`; there is no dependency on a system
//! monitor, UPower or `nvidia-smi`. A sample covers Ferramari plus its mpv and
//! WebKit children. GPU utilisation is only available on drivers that expose
//! `gpu_busy_percent` (currently AMDGPU); other systems show “unavailable”.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::Instant;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    /// False when this platform has no supported process-tree sampler.
    pub available: bool,
    /// CPU used by the whole process tree since the previous sample. The first
    /// sample reports 0 because there is no interval to divide by.
    pub cpu_percent: f64,
    pub rss_bytes: u64,
    pub threads: u32,
    pub processes: u32,
    pub gpu_percent: Option<f64>,
}

#[derive(Debug, Default)]
pub struct Monitor {
    previous: Option<(Instant, u64)>,
}

impl Monitor {
    pub fn sample(&mut self) -> Sample {
        self.sample_of(std::process::id() as u32)
    }

    /// Sample a specific process tree. The UI needs this to report the
    /// server's footprint (mpv children included) instead of its own.
    pub fn sample_of(&mut self, root: u32) -> Sample {
        let tree = read_tree(root);
        let now = Instant::now();
        let cpu_percent = match self.previous {
            Some((previous_at, previous_ticks)) => {
                let elapsed = now.duration_since(previous_at).as_secs_f64();
                let ticks = tree.ticks.saturating_sub(previous_ticks);
                if elapsed > 0.0 {
                    ticks as f64 / clock_ticks() as f64 / elapsed * 100.0
                } else {
                    0.0
                }
            }
            None => 0.0,
        };
        self.previous = Some((now, tree.ticks));
        Sample {
            available: tree.processes > 0,
            cpu_percent,
            rss_bytes: tree.rss_bytes,
            threads: tree.threads,
            processes: tree.processes,
            gpu_percent: gpu_percent(Path::new("/sys/class/drm")),
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

fn read_tree(root: u32) -> Tree {
    let Ok(entries) = fs::read_dir("/proc") else {
        return Tree::default();
    };
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
    tree
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
    // /proc-based process sampling is currently unavailable on Windows.
    100
}

fn gpu_percent(root: &Path) -> Option<f64> {
    let entries = fs::read_dir(root).ok()?;
    entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_name().to_string_lossy().starts_with("card") {
                return None;
            }
            fs::read_to_string(entry.path().join("device/gpu_busy_percent"))
                .ok()?
                .trim()
                .parse::<f64>()
                .ok()
        })
        .reduce(f64::max)
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
    fn gpu_reader_takes_the_busiest_card_and_tolerates_absence() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("ferramari-gpu-{}-{unique}", std::process::id()));
        let device = root.join("card0/device");
        fs::create_dir_all(&device).unwrap();
        fs::write(device.join("gpu_busy_percent"), "37\n").unwrap();
        assert_eq!(gpu_percent(&root), Some(37.0));
        fs::write(device.join("gpu_busy_percent"), "91\n").unwrap();
        assert_eq!(gpu_percent(&root), Some(91.0));
        let _ = fs::remove_dir_all(root);
        assert_eq!(gpu_percent(Path::new("/definitely/not/drm")), None);
    }

    #[test]
    fn current_process_tree_contains_this_test() {
        let mut monitor = Monitor::default();
        let sample = monitor.sample();
        assert!(sample.available);
        assert!(sample.processes >= 1);
        assert!(sample.threads >= 1);
        assert!(sample.rss_bytes > 0);
    }
}
