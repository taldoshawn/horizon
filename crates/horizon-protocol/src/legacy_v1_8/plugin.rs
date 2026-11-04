//! Plugin messages and early compatibility checks for Minecraft 1.8.9.
//!
//! Vanilla/Bukkit/Spigot plugin messages do not imply a client mod loader.
//! Legacy Forge requires an entirely separate mod registry and FML handshake;
//! never fake a completed Forge handshake if native mods are absent.
use crate::{Error, Result, buf::{Reader, Writer}};

const MAX_PLUGIN_DATA: usize = 32767;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMessage {
    pub channel: String,
    pub data: Vec<u8>,
}

fn check_channel(channel: &str) -> Result<()> {
    if channel.is_empty() || channel.encode_utf16().count() > 20 || channel.contains('\0') {
        return Err(Error::new("invalid 1.8 plugin channel (max 20 UTF-16 units)"));
    }
    Ok(())
}

/// Clientbound 0x3f. The remaining bytes ARE the payload; unlike newer
/// protocols there is no separate VarInt byte-array length.
pub fn decode_plugin_message(data: &[u8]) -> Result<PluginMessage> {
    let mut r = Reader::new(data);
    let channel = r.string(20)?;
    check_channel(&channel)?;
    let payload = r.rest();
    if payload.len() > MAX_PLUGIN_DATA {
        return Err(Error::new("1.8 plugin message exceeds the safe payload limit"));
    }
    Ok(PluginMessage { channel, data: payload.to_vec() })
}

/// Serverbound 0x17 (the inverse of 0x3f clientbound).
pub fn plugin_message_body(channel: &str, payload: &[u8]) -> Result<Vec<u8>> {
    check_channel(channel)?;
    if payload.len() > MAX_PLUGIN_DATA {
        return Err(Error::new("1.8 plugin message exceeds the safe payload limit"));
    }
    let mut w = Writer::new();
    w.varint(0x17);
    w.string(channel);
    w.bytes(payload);
    Ok(w.data)
}

/// "MC|Brand" contains an inner VarInt-prefixed string on protocol 47.
pub fn client_brand_body(brand: &str) -> Result<Vec<u8>> {
    if brand.encode_utf16().count() > 64 {
        return Err(Error::new("1.8 client brand is too long"));
    }
    let mut w = Writer::new();
    w.string(brand);
    plugin_message_body("MC|Brand", &w.data)
}

/// Vanilla REGISTER / UNREGISTER uses NUL-separated legacy channel names.
pub fn register_channels_body(channels: &[&str]) -> Result<Vec<u8>> {
    if channels.len() > 128 { return Err(Error::new("too many 1.8 plugin channels")); }
    let mut bytes = Vec::new();
    for (i, channel) in channels.iter().enumerate() {
        check_channel(channel)?;
        if i != 0 { bytes.push(0); }
        bytes.extend_from_slice(channel.as_bytes());
    }
    plugin_message_body("REGISTER", &bytes)
}

pub fn decode_registered_channels(payload: &[u8]) -> Result<Vec<String>> {
    if payload.len() > MAX_PLUGIN_DATA { return Err(Error::new("channel list too large")); }
    if payload.is_empty() { return Ok(vec![]); }
    let mut channels = Vec::new();
    for part in payload.split(|&b| b == 0) {
        if channels.len() >= 128 { return Err(Error::new("too many registered channels")); }
        let name = std::str::from_utf8(part)
            .map_err(|_| Error::new("invalid UTF-8 in plugin channel list"))?;
        check_channel(name)?;
        channels.push(name.to_owned());
    }
    Ok(channels)
}

/// Identify FML handshake packets without pretending to be Forge. The 1.8
/// custom payload channel is "FML|HS"; the mod registry, blocks and client
/// Java mod classes are not reproduced by this implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForgeHandshake {
    ServerHello { fml_protocol: u8 },
    ModList,
    RegistryData,
    HandshakeAck,
    Unknown(u8),
}

pub fn inspect_forge_handshake(packet: &PluginMessage) -> Result<Option<ForgeHandshake>> {
    if packet.channel != "FML|HS" { return Ok(None); }
    let discriminator = *packet.data.first()
        .ok_or_else(|| Error::new("empty legacy Forge handshake"))?;
    let value = match discriminator {
        0 => ForgeHandshake::ServerHello {
            fml_protocol: *packet.data.get(1)
                .ok_or_else(|| Error::new("truncated Forge ServerHello"))?,
        },
        2 => ForgeHandshake::ModList,
        3 => ForgeHandshake::RegistryData,
        255 => ForgeHandshake::HandshakeAck,
        x => ForgeHandshake::Unknown(x),
    };
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_messages_have_separate_bound_ids() {
        let message = plugin_message_body("BungeeCord", &[1,2,3]).unwrap();
        assert_eq!(message[0], 0x17);
        let decoded = decode_plugin_message(&message[1..]).unwrap();
        assert_eq!(decoded, PluginMessage {
            channel: "BungeeCord".into(), data: vec![1,2,3]
        });
    }

    #[test]
    fn brand_and_registered_channels_use_legacy_format() {
        let msg = client_brand_body("horizon").unwrap();
        let packet = decode_plugin_message(&msg[1..]).unwrap();
        assert_eq!(packet.channel, "MC|Brand");
        let mut r = Reader::new(&packet.data);
        assert_eq!(r.string(64).unwrap(), "horizon");
        assert_eq!(r.remaining(), 0);

        let register = register_channels_body(&["MC|Brand", "BungeeCord"]).unwrap();
        let p = decode_plugin_message(&register[1..]).unwrap();
        assert_eq!(decode_registered_channels(&p.data).unwrap(),
            ["MC|Brand", "BungeeCord"]);
        assert!(register_channels_body(&["bad\0name"]).is_err());
    }

    #[test]
    fn forge_is_detected_not_spoofed() {
        let p = PluginMessage { channel: "FML|HS".into(), data: vec![0,2] };
        assert_eq!(inspect_forge_handshake(&p).unwrap(),
            Some(ForgeHandshake::ServerHello { fml_protocol: 2 }));
        assert!(inspect_forge_handshake(&PluginMessage {
            channel: "FML|HS".into(), data: vec![0]
        }).is_err());
    }

    #[test]
    fn invalid_channels_and_oversize_payloads_are_rejected() {
        assert!(plugin_message_body("", &[1]).is_err());
        assert!(plugin_message_body(&"q".repeat(21), &[1]).is_err());
        assert!(plugin_message_body("MC|Brand", &vec![0; MAX_PLUGIN_DATA+1]).is_err());
        assert!(decode_registered_channels(&[0]).is_err());
        assert!(decode_plugin_message(&[3, b'a']).is_err());
    }
}
