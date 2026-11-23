//! Minimal libmpv control through its JSON IPC channel.
//!
//! mpv is started as a child process and draws directly into the native window
//! identified by `--wid`, so no C headers or FFI are required. All commands go
//! through `--input-ipc-server`, which keeps the interface tiny: the rest of
//! the program only sees `set_volume`, `pause`, `set_position`, and friends.
//! On Linux that channel is a Unix socket; on Windows it is a named pipe.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// The connected end of mpv's IPC channel: a Unix socket on Linux, a byte-mode
/// named pipe on Windows. Both behave as a duplex byte stream; only the
/// connect call and the native object differ.
#[cfg(unix)]
mod transport {
    use std::io::{self, Read, Write};
    use std::os::unix::net::UnixStream;
    use std::path::Path;
    use std::time::Duration;

    pub struct PipeStream(UnixStream);

    impl PipeStream {
        pub fn connect(path: &Path) -> io::Result<Self> {
            UnixStream::connect(path).map(Self)
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            self.0.try_clone().map(Self)
        }

        pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_read_timeout(timeout)
        }

        pub fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.0.set_write_timeout(timeout)
        }

        #[cfg(test)]
        pub fn from_stream(stream: UnixStream) -> Self {
            Self(stream)
        }
    }

    impl Read for PipeStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.0.read(buffer)
        }
    }

    impl Write for PipeStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.0.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.0.flush()
        }
    }
}

#[cfg(windows)]
mod transport {
    use std::fs::{File, OpenOptions};
    use std::io::{self, Read, Write};
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::Pipes::PeekNamedPipe;

    /// A client connection to mpv's named pipe. The `windows` crate has no
    /// stream wrapper, so the raw handle is adapted to `Read`/`Write`. Reads
    /// poll `PeekNamedPipe` first and honour the same timeout the Unix socket
    /// uses; without that, a wedged mpv would block the IPC worker forever.
    pub struct PipeStream {
        file: File,
        read_timeout: Option<Duration>,
    }

    impl PipeStream {
        pub fn connect(path: &Path) -> io::Result<Self> {
            // Opening read+write gives the duplex handle mpv's server expects.
            // `OpenOptions` shares read/write/delete by default, so a second
            // probe from a diagnostics tool cannot lock the player out.
            let file = OpenOptions::new().read(true).write(true).open(path)?;
            Ok(Self {
                file,
                read_timeout: None,
            })
        }

        pub fn try_clone(&self) -> io::Result<Self> {
            Ok(Self {
                file: self.file.try_clone()?,
                read_timeout: self.read_timeout,
            })
        }

        pub fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
            self.read_timeout = timeout;
            Ok(())
        }

        pub fn set_write_timeout(&mut self, _timeout: Option<Duration>) -> io::Result<()> {
            // Command payloads are far smaller than a pipe buffer, so a write
            // to a live mpv cannot block long enough to need a deadline.
            Ok(())
        }
    }

    impl Read for PipeStream {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let deadline = self.read_timeout.map(|timeout| Instant::now() + timeout);
            loop {
                let mut available = 0u32;
                unsafe {
                    PeekNamedPipe(
                        HANDLE(self.file.as_raw_handle() as _),
                        None,
                        0,
                        None,
                        Some(&mut available),
                        None,
                    )
                }
                .map_err(|err| io::Error::other(err.to_string()))?;
                if available > 0 {
                    return self.file.read(buffer);
                }
                if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "mpv IPC read timed out",
                    ));
                }
                // A named pipe has no blocking read with a deadline on the
                // synchronous handle this transport uses. Polling every 2 ms
                // kept the reader awake ~50 times per 100 ms timeout, which is
                // measurable while a wallpaper is paused. Commands are tiny
                // and the app's fades tick at 50-100 ms, so a 20 ms poll keeps
                // replies well inside that budget while cutting idle wake-ups
                // by ten. Measured on Windows: paused 1080p30 server CPU
                // 1.82% -> 0.65% of one core (60 s, same binary otherwise).
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }

    impl Write for PipeStream {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            self.file.write(buffer)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.file.flush()
        }
    }

    #[cfg(test)]
    mod tests {
        use std::fs::File;
        use std::io::{BufRead, BufReader, Read, Write};
        use std::os::windows::io::FromRawHandle;
        use std::path::Path;
        use std::time::Duration;

        use windows::core::PCWSTR;
        use windows::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
        use windows::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
        use windows::Win32::System::Pipes::{
            ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
        };

        use super::PipeStream;

        fn wide(text: &str) -> Vec<u16> {
            text.encode_utf16().chain(std::iter::once(0)).collect()
        }

        fn unique_pipe_name() -> String {
            // A monotonic-ish suffix keeps parallel test runs and repeated
            // invocations from colliding on the same pipe instance.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_nanos())
                .unwrap_or(0);
            format!(r"\\.\pipe\ferramari-test-{}-{nanos}", std::process::id())
        }

        fn server_pipe(name: &str) -> HANDLE {
            let name = wide(name);
            let handle = unsafe {
                CreateNamedPipeW(
                    PCWSTR(name.as_ptr()),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    1,
                    4096,
                    4096,
                    0,
                    None,
                )
            };
            assert_ne!(handle, INVALID_HANDLE_VALUE);
            handle
        }

        /// The whole point of the wrapper: a reply written by mpv's server
        /// must arrive through the byte stream `BufReader` reads. `HANDLE` is
        /// `!Send`, so the server side crosses the thread boundary as a raw
        /// value, exactly like the window handles in the platform crate.
        #[test]
        fn a_reply_crosses_a_real_named_pipe() {
            let name = unique_pipe_name();
            let server = server_pipe(&name);
            let raw = server.0 as isize;
            let peer = std::thread::spawn(move || {
                let server = HANDLE(raw as *mut core::ffi::c_void);
                // The client may already be connected when this runs.
                let _ = unsafe { ConnectNamedPipe(server, None) };
                let mut file = unsafe { File::from_raw_handle(server.0) };
                file.write_all(b"{\"request_id\":0,\"error\":\"success\"}\n")
                    .unwrap();
                file.flush().unwrap();
            });

            let mut client = PipeStream::connect(Path::new(&name)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut reader = BufReader::new(client.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert!(line.contains("\"request_id\":0"), "{line}");
            peer.join().unwrap();
        }

        #[test]
        fn a_silent_pipe_times_out_instead_of_blocking() {
            let name = unique_pipe_name();
            let server = server_pipe(&name);
            let raw = server.0 as isize;
            let peer = std::thread::spawn(move || {
                let server = HANDLE(raw as *mut core::ffi::c_void);
                let _ = unsafe { ConnectNamedPipe(server, None) };
                // Hold the connection open without writing.
                std::thread::sleep(Duration::from_secs(2));
            });

            let mut client = PipeStream::connect(Path::new(&name)).unwrap();
            client
                .set_read_timeout(Some(Duration::from_millis(50)))
                .unwrap();
            let mut buffer = [0u8; 32];
            let error = client.read(&mut buffer).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
            peer.join().unwrap();
        }
    }
}

use transport::PipeStream;

static SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// How long a single command may wait for mpv to answer. Commands only set
/// properties, so a healthy mpv replies immediately. The wait happens on the
/// IPC worker thread, never on the main application event loop.
const IPC_TIMEOUT: Duration = Duration::from_secs(1);

/// How often the reader wakes to notice `reader_stop`. Shorter than
/// [`IPC_TIMEOUT`] so dropping a player does not stall the supervisor for a
/// whole command timeout; the command wait is independent of this poll.
const READER_POLL: Duration = Duration::from_millis(100);

/// How long to wait for the `--input-ipc-server` socket to appear. mpv usually
/// creates it in ~120 ms; a missing socket points at a broken binary or a
/// severely wedged process.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Native window to render into: an X11 window id on Linux, an HWND on
    /// Windows. `None` opens mpv's own window.
    pub wid: Option<u64>,
    pub source: String,
    /// Separate audio track (yt-dlp streams).
    pub audio_file: Option<String>,
    pub no_audio: bool,
    pub volume: i64,
    pub muted: bool,
    pub paused: bool,
    pub loop_file: bool,
    /// `performance`, `balanced` or `power`.
    pub profile: String,
    /// `auto`, `vaapi` or `none`.
    pub hwdec: String,
}

// The user-facing hardware-decoding vocabulary (`hwdec_arg`, `HwDec`,
// `HwDecCheck`) lives in `ferramari_core::hwdec`, because the control surface
// has to render the same verdict without depending on this crate. It is
// re-exported here so media call sites read naturally. The `hwdec-current`
// reply (verified on mpv 0.41; the older `current-hwdec` name does not exist)
// reads `no` when the requested API silently fell back, names the API when
// hardware is active, and is *unavailable* before a file has been decoded —
// which is what `HwDec::from_report` interprets.
pub use ferramari_core::hwdec::{hwdec_arg, HwDec, HwDecCheck};

/// The base `vf` chain for a profile. Eco starts with a 30 fps cap; the idle
/// policy may replace it with a lower cap and must restore this value later.
pub fn profile_video_filter(profile: &str) -> &'static str {
    match profile {
        "power" => "fps=30",
        _ => "",
    }
}

/// Extra mpv flags for the selected power profile.
fn profile_args(profile: &str) -> &'static [&'static str] {
    match profile {
        "performance" => &["--video-sync=display-resample", "--interpolation=yes"],
        "power" => &[
            "--framedrop=vo",
            "--vd-lavc-threads=1",
            "--scale=bilinear",
            "--cscale=bilinear",
            "--dscale=bilinear",
            "--vf=fps=30",
        ],
        _ => &[],
    }
}

pub struct Mpv {
    child: Child,
    /// Unix socket path or Windows named-pipe path, kept for cleanup/logging.
    endpoint: String,
    requests: Sender<Request>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

pub use ferramari_core::stats::PlaybackStats;

/// State readable from the command-loop thread without touching the IPC socket.
struct Shared {
    volume: AtomicI64,
    muted: AtomicBool,
    paused: AtomicBool,
    /// Set when the IPC worker loses the connection. The child may still be
    /// alive but can no longer be controlled, so the caller restarts it.
    ipc_dead: AtomicBool,
    /// True while the worker thread is running. A panic or an early return
    /// must not look like a healthy player just because the child is alive.
    worker_alive: AtomicBool,
    /// Last command-level error worth showing to the user (for example a
    /// `loadfile` of a corrupt file). The UI polls this on the supervision
    /// tick instead of blocking on the reply.
    last_error: Mutex<Option<String>>,
    /// Playback position captured before a parked decoder released the file.
    parked_position: Mutex<Option<f64>>,
    /// True between a successful `stop` and the next `unpark`.
    parked: AtomicBool,
    /// Last `hwdec-current` reading, so the owning thread can validate the
    /// hardware decoding request without waiting on the IPC socket.
    hwdec_current: Mutex<Option<String>>,
    /// Last full statistics probe, written by the IPC worker and read by the
    /// command loop once a second.
    stats: Mutex<PlaybackStats>,
}

/// Non-blocking liveness of one player's control channel. Kept separate from
/// the child poll so snapshots can ask without touching the process handle.
fn shared_is_alive(shared: &Shared) -> bool {
    !shared.ipc_dead.load(Ordering::Relaxed) && shared.worker_alive.load(Ordering::Relaxed)
}

/// Commands for the dedicated IPC thread. Keeping them here means a wedged
/// mpv can never stall the application event loop: the caller only enqueues
/// work.
enum Request {
    Execute(Value),
    /// Read the decoder facts the control surface shows: the hardware API in
    /// use, the reported frame rate, codec, geometry, dropped frames and the
    /// position. The properties only exist once a file is being decoded, so a
    /// missing reply simply leaves the previous reading in place and the
    /// caller probes again next tick.
    ProbeStats,
    LoadFile(String),
    /// Remember `time-pos`, switch mpv to idle and unload the file. This frees
    /// the decoder and its buffers while keeping the process and IPC socket.
    Park,
    /// Reload the source at the remembered position. Falls back to the start
    /// when no position was captured.
    Unpark(String),
    Quit,
}

/// One JSON IPC connection. mpv interleaves unsolicited events with command
/// replies on the same stream, so the reader has to be persistent: creating a
/// `BufReader` per command would buffer (and drop) replies that arrived early.
///
/// The reader also runs on its own thread. mpv's Windows IPC server writes
/// events from the same thread that reads commands; if the client stops
/// reading between commands, the 4 KiB pipe buffer fills and mpv blocks in
/// `WriteFile`, so it never reads the next command. Draining the stream
/// continuously is what keeps both directions moving. On Unix the same shape
/// costs one thread per player and removes the same class of stall.
struct Ipc {
    /// Second handle on the same channel: `BufReader` does not implement
    /// `Write`, and replies must be read through the buffered reader.
    writer: PipeStream,
    /// Lines forwarded by the reader thread. A disconnected channel means the
    /// reader hit EOF or an I/O error.
    lines: Receiver<String>,
    reader_stop: Arc<AtomicBool>,
    reader: Option<JoinHandle<()>>,
    next_request: u64,
}

/// Whether a read error is the reader's idle tick rather than a lost channel.
///
/// Unix reports `SO_RCVTIMEO` as `WouldBlock` (EAGAIN); Windows named pipes
/// report `TimedOut`. Both are the normal "no event yet" path in
/// [`Ipc::new`]'s reader, and neither means the player is gone.
fn is_read_poll_timeout(kind: std::io::ErrorKind) -> bool {
    matches!(
        kind,
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

impl Ipc {
    fn new(mut stream: PipeStream) -> anyhow::Result<Self> {
        // The reader must wake periodically so it can notice `reader_stop`;
        // without a timeout a dropped connection would keep it blocked and
        // `Drop` would join it forever.
        stream.set_read_timeout(Some(READER_POLL))?;
        let reader_stream = stream.try_clone()?;
        let (lines, incoming) = mpsc::channel();
        let reader_stop = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&reader_stop);
        let reader = std::thread::Builder::new()
            .name("ferramari-mpv-reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(reader_stream);
                let mut line = String::new();
                loop {
                    match reader.read_line(&mut line) {
                        Ok(0) => break,
                        Ok(_) => {
                            if lines.send(std::mem::take(&mut line)).is_err() {
                                break;
                            }
                        }
                        // The socket timeout is the worker's idle tick, not a
                        // failure: keep a partial line and wait for the rest.
                        // Unix reports `SO_RCVTIMEO` as `WouldBlock` (EAGAIN),
                        // while Windows named pipes report `TimedOut`; both
                        // mean "no event yet".
                        Err(err) if is_read_poll_timeout(err.kind()) => {
                            if stop.load(Ordering::Relaxed) {
                                break;
                            }
                        }
                        Err(err) => {
                            if !stop.load(Ordering::Relaxed) {
                                log::debug!("[mpv] IPC reader stopped: {err}");
                            }
                            break;
                        }
                    }
                }
            })?;
        Ok(Self {
            writer: stream,
            lines: incoming,
            reader_stop,
            reader: Some(reader),
            next_request: 0,
        })
    }
}

impl Drop for Ipc {
    fn drop(&mut self) {
        // The reader wakes on its next socket timeout and exits; joining here
        // reaps it instead of leaving a thread behind after a restart.
        self.reader_stop.store(true, Ordering::Relaxed);
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

impl Mpv {
    pub fn spawn(options: &Options) -> anyhow::Result<Self> {
        let unique = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let endpoint = ipc_endpoint(unique);

        let mut command = Command::new(ferramari_core::paths::tool("mpv"));
        command
            .arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--no-config")
            .arg("--no-input-default-bindings")
            // Let the wallpaper window receive clicks, disabling mpv's own
            // mouse/key input, so the right-click menu works.
            .arg("--input-cursor=no")
            .arg("--input-vo-keyboard=no")
            .arg("--no-osc")
            .arg("--no-osd-bar")
            .arg("--osd-level=0")
            .arg("--cursor-autohide=no")
            .arg("--no-stop-screensaver")
            .arg("--keepaspect=yes")
            .arg("--panscan=1.0")
            .arg(format!("--hwdec={}", hwdec_arg(&options.hwdec)))
            .arg("--no-border");
        #[cfg(unix)]
        {
            // mpv must render through X11 even on a Wayland session: it is
            // given an XWayland window id, and its GPU context
            // auto-detection would otherwise pick a Wayland surface that does
            // not belong to our window.
            command.env_remove("WAYLAND_DISPLAY");
            command.arg("--gpu-context=x11egl");
        }
        for argument in profile_args(&options.profile) {
            command.arg(argument);
        }
        command
            .arg(format!("--input-ipc-server={endpoint}"))
            .arg(format!("--volume={}", options.volume.clamp(0, 100)))
            .arg(format!(
                "--mute={}",
                if options.muted { "yes" } else { "no" }
            ))
            .arg(format!(
                "--pause={}",
                if options.paused { "yes" } else { "no" }
            ))
            .arg(format!(
                "--loop-file={}",
                if options.loop_file { "inf" } else { "no" }
            ));
        if let Some(wid) = options.wid {
            command.arg(format!("--wid={wid}"));
        }
        if options.no_audio {
            command.arg("--no-audio");
        }
        if let Some(audio) = &options.audio_file {
            command.arg(format!("--audio-file={audio}"));
        }
        command
            .arg("--")
            .arg(&options.source)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());

        log::debug!("[mpv] spawn {:?}", command);
        let mut child = command.spawn().map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!("mpv was not found in PATH (install mpv to play videos)")
            } else {
                anyhow::Error::from(err)
            }
        })?;

        // Connecting to the socket and every later command happen on this
        // worker. `spawn` returns as soon as the child process exists, which
        // keeps a broken or slow mpv away from the command-loop thread.
        let shared = Arc::new(Shared {
            volume: AtomicI64::new(options.volume.clamp(0, 100)),
            muted: AtomicBool::new(options.muted),
            paused: AtomicBool::new(options.paused),
            ipc_dead: AtomicBool::new(false),
            worker_alive: AtomicBool::new(true),
            last_error: Mutex::new(None),
            parked_position: Mutex::new(None),
            parked: AtomicBool::new(false),
            hwdec_current: Mutex::new(None),
            stats: Mutex::new(PlaybackStats::default()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();
        let worker = {
            let endpoint = endpoint.clone();
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            match std::thread::Builder::new()
                .name("ferramari-mpv-ipc".to_string())
                .spawn(move || run_ipc_worker(&endpoint, receiver, &shared, &stop))
            {
                Ok(worker) => worker,
                Err(err) => {
                    // The child was already spawned; do not leak an mpv
                    // process just because the IPC thread could not start.
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(err.into());
                }
            }
        };

        Ok(Self {
            child,
            endpoint,
            requests,
            shared,
            stop,
            worker: Some(worker),
        })
    }

    fn set_property(&self, name: &str, value: Value) -> anyhow::Result<()> {
        self.enqueue(Request::Execute(json!(["set_property", name, value])))
    }

    fn enqueue(&self, request: Request) -> anyhow::Result<()> {
        if self.shared.ipc_dead.load(Ordering::Relaxed) {
            anyhow::bail!("mpv IPC connection is closed");
        }
        self.requests
            .send(request)
            .map_err(|_| anyhow::anyhow!("mpv IPC worker has stopped"))
    }

    pub fn set_volume(&mut self, volume: i64) -> anyhow::Result<()> {
        let volume = volume.clamp(0, 100);
        self.shared.volume.store(volume, Ordering::Relaxed);
        self.set_property("volume", json!(volume))
    }

    pub fn set_mute(&mut self, muted: bool) -> anyhow::Result<()> {
        self.shared.muted.store(muted, Ordering::Relaxed);
        self.set_property("mute", json!(muted))
    }

    pub fn set_pause(&mut self, paused: bool) -> anyhow::Result<()> {
        self.shared.paused.store(paused, Ordering::Relaxed);
        self.set_property("pause", json!(paused))
    }

    /// Replace the video filter chain. mpv accepts `vf` as a runtime property
    /// (verified with `set_property vf "fps=N"`); an empty string clears it.
    /// This is used by the Eco Engine to lower the frame rate while idle.
    pub fn set_video_filter(&mut self, filter: &str) -> anyhow::Result<()> {
        self.set_property("vf", json!(filter))
    }

    /// Ask the worker to read the decoder properties. Queued, never blocking:
    /// the results appear in [`Mpv::hwdec_current`] and [`Mpv::stats`] on a
    /// later supervision tick.
    pub fn probe_hwdec(&mut self) -> anyhow::Result<()> {
        self.enqueue(Request::ProbeStats)
    }

    /// The decoder mpv is actually using, from the last successful probe.
    /// [`HwDec::Unknown`] until a file is decoded and a reply has arrived.
    pub fn hwdec_current(&self) -> HwDec {
        let value = self
            .shared
            .hwdec_current
            .lock()
            .ok()
            .and_then(|current| current.clone());
        HwDec::from_report(value.as_deref())
    }

    /// Everything the last probe learned about the running decoder. Reads a
    /// mutex written by the IPC worker, so the command loop never waits on mpv.
    pub fn stats(&self) -> PlaybackStats {
        self.shared
            .stats
            .lock()
            .map(|stats| stats.clone())
            .unwrap_or_default()
    }

    /// The volume last applied to mpv. Reads an atomic, so fade steps never
    /// wait for the IPC worker.
    pub fn volume(&self) -> i64 {
        self.shared.volume.load(Ordering::Relaxed)
    }

    /// Take the last command error for the UI. Called on the supervision tick,
    /// never from the IPC worker.
    pub fn take_error(&self) -> Option<String> {
        self.shared
            .last_error
            .lock()
            .ok()
            .and_then(|mut error| error.take())
    }

    /// Replace the current file without restarting mpv. Used when a monitor
    /// switches to another local video.
    ///
    /// The request is queued; the worker waits for mpv's reply off the
    /// command loop. This keeps the UI responsive even when the decoder needs
    /// a moment to open the new file.
    pub fn load_file(&mut self, source: &str) -> anyhow::Result<()> {
        self.shared.parked.store(false, Ordering::Relaxed);
        if let Ok(mut position) = self.shared.parked_position.lock() {
            *position = None;
        }
        self.enqueue(Request::LoadFile(source.to_string()))
    }

    /// Release the decoder after a long hidden pause. The IPC worker reads
    /// `time-pos`, enables idle mode and sends `stop`; the process and socket
    /// stay alive, so resuming does not pay the cold-start cost.
    pub fn park(&mut self) -> anyhow::Result<()> {
        self.enqueue(Request::Park)
    }

    /// Reload the parked source and seek to the position captured by `park`.
    pub fn unpark(&mut self, source: &str) -> anyhow::Result<()> {
        self.enqueue(Request::Unpark(source.to_string()))
    }

    /// Whether the player still has a live IPC worker and has not reported a
    /// fatal transport failure. This is the non-blocking half of
    /// [`Mpv::is_running`], usable from a snapshot without polling the child.
    pub fn is_alive(&self) -> bool {
        shared_is_alive(&self.shared)
    }

    /// Reap the child without blocking. A missing status means mpv exited or
    /// the IPC worker lost the connection; the caller restarts the window.
    pub fn is_running(&mut self) -> bool {
        if self.shared.ipc_dead.load(Ordering::Relaxed) {
            log::warn!("[mpv] IPC connection lost unexpectedly");
            return false;
        }
        if !self.shared.worker_alive.load(Ordering::Relaxed) {
            log::warn!("[mpv] IPC worker stopped unexpectedly");
            return false;
        }
        match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                log::warn!("[mpv] exited unexpectedly ({status})");
                false
            }
            Err(err) => {
                log::warn!("[mpv] could not poll the child: {err}");
                false
            }
        }
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Ask mpv to quit, then give it a short grace period. A wedged
        // decoder must not keep the Ferramari process (or the session) alive.
        // Always wake the IPC worker, even when the child already exited:
        // otherwise it would stay blocked in `recv()` and `join()` below would
        // deadlock (the request sender is only dropped after this function).
        let _ = self.requests.send(Request::Quit);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        log::debug!("[mpv] stopping {}", self.endpoint);
        // Named pipes disappear with their last handle; only the Unix socket
        // leaves a filesystem entry behind.
        #[cfg(unix)]
        let _ = std::fs::remove_file(&self.endpoint);
        // The worker exits on `Quit`, on a closed request channel or after a
        // failed connection; the stop flag only wakes its connect loop.
        // Joining it reaps the IPC thread without a long wait.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The address mpv should listen on for this player instance.
///
/// Unix gets a per-process socket under the user cache directory; Windows a
/// named pipe in the local pipe namespace. Neither is persisted, and the
/// counter keeps several monitors' players apart within one process.
#[cfg(unix)]
fn ipc_endpoint(unique: usize) -> String {
    let socket_dir = ferramari_core::paths::mpv_socket_dir();
    let _ = std::fs::create_dir_all(&socket_dir);
    let socket = socket_dir.join(format!(
        "ferramari-mpv-{}-{unique}.sock",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&socket);
    socket.to_string_lossy().into_owned()
}

#[cfg(windows)]
fn ipc_endpoint(unique: usize) -> String {
    format!(r"\\.\pipe\ferramari-mpv-{}-{unique}", std::process::id())
}

/// Wait for mpv's IPC endpoint, then serve requests until the channel closes
/// or a fatal I/O error occurs. Runs on its own worker thread.
fn run_ipc_worker(endpoint: &str, receiver: Receiver<Request>, shared: &Shared, stop: &AtomicBool) {
    // Covers every exit path, including a panic: `is_running` must never
    // report a player whose command channel is gone as healthy.
    struct AliveGuard<'a>(&'a AtomicBool);
    impl Drop for AliveGuard<'_> {
        fn drop(&mut self) {
            self.0.store(false, Ordering::Relaxed);
        }
    }
    let _alive = AliveGuard(&shared.worker_alive);
    let Some(ipc) = connect_ipc(endpoint, stop) else {
        shared.ipc_dead.store(true, Ordering::Relaxed);
        return;
    };
    serve_ipc(ipc, receiver, shared, stop);
}

fn serve_ipc(mut ipc: Ipc, receiver: Receiver<Request>, shared: &Shared, _stop: &AtomicBool) {
    while let Ok(request) = receiver.recv() {
        match request {
            Request::Execute(args) => {
                if let Err(err) = exchange(&mut ipc, args) {
                    if record_failure(&err, false, shared) {
                        break;
                    }
                }
            }
            Request::ProbeStats => match probe_stats(&mut ipc, shared) {
                Ok(()) => {}
                Err(err) => {
                    record_failure(&err, false, shared);
                    if is_fatal(&err) {
                        break;
                    }
                }
            },
            Request::LoadFile(source) => {
                if let Err(err) = exchange(&mut ipc, json!(["loadfile", source, "replace"])) {
                    if record_failure(&err, true, shared) {
                        break;
                    }
                }
                // A park request enqueued just before this load may have left
                // mpv in idle mode with a saved position. The new file must
                // start from its own beginning and keep normal end of file.
                if let Ok(mut slot) = shared.parked_position.lock() {
                    *slot = None;
                }
                // The new file may decode on another path, so the previous
                // reading is dropped and the next probe reports the truth
                // about this source.
                if let Ok(mut slot) = shared.hwdec_current.lock() {
                    *slot = None;
                }
                shared.parked.store(false, Ordering::Relaxed);
                if let Err(err) = exchange(&mut ipc, json!(["set_property", "idle", "no"])) {
                    if record_failure(&err, false, shared) {
                        break;
                    }
                }
            }
            Request::Park => {
                let position = match exchange(&mut ipc, json!(["get_property", "time-pos"])) {
                    Ok(reply) => reply.get("data").and_then(Value::as_f64),
                    Err(err) => {
                        if record_failure(&err, false, shared) {
                            break;
                        }
                        None
                    }
                };
                if let Ok(mut slot) = shared.parked_position.lock() {
                    *slot = position.filter(|position| position.is_finite() && *position >= 0.0);
                }
                // `stop` quits mpv unless idle mode is on, and the option can
                // be changed at runtime. Unpark switches it off again so a
                // later end of file still behaves like a normal player.
                if let Err(err) = exchange(&mut ipc, json!(["set_property", "idle", "yes"])) {
                    if record_failure(&err, false, shared) {
                        break;
                    }
                }
                match exchange(&mut ipc, json!(["stop"])) {
                    Ok(_) => {
                        shared.parked.store(true, Ordering::Relaxed);
                        log::debug!(
                            "[mpv] decoder parked at {:?}",
                            shared.parked_position.lock().ok().and_then(|slot| *slot)
                        );
                    }
                    Err(err) => {
                        if record_failure(&err, false, shared) {
                            break;
                        }
                    }
                }
            }
            Request::Unpark(source) => {
                let position = shared
                    .parked_position
                    .lock()
                    .ok()
                    .and_then(|mut slot| slot.take());
                let args = match position {
                    Some(position) => {
                        json!([
                            "loadfile",
                            source,
                            "replace",
                            -1,
                            format!("start={position}")
                        ])
                    }
                    None => json!(["loadfile", source, "replace"]),
                };
                if let Err(err) = exchange(&mut ipc, args) {
                    if record_failure(&err, true, shared) {
                        break;
                    }
                }
                if let Err(err) = exchange(&mut ipc, json!(["set_property", "idle", "no"])) {
                    if record_failure(&err, false, shared) {
                        break;
                    }
                }
                shared.parked.store(false, Ordering::Relaxed);
                log::debug!("[mpv] decoder unparked");
            }
            Request::Quit => {
                log::debug!("[mpv] IPC quit requested");
                let _ = exchange(&mut ipc, json!(["quit"]));
                break;
            }
        }
    }
}

/// One `get_property` round-trip. `Ok(None)` means mpv declined to answer
/// (the usual reply before a file is decoded); an `Err` is a transport
/// failure and the caller must stop using the socket.
fn read_property(ipc: &mut Ipc, name: &str) -> anyhow::Result<Option<Value>> {
    match exchange(ipc, json!(["get_property", name])) {
        Ok(reply) => Ok(reply.get("data").cloned()),
        Err(err) if is_fatal(&err) => Err(err),
        Err(_) => Ok(None),
    }
}

/// Read the decoder properties one at a time and publish them.
///
/// mpv's batched `get` form is rejected by the builds this project targets,
/// and a list would lose every value when one property is unavailable — which
/// is the normal answer before the first frame. Property-level rejections are
/// therefore skipped; only a transport error is returned, and then whatever
/// was read up to that point is still published.
fn probe_stats(ipc: &mut Ipc, shared: &Shared) -> anyhow::Result<()> {
    let mut stats = PlaybackStats::default();
    let transport_result = (|| -> anyhow::Result<()> {
        if let Some(value) = read_property(ipc, "hwdec-current")? {
            stats.hwdec = value.as_str().unwrap_or_default().to_string();
        }
        if let Some(value) = read_property(ipc, "container-fps")? {
            stats.fps = value.as_f64().filter(|value| *value > 0.0);
        }
        // The rate the decoder is producing, which is what the frame-rate
        // tile is about: a 60 FPS file capped to 30 by the Eco Engine
        // decodes at 30, and saying so is the point of the tile.
        if let Some(value) = read_property(ipc, "estimated-vf-fps")? {
            stats.decoded_fps = value.as_f64().filter(|value| *value > 0.0);
        }
        if let Some(value) = read_property(ipc, "display-fps")? {
            stats.display_fps = value.as_f64().filter(|value| *value > 0.0);
        }
        if let Some(value) = read_property(ipc, "video-codec")? {
            stats.codec = value.as_str().unwrap_or_default().to_string();
        }
        if let Some(value) = read_property(ipc, "width")? {
            stats.width = value.as_u64().map(|value| value as u32);
        }
        if let Some(value) = read_property(ipc, "height")? {
            stats.height = value.as_u64().map(|value| value as u32);
        }
        if let Some(value) = read_property(ipc, "frame-drop-count")? {
            stats.dropped = value.as_u64();
        }
        if let Some(value) = read_property(ipc, "duration")? {
            stats.duration = value
                .as_f64()
                .filter(|value| value.is_finite() && *value > 0.0);
        }
        if let Some(value) = read_property(ipc, "time-pos")? {
            stats.position = value
                .as_f64()
                .filter(|value| value.is_finite() && *value >= 0.0);
        }
        Ok(())
    })();

    // Something decoded is what makes the rest of the card honest: a frame
    // rate or a geometry is only a fact when a file is actually running.
    stats.active = stats.fps.is_some() || stats.width.is_some();
    if let Ok(mut slot) = shared.stats.lock() {
        *slot = stats.clone();
    }
    if let Ok(mut current) = shared.hwdec_current.lock() {
        // Keep the previous reading while nothing answers, so a parked
        // decoder does not look like it fell back to software.
        if stats.active || !stats.hwdec.is_empty() {
            *current = Some(stats.hwdec.clone());
        }
    }
    transport_result
}

/// Log a command failure and update the shared error slot. Returns true when
/// the connection is unusable and the worker must stop.
fn record_failure(error: &anyhow::Error, report: bool, shared: &Shared) -> bool {
    if is_fatal(error) {
        log::warn!("[mpv] IPC connection failed: {error}");
        shared.ipc_dead.store(true, Ordering::Relaxed);
        return true;
    }
    // A rejected property or a missing file is not a transport failure; keep
    // the connection for the next command.
    log::warn!("[mpv] command failed: {error}");
    if report {
        if let Ok(mut slot) = shared.last_error.lock() {
            *slot = Some(error.to_string());
        }
    }
    false
}

fn connect_ipc(endpoint: &str, stop: &AtomicBool) -> Option<Ipc> {
    let deadline = Instant::now() + SOCKET_TIMEOUT;
    let path = std::path::Path::new(endpoint);
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        match PipeStream::connect(path) {
            Ok(mut stream) => {
                stream.set_read_timeout(Some(IPC_TIMEOUT)).ok()?;
                stream.set_write_timeout(Some(IPC_TIMEOUT)).ok()?;
                match Ipc::new(stream) {
                    Ok(ipc) => {
                        log::debug!("[mpv] IPC connected: {endpoint}");
                        return Some(ipc);
                    }
                    Err(err) => {
                        log::warn!("[mpv] could not start the IPC reader: {err}");
                        return None;
                    }
                }
            }
            Err(err) if Instant::now() < deadline => {
                let _ = err;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => {
                log::warn!("[mpv] IPC endpoint {endpoint} never appeared: {err}");
                return None;
            }
        }
    }
}

/// An I/O failure leaves unread bytes behind, so the stream cannot be reused.
/// A command-level rejection (`error` != `success`) is recoverable.
fn is_fatal(error: &anyhow::Error) -> bool {
    error.downcast_ref::<std::io::Error>().is_some()
}

/// Send one command and return the matching reply, skipping events and replies
/// to requests that already timed out.
fn exchange(ipc: &mut Ipc, args: Value) -> anyhow::Result<Value> {
    let request_id = ipc.next_request;
    ipc.next_request += 1;
    let request = json!({ "command": args, "request_id": request_id });
    writeln!(ipc.writer, "{request}")?;
    ipc.writer.flush()?;
    loop {
        let line = match ipc.lines.recv_timeout(IPC_TIMEOUT) {
            Ok(line) => line,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("mpv did not answer request {request_id} within {IPC_TIMEOUT:?}"),
                )
                .into());
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof).into());
            }
        };
        let Some(response) = next_response(&line, request_id)? else {
            continue;
        };
        return Ok(response);
    }
}

/// Pick the reply for `request_id` out of one IPC line.
///
/// Returns `Ok(None)` for events, blank lines and replies to other requests.
/// mpv sends `start-file`, `file-loaded`, `audio-reconfig` and friends without
/// being asked, so those lines must never be mistaken for command results.
fn next_response(line: &str, request_id: u64) -> anyhow::Result<Option<Value>> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let Ok(response) = serde_json::from_str::<Value>(line) else {
        log::debug!("[mpv] ignoring malformed IPC line: {line}");
        return Ok(None);
    };
    if response.get("event").is_some() {
        return Ok(None);
    }
    if response.get("request_id").and_then(Value::as_u64) != Some(request_id) {
        return Ok(None);
    }
    if response.get("error").and_then(Value::as_str) != Some("success") {
        anyhow::bail!("mpv rejected the command: {line}");
    }
    Ok(Some(response))
}

#[cfg(test)]
mod tests {
    use super::*;

    // The IPC tests drive a real bidirectional byte stream. On Unix that is a
    // socket pair; `PipeStream` is exercised on Windows by the named-pipe test
    // inside the transport module.
    #[cfg(unix)]
    use std::os::unix::net::UnixStream;

    #[test]
    fn power_profiles_add_only_their_own_flags() {
        assert!(profile_args("balanced").is_empty());
        assert!(profile_args("performance").contains(&"--interpolation=yes"));
        assert!(profile_args("power").contains(&"--vd-lavc-threads=1"));
        assert!(profile_args("power").contains(&"--scale=bilinear"));
        assert_eq!(profile_video_filter("power"), "fps=30");
        assert_eq!(profile_video_filter("balanced"), "");
        // The runtime cap must replace the same filter the profile starts with.
        assert!(profile_args("power").contains(&"--vf=fps=30"));
        assert_eq!(hwdec_arg("auto"), "auto-safe");
        assert_eq!(hwdec_arg("vaapi"), "vaapi");
        assert_eq!(hwdec_arg("none"), "no");
        assert_eq!(hwdec_arg("unknown"), "auto-safe");
    }

    /// A silent Unix socket reports `SO_RCVTIMEO` as `WouldBlock`, not
    /// `TimedOut`. Treating it as a lost channel made the IPC reader exit
    /// after the first 100 ms with no events, so mpv was restarted in a loop.
    #[cfg(unix)]
    #[test]
    fn a_silent_unix_socket_is_an_idle_tick() {
        use std::io::Read as _;

        let (mut client, _server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_millis(50)))
            .unwrap();
        let mut buffer = [0u8; 8];
        let error = client.read(&mut buffer).unwrap_err();
        assert!(
            is_read_poll_timeout(error.kind()),
            "unexpected read timeout kind: {:?}",
            error.kind()
        );
    }

    #[test]
    fn only_timeouts_are_idle_ticks() {
        use std::io::ErrorKind;

        assert!(is_read_poll_timeout(ErrorKind::TimedOut));
        assert!(is_read_poll_timeout(ErrorKind::WouldBlock));
        assert!(!is_read_poll_timeout(ErrorKind::UnexpectedEof));
        assert!(!is_read_poll_timeout(ErrorKind::ConnectionReset));
    }

    #[test]
    fn events_are_not_command_replies() {
        let event = r#"{"event":"start-file"}"#;
        assert!(next_response(event, 4).unwrap().is_none());
        let reconfigure = r#"{"event":"audio-reconfig","id":0}"#;
        assert!(next_response(reconfigure, 4).unwrap().is_none());
    }

    #[test]
    fn stale_replies_are_skipped() {
        let stale = r#"{"request_id":3,"error":"success"}"#;
        assert!(next_response(stale, 4).unwrap().is_none());
        let current = r#"{"request_id":4,"error":"success","data":null}"#;
        assert!(next_response(current, 4).unwrap().is_some());
    }

    #[test]
    fn command_errors_are_reported() {
        let error = r#"{"request_id":1,"error":"property unavailable"}"#;
        let err = next_response(error, 1).unwrap_err().to_string();
        assert!(err.contains("property unavailable"), "{err}");
    }

    #[test]
    fn malformed_lines_are_ignored() {
        assert!(next_response("not json", 1).unwrap().is_none());
        assert!(next_response("", 1).unwrap().is_none());
    }

    /// End-to-end: events written before the reply must not desync the stream,
    /// and the reply must be read even though it arrives after them.
    #[cfg(unix)]
    #[test]
    fn exchange_reads_past_events() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut ipc = Ipc::new(PipeStream::from_stream(client)).unwrap();

        let peer = std::thread::spawn(move || {
            {
                let mut request = String::new();
                BufReader::new(&mut server).read_line(&mut request).unwrap();
                assert!(request.contains("\"request_id\":0"), "{request}");
            }
            server
                .write_all(b"{\"event\":\"audio-reconfig\"}\n")
                .unwrap();
            server
                .write_all(b"{\"request_id\":0,\"error\":\"success\"}\n")
                .unwrap();
            server.flush().unwrap();
        });

        let response = exchange(&mut ipc, json!(["set_property", "volume", 12])).unwrap();
        peer.join().unwrap();
        assert_eq!(response["error"], "success");
    }

    #[cfg(unix)]
    fn test_ipc(client: UnixStream) -> Ipc {
        Ipc::new(PipeStream::from_stream(client)).unwrap()
    }

    #[test]
    fn a_dead_ipc_or_stopped_worker_is_not_alive() {
        let shared = test_shared();
        assert!(shared_is_alive(&shared));
        shared.worker_alive.store(false, Ordering::Relaxed);
        assert!(!shared_is_alive(&shared));
        shared.worker_alive.store(true, Ordering::Relaxed);
        shared.ipc_dead.store(true, Ordering::Relaxed);
        assert!(!shared_is_alive(&shared));
    }

    fn test_shared() -> Shared {
        Shared {
            volume: AtomicI64::new(50),
            muted: AtomicBool::new(false),
            paused: AtomicBool::new(true),
            ipc_dead: AtomicBool::new(false),
            worker_alive: AtomicBool::new(true),
            last_error: Mutex::new(None),
            parked_position: Mutex::new(None),
            parked: AtomicBool::new(false),
            hwdec_current: Mutex::new(None),
            stats: Mutex::new(PlaybackStats::default()),
        }
    }

    #[cfg(unix)]
    #[test]
    fn worker_executes_commands_in_order_and_survives_a_rejection() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let ipc = test_ipc(client);
        let shared = Arc::new(test_shared());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();

        let peer = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            // First command is rejected by mpv; the worker must keep going.
            for (expected, error) in [
                ("set_property", "property unavailable"),
                ("loadfile", "loading failed"),
                ("set_property", "success"),
                ("loadfile", "success"),
                ("set_property", "success"),
                ("set_property", "success"),
                ("quit", "success"),
            ] {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.contains(expected), "{line}");
                let request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request["request_id"].as_u64().unwrap();
                let reply = json!({ "request_id": request_id, "error": error });
                writeln!(server, "{reply}").unwrap();
                server.flush().unwrap();
            }
        });

        let worker = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || serve_ipc(ipc, receiver, &shared, &stop))
        };
        requests
            .send(Request::Execute(json!(["set_property", "volume", 10])))
            .unwrap();
        requests
            .send(Request::LoadFile("/tmp/corrupt.mp4".into()))
            .unwrap();
        requests
            .send(Request::LoadFile("/tmp/a.mp4".into()))
            .unwrap();
        requests
            .send(Request::Execute(json!(["set_property", "mute", true])))
            .unwrap();
        requests.send(Request::Quit).unwrap();
        drop(requests);

        worker.join().unwrap();
        peer.join().unwrap();
        assert!(!shared.ipc_dead.load(Ordering::Relaxed));
        let error = shared.last_error.lock().unwrap().take().unwrap();
        assert!(error.contains("loading failed"), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn worker_parks_and_unparks_at_the_saved_position() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let ipc = test_ipc(client);
        let shared = Arc::new(test_shared());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();

        let peer = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            // (expected substring, reply data, error)
            let replies = [
                ("\"time-pos\"", Some(json!(42.5)), "success"),
                ("\"set_property\",\"idle\",\"yes\"", None, "success"),
                ("\"stop\"", None, "success"),
                (
                    "\"loadfile\",\"/tmp/a.mp4\",\"replace\",-1,\"start=42.5\"",
                    None,
                    "success",
                ),
                ("\"set_property\",\"idle\",\"no\"", None, "success"),
                ("\"quit\"", None, "success"),
            ];
            for (expected, data, error) in replies {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                assert!(line.contains(expected), "{line}");
                let request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request["request_id"].as_u64().unwrap();
                let mut reply = json!({ "request_id": request_id, "error": error });
                if let Some(data) = data {
                    reply["data"] = data;
                }
                writeln!(server, "{reply}").unwrap();
                server.flush().unwrap();
            }
        });

        let worker = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || serve_ipc(ipc, receiver, &shared, &stop))
        };
        requests.send(Request::Park).unwrap();
        // Wait for the worker to finish `Park` before checking its state.
        let deadline = Instant::now() + Duration::from_secs(2);
        while !shared.parked.load(Ordering::Relaxed) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(shared.parked.load(Ordering::Relaxed));
        assert_eq!(*shared.parked_position.lock().unwrap(), Some(42.5));
        assert!(requests.send(Request::Unpark("/tmp/a.mp4".into())).is_ok());
        requests.send(Request::Quit).unwrap();
        drop(requests);

        worker.join().unwrap();
        peer.join().unwrap();
        assert!(!shared.parked.load(Ordering::Relaxed));
        assert_eq!(*shared.parked_position.lock().unwrap(), None);
        assert!(!shared.ipc_dead.load(Ordering::Relaxed));
    }

    /// A successful probe records the decoder the player picked, which is the
    /// only way to learn that a requested API silently fell back.
    #[cfg(unix)]
    #[test]
    fn a_hwdec_probe_records_the_reported_decoder() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let ipc = test_ipc(client);
        let shared = Arc::new(test_shared());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();

        // The probe reads every decoder property in one pass, so the peer
        // answers by the property named in the request rather than by a
        // fixed script: a script of three replies would be exhausted by the
        // first probe and the worker would sit out its read timeout on the
        // fourth property. `hwdec-current` is the one answer that changes
        // between the two rounds, which is what the test asserts on.
        let peer = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let mut probed = 0u32;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request["request_id"].as_u64().unwrap();
                let property = request["command"]
                    .get(1)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();

                let data = match property.as_str() {
                    "hwdec-current" => {
                        probed += 1;
                        Some(json!(if probed == 1 { "vulkan" } else { "no" }))
                    }
                    // The rest of the probe's properties, answered with the
                    // shape mpv uses. A number where mpv would say
                    // "unavailable" is what the probe's per-property skip is
                    // meant to survive, so they are given real values here.
                    "container-fps" => Some(json!(59.94)),
                    "estimated-vf-fps" => Some(json!(58.7)),
                    "display-fps" => Some(json!(119.8)),
                    "video-codec" => Some(json!("H.264 / AVC / MPEG-4 AVC")),
                    "width" => Some(json!(3840)),
                    "height" => Some(json!(2160)),
                    "frame-drop-count" => Some(json!(3)),
                    "duration" => Some(json!(35.3)),
                    "time-pos" => Some(json!(1.6)),
                    "quit" => None,
                    _ => None,
                };

                let mut reply = json!({ "request_id": request_id, "error": "success" });
                if let Some(data) = data {
                    reply["data"] = data;
                }
                writeln!(server, "{reply}").unwrap();
                server.flush().unwrap();
                if property == "quit" {
                    break;
                }
            }
        });

        let worker = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || serve_ipc(ipc, receiver, &shared, &stop))
        };

        requests.send(Request::ProbeStats).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.hwdec_current.lock().unwrap().as_deref() != Some("vulkan")
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            shared.hwdec_current.lock().unwrap().as_deref(),
            Some("vulkan")
        );

        // `no` is mpv's way of saying the requested API is not in use.
        requests.send(Request::ProbeStats).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while shared.hwdec_current.lock().unwrap().as_deref() != Some("no")
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(shared.hwdec_current.lock().unwrap().as_deref(), Some("no"));

        requests.send(Request::Quit).unwrap();
        drop(requests);
        worker.join().unwrap();
        peer.join().unwrap();
        assert!(!shared.ipc_dead.load(Ordering::Relaxed));
    }

    /// mpv reports `hwdec-current` as *unavailable* until a file is decoded.
    /// That is the normal answer while starting up or parked, so it must not
    /// be treated as an error and must not destroy the reading or the socket.
    #[cfg(unix)]
    #[test]
    fn an_unavailable_hwdec_reading_is_not_a_failure() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let ipc = test_ipc(client);
        let shared = Arc::new(test_shared());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();

        let peer = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap() == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let request_id = request["request_id"].as_u64().unwrap();
                // Only the decoder probe is rejected; real commands succeed.
                let error = if line.contains("hwdec-current") {
                    "property unavailable"
                } else {
                    "success"
                };
                writeln!(
                    server,
                    "{}",
                    json!({ "request_id": request_id, "error": error })
                )
                .unwrap();
                server.flush().unwrap();
                if line.contains("quit") {
                    break;
                }
            }
        });

        let worker = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || serve_ipc(ipc, receiver, &shared, &stop))
        };

        // A rejected probe followed by a live command proves the worker kept
        // the connection, and no user-facing error was recorded.
        requests.send(Request::ProbeStats).unwrap();
        requests
            .send(Request::Execute(json!(["set_property", "volume", 20])))
            .unwrap();
        requests.send(Request::ProbeStats).unwrap();
        requests.send(Request::Quit).unwrap();
        drop(requests);
        // Joining drains the queue, so every request above has been answered.
        worker.join().unwrap();
        peer.join().unwrap();

        assert!(
            shared.last_error.lock().unwrap().is_none(),
            "a rejected probe must not look like a playback failure"
        );
        assert_eq!(*shared.hwdec_current.lock().unwrap(), None);
        assert!(!shared.ipc_dead.load(Ordering::Relaxed));
    }

    #[test]
    fn only_io_errors_are_fatal() {
        let rejected = anyhow::anyhow!("mpv rejected the command: property unavailable");
        assert!(!is_fatal(&rejected));
        let closed: anyhow::Error =
            anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        assert!(is_fatal(&closed));
    }
}
