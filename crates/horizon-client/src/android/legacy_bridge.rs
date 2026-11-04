//! Explicit transport selection: native protocol-47 development mode or the
//! older external ViaProxy path. Neither implies fully playable 1.8.9.
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;

use crate::catalog::RuntimeMode;

pub const LOOPBACK: &str = "127.0.0.1:25568";
const PROBE_TIMEOUT: Duration = Duration::from_millis(450);

pub fn server(mode: RuntimeMode, configured: &str) -> String {
    match mode {
        RuntimeMode::Native26_3 | RuntimeMode::Native1_8_9Experimental => configured.to_string(),
        RuntimeMode::Legacy1_8_9ViaProxy => LOOPBACK.to_string(),
    }
}

/// Check whether a process actually accepts TCP connections on the loopback
/// port. This DOES NOT prove it is ViaProxy or that 1.8.9 will play correctly.
pub fn check(mode: RuntimeMode) -> Result<(), String> {
    if mode != RuntimeMode::Legacy1_8_9ViaProxy { return Ok(()); }
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
    fn native_legacy_uses_real_server_and_needs_no_local_proxy() {
        assert_eq!(server(RuntimeMode::Native1_8_9Experimental, "localhost:25565"), "localhost:25565");
        assert!(check(RuntimeMode::Native1_8_9Experimental).is_ok());
    }
    #[test]
    fn bridge_only_uses_loopback() {
        assert_eq!(server(RuntimeMode::Legacy1_8_9ViaProxy, "evil.example"), LOOPBACK);
        assert_eq!(LOOPBACK.parse::<SocketAddr>().unwrap().ip().to_string(), "127.0.0.1");
    }
}
