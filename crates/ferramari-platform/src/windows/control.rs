//! Loopback control transport used instead of D-Bus on Windows.
//!
//! The server and the Iced control surface are separate processes on every
//! platform. Linux uses the session bus; Windows has no freedesktop bus, so the
//! server listens on `127.0.0.1` with an ephemeral port and publishes the port
//! plus a per-process token in `control-endpoint.json` under the user's config
//! directory. The token is the access control: another local user cannot read
//! the file because it lives in this user's profile, and a stale file left by a
//! crashed server is detected by a failed `ping` before it is replaced.
//!
//! The protocol is newline-delimited JSON. One request per connection keeps
//! the framing trivial and means a client crash cannot leave a half-read
//! message that breaks every later client. Requests are small and the server
//! answers from an in-memory snapshot, so reconnecting costs nothing.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use ferramari_core::paths;

/// Bumped when the request/response shape changes incompatibly. A client that
/// reads a different number reports "server is from another version" instead
/// of failing on an unreadable reply.
pub const PROTOCOL_VERSION: u32 = 1;

/// Connect timeout for a local socket. Loopback connects either succeed at
/// once or the listener is gone; a multi-second wait would only freeze a UI
/// poll.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// A malformed or wedged client must not hold the server's connection thread
/// forever. Every request is one short line.
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

/// Upper bound for one control request, including its terminating newline.
/// A local client must not be able to make the server allocate an unbounded
/// line before the token is even checked.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// Maximum control connections handled at once. The UI uses one; the spare
/// slots cover scripts and a reconnect overlapping the old connection.
pub const MAX_CLIENTS: usize = 16;

/// What `control-endpoint.json` contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub protocol: u32,
    pub pid: u32,
    pub port: u16,
    pub token: String,
}

/// One request. `token` is checked before anything is dispatched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub token: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

impl Request {
    pub fn new(token: &str, method: &str, params: Value) -> Self {
        Self {
            token: token.to_string(),
            method: method.to_string(),
            params,
        }
    }
}

/// One reply. `ok == false` carries a human-readable `error`; a refused request
/// still has a valid response shape so the client needs no second code path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub result: Value,
    #[serde(default)]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(result: Value) -> Self {
        Self {
            ok: true,
            result,
            error: None,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            result: Value::Null,
            error: Some(message.into()),
        }
    }
}

pub fn endpoint_path() -> PathBuf {
    paths::config_dir().join("control-endpoint.json")
}

pub fn read_endpoint() -> Option<Endpoint> {
    let text = fs::read_to_string(endpoint_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Publish the endpoint atomically: a reader either sees the old complete file
/// or the new complete file, never a truncated one.
pub fn write_endpoint(endpoint: &Endpoint) -> std::io::Result<()> {
    let path = endpoint_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(endpoint)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, format!("{text}\n"))?;
    fs::rename(&temp, &path)
}

/// Remove the endpoint file. Only the owner of the exact path removes it; a
/// server that lost a startup race must not delete the winner's file.
pub fn remove_endpoint(endpoint: &Endpoint) {
    if read_endpoint().as_ref() == Some(endpoint) {
        let _ = fs::remove_file(endpoint_path());
    }
}

/// A fresh per-process secret. 16 random bytes rendered as hex is plenty for a
/// loopback token that only gates accidental cross-client confusion; the real
/// boundary is the file ACL.
pub fn new_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Perform one request against `endpoint` and return its result.
pub fn call(endpoint: &Endpoint, method: &str, params: Value) -> Result<Value, String> {
    let address = SocketAddr::from(([127, 0, 0, 1], endpoint.port));
    let stream = TcpStream::connect_timeout(&address, CONNECT_TIMEOUT)
        .map_err(|err| format!("ferramari server is not reachable: {err}"))?;
    stream
        .set_read_timeout(Some(IO_TIMEOUT))
        .map_err(|err| err.to_string())?;
    stream
        .set_write_timeout(Some(IO_TIMEOUT))
        .map_err(|err| err.to_string())?;

    let request = Request::new(&endpoint.token, method, params);
    let line = serde_json::to_string(&request).map_err(|err| err.to_string())?;
    let mut writer = stream.try_clone().map_err(|err| err.to_string())?;
    writeln!(writer, "{line}").map_err(|err| err.to_string())?;
    writer.flush().map_err(|err| err.to_string())?;

    let mut reader = BufReader::new(stream);
    let mut reply = String::new();
    reader
        .read_line(&mut reply)
        .map_err(|err| err.to_string())?;
    if reply.trim().is_empty() {
        return Err("ferramari server closed the connection without a reply".to_string());
    }
    let response: Response = serde_json::from_str(&reply).map_err(|err| err.to_string())?;
    if response.ok {
        Ok(response.result)
    } else {
        Err(response
            .error
            .unwrap_or_else(|| "request failed".to_string()))
    }
}

/// Whether a server still owns the published endpoint.
pub fn ping(endpoint: &Endpoint) -> bool {
    call(endpoint, "ping", Value::Null)
        .ok()
        .is_some_and(|result| result.get("pid").and_then(Value::as_u64).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_hex_and_distinct() {
        let first = new_token();
        let second = new_token();
        assert_eq!(first.len(), 32);
        assert!(first.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_ne!(first, second);
    }

    #[test]
    fn endpoint_roundtrips_through_json() {
        let endpoint = Endpoint {
            protocol: PROTOCOL_VERSION,
            pid: 42,
            port: 54321,
            token: "abc".to_string(),
        };
        let text = serde_json::to_string(&endpoint).unwrap();
        let parsed: Endpoint = serde_json::from_str(&text).unwrap();
        assert_eq!(parsed, endpoint);
    }

    #[test]
    fn requests_default_their_params() {
        let request: Request = serde_json::from_str(r#"{"token":"t","method":"ping"}"#).unwrap();
        assert!(request.params.is_null());
        assert_eq!(request.method, "ping");
    }
}
