//! Minimal libmpv control through its JSON IPC socket.
//!
//! mpv is started as a child process and draws directly into the X11 window
//! identified by `--wid`, so no C headers or FFI are required. All commands go
//! through `--input-ipc-server`, which keeps the interface tiny: the rest of
//! the program only sees `set_volume`, `pause`, `set_position`, and friends.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

static SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// How long a single command may wait for mpv to answer. Commands only set
/// properties, so a healthy mpv replies immediately. The wait happens on the
/// IPC worker thread, never on the GTK main thread.
const IPC_TIMEOUT: Duration = Duration::from_secs(1);

/// How long to wait for the `--input-ipc-server` socket to appear. mpv usually
/// creates it in ~120 ms; a missing socket points at a broken binary or a
/// severely wedged process.
const SOCKET_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// X11 window id to render into. `None` opens mpv's own window.
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
    socket: PathBuf,
    requests: Sender<Request>,
    shared: Arc<Shared>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

/// State readable from the GTK thread without touching the IPC socket.
struct Shared {
    volume: AtomicI64,
    muted: AtomicBool,
    paused: AtomicBool,
    /// Set when the IPC worker loses the connection. The child may still be
    /// alive but can no longer be controlled, so the caller restarts it.
    ipc_dead: AtomicBool,
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
}

/// Commands for the dedicated IPC thread. Keeping them here means a wedged
/// mpv can never stall the GTK main loop: the UI only enqueues work.
enum Request {
    Execute(Value),
    /// Read `hwdec-current` into the shared slot. The property only exists
    /// once a file is being decoded, so a missing reply simply leaves the
    /// previous reading in place and the caller probes again next tick.
    ProbeHwdec,
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
struct Ipc {
    reader: BufReader<UnixStream>,
    /// Second handle on the same socket: `BufReader` does not implement
    /// `Write`, and replies must be read through the buffered reader.
    writer: UnixStream,
    next_request: u64,
}

impl Mpv {
    pub fn spawn(options: &Options) -> anyhow::Result<Self> {
        let unique = SOCKET_COUNTER.fetch_add(1, Ordering::Relaxed);
        let socket_dir = ferramari_core::paths::mpv_socket_dir();
        let _ = std::fs::create_dir_all(&socket_dir);
        let socket = socket_dir.join(format!(
            "ferramari-mpv-{}-{unique}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket);

        let mut command = Command::new(ferramari_core::paths::tool("mpv"));
        // mpv must render through X11 even on a Wayland session: it is given
        // an XWayland window id, and its GPU context auto-detection would
        // otherwise pick a Wayland surface that does not belong to our window.
        command.env_remove("WAYLAND_DISPLAY");
        command
            .arg("--no-terminal")
            .arg("--really-quiet")
            .arg("--no-config")
            .arg("--no-input-default-bindings")
            // Let the GTK window receive clicks, disabling mpv's own
            // mouse/key input disabled, so the right-click menu works.
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
            .arg("--gpu-context=x11egl")
            .arg("--no-border");
        for argument in profile_args(&options.profile) {
            command.arg(argument);
        }
        command
            .arg(format!("--input-ipc-server={}", socket.display()))
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
        // keeps a broken or slow mpv away from the GTK thread.
        let shared = Arc::new(Shared {
            volume: AtomicI64::new(options.volume.clamp(0, 100)),
            muted: AtomicBool::new(options.muted),
            paused: AtomicBool::new(options.paused),
            ipc_dead: AtomicBool::new(false),
            last_error: Mutex::new(None),
            parked_position: Mutex::new(None),
            parked: AtomicBool::new(false),
            hwdec_current: Mutex::new(None),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();
        let worker = {
            let socket = socket.clone();
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            match std::thread::Builder::new()
                .name("ferramari-mpv-ipc".to_string())
                .spawn(move || run_ipc_worker(&socket, receiver, &shared, &stop))
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
            socket,
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

    /// Ask the worker to read `hwdec-current`. Queued, never blocking: the
    /// result appears in [`Mpv::hwdec_current`] on a later supervision tick.
    pub fn probe_hwdec(&mut self) -> anyhow::Result<()> {
        self.enqueue(Request::ProbeHwdec)
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
    /// The request is queued; the worker waits for mpv's reply off the GTK
    /// thread. This keeps the UI responsive even when the decoder needs a
    /// moment to open the new file.
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

    /// Reap the child without blocking. A missing status means mpv exited or
    /// the IPC worker lost the connection; the caller restarts the window.
    pub fn is_running(&mut self) -> bool {
        if self.shared.ipc_dead.load(Ordering::Relaxed) {
            log::warn!("[mpv] IPC connection lost unexpectedly");
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
        // decoder must not keep the GTK process (or the session) alive.
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
        let _ = std::fs::remove_file(&self.socket);
        // The worker exits on `Quit`, on a closed request channel or after a
        // failed connection; the stop flag only wakes its connect loop.
        // Joining it reaps the IPC thread without a long wait.
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Wait for mpv's IPC socket, then serve requests until the channel closes or
/// a fatal I/O error occurs. Runs off the GTK thread.
fn run_ipc_worker(
    socket: &std::path::Path,
    receiver: Receiver<Request>,
    shared: &Shared,
    stop: &AtomicBool,
) {
    let Some(ipc) = connect_ipc(socket, stop) else {
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
            Request::ProbeHwdec => {
                match exchange(&mut ipc, json!(["get_property", "hwdec-current"])) {
                    Ok(reply) => {
                        let value = reply
                            .get("data")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .filter(|value| !value.is_empty());
                        if let Ok(mut current) = shared.hwdec_current.lock() {
                            *current = value;
                        }
                    }
                    // `hwdec-current` is *unavailable* until a file is being
                    // decoded, so a rejection here is the normal answer during
                    // startup and while a decoder is parked. Only a transport
                    // failure deserves the usual error path.
                    Err(err) if is_fatal(&err) => {
                        record_failure(&err, false, shared);
                        break;
                    }
                    Err(_) => {}
                }
            }
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

fn connect_ipc(socket: &std::path::Path, stop: &AtomicBool) -> Option<Ipc> {
    let deadline = Instant::now() + SOCKET_TIMEOUT;
    loop {
        if stop.load(Ordering::Relaxed) {
            return None;
        }
        match UnixStream::connect(socket) {
            Ok(stream) => {
                stream.set_read_timeout(Some(IPC_TIMEOUT)).ok()?;
                stream.set_write_timeout(Some(IPC_TIMEOUT)).ok()?;
                let reader = stream.try_clone().ok()?;
                log::debug!("[mpv] IPC connected: {}", socket.display());
                return Some(Ipc {
                    reader: BufReader::new(reader),
                    writer: stream,
                    next_request: 0,
                });
            }
            Err(err) if Instant::now() < deadline => {
                let _ = err;
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(err) => {
                log::warn!(
                    "[mpv] IPC socket {} never appeared: {err}",
                    socket.display()
                );
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
        let mut line = String::new();
        if ipc.reader.read_line(&mut line)? == 0 {
            anyhow::bail!(
                "mpv closed the IPC socket: {}",
                std::io::Error::from(std::io::ErrorKind::UnexpectedEof)
            );
        }
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
    #[test]
    fn exchange_reads_past_events() {
        let (client, mut server) = UnixStream::pair().unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut ipc = Ipc {
            reader: BufReader::new(client.try_clone().unwrap()),
            writer: client,
            next_request: 0,
        };

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

    fn test_ipc(client: UnixStream) -> Ipc {
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        Ipc {
            reader: BufReader::new(client.try_clone().unwrap()),
            writer: client,
            next_request: 0,
        }
    }

    fn test_shared() -> Shared {
        Shared {
            volume: AtomicI64::new(50),
            muted: AtomicBool::new(false),
            paused: AtomicBool::new(true),
            ipc_dead: AtomicBool::new(false),
            last_error: Mutex::new(None),
            parked_position: Mutex::new(None),
            parked: AtomicBool::new(false),
            hwdec_current: Mutex::new(None),
        }
    }

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
    #[test]
    fn a_hwdec_probe_records_the_reported_decoder() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let ipc = test_ipc(client);
        let shared = Arc::new(test_shared());
        let stop = Arc::new(AtomicBool::new(false));
        let (requests, receiver) = mpsc::channel();

        let peer = std::thread::spawn(move || {
            let mut reader = BufReader::new(server.try_clone().unwrap());
            let replies = [
                ("hwdec-current", Some(json!("vulkan")), "success"),
                ("hwdec-current", Some(json!("no")), "success"),
                ("quit", None, "success"),
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

        requests.send(Request::ProbeHwdec).unwrap();
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
        requests.send(Request::ProbeHwdec).unwrap();
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
        requests.send(Request::ProbeHwdec).unwrap();
        requests
            .send(Request::Execute(json!(["set_property", "volume", 20])))
            .unwrap();
        requests.send(Request::ProbeHwdec).unwrap();
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
