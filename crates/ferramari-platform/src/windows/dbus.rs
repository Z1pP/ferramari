//! Windows control API. It mirrors the D-Bus interface names and operations so
//! `App`, the Iced UI and scripts keep one vocabulary; only the transport
//! differs. See [`crate::control`] for the wire protocol.
//!
//! Single-instance ownership uses a named mutex plus the published endpoint.
//! The mutex closes the startup race (two processes launched at once), and the
//! endpoint tells a later launch where to send `show_gui`.
//!
//! Shutdown: the accept loop lives until the process exits, matching the
//! documented D-Bus watcher gap in `docs/BACKEND_CONTRACT.md`. The token does
//! not stop it early; there is no in-process restart of the server today.

use std::collections::HashMap;
use std::io::{BufRead, Read};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};

use ferramari_core::bus::{Command, CommandSender, Snapshot};

use crate::control::{self, Endpoint, Request, Response, PROTOCOL_VERSION};
use crate::win32util::{claim_named_mutex, MutexClaim, NamedMutex};

/// The mutex name is session-local: one server per interactive session, the
/// same scope as the Linux session-bus name.
const INSTANCE_MUTEX: &str = r"Local\io.github.ferramari.Ferramari.server";

pub enum Acquired {
    Primary(ControlServer),
    AlreadyRunning,
}

/// The listening server. Kept in `main` for the process lifetime; dropping it
/// removes the endpoint file so a later launch does not probe a dead port.
pub struct ControlServer {
    endpoint: Endpoint,
    listener: TcpListener,
    /// Named mutex owned for the process lifetime. `None` when the kernel
    /// object could not be created; the close-on-drop then only removes the
    /// endpoint.
    _instance: Option<NamedMutex>,
}

impl Drop for ControlServer {
    fn drop(&mut self) {
        control::remove_endpoint(&self.endpoint);
    }
}

/// Claim the server instance, or report that another one owns it.
pub fn acquire() -> anyhow::Result<Acquired> {
    let instance = match claim_named_mutex(INSTANCE_MUTEX) {
        MutexClaim::Owned(mutex) => Some(mutex),
        MutexClaim::AlreadyRunning => return Ok(Acquired::AlreadyRunning),
        MutexClaim::Unavailable(err) => {
            log::warn!("[Control] Could not create the instance mutex: {err}");
            None
        }
    };

    // A crashed server leaves the endpoint behind. Probe it before replacing
    // the file: a live server that owns the mutex cannot be reached here
    // because the claim already said the name is free, but the check keeps the
    // failure mode honest if the mutex API is unavailable.
    if let Some(existing) = control::read_endpoint() {
        if control::ping(&existing) {
            drop(instance);
            return Ok(Acquired::AlreadyRunning);
        }
        log::debug!("[Control] Removing a stale endpoint");
        let _ = std::fs::remove_file(control::endpoint_path());
    }

    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let endpoint = Endpoint {
        protocol: PROTOCOL_VERSION,
        pid: std::process::id(),
        port,
        token: control::new_token(),
    };
    control::write_endpoint(&endpoint)?;
    log::info!("[Control] Listening on 127.0.0.1:{port}");
    Ok(Acquired::Primary(ControlServer {
        endpoint,
        listener,
        _instance: instance,
    }))
}

/// Ask the running instance to show its window.
pub fn request_show_gui() -> anyhow::Result<()> {
    // The mutex can be owned before the endpoint file is published or the
    // accept loop is ready. Retry briefly so a second launch during startup
    // does not silently do nothing.
    ferramari_core::util::retry_until(
        std::time::Duration::from_secs(3),
        std::time::Duration::from_millis(100),
        || {
            let endpoint = control::read_endpoint()
                .ok_or_else(|| anyhow::anyhow!("the server is not running"))?;
            control::call(&endpoint, "command", json!({ "name": "show_gui" }))
                .map(|_| ())
                .map_err(anyhow::Error::msg)
        },
    )?;
    // `show_gui` spawns a UI process; when a window is already open that
    // process loses the instance mutex and exits. Raise the existing window
    // from this freshly launched process, which still has foreground rights.
    crate::ui_instance::focus_existing_window();
    Ok(())
}

/// The decoder the running server reports, for `--doctor`.
pub fn hwdec_running() -> Option<String> {
    let endpoint = control::read_endpoint()?;
    let snapshot = control::call(&endpoint, "snapshot", Value::Null).ok()?;
    snapshot
        .get("hwdec_running")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Start answering requests. `server` stays owned by the caller; only a cloned
/// listener moves into the accept thread.
pub fn register(
    server: &ControlServer,
    tx: CommandSender,
    snapshot: Arc<Mutex<Snapshot>>,
) -> anyhow::Result<()> {
    let listener = server.listener.try_clone()?;
    let token = server.endpoint.token.clone();
    std::thread::Builder::new()
        .name("ferramari-control".to_string())
        .spawn(move || {
            let active = Arc::new(AtomicUsize::new(0));
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        let Some(slot) = try_acquire_client(&active) else {
                            log::debug!("[Control] Rejecting a client: all slots are busy");
                            drop(stream);
                            continue;
                        };
                        let tx = tx.clone();
                        let snapshot = Arc::clone(&snapshot);
                        let token = token.clone();
                        let _ = std::thread::Builder::new()
                            .name("ferramari-control-client".to_string())
                            .spawn(move || {
                                let _slot = slot;
                                handle_client(stream, &token, &tx, &snapshot);
                            });
                    }
                    Err(err) => log::warn!("[Control] Accept failed: {err}"),
                }
            }
        })?;
    Ok(())
}

/// Releases one connection slot even if the handler unwinds.
struct ClientSlot(Arc<AtomicUsize>);

impl Drop for ClientSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn try_acquire_client(active: &Arc<AtomicUsize>) -> Option<ClientSlot> {
    if active.fetch_add(1, Ordering::AcqRel) >= control::MAX_CLIENTS {
        active.fetch_sub(1, Ordering::AcqRel);
        None
    } else {
        Some(ClientSlot(Arc::clone(active)))
    }
}

fn handle_client(
    stream: std::net::TcpStream,
    token: &str,
    tx: &CommandSender,
    snapshot: &Arc<Mutex<Snapshot>>,
) {
    let _ = stream.set_read_timeout(Some(control::IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(control::IO_TIMEOUT));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = std::io::BufReader::new(stream);
    let response = match read_request_line(&mut reader) {
        Ok(Some(line)) => match serde_json::from_str::<Request>(&line) {
            Ok(request) if request.token == token => dispatch(&request, tx, snapshot),
            Ok(_) => Response::error("invalid control token"),
            Err(err) => Response::error(format!("invalid request: {err}")),
        },
        Ok(None) => return,
        Err(err) => Response::error(format!("could not read the request: {err}")),
    };
    if let Ok(text) = serde_json::to_string(&response) {
        use std::io::Write as _;
        let _ = writeln!(writer, "{text}");
        let _ = writer.flush();
    }
}

/// Read one newline-delimited request with a hard upper bound.
///
/// `read_line` on a plain `BufReader` grows until it finds a newline, so a
/// local client could force an allocation before the token is checked. The
/// `take` limit makes the failure bounded and explicit.
fn read_request_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut line = String::new();
    let mut limited = reader
        .by_ref()
        .take((control::MAX_REQUEST_BYTES + 1) as u64);
    let read = limited.read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if line.len() > control::MAX_REQUEST_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("request exceeds {} bytes", control::MAX_REQUEST_BYTES),
        ));
    }
    Ok(Some(line))
}

fn dispatch(request: &Request, tx: &CommandSender, snapshot: &Arc<Mutex<Snapshot>>) -> Response {
    match request.method.as_str() {
        "ping" => Response::ok(json!({
            "pid": std::process::id(),
            "protocol": PROTOCOL_VERSION,
        })),
        "snapshot" => Response::ok(snapshot_json(snapshot)),
        "command" => match command_from(&request.params) {
            Ok(command) => {
                send(tx, command);
                Response::ok(Value::Null)
            }
            Err(err) => Response::error(err),
        },
        "property_set" => match property_command(&request.params) {
            Ok(command) => {
                send(tx, command);
                Response::ok(Value::Null)
            }
            Err(err) => Response::error(err),
        },
        "request_command" => {
            let method = request
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let args = request.params.get("args").cloned().unwrap_or(Value::Null);
            match ferramari_core::command_api::command_from_json(method, &json!({ "args": args })) {
                Ok(command) => request_result(command, tx),
                Err(err) => Response::error(err),
            }
        }
        "request_property" => {
            let property = request
                .params
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let value = request.params.get("value").cloned().unwrap_or(Value::Null);
            match ferramari_core::command_api::property_from_json(property, &value) {
                Ok(command) => request_result(command, tx),
                Err(err) => Response::error(err),
            }
        }
        other => Response::error(format!("unknown control method {other:?}")),
    }
}

fn send(tx: &CommandSender, command: Command) {
    if tx.send(command).is_err() {
        log::debug!("[Control] Main loop is gone, dropping command");
    }
}

fn request_result(command: Command, tx: &CommandSender) -> Response {
    let (reply, response) = mpsc::sync_channel(1);
    if tx
        .send(Command::Request {
            command: Box::new(command),
            reply,
        })
        .is_err()
    {
        return Response::error("the server command loop is not running");
    }
    match response.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Response::ok(Value::Null),
        Ok(Err(err)) => Response::error(err),
        Err(_) => Response::error("the server did not apply the command in time"),
    }
}

/// Map the shared method vocabulary onto the command bus. The spelling follows
/// the D-Bus interface so a script and the Iced UI send identical requests.
fn command_from(params: &Value) -> Result<Command, String> {
    let method = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing command name".to_string())?;
    ferramari_core::command_api::command_from_json(method, params)
}

/// Property writes, `name` plus `value`, matching the D-Bus property names.
fn property_command(params: &Value) -> Result<Command, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing property name".to_string())?;
    let value = params
        .get("value")
        .ok_or_else(|| format!("missing value for {name}"))?;
    ferramari_core::command_api::property_from_json(name, value)
}

/// The merged state the UI polls. Keys are exactly the `ServerState` fields;
/// keeping both interfaces in one object costs nothing locally and makes the
/// Windows client a single request per tick instead of two.
fn snapshot_json(snapshot: &Arc<Mutex<Snapshot>>) -> Value {
    let state = snapshot
        .lock()
        .map(|snapshot| snapshot.clone())
        .unwrap_or_default();
    let player_stats: HashMap<String, String> = state.player_stats.to_bus_map();
    json!({
        "mode": state.mode.as_str(),
        "volume": state.volume,
        "blur_radius": state.blur_radius,
        "is_mute": state.is_mute,
        "is_playing": state.is_playing,
        "is_paused_by_user": state.is_paused_by_user,
        "is_static_wallpaper": state.is_static_wallpaper,
        "is_pause_when_maximized": state.is_pause_when_maximized,
        "is_mute_when_maximized": state.is_mute_when_maximized,
        "theme": state.theme,
        "language": state.language,
        "rotation_mode": state.rotation_mode,
        "rotation_interval_sec": state.rotation_interval_sec,
        "playback_profile": state.playback_profile,
        "pause_on_battery": state.pause_on_battery,
        "idle_timeout_sec": state.idle_timeout_sec,
        "hwdec": state.hwdec,
        "hwdec_running": state.hwdec_running,
        "player_stats": player_stats,
        "web_allow_network": state.web_allow_network,
        "autostart_enabled": state.autostart_enabled,
        "is_show_systray": state.is_show_systray,
        "operation_status": state.operation_status,
        "operation_error": state.operation_error,
        "operation_seq": state.operation_seq,
        "data_source": state.data_source,
        "monitor_states": state.monitor_states,
        "monitor_resolutions": state.monitor_resolutions,
        "monitor_refresh": state.monitor_refresh,
        "primary_monitor": state.primary_monitor,
        "rotations": state.rotations,
        "server_pid": std::process::id(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_spelling_matches_the_bus_api() {
        let command = command_from(&json!({
            "name": "video",
            "args": ["C:/wall.mp4", "DISPLAY1"],
        }))
        .unwrap();
        match command {
            Command::Video { path, monitor } => {
                assert_eq!(path.as_deref(), Some("C:/wall.mp4"));
                assert_eq!(monitor.as_deref(), Some("DISPLAY1"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn empty_optional_arguments_mean_default() {
        let command =
            command_from(&json!({"name": "video", "args": ["", ""]})).expect("valid command");
        match command {
            Command::Video { path, monitor } => {
                assert!(path.is_none());
                assert!(monitor.is_none());
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn property_writes_map_to_the_same_commands_as_dbus() {
        let command = property_command(&json!({
            "name": "idle_timeout_sec",
            "value": 600,
        }))
        .unwrap();
        assert!(matches!(command, Command::SetIdleTimeout(600)));

        let error = property_command(&json!({
            "name": "mode",
            "value": "MODE_VIDEO",
        }))
        .unwrap_err();
        assert!(error.contains("read-only"), "{error}");
    }

    #[test]
    fn a_snapshot_contains_every_ui_field() {
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let value = snapshot_json(&snapshot);
        for key in [
            "mode",
            "volume",
            "data_source",
            "monitor_refresh",
            "primary_monitor",
            "hwdec_running",
            "player_stats",
            "operation_status",
            "operation_seq",
            "server_pid",
        ] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
    }

    #[test]
    fn a_tracked_request_returns_the_application_error() {
        let (sender, receiver) = std::sync::mpsc::channel::<Command>();
        let tx = CommandSender::new(sender);
        let worker = std::thread::spawn(move || {
            let Command::Request { reply, .. } = receiver.recv().unwrap() else {
                panic!("expected a tracked request");
            };
            reply.send(Err("playback failed".to_string())).unwrap();
        });
        let response = request_result(Command::Null, &tx);
        worker.join().unwrap();
        assert!(!response.ok);
        assert_eq!(response.error.as_deref(), Some("playback failed"));
    }

    #[test]
    fn control_connections_have_a_concurrency_limit() {
        let active = Arc::new(AtomicUsize::new(0));
        let mut slots = Vec::new();
        for _ in 0..control::MAX_CLIENTS {
            slots.push(try_acquire_client(&active).expect("slot available"));
        }
        assert!(try_acquire_client(&active).is_none());
        assert_eq!(active.load(Ordering::Acquire), control::MAX_CLIENTS);

        slots.pop();
        let extra = try_acquire_client(&active).expect("slot reused");
        assert_eq!(active.load(Ordering::Acquire), control::MAX_CLIENTS);
        drop(extra);
        assert_eq!(active.load(Ordering::Acquire), control::MAX_CLIENTS - 1);
    }

    #[test]
    fn control_request_lines_have_a_hard_size_limit() {
        let mut normal = std::io::Cursor::new(
            br#"{"token":"t","method":"ping"}
"#
            .to_vec(),
        );
        assert!(read_request_line(&mut normal)
            .expect("normal line")
            .is_some());

        let mut empty = std::io::Cursor::new(Vec::new());
        assert!(read_request_line(&mut empty).expect("eof").is_none());

        let mut oversized = std::io::Cursor::new(vec![b'x'; control::MAX_REQUEST_BYTES + 2]);
        let error = read_request_line(&mut oversized).expect_err("oversized line");
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    }
}
