//! Lightweight process-tree metrics for the Performance page.
//!
//! Linux reads `/proc`; Windows reads the Toolhelp process table and
//! per-process counters through `GetProcessTimes` and `GetProcessMemoryInfo`.
//! Neither needs a system monitor, UPower or `nvidia-smi`. A sample covers
//! Ferramari plus its mpv children.
//!
//! Every number is a [`Metric`], so a missing value carries *why* it is
//! missing. Showing `0%` for a platform that cannot measure CPU at all, or for
//! the very first sample, makes an unmeasurable thing look like an idle one —
//! which is the opposite of what the Eco work is trying to prove.

#[cfg(any(target_os = "linux", test))]
use std::collections::HashMap;
#[cfg(any(not(target_os = "windows"), test))]
use std::fs;
#[cfg(any(not(target_os = "windows"), test))]
use std::path::Path;
use std::time::Instant;

#[cfg(target_os = "windows")]
mod windows_gpu;

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
    /// GPU utilisation: the busiest engine on Windows (`GPU Engine` counters)
    /// or the busiest card exposing `gpu_busy_percent` on Linux (AMDGPU).
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
    /// PDH query kept across samples: the GPU utilization counter is a rate
    /// and needs the previous collection to produce a number.
    #[cfg(target_os = "windows")]
    gpu: windows_gpu::Sampler,
}

impl Monitor {
    pub fn sample(&mut self) -> Sample {
        self.sample_of(std::process::id())
    }

    /// Sample a specific process tree. The UI needs this to report the
    /// server's footprint (mpv children included) instead of its own.
    pub fn sample_of(&mut self, root: u32) -> Sample {
        #[cfg(target_os = "windows")]
        let gpu = self.gpu.sample();
        #[cfg(not(target_os = "windows"))]
        let gpu = gpu_percent(Path::new("/sys/class/drm"));
        let memory_total = total_memory();
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
                    Metric::Value(ticks as f64 / ticks_per_second() as f64 / elapsed * 100.0)
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

#[cfg(any(target_os = "linux", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stat {
    ppid: u32,
    ticks: u64,
    threads: u32,
    rss_pages: u64,
}

/// Walk a `/proc`-shaped tree. Kept platform-neutral under `test` so the Linux
/// field parsing and aggregation can be exercised on any host; production
/// only calls it with `/proc`.
#[cfg(any(target_os = "linux", test))]
fn read_tree_from(proc_root: &Path, root: u32, page_size: u64) -> Option<Tree> {
    let entries = fs::read_dir(proc_root).ok()?;
    let mut stats: HashMap<u32, Stat> = HashMap::new();
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse().ok())
        else {
            continue;
        };
        // `/proc/<pid>/stat` already carries the thread count and resident
        // pages (fields 20 and 24). Reading the larger `status` file for every
        // unrelated process only to discard it was the expensive half of the
        // old sampler.
        let Some(stat) = fs::read_to_string(entry.path().join("stat"))
            .ok()
            .and_then(|text| parse_stat(&text))
        else {
            continue;
        };
        stats.insert(pid, stat);
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
        tree.ticks = tree.ticks.saturating_add(stat.ticks);
        tree.rss_bytes = tree
            .rss_bytes
            .saturating_add(stat.rss_pages.saturating_mul(page_size));
        tree.threads = tree.threads.saturating_add(stat.threads.max(1));
        tree.processes = tree.processes.saturating_add(1);
        if let Some(children) = children.get(&pid) {
            stack.extend(children);
        }
    }
    Some(tree)
}

/// Walk the process tree below `root`. `None` means this platform has no
/// process table the sampler understands; an empty [`Tree`] means the tree
/// was read and the root was not in it.
#[cfg(target_os = "linux")]
fn read_tree(root: u32) -> Option<Tree> {
    read_tree_from(Path::new("/proc"), root, page_size())
}

/// Walk the Windows process table below `root`. The Toolhelp snapshot gives
/// the parent links and thread counts in one pass; each process that can be
/// opened contributes its kernel+user time (100 ns units) and working set.
/// A process that cannot be opened still counts as a process and keeps the
/// tree shape, so the reading is "some of it" rather than "nothing".
#[cfg(target_os = "windows")]
fn read_tree(root: u32) -> Option<Tree> {
    use std::collections::HashMap;

    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, PROCESS_ACCESS_RIGHTS, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_VM_READ,
    };

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
    let mut processes: HashMap<u32, (u32, u32)> = HashMap::new();
    let mut entry = PROCESSENTRY32W {
        dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    if unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok() {
        loop {
            processes.insert(
                entry.th32ProcessID,
                (entry.th32ParentProcessID, entry.cntThreads),
            );
            if unsafe { Process32NextW(snapshot, &mut entry) }.is_err() {
                break;
            }
        }
    }
    let _ = unsafe { CloseHandle(snapshot) };

    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (pid, (ppid, _)) in &processes {
        children.entry(*ppid).or_default().push(*pid);
    }

    // `GetProcessMemoryInfo` wants `PROCESS_VM_READ`; `GetProcessTimes` only
    // wants query access. Try the wider right first and fall back so a
    // protected child still contributes CPU time.
    let wide_access =
        PROCESS_ACCESS_RIGHTS(PROCESS_QUERY_LIMITED_INFORMATION.0 | PROCESS_VM_READ.0);
    let mut tree = Tree::default();
    let mut stack = vec![root];
    while let Some(pid) = stack.pop() {
        let Some((_, threads)) = processes.get(&pid).copied() else {
            continue;
        };
        tree.processes = tree.processes.saturating_add(1);
        tree.threads = tree.threads.saturating_add(threads.max(1));
        if let Ok(handle) = unsafe { OpenProcess(wide_access, false, pid) }
            .or_else(|_| unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) })
        {
            let mut created = FILETIME::default();
            let mut exited = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            if unsafe { GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) }
                .is_ok()
            {
                tree.ticks = tree
                    .ticks
                    .saturating_add(filetime_ticks(kernel) + filetime_ticks(user));
            }
            let mut counters = PROCESS_MEMORY_COUNTERS {
                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32,
                ..Default::default()
            };
            if unsafe { GetProcessMemoryInfo(handle, &mut counters, counters.cb) }.is_ok() {
                tree.rss_bytes = tree
                    .rss_bytes
                    .saturating_add(counters.WorkingSetSize as u64);
            }
            let _ = unsafe { CloseHandle(handle) };
        }
        if let Some(children) = children.get(&pid) {
            stack.extend(children);
        }
    }
    Some(tree)
}

/// FILETIME stores 100 ns units in two 32-bit halves.
#[cfg(target_os = "windows")]
fn filetime_ticks(time: windows::Win32::Foundation::FILETIME) -> u64 {
    (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
}

/// Platforms without a process table the sampler understands. macOS needs
/// `proc_pid_rusage`; until that is wired up the metrics page must report that
/// rather than zeroes.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn read_tree(_root: u32) -> Option<Tree> {
    None
}

/// Parse `/proc/<pid>/stat`. The command name can contain spaces and `)`, so
/// split after the last `)` instead of using `split_whitespace` on the whole
/// line.
///
/// Field numbers in `proc(5)`: state 3, ppid 4, utime 14, stime 15,
/// num_threads 20, rss 24 (pages). After stripping the comm prefix the
/// zero-based indices are 1, 11, 12, 17 and 21.
#[cfg(any(target_os = "linux", test))]
fn parse_stat(text: &str) -> Option<Stat> {
    let close = text.rfind(')')?;
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let ppid = fields.get(1)?.parse().ok()?;
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;
    let threads: u32 = fields.get(17)?.parse().ok()?;
    let rss_pages: u64 = fields.get(21)?.parse().ok()?;
    Some(Stat {
        ppid,
        ticks: utime.saturating_add(stime),
        threads,
        rss_pages,
    })
}

/// Resident page size, used to turn `stat`'s `rss` field into bytes.
#[cfg(target_os = "linux")]
fn page_size() -> u64 {
    let size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if size > 0 {
        size as u64
    } else {
        4096
    }
}

#[cfg(unix)]
fn ticks_per_second() -> u64 {
    let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if ticks > 0 {
        ticks as u64
    } else {
        100
    }
}

/// Windows `GetProcessTimes` reports 100 ns intervals, so there are ten
/// million per second.
#[cfg(target_os = "windows")]
fn ticks_per_second() -> u64 {
    10_000_000
}

#[cfg(not(any(unix, target_os = "windows")))]
fn ticks_per_second() -> u64 {
    // Only reached by code paths that already reported Unsupported.
    100
}

/// Total physical memory in bytes. Windows exposes it through
/// `GlobalMemoryStatusEx`; Linux reads `MemTotal` from `/proc/meminfo`.
#[cfg(target_os = "windows")]
fn total_memory() -> Metric<u64> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    match unsafe { GlobalMemoryStatusEx(&mut status) } {
        Ok(()) => Metric::Value(status.ullTotalPhys),
        Err(_) => Metric::Unavailable,
    }
}

#[cfg(not(target_os = "windows"))]
fn total_memory() -> Metric<u64> {
    mem_total(Path::new("/proc/meminfo"))
}

/// Busiest GPU, or the reason nothing is known.
///
/// `gpu_busy_percent` is an AMDGPU-specific sysfs file; i915, nouveau and
/// NVIDIA do not provide it. This path-based reader is used on Linux and by
/// the unit tests; Windows has its own PDH sampler in `metrics::windows_gpu`.
#[cfg(any(not(target_os = "windows"), test))]
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
#[cfg(any(not(target_os = "windows"), test))]
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
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn stat_parser_reads_ppid_ticks_threads_and_rss_pages() {
        // comm = "a) b c", then state S, ppid 42, nine fields, utime 100,
        // stime 50, four fields, num_threads 7, three fields, rss 123 pages.
        let mut fields = vec!["S", "42"];
        fields.extend(std::iter::repeat_n("0", 9));
        fields.extend(["100", "50", "0", "0", "0", "0"]);
        fields.push("7");
        fields.extend(["0", "0", "0"]);
        fields.push("123");
        let line = format!("123 (a) b c) {}", fields.join(" "));
        let stat = parse_stat(&line).unwrap();
        assert_eq!(stat.ppid, 42);
        assert_eq!(stat.ticks, 150);
        assert_eq!(stat.threads, 7);
        assert_eq!(stat.rss_pages, 123);
    }

    fn write_fake_stat(
        root: &Path,
        pid: u32,
        comm: &str,
        ppid: u32,
        ticks: u64,
        threads: u32,
        rss_pages: u64,
    ) {
        let directory = root.join(pid.to_string());
        std::fs::create_dir_all(&directory).unwrap();
        let mut fields = vec!["S".to_string(), ppid.to_string()];
        fields.extend(std::iter::repeat_n("0".to_string(), 9));
        fields.push(ticks.to_string());
        fields.push("0".to_string());
        fields.extend(std::iter::repeat_n("0".to_string(), 4));
        fields.push(threads.to_string());
        fields.extend(std::iter::repeat_n("0".to_string(), 3));
        fields.push(rss_pages.to_string());
        let line = format!("{pid} ({comm}) {}", fields.join(" "));
        std::fs::write(directory.join("stat"), line).unwrap();
        // Deliberately no `status`: the sampler must not need it.
    }

    #[test]
    fn the_linux_tree_walk_aggregates_only_descendants_from_stat() {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ferramari-proc-{unique}"));
        write_fake_stat(&root, 100, "ferramari", 1, 10, 2, 5);
        write_fake_stat(&root, 101, "mpv", 100, 20, 3, 7);
        write_fake_stat(&root, 102, "conhost", 100, 5, 1, 2);
        write_fake_stat(&root, 999, "unrelated", 1, 100, 9, 100);
        let tree = read_tree_from(&root, 100, 4096).unwrap();
        assert_eq!(tree.processes, 3);
        assert_eq!(tree.ticks, 35);
        assert_eq!(tree.rss_bytes, (5 + 7 + 2) * 4096);
        assert_eq!(tree.threads, 6);
        let _ = std::fs::remove_dir_all(root);
    }

    /// Synthetic before/after for the Linux sampler. This is not a Linux
    /// kernel measurement: it creates a `/proc`-shaped directory tree and
    /// compares the old stat+status walk with the stat-only walk. Run it with
    /// `--ignored` when changing the Linux path.
    #[test]
    #[ignore]
    fn linux_read_tree_fake_before_after() {
        use std::time::Instant;

        fn parse_status_local(text: &str) -> (u64, u32) {
            let mut rss_kb = 0;
            let mut threads = 0;
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("VmRSS:") {
                    rss_kb = value
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(0);
                } else if let Some(value) = line.strip_prefix("Threads:") {
                    threads = value.trim().parse().unwrap_or(0);
                }
            }
            (rss_kb, threads)
        }

        fn read_tree_legacy(proc_root: &Path, root: u32, page_size: u64) -> Option<Tree> {
            let entries = fs::read_dir(proc_root).ok()?;
            let mut stats: HashMap<u32, Stat> = HashMap::new();
            let mut statuses: HashMap<u32, (u64, u32)> = HashMap::new();
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
                    .map(|text| parse_status_local(&text))
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
                let (rss_kb, threads) = statuses.get(&pid).copied().unwrap_or_default();
                tree.ticks = tree.ticks.saturating_add(stat.ticks);
                tree.rss_bytes = tree
                    .rss_bytes
                    .saturating_add(rss_kb.saturating_mul(1024).max(stat.rss_pages * page_size));
                tree.threads = tree.threads.saturating_add(threads.max(1));
                tree.processes = tree.processes.saturating_add(1);
                if let Some(children) = children.get(&pid) {
                    stack.extend(children);
                }
            }
            Some(tree)
        }

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("ferramari-proc-bench-{unique}"));
        let total = 500u32;
        let tree_root = 10_000u32;
        write_fake_stat(&root, tree_root, "ferramari", 1, 10, 4, 20);
        for index in 1..total {
            let pid = tree_root + index;
            let ppid = if index <= 40 { tree_root } else { 1 };
            write_fake_stat(&root, pid, "worker", ppid, 10, 2, 8);
            let directory = root.join(pid.to_string());
            std::fs::write(
                directory.join("status"),
                format!("Name:\tworker\nVmRSS:\t  {} kB\nThreads:\t2\n", 8 * 4),
            )
            .unwrap();
        }
        let measure = |legacy: bool| {
            let mut total = 0.0;
            for _ in 0..50 {
                let started = Instant::now();
                let tree = if legacy {
                    read_tree_legacy(&root, tree_root, 4096)
                } else {
                    read_tree_from(&root, tree_root, 4096)
                };
                assert!(tree.is_some());
                total += started.elapsed().as_secs_f64() * 1000.0;
            }
            total / 50.0
        };
        let legacy = measure(true);
        let current = measure(false);
        println!(
            "linux fake /proc: {total} PIDs, legacy(stat+status)={legacy:.3} ms, \
             stat-only={current:.3} ms, speed-up={:.2}x",
            legacy / current
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn filetime_ticks_combine_the_two_words() {
        let time = windows::Win32::Foundation::FILETIME {
            dwLowDateTime: 1,
            dwHighDateTime: 2,
        };
        assert_eq!(filetime_ticks(time), (2u64 << 32) | 1);
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

    #[cfg(any(target_os = "linux", target_os = "windows"))]
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

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    #[test]
    fn a_platform_without_a_process_sampler_says_so() {
        let sample = Monitor::default().sample();
        assert_eq!(sample.cpu, Metric::Unsupported);
        assert_eq!(sample.memory, Metric::Unsupported);
        assert_eq!(sample.processes, Metric::Unsupported);
        assert_eq!(sample.threads, Metric::Unsupported);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
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
        // Prime the GPU rate (PDH needs two collections on Windows) so this
        // assertion is about the dead tree, not about the first sample.
        monitor.sample();
        let sample = monitor.sample_of(u32::MAX);
        if cfg!(any(target_os = "linux", target_os = "windows")) {
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

    #[cfg(any(target_os = "linux", target_os = "windows"))]
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

    /// `cargo test --release -p ferramari-core metrics_sample_cost -- --ignored --nocapture`
    ///
    /// The UI takes this sample once a second on Home and Performance. The
    /// distribution matters more than one stopwatch reading, so this benchmark
    /// is kept next to the sampler for both Windows and Linux. It does not run
    /// in the normal suite.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    #[ignore]
    fn metrics_sample_cost() {
        use std::time::Instant;

        let mut monitor = Monitor::default();
        let _ = monitor.sample_of(std::process::id());
        let iterations = 200;
        let mut times = Vec::with_capacity(iterations);
        for _ in 0..iterations {
            let started = Instant::now();
            let _ = monitor.sample_of(std::process::id());
            times.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean = times.iter().sum::<f64>() / times.len() as f64;
        let p50 = times[times.len() / 2];
        let p95 = times[(times.len() * 95) / 100];
        println!(
            "metrics::Monitor::sample_of: n={iterations} mean={mean:.3} ms p50={p50:.3} ms p95={p95:.3} ms max={:.3} ms",
            times.last().copied().unwrap_or(0.0)
        );
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    #[ignore]
    fn metrics_sample_breakdown() {
        use std::time::Instant;

        let pid = std::process::id();
        let iterations = 200;
        let measure = |mut action: Box<dyn FnMut()>| {
            let mut times = Vec::with_capacity(iterations);
            for _ in 0..iterations {
                let started = Instant::now();
                action();
                times.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = times.iter().sum::<f64>() / times.len() as f64;
            (
                mean,
                times[times.len() / 2],
                times[(times.len() * 95) / 100],
            )
        };
        let (tree_mean, tree_p50, tree_p95) = measure(Box::new(|| {
            let _ = read_tree(pid);
        }));
        println!("read_tree: mean={tree_mean:.3} ms p50={tree_p50:.3} ms p95={tree_p95:.3} ms");
        let (memory_mean, _, _) = measure(Box::new(|| {
            let _ = total_memory();
        }));
        println!("total_memory: mean={memory_mean:.3} ms");
        #[cfg(target_os = "windows")]
        {
            let mut sampler = windows_gpu::Sampler::default();
            let _ = sampler.sample();
            let (gpu_mean, gpu_p50, gpu_p95) = measure(Box::new(move || {
                let _ = sampler.sample();
            }));
            println!("gpu sample: mean={gpu_mean:.3} ms p50={gpu_p50:.3} ms p95={gpu_p95:.3} ms");
        }
        #[cfg(target_os = "linux")]
        {
            let (gpu_mean, gpu_p50, gpu_p95) = measure(Box::new(|| {
                let _ = gpu_percent(Path::new("/sys/class/drm"));
            }));
            println!("gpu sample: mean={gpu_mean:.3} ms p50={gpu_p50:.3} ms p95={gpu_p95:.3} ms");
        }
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
