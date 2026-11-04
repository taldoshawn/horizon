//! Safe, explicit bridge selection. No protocol-47 play compatibility is claimed.
//! The game still speaks 26.3; ViaProxy must translate it to the 1.8.9 server.
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::catalog::RuntimeMode;

pub const LOOPBACK: &str = "127.0.0.1:25568";
const PROBE_TIMEOUT: Duration = Duration::from_millis(450);

pub fn server(mode: RuntimeMode, configured: &str) -> String {
    match mode {
        RuntimeMode::Native26_3 => configured.to_string(),
        RuntimeMode::Legacy1_8_9ViaProxy => LOOPBACK.to_string(),
    }
}

/// Check whether a process actually accepts TCP connections on the loopback
/// port. This DOES NOT prove it is ViaProxy or that 1.8.9 will play correctly.
pub fn check(mode: RuntimeMode) -> Result<(), String> {
    if mode == RuntimeMode::Native26_3 { return Ok(()); }
    let addr: SocketAddr = LOOPBACK.parse().expect("constant valid socket address");
    TcpStream::connect_timeout(&addr, PROBE_TIMEOUT)
        .map(|stream| drop(stream))
        .map_err(|_| format!(
            "ViaProxy not detected at {LOOPBACK}. Start the Termux bridge first, then tap Play."
        ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_keeps_existing_server() {
        assert_eq!(server(RuntimeMode::Native26_3, "example.org:25565"), "example.org:25565");
    }
    #[test]
    fn bridge_only_uses_loopback() {
        assert_eq!(server(RuntimeMode::Legacy1_8_9ViaProxy, "evil.example"), LOOPBACK);
        assert_eq!(LOOPBACK.parse::<SocketAddr>().unwrap().ip().to_string(), "127.0.0.1");
    }
}
