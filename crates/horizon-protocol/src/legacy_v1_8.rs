//! Minecraft Java 1.8.x (protocol 47) transport and login foundation.
//!
//! This module is deliberately separate from the 26.3 packet enums. The old
//! wire format has no configuration phase, has a different login profile
//! representation, and its play/chunk/entity formats cannot be made compatible
//! by merely changing the handshake's protocol integer.
//!
//! The handshake, status, compression, offline and authenticated login are
//! implemented here. A *play-state translator* and its integration into the
//! renderer are NOT implemented. Do not advertise this module as a playable
//! 1.8 client. See docs/legacy-1.8-port.md.
pub mod play;
pub mod inventory;
pub mod plugin;
pub mod world;
pub mod state;
pub mod physics;

use std::time::Duration;

use crate::{
    Error, Result,
    buf::{Reader, Writer},
    connection::Connection,
    crypto::{generate_secret, server_hash},
    packets::handshake,
    rsa::RsaPublicKey,
    servers::parse_address,
};

/// Minecraft Java 1.8.0 through 1.8.9 (the protocol shared by these releases).
pub const PROTOCOL: i32 = 47;

/// 1.8 clientbound login state (before play, no configuration state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginPacket {
    Disconnect(String),
    EncryptionRequest {
        server_id: String,
        public_key: Vec<u8>,
        verify_token: Vec<u8>,
    },
    Success { uuid: String, username: String },
    SetCompression(i32),
}

fn no_trailing(r: &Reader<'_>) -> Result<()> {
    if r.remaining() == 0 {
        Ok(())
    } else {
        Err(Error::new(format!("extra {} bytes in 1.8 login packet", r.remaining())))
    }
}

/// Decode exactly the 1.8 login packets; the 26.3 login codec is not wire compatible.
/// The byte array fields in 1.8 are VarInt-length-prefixed, not u16.
pub fn decode_login(id: i32, payload: &[u8]) -> Result<LoginPacket> {
    let mut r = Reader::new(payload);
    let packet = match id {
        0x00 => LoginPacket::Disconnect(r.string(32767)?),
        0x01 => LoginPacket::EncryptionRequest {
            server_id: r.string(20)?,
            public_key: r.byte_array(8192)?,
            verify_token: r.byte_array(512)?,
        },
        0x02 => LoginPacket::Success {
            // In protocol 47, UUID is transmitted as a string, not 16 raw bytes.
            uuid: r.string(36)?,
            username: r.string(16)?,
        },
        0x03 => LoginPacket::SetCompression(r.varint()?),
        _ => return Err(Error::new(format!("unknown 1.8 login packet {id:#x}"))),
    };
    no_trailing(&r)?;
    Ok(packet)
}

/// Handshake packet body (without TCP packet-length prefix); compatible with
/// the old and new handshake framing, but with protocol 47 encoded as VarInt.
pub fn handshake_body(host: &str, port: u16, intent: i32) -> Vec<u8> {
    handshake::Serverbound::Intention(handshake::Intention {
        protocol: PROTOCOL,
        host: host.to_owned(),
        port,
        intent,
    })
    .encode()
}

/// 1.8 Login Start contains *only* the name, not a UUID.
pub fn login_start_body(name: &str) -> Result<Vec<u8>> {
    if name.is_empty() || name.encode_utf16().count() > 16 {
        return Err(Error::new("Minecraft 1.8 username must have 1..16 UTF-16 units"));
    }
    let mut w = Writer::new();
    w.varint(0x00);
    w.string(name);
    Ok(w.data)
}

/// 1.8 Encryption Response; the two encrypted byte arrays use VarInt lengths.
pub fn encryption_response_body(secret: &[u8], verify_token: &[u8]) -> Vec<u8> {
    let mut w = Writer::new();
    w.varint(0x01);
    w.byte_array(secret);
    w.byte_array(verify_token);
    w.data
}

/// Incoming play-state keep-alive (0x00) in 1.8 has a VarInt id.
/// This is a wire-level primitive, not a functional play-state bridge.
pub fn keep_alive_id(id: i32, payload: &[u8]) -> Result<Option<i32>> {
    if id != 0x00 {
        return Ok(None);
    }
    let mut r = Reader::new(payload);
    let keep_alive = r.varint()?;
    no_trailing(&r)?;
    Ok(Some(keep_alive))
}

pub fn keep_alive_response_body(id: i32) -> Vec<u8> {
    let mut w = Writer::new();
    w.varint(0x00);
    w.varint(id);
    w.data
}

/// Completes a 1.8 login and returns the *untranslated* PLAY connection.
/// The caller is responsible for a version-specific play implementation.
///
/// `authenticate` is the caller's session-service join function. It receives
/// the standard signed Minecraft server hash and must reject missing/expired
/// ownership credentials. Pass `None` only for a server in offline mode.
/// No passwords or auth tokens are persisted by this API.
pub fn connect_login(
    address: &str,
    username: &str,
    timeout: Duration,
    mut authenticate: Option<&mut dyn FnMut(&str) -> Result<()>>,
) -> Result<(Connection, String)> {
    let (host, port) = parse_address(address).ok_or_else(|| Error::new("Invalid server address"))?;
    let (mut c, resolved_host, resolved_port) =
        Connection::connect_server_with_address(&host, port, timeout)?;
    c.reader.set_read_timeout(Some(timeout))?;
    c.send(&handshake_body(&resolved_host, resolved_port, handshake::LOGIN))?;
    c.send(&login_start_body(username)?)?;
    loop {
        let (id, payload) = c.recv()?;
        match decode_login(id, &payload)? {
            LoginPacket::Disconnect(reason) => {
                return Err(Error::new(format!("1.8 server refused login: {reason}")));
            }
            LoginPacket::SetCompression(threshold) => {
                if threshold < 0 {
                    return Err(Error::new("negative 1.8 compression threshold"));
                }
                c.set_compression(threshold);
            }
            LoginPacket::EncryptionRequest { server_id, public_key, verify_token } => {
                let Some(join) = authenticate.as_mut() else {
                    return Err(Error::new(
                        "1.8 server requires Microsoft/Mojang session authentication",
                    ));
                };
                let public = RsaPublicKey::from_der(&public_key)?;
                let secret = generate_secret()?;
                let hash = server_hash(&server_id, &secret, &public_key);
                join(&hash)?;
                c.send(&encryption_response_body(
                    &public.encrypt(&secret)?,
                    &public.encrypt(&verify_token)?,
                ))?;
                c.enable_encryption(&secret);
            }
            LoginPacket::Success { uuid, username: name } => {
                if name != username {
                    return Err(Error::new("1.8 login username mismatch"));
                }
                // 1.8 enters PLAY immediately: no LoginAcknowledged packet.
                return Ok((c, uuid));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame;
    use std::{net::TcpListener, thread};

    #[test]
    fn handshake_is_protocol_47_login() {
        assert_eq!(
            handshake_body("localhost", 25565, handshake::LOGIN),
            [0, 47, 9, b'l', b'o', b'c', b'a', b'l', b'h', b'o', b's', b't', 99, 221, 2]
        );
    }

    #[test]
    fn login_start_has_no_uuid() {
        assert_eq!(login_start_body("Steve").unwrap(), b"\x00\x05Steve");
        assert!(login_start_body("").is_err());
        assert!(login_start_body("abcdefghijklmnopq").is_err());
    }

    #[test]
    fn login_wire_packets_and_compression() {
        let mut success = Writer::new();
        success.string("12345678-1234-1234-1234-123456789abc");
        success.string("Steve");
        assert_eq!(
            decode_login(2, &success.data).unwrap(),
            LoginPacket::Success {
                uuid: "12345678-1234-1234-1234-123456789abc".into(),
                username: "Steve".into()
            }
        );
        assert_eq!(decode_login(3, &[0x80, 1]).unwrap(), LoginPacket::SetCompression(128));
        assert!(decode_login(2, &[2, b'a']).is_err());
        let mut encrypt = Writer::new();
        encrypt.string("");
        encrypt.byte_array(&[0xaa, 0xbb]);
        encrypt.byte_array(&[0x33]);
        assert_eq!(
            decode_login(1, &encrypt.data).unwrap(),
            LoginPacket::EncryptionRequest {
                server_id: "".into(), public_key: vec![0xaa, 0xbb], verify_token: vec![0x33]
            }
        );
        let body = encryption_response_body(&[1, 2], &[3]);
        assert_eq!(body, [1, 2, 1, 2, 1, 3]);
        let wire = frame::encode(&body, None).unwrap();
        assert_eq!(wire, [6, 1, 2, 1, 2, 1, 3]);
    }

    #[test]
    fn keep_alive_round_trip() {
        assert_eq!(keep_alive_id(0, &[0xac, 0x02]).unwrap(), Some(300));
        assert_eq!(keep_alive_response_body(300), [0x00, 0xac, 0x02]);
        assert!(keep_alive_id(0, &[0x80]).is_err());
        assert_eq!(keep_alive_id(1, &[0]).unwrap(), None);
    }

    #[test]
    fn offline_login_against_mock_1_8_server() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut c = Connection::from_stream(stream).unwrap();
            let (id, payload) = c.recv().unwrap();
            assert_eq!(id, 0);
            let mut r = Reader::new(&payload);
            assert_eq!(r.varint().unwrap(), PROTOCOL);
            assert_eq!(r.string(255).unwrap(), "127.0.0.1");
            assert_eq!(r.u16().unwrap(), address.port());
            assert_eq!(r.varint().unwrap(), handshake::LOGIN);
            let (id, payload) = c.recv().unwrap();
            assert_eq!(id, 0);
            assert_eq!(Reader::new(&payload).string(16).unwrap(), "Steve");
            let mut success = Writer::new();
            success.varint(2);
            success.string("12345678-1234-1234-1234-123456789abc");
            success.string("Steve");
            c.send(&success.data).unwrap();
            c.send(&keep_alive_response_body(300)).unwrap();
            let (id, payload) = c.recv().unwrap();
            assert_eq!(keep_alive_id(id, &payload).unwrap(), Some(300));
        });
        let (mut c, uuid) = connect_login(
            &address.to_string(), "Steve", Duration::from_secs(5), None,
        ).unwrap();
        assert_eq!(uuid, "12345678-1234-1234-1234-123456789abc");
        let (id, payload) = c.recv().unwrap();
        let keep_alive = keep_alive_id(id, &payload).unwrap().unwrap();
        c.send(&keep_alive_response_body(keep_alive)).unwrap();
        server.join().unwrap();
    }
}
