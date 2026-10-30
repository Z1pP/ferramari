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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

static SOCKET_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// How long a single command may wait for mpv to answer. Commands only set
/// properties, so a healthy mpv replies immediately; the limit exists to bound
/// a main-thread stall when the player has wedged.
const IPC_TIMEOUT: Duration = Duration::from_secs(2);

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
}

pub struct Mpv {
    child: Child,
    socket: PathBuf,
    ipc: Mutex<Option<Ipc>>,
    pub volume: i64,
    pub muted: bool,
    pub paused: bool,
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
        let socket = std::env::temp_dir().join(format!(
            "ferramari-mpv-{}-{unique}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&socket);

        let mut command = Command::new(crate::paths::tool("mpv"));
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
            .arg("--hwdec=auto-safe")
            .arg("--gpu-context=x11egl")
            .arg("--no-border")
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

        let deadline = Instant::now() + Duration::from_secs(3);
        let stream = loop {
            match UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(err) if Instant::now() < deadline => {
                    let _ = err;
                    std::thread::sleep(Duration::from_millis(25));
                }
                Err(err) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!("mpv IPC socket {} never appeared: {err}", socket.display())
                }
            }
        };
        stream.set_read_timeout(Some(IPC_TIMEOUT))?;
        stream.set_write_timeout(Some(IPC_TIMEOUT))?;
        let reader = stream.try_clone()?;

        Ok(Self {
            child,
            socket,
            ipc: Mutex::new(Some(Ipc {
                reader: BufReader::new(reader),
                writer: stream,
                next_request: 0,
            })),
            volume: options.volume,
            muted: options.muted,
            paused: options.paused,
        })
    }

    fn command(&self, args: Value) -> anyhow::Result<Value> {
        let mut guard = self
            .ipc
            .lock()
            .map_err(|_| anyhow::anyhow!("mpv IPC mutex poisoned"))?;
        let mut ipc = guard
            .take()
            .ok_or_else(|| anyhow::anyhow!("mpv IPC connection is closed"))?;

        let result = exchange(&mut ipc, args);

        // I/O failures (EOF, timeout) leave unread bytes behind: the stream
        // cannot be reused. A rejected command, on the other hand, keeps it.
        let fatal = result
            .as_ref()
            .err()
            .is_some_and(|err| err.downcast_ref::<std::io::Error>().is_some());
        if !fatal {
            *guard = Some(ipc);
        }
        result
    }

    fn set_property(&self, name: &str, value: Value) -> anyhow::Result<()> {
        self.command(json!(["set_property", name, value]))?;
        Ok(())
    }

    pub fn set_volume(&mut self, volume: i64) -> anyhow::Result<()> {
        self.volume = volume.clamp(0, 100);
        self.set_property("volume", json!(self.volume))
    }

    pub fn set_mute(&mut self, muted: bool) -> anyhow::Result<()> {
        self.muted = muted;
        self.set_property("mute", json!(muted))
    }

    pub fn set_pause(&mut self, paused: bool) -> anyhow::Result<()> {
        self.paused = paused;
        self.set_property("pause", json!(paused))
    }

    /// Reap the child without blocking. A missing status means mpv exited or
    /// the handle is no longer usable; the caller restarts the window.
    pub fn is_running(&mut self) -> bool {
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
        // Ask mpv to quit, then give it a short grace period. A wedged
        // decoder must not keep the GTK process (or the session) alive. A
        // child that already exited is not asked again: its IPC socket may
        // still be open, and waiting for the reply would only delay shutdown.
        let already_exited = matches!(self.child.try_wait(), Ok(Some(_)));
        if !already_exited {
            let _ = self.command(json!(["quit"]));
        }
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
    }
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
}
