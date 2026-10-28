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
    stream: Mutex<UnixStream>,
    pub volume: i64,
    pub muted: bool,
    pub paused: bool,
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
            // Let the GTK window receive clicks, like upstream does with VLC's
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
        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
        stream.set_write_timeout(Some(Duration::from_secs(3)))?;

        Ok(Self {
            child,
            socket,
            stream: Mutex::new(stream),
            volume: options.volume,
            muted: options.muted,
            paused: options.paused,
        })
    }

    fn command(&self, args: Value) -> anyhow::Result<Value> {
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| anyhow::anyhow!("mpv IPC mutex poisoned"))?;
        let request = json!({ "command": args });
        writeln!(stream, "{request}")?;
        stream.flush()?;
        let mut line = String::new();
        BufReader::new(&mut *stream).read_line(&mut line)?;
        let response: Value = serde_json::from_str(&line)?;
        if response.get("error").and_then(Value::as_str) != Some("success") {
            anyhow::bail!("mpv rejected {request}: {line}");
        }
        Ok(response)
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
}

impl Drop for Mpv {
    fn drop(&mut self) {
        let _ = self.command(json!(["quit"]));
        if let Ok(Some(_)) = self.child.try_wait() {
            // already gone
        } else {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        let _ = std::fs::remove_file(&self.socket);
    }
}
