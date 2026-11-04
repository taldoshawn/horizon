//! Selected Minecraft Java 1.8.x (protocol 47) PLAY wire types.
//!
//! These are not the modern PLAY packets: callers MUST NOT pass them into
//! Horizon's 26.3 packet decoder. In particular, a 1.8 chunk contains packed
//! numeric block ID + metadata, not a modern paletted block state array.
use crate::{Error, Result, buf::{Reader, Writer}};

/// The 1.8 server's join-game packet (clientbound PLAY 0x01).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinGame {
    pub entity_id: i32,
    pub game_mode: u8,
    pub hardcore: bool,
    pub dimension: i8,
    pub difficulty: u8,
    pub max_players: u8,
    pub world_type: String,
    pub reduced_debug_info: bool,
}

/// The 1.8 server's position correction (clientbound PLAY 0x08).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionLook {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
    /// Relative coordinate flags (X Y Z yaw pitch), bits 0..4.
    pub flags: u8,
}

impl PositionLook {
    /// Apply the server's relative flags to the last client position and rotation.
    /// The server requires the *absolute* result in a C06 confirmation packet.
    pub fn absolute(self, previous: Self) -> Self {
        Self {
            x: if self.flags & 1 != 0 { previous.x + self.x } else { self.x },
            y: if self.flags & 2 != 0 { previous.y + self.y } else { self.y },
            z: if self.flags & 4 != 0 { previous.z + self.z } else { self.z },
            yaw: if self.flags & 8 != 0 { previous.yaw + self.yaw } else { self.yaw },
            pitch: if self.flags & 16 != 0 { previous.pitch + self.pitch } else { self.pitch },
            flags: 0,
        }
    }
}

fn ensure_consumed(r: &Reader<'_>, what: &str) -> Result<()> {
    if r.remaining() == 0 { Ok(()) } else {
        Err(Error::new(format!("1.8 {what} has {} trailing bytes", r.remaining())))
    }
}

/// Decode a 1.8 JoinGame payload (ID 0x01 already removed).
pub fn decode_join_game(payload: &[u8]) -> Result<JoinGame> {
    let mut r = Reader::new(payload);
    let entity_id = r.i32()?;
    let game_mode_and_hardcore = r.u8()?;
    let game_mode = game_mode_and_hardcore & 7;
    let hardcore = game_mode_and_hardcore & 8 != 0;
    let dimension = r.i8()?;
    let difficulty = r.u8()?;
    let max_players = r.u8()?;
    let world_type = r.string(16)?;
    let reduced_debug_info = r.bool()?;
    ensure_consumed(&r, "join game")?;
    Ok(JoinGame { entity_id, game_mode, hardcore, dimension, difficulty,
        max_players, world_type, reduced_debug_info })
}

/// Decode a 1.8 Player Position & Look payload (ID 0x08 already removed).
pub fn decode_position_look(payload: &[u8]) -> Result<PositionLook> {
    let mut r = Reader::new(payload);
    let pos = PositionLook {
        x: r.f64()?, y: r.f64()?, z: r.f64()?,
        yaw: r.f32()?, pitch: r.f32()?, flags: r.u8()?,
    };
    if pos.flags & !31 != 0 {
        return Err(Error::new("unknown 1.8 position flags"));
    }
    if !pos.x.is_finite() || !pos.y.is_finite() || !pos.z.is_finite()
        || !pos.yaw.is_finite() || !pos.pitch.is_finite() {
        return Err(Error::new("1.8 server position contains NaN or infinity"));
    }
    ensure_consumed(&r, "position look")?;
    Ok(pos)
}

/// Movement packet 0x06: respond to the 1.8 position correction (there is NO teleport id).
/// `position` must have flags already resolved to absolute coordinates.
pub fn position_look_response_body(position: PositionLook, on_ground: bool) -> Vec<u8> {
    let mut w = Writer::new();
    w.varint(0x06);
    w.f64(position.x); w.f64(position.y); w.f64(position.z);
    w.f32(position.yaw); w.f32(position.pitch);
    w.bool(on_ground);
    w.data
}

/// 1.8 client settings (PLAY 0x15), including view distance and skin parts.
pub fn client_settings_body(locale: &str, view_distance: u8, skin_parts: u8) -> Result<Vec<u8>> {
    if locale.is_empty() || locale.encode_utf16().count() > 7 {
        return Err(Error::new("1.8 locale must have 1..7 characters"));
    }
    let mut w = Writer::new();
    w.varint(0x15);
    w.string(locale);
    w.u8(view_distance.min(32));
    w.u8(0); // Chat mode: enabled
    w.bool(true); // Chat colors
    w.u8(skin_parts);
    Ok(w.data)
}

/// One 16x16x16 section from the old on-wire block and lighting arrays.
#[derive(Debug, Clone)]
pub struct ChunkSection<'a> {
    pub y: u8,
    /// 4096 little-endian u16 entries. bits 4..15: numeric block ID; 0..3: metadata.
    pub packed_blocks: &'a [u8],
    pub block_light: &'a [u8],
    pub sky_light: Option<&'a [u8]>,
}
impl ChunkSection<'_> {
    /// Coordinates local to a 16x16x16 section; returns numeric ID and metadata.
    pub fn block(&self, x: u8, y: u8, z: u8) -> Option<(u16, u8)> {
        if x >= 16 || y >= 16 || z >= 16 { return None; }
        let index = (usize::from(y) * 256 + usize::from(z) * 16 + usize::from(x)) * 2;
        let packed = u16::from_le_bytes([self.packed_blocks[index], self.packed_blocks[index + 1]]);
        Some((packed >> 4, (packed & 15) as u8))
    }
}

/// Decoded payload of 0x21 ChunkData, with borrowed blocks and light arrays.
/// The caller supplies `has_sky_light` based on JoinGame.dimension (Nether = no).
#[derive(Debug, Clone)]
pub struct ChunkData<'a> {
    pub x: i32,
    pub z: i32,
    pub full: bool,
    pub sections: Vec<ChunkSection<'a>>,
    pub biomes: Option<&'a [u8]>,
}

/// The maximum raw size of a single Minecraft 1.8 chunk column.
const MAX_CHUNK_BYTES: usize = 16 * (8192 + 2048 + 2048) + 256;

/// Parse sections with the vanilla array order: all packed block arrays,
/// then all block light arrays, then all skylight arrays, then the 256 biomes.
fn chunk_sections<'a>(
    x: i32, z: i32, full: bool, bitmask: u16, bytes: &'a [u8], has_sky_light: bool,
) -> Result<ChunkData<'a>> {
    let section_count = bitmask.count_ones() as usize;
    let expected = section_count * (8192 + 2048 + if has_sky_light { 2048 } else { 0 })
        + if full { 256 } else { 0 };
    if bytes.len() != expected {
        return Err(Error::new(format!("1.8 chunk has {} bytes, expected {expected}", bytes.len())));
    }
    let mut data = Reader::new(bytes);
    let mut sections = Vec::with_capacity(section_count);
    for y in 0..16 {
        if bitmask & (1 << y) != 0 {
            sections.push(ChunkSection { y: y as u8, packed_blocks: data.bytes(8192)?,
                block_light: &[], sky_light: None });
        }
    }
    for section in &mut sections { section.block_light = data.bytes(2048)?; }
    if has_sky_light {
        for section in &mut sections { section.sky_light = Some(data.bytes(2048)?); }
    }
    let biomes = if full { Some(data.bytes(256)?) } else { None };
    ensure_consumed(&data, "chunk bytes")?;
    Ok(ChunkData { x, z, full, sections, biomes })
}

/// Decode 0x21 ChunkData. The caller supplies `has_sky_light` based on
/// JoinGame.dimension (Overworld only). There is no *nested* zlib stream
/// inside the chunk: 1.8 packet-frame compression happens before this codec.
pub fn decode_chunk_data(payload: &[u8], has_sky_light: bool) -> Result<ChunkData<'_>> {
    let mut r = Reader::new(payload);
    let x = r.i32()?;
    let z = r.i32()?;
    let full = r.bool()?;
    let bitmask = r.u16()?;
    let len = r.count(MAX_CHUNK_BYTES)?;
    let bytes = r.bytes(len)?;
    ensure_consumed(&r, "chunk data")?;
    // Vanilla 1.8 unloads a chunk with full=true, mask=0 and a zero-size
    // data array. The normal fully-empty column has 256 biome bytes instead.
    if full && bitmask == 0 && bytes.is_empty() {
        return Ok(ChunkData { x, z, full, sections: vec![], biomes: None });
    }
    chunk_sections(x, z, full, bitmask, bytes, has_sky_light)
}

/// Decode the 1.8 MapChunkBulk packet 0x26. Its wire format differs from 0x21:
/// a global sky-light flag, a list of x/z/bitmasks, followed by raw per-chunk
/// arrays of *implicit* lengths (NO VarInt preceding each chunk's bytes).
pub fn decode_chunk_bulk(payload: &[u8]) -> Result<Vec<ChunkData<'_>>> {
    let mut r = Reader::new(payload);
    let sky_light = r.bool()?;
    let n = r.count(1024)?;
    let mut metadata = Vec::with_capacity(n);
    for _ in 0..n {
        metadata.push((r.i32()?, r.i32()?, r.u16()?));
    }
    let mut chunks = Vec::with_capacity(n);
    for (x, z, mask) in metadata {
        let size = mask.count_ones() as usize * (8192 + 2048 + if sky_light { 2048 } else { 0 }) + 256;
        chunks.push(chunk_sections(x, z, true, mask, r.bytes(size)?, sky_light)?);
    }
    ensure_consumed(&r, "bulk chunks")?;
    Ok(chunks)
}

/// Serverbound UseEntity packet 0x02, with 1.8's VarInt action code.
/// The native game must make its own attack decision; this function does not
/// implement or emulate the 1.8 combat rules.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UseEntityAction {
    Interact,
    Attack,
    InteractAt { x: f32, y: f32, z: f32 },
}
pub fn use_entity_body(entity_id: i32, action: UseEntityAction) -> Result<Vec<u8>> {
    if entity_id < 0 {
        return Err(Error::new("negative 1.8 target entity ID"));
    }
    let mut w = Writer::new();
    w.varint(0x02);
    w.varint(entity_id);
    match action {
        UseEntityAction::Interact => w.varint(0),
        UseEntityAction::Attack => w.varint(1),
        UseEntityAction::InteractAt { x, y, z } => {
            if ![x, y, z].into_iter().all(f32::is_finite) {
                return Err(Error::new("1.8 interaction has non-finite coordinates"));
            }
            w.varint(2);
            w.f32(x);
            w.f32(y);
            w.f32(z);
        }
    }
    Ok(w.data)
}

/// Clientbound 0x02 chat, including chat/system/action-bar position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub json: String,
    pub position: u8,
}
pub fn decode_chat_message(payload: &[u8]) -> Result<ChatMessage> {
    let mut r = Reader::new(payload);
    let json = r.string(32767)?;
    let position = r.u8()?;
    if position > 2 {
        return Err(Error::new("unknown 1.8 chat position"));
    }
    ensure_consumed(&r, "chat")?;
    Ok(ChatMessage { json, position })
}

/// Serverbound 0x01 chat/command; 1.8's vanilla chat max is 100 UTF-16 units.
pub fn chat_message_body(input: &str) -> Result<Vec<u8>> {
    if input.is_empty() || input.encode_utf16().count() > 100 {
        return Err(Error::new("1.8 chat must contain 1..100 UTF-16 units"));
    }
    let mut w = Writer::new();
    w.varint(0x01);
    w.string(input);
    Ok(w.data)
}

/// Serverbound 0x04 Player Position, with absolute feet Y (no head-Y field).
pub fn player_position_body(x: f64, y: f64, z: f64, on_ground: bool) -> Result<Vec<u8>> {
    if ![x, y, z].into_iter().all(f64::is_finite) {
        return Err(Error::new("1.8 player position must be finite"));
    }
    let mut w = Writer::new();
    w.varint(0x04);
    w.f64(x);
    w.f64(y);
    w.f64(z);
    w.bool(on_ground);
    Ok(w.data)
}

/// Clientbound 0x13 DestroyEntities, with a bounded VarInt list.
pub fn decode_destroy_entities(payload: &[u8]) -> Result<Vec<i32>> {
    let mut r = Reader::new(payload);
    let count = r.count(65535)?;
    // Reject absurd allocation requests with fewer wire bytes than entries.
    if count > r.remaining() {
        return Err(Error::new("1.8 entity count exceeds available bytes"));
    }
    let mut entities = Vec::with_capacity(count);
    for _ in 0..count {
        entities.push(r.varint()?);
    }
    ensure_consumed(&r, "destroy entities")?;
    Ok(entities)
}

/// Clientbound 0x15 entity-relative move: 1.8's signed byte deltas divide by 32.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityRelativeMove {
    pub entity_id: i32,
    pub dx: f64,
    pub dy: f64,
    pub dz: f64,
    pub on_ground: bool,
}
pub fn decode_entity_relative_move(payload: &[u8]) -> Result<EntityRelativeMove> {
    let mut r = Reader::new(payload);
    let value = EntityRelativeMove {
        entity_id: r.varint()?,
        dx: f64::from(r.i8()?) / 32.0,
        dy: f64::from(r.i8()?) / 32.0,
        dz: f64::from(r.i8()?) / 32.0,
        on_ground: r.bool()?,
    };
    ensure_consumed(&r, "entity relative move")?;
    Ok(value)
}


/// Clientbound 0x16 — rotation only. Angle is byte/256 of a turn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityLook {
    pub entity_id:i32,pub yaw:f32,pub pitch:f32,pub on_ground:bool,
}
pub fn decode_entity_look(payload:&[u8])->Result<EntityLook>{
    let mut r=Reader::new(payload);
    let result=EntityLook{
        entity_id:r.varint()?,
        yaw:f32::from(r.u8()?)*(360.0/256.0),
        pitch:f32::from(r.u8()?)*(360.0/256.0),
        on_ground:r.bool()?,
    };
    ensure_consumed(&r,"entity look")?;
    Ok(result)
}
/// Clientbound 0x17 — relative movement with yaw/pitch and ground flag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityLookMove {
    pub entity_id:i32,pub dx:f64,pub dy:f64,pub dz:f64,
    pub yaw:f32,pub pitch:f32,pub on_ground:bool,
}
pub fn decode_entity_look_move(payload:&[u8])->Result<EntityLookMove>{
    let mut r=Reader::new(payload);
    let result=EntityLookMove{
        entity_id:r.varint()?,
        dx:f64::from(r.i8()?)/32.0,dy:f64::from(r.i8()?)/32.0,
        dz:f64::from(r.i8()?)/32.0,
        yaw:f32::from(r.u8()?)*(360.0/256.0),
        pitch:f32::from(r.u8()?)*(360.0/256.0),
        on_ground:r.bool()?,
    };
    ensure_consumed(&r,"entity look and move")?;
    Ok(result)
}
/// Clientbound 0x19 — head yaw independent of body yaw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityHeadLook {pub entity_id:i32,pub yaw:f32}
pub fn decode_entity_head_look(payload:&[u8])->Result<EntityHeadLook>{
    let mut r=Reader::new(payload);
    let result=EntityHeadLook{
        entity_id:r.varint()?,yaw:f32::from(r.u8()?)*(360.0/256.0),
    };
    ensure_consumed(&r,"entity head look")?;
    Ok(result)
}

/// Signed x/z and 12-bit Y, packed as (x:26, z:26, y:12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockPosition {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}
impl BlockPosition {
    pub fn from_wire(bits: i64) -> Self {
        Self {
            x: (bits >> 38) as i32,
            y: (bits & 0xfff) as i32,
            z: ((bits << 26) >> 38) as i32,
        }
    }
    pub fn to_wire(self) -> Result<i64> {
        if !(-33_554_432..=33_554_431).contains(&self.x)
            || !(-33_554_432..=33_554_431).contains(&self.z)
            || !(0..=4095).contains(&self.y) {
            return Err(Error::new("1.8 block position is outside 26/12-bit bounds"));
        }
        Ok(((i64::from(self.x) & 0x3ffffff) << 38)
            | ((i64::from(self.z) & 0x3ffffff) << 12)
            | i64::from(self.y))
    }
}

/// Server block-state identifier (Block.BLOCK_STATE_IDS, not 26.3 registry ID).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockChange {
    pub position: BlockPosition,
    pub state_id: i32,
}
/// Clientbound 0x23: one block change.
pub fn decode_block_change(data: &[u8]) -> Result<BlockChange> {
    let mut r = Reader::new(data);
    let position = BlockPosition::from_wire(r.i64()?);
    let state_id = r.varint()?;
    if state_id < 0 { return Err(Error::new("negative 1.8 block state ID")); }
    ensure_consumed(&r, "block change")?;
    Ok(BlockChange { position, state_id })
}

/// Clientbound 0x22: multiple block changes in one chunk column.
pub fn decode_multi_block_change(data: &[u8]) -> Result<Vec<BlockChange>> {
    let mut r = Reader::new(data);
    let chunk_x = r.i32()?;
    let chunk_z = r.i32()?;
    let n = r.count(65_536)?;
    if n > r.remaining() / 3 {
        return Err(Error::new("not enough bytes for 1.8 block-change records"));
    }
    let mut changed = Vec::with_capacity(n);
    for _ in 0..n {
        let xz = r.u8()?;
        let y = i32::from(r.u8()?);
        let state_id = r.varint()?;
        if state_id < 0 { return Err(Error::new("negative 1.8 block state ID")); }
        changed.push(BlockChange {
            position: BlockPosition {
                x: chunk_x.saturating_mul(16).saturating_add(i32::from(xz >> 4)),
                y,
                z: chunk_z.saturating_mul(16).saturating_add(i32::from(xz & 15)),
            },
            state_id,
        });
    }
    ensure_consumed(&r, "multi block change")?;
    Ok(changed)
}

/// Clientbound 0x18 entity teleport (x/y/z are fixed point / 32).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityTeleport {
    pub entity_id: i32,
    pub x: f64, pub y: f64, pub z: f64,
    pub yaw: f32, pub pitch: f32,
    pub on_ground: bool,
}
pub fn decode_entity_teleport(data: &[u8]) -> Result<EntityTeleport> {
    let mut r = Reader::new(data);
    let value = EntityTeleport {
        entity_id: r.varint()?,
        x: f64::from(r.i32()?) / 32.0,
        y: f64::from(r.i32()?) / 32.0,
        z: f64::from(r.i32()?) / 32.0,
        yaw: f32::from(r.u8()?) * (360.0 / 256.0),
        pitch: f32::from(r.u8()?) * (360.0 / 256.0),
        on_ground: r.bool()?,
    };
    ensure_consumed(&r, "entity teleport")?;
    Ok(value)
}

/// Clientbound 0x12 velocity, fixed point /8000.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityVelocity {
    pub entity_id: i32,
    pub x: f64, pub y: f64, pub z: f64,
}
pub fn decode_entity_velocity(data: &[u8]) -> Result<EntityVelocity> {
    let mut r = Reader::new(data);
    let value = EntityVelocity {
        entity_id: r.varint()?,
        x: f64::from(r.i16()?) / 8000.0,
        y: f64::from(r.i16()?) / 8000.0,
        z: f64::from(r.i16()?) / 8000.0,
    };
    ensure_consumed(&r, "entity velocity")?;
    Ok(value)
}

/// Serverbound 0x03, movement heartbeat and ground state.
pub fn player_ground_body(on_ground: bool) -> Vec<u8> {
    let mut w = Writer::new(); w.varint(0x03); w.bool(on_ground); w.data
}

/// Serverbound 0x05, look-only tick.
pub fn player_look_body(yaw: f32, pitch: f32, on_ground: bool) -> Result<Vec<u8>> {
    if !yaw.is_finite() || !pitch.is_finite() {
        return Err(Error::new("1.8 rotation must be finite"));
    }
    let mut w = Writer::new();
    w.varint(0x05); w.f32(yaw); w.f32(pitch); w.bool(on_ground);
    Ok(w.data)
}

/// Serverbound 0x07, block digging / abort / completion / dropping.
pub fn digging_body(status: u8, position: BlockPosition, face: u8) -> Result<Vec<u8>> {
    if status > 5 || face > 5 {
        return Err(Error::new("invalid 1.8 digging status or face"));
    }
    let mut w = Writer::new();
    w.varint(0x07); w.u8(status); w.i64(position.to_wire()?); w.u8(face);
    Ok(w.data)
}


/// Serverbound 0x0b: 1.8 EntityAction. The local player entity ID comes
/// from JoinGame (0x01), and action IDs match the pre-1.9 server protocol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum PlayerAction {
    StartSneaking=0,
    StopSneaking=1,
    LeaveBed=2,
    StartSprinting=3,
    StopSprinting=4,
    StartHorseJump=5,
    StopHorseJump=6,
    OpenHorseInventory=7,
}
pub fn player_action_body(entity_id:i32,action:PlayerAction,jump_boost:i32)->Result<Vec<u8>> {
    if entity_id<0 {return Err(Error::new("negative player entity ID"));}
    if jump_boost<0 {return Err(Error::new("negative 1.8 horse jump boost"));}
    let mut w=Writer::new();
    w.varint(0x0b);w.varint(entity_id);w.varint(action as i32);w.varint(jump_boost);
    Ok(w.data)
}

/// Serverbound 0x0a: swing arm (without a modern hand enum).
pub fn swing_arm_body() -> Vec<u8> { vec![0x0a] }


/// C08 in 1.8: right-click sword in air for blocking. This predates the
/// dual-wield/hand enum; the sentinel position is (-1,-1,-1), face 255.
/// The held item is a legacy NBT Slot, not a modern data-component item.
pub fn sword_block_body(item:Option<&super::inventory::ItemStack>)->Result<Vec<u8>> {
    let mut w=Writer::new();
    w.varint(0x08);
    w.i64(-1); // Sentinel: all 64 packed position bits set.
    w.u8(255);
    super::inventory::write_slot(&mut w,item)?;
    w.u8(0);w.u8(0);w.u8(0);
    Ok(w.data)
}

/// C07 release item use: status=5 with an unused BlockPos and face.
pub fn release_use_item_body()->Vec<u8>{
    let mut w=Writer::new();
    w.varint(0x07);w.u8(5);w.i64(0);w.u8(0);
    w.data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_game_1_8_packet() {
        let mut w = Writer::new();
        w.i32(7); w.u8(8 | 1); w.i8(-1); w.u8(2); w.u8(20);
        w.string("default"); w.bool(true);
        assert_eq!(decode_join_game(&w.data).unwrap(), JoinGame {
            entity_id: 7, game_mode: 1, hardcore: true, dimension: -1,
            difficulty: 2, max_players: 20, world_type: "default".to_string(),
            reduced_debug_info: true,
        });
        w.u8(0);
        assert!(decode_join_game(&w.data).is_err());
    }

    #[test]
    fn position_and_confirmation_have_no_teleport_id() {
        let mut w = Writer::new();
        w.f64(2.0); w.f64(-1.0); w.f64(3.0);
        w.f32(10.0); w.f32(20.0); w.u8(1 | 2 | 16);
        let relative = decode_position_look(&w.data).unwrap();
        let previous = PositionLook { x: 100.0, y: 63.0, z: 0.0,
            yaw: 90.0, pitch: 5.0, flags: 0 };
        let absolute = relative.absolute(previous);
        assert_eq!(absolute.x, 102.0);
        assert_eq!(absolute.y, 62.0);
        assert_eq!(absolute.pitch, 25.0);
        let bytes = position_look_response_body(absolute, false);
        let mut r = Reader::new(&bytes);
        assert_eq!(r.varint().unwrap(), 6);
        assert_eq!(r.f64().unwrap(), 102.0);
        assert_eq!(r.f64().unwrap(), 62.0);
        assert_eq!(r.f64().unwrap(), 3.0);
        assert_eq!(r.f32().unwrap(), 10.0);
        assert_eq!(r.f32().unwrap(), 25.0);
        assert!(!r.bool().unwrap());
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn a_section_contains_packed_numeric_blocks_and_light() {
        let mut packet = Writer::new();
        packet.i32(-2); packet.i32(9); packet.bool(true); packet.u16(0x0002);
        let mut data = vec![0; 8192 + 2048 + 2048 + 256];
        // Section y=1, local (2,3,4); ID=35 wool, metadata=14 red.
        let byte_index = (3*256 + 4*16 + 2) * 2;
        let packed: u16 = (35 << 4) | 14;
        data[byte_index..byte_index+2].copy_from_slice(&packed.to_le_bytes());
        packet.byte_array(&data);
        let chunk = decode_chunk_data(&packet.data, true).unwrap();
        assert_eq!((chunk.x, chunk.z), (-2, 9));
        assert_eq!(chunk.sections.len(), 1);
        assert_eq!(chunk.sections[0].y, 1);
        assert_eq!(chunk.sections[0].block(2, 3, 4), Some((35, 14)));
        assert_eq!(chunk.sections[0].block(16, 3, 4), None);
        assert_eq!(chunk.sections[0].block_light.len(), 2048);
        assert_eq!(chunk.sections[0].sky_light.unwrap().len(), 2048);
        assert_eq!(chunk.biomes.unwrap().len(), 256);
        let mut corrupted = packet.data.clone();
        corrupted.pop();
        assert!(decode_chunk_data(&corrupted, true).is_err());
        assert!(decode_chunk_data(&packet.data, false).is_err());
    }

    #[test]
    fn zero_length_chunk_unload_uses_protocol_47_sentinel() {
        let mut w=Writer::new();
        w.i32(-3);w.i32(4);w.bool(true);w.u16(0);w.byte_array(&[]);
        let chunk=decode_chunk_data(&w.data,true).unwrap();
        assert_eq!((chunk.x,chunk.z),(-3,4));
        assert!(chunk.sections.is_empty());
        assert!(chunk.biomes.is_none());
    }

    #[test]
    fn no_skylight_and_chunk_unload() {
        let mut packet = Writer::new();
        packet.i32(0); packet.i32(0); packet.bool(true); packet.u16(0);
        packet.byte_array(&[0u8; 256]);
        let empty = decode_chunk_data(&packet.data, false).unwrap();
        assert!(empty.sections.is_empty());
        assert_eq!(empty.biomes.unwrap().len(), 256);
    }

    #[test]
    fn client_settings_uses_1_8_layout() {
        let settings = client_settings_body("en_US", 12, 0x7f).unwrap();
        assert_eq!(settings, [0x15, 5, b'e', b'n', b'_', b'U', b'S', 12, 0, 1, 127]);
    }

    #[test]
    fn bulk_chunks_follow_metadata_then_implicit_payload_lengths() {
        let mut w = Writer::new();
        w.bool(false); w.varint(2);
        w.i32(1); w.i32(2); w.u16(0);
        w.i32(3); w.i32(4); w.u16(1);
        w.bytes(&[42u8; 256]);
        w.bytes(&vec![0u8; 8192 + 2048 + 256]);
        let chunks = decode_chunk_bulk(&w.data).unwrap();
        assert_eq!(chunks.len(), 2);
        assert_eq!((chunks[0].x, chunks[0].z), (1, 2));
        assert_eq!(chunks[0].biomes.unwrap()[0], 42);
        assert_eq!((chunks[1].x, chunks[1].z), (3, 4));
        assert_eq!(chunks[1].sections.len(), 1);
        assert!(chunks[1].sections[0].sky_light.is_none());
        w.u8(0);
        assert!(decode_chunk_bulk(&w.data).is_err());
    }

    #[test]
    fn pvp_attack_and_interaction_wire_47() {
        assert_eq!(use_entity_body(300, UseEntityAction::Attack).unwrap(),
            [0x02, 0xac, 0x02, 0x01]);
        assert_eq!(use_entity_body(1, UseEntityAction::Interact).unwrap(),
            [0x02, 0x01, 0x00]);
        assert!(use_entity_body(-1, UseEntityAction::Attack).is_err());
        assert!(use_entity_body(1, UseEntityAction::InteractAt {
            x: f32::NAN, y: 1.0, z: 1.0,
        }).is_err());
        let body = use_entity_body(2, UseEntityAction::InteractAt {
            x: 0.5, y: 1.0, z: 0.25,
        }).unwrap();
        let mut r = Reader::new(&body);
        assert_eq!((r.varint().unwrap(), r.varint().unwrap(), r.varint().unwrap()), (2, 2, 2));
        assert_eq!([r.f32().unwrap(), r.f32().unwrap(), r.f32().unwrap()], [0.5, 1.0, 0.25]);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn chat_and_movement_wire_47() {
        assert_eq!(chat_message_body("gg").unwrap(), [0x01, 2, b'g', b'g']);
        assert!(chat_message_body("").is_err());
        assert!(chat_message_body(&"a".repeat(101)).is_err());
        let mut w = Writer::new();
        w.string(r#"{"text":"hello"}"#);
        w.u8(2);
        assert_eq!(decode_chat_message(&w.data).unwrap().position, 2);
        w.u8(0);
        assert!(decode_chat_message(&w.data).is_err());
        let bytes = player_position_body(1.0, 64.0, -3.0, true).unwrap();
        let mut r = Reader::new(&bytes);
        assert_eq!(r.varint().unwrap(), 4);
        assert_eq!([r.f64().unwrap(), r.f64().unwrap(), r.f64().unwrap()], [1.0,64.0,-3.0]);
        assert!(r.bool().unwrap());
        assert!(player_position_body(f64::INFINITY, 1.0, 2.0, false).is_err());
    }

    #[test]
    fn destroy_entities_and_relative_movement() {
        let mut w = Writer::new();
        w.varint(2); w.varint(4); w.varint(300);
        assert_eq!(decode_destroy_entities(&w.data).unwrap(), vec![4, 300]);
        w.varint(7);
        assert!(decode_destroy_entities(&w.data).is_err());
        let mut m = Writer::new();
        m.varint(5); m.i8(-32); m.i8(16); m.i8(8); m.bool(true);
        assert_eq!(decode_entity_relative_move(&m.data).unwrap(), EntityRelativeMove {
            entity_id:5, dx:-1.0, dy:0.5, dz:0.25, on_ground:true,
        });
    }

    #[test]
    fn legacy_block_updates_decode_signed_position_and_chunk_local_coordinates() {
        let pos = BlockPosition { x: -123, y: 70, z: -17 };
        assert_eq!(BlockPosition::from_wire(pos.to_wire().unwrap()), pos);
        let mut w = Writer::new();
        w.i64(pos.to_wire().unwrap()); w.varint(35 << 4 | 14);
        assert_eq!(decode_block_change(&w.data).unwrap().state_id, 574);
        let mut bulk = Writer::new();
        bulk.i32(-2); bulk.i32(4); bulk.varint(2);
        bulk.u8((3 << 4) | 5); bulk.u8(64); bulk.varint(1);
        bulk.u8(15); bulk.u8(70); bulk.varint(0);
        let changes = decode_multi_block_change(&bulk.data).unwrap();
        assert_eq!(changes[0].position, BlockPosition { x: -29, y: 64, z: 69 });
        assert_eq!(changes[1].position, BlockPosition { x: -32, y: 70, z: 79 });
        assert!(decode_multi_block_change(&bulk.data[..bulk.data.len()-1]).is_err());
    }

    #[test]
    fn old_entity_rotations_decode_full_movement_and_reject_trailing_bytes() {
        let mut look=Writer::new();
        look.varint(123);look.u8(64);look.u8(192);look.bool(true);
        let l=decode_entity_look(&look.data).unwrap();
        assert_eq!((l.entity_id,l.yaw,l.pitch,l.on_ground),(123,90.0,270.0,true));
        look.u8(0);assert!(decode_entity_look(&look.data).is_err());

        let mut move_and_look=Writer::new();
        move_and_look.varint(123);move_and_look.i8(-32);
        move_and_look.i8(8);move_and_look.i8(16);
        move_and_look.u8(128);move_and_look.u8(0);move_and_look.bool(false);
        let m=decode_entity_look_move(&move_and_look.data).unwrap();
        assert_eq!((m.dx,m.dy,m.dz),(-1.0,0.25,0.5));
        assert_eq!((m.yaw,m.pitch,m.on_ground),(180.0,0.0,false));

        let mut head=Writer::new();head.varint(123);head.u8(32);
        assert_eq!(decode_entity_head_look(&head.data).unwrap().yaw,45.0);
        assert!(decode_entity_head_look(&head.data[..1]).is_err());
    }

    #[test]
    fn sprint_sneak_packets_use_legacy_entity_actions() {
        assert_eq!(player_action_body(5,PlayerAction::StartSprinting,0).unwrap(),
            [0x0b,5,3,0]);
        assert_eq!(player_action_body(5,PlayerAction::StopSprinting,0).unwrap(),
            [0x0b,5,4,0]);
        assert_eq!(player_action_body(5,PlayerAction::StartSneaking,0).unwrap(),
            [0x0b,5,0,0]);
        assert!(player_action_body(-1,PlayerAction::StartSprinting,0).is_err());
    }

    #[test]
    fn old_entity_teleport_and_velocity_and_player_actions() {
        let mut w = Writer::new();
        w.varint(10); w.i32(-64); w.i32(2048); w.i32(96);
        w.u8(64); w.u8(128); w.bool(true);
        let teleport = decode_entity_teleport(&w.data).unwrap();
        assert_eq!((teleport.x, teleport.y, teleport.z), (-2.0, 64.0, 3.0));
        assert_eq!(teleport.yaw, 90.0);
        let mut vel = Writer::new();
        vel.varint(10); vel.i16(8000); vel.i16(-4000); vel.i16(0);
        let velocity = decode_entity_velocity(&vel.data).unwrap();
        assert_eq!((velocity.x, velocity.y, velocity.z), (1.0, -0.5, 0.0));
        assert_eq!(player_ground_body(true), [3,1]);
        assert_eq!(swing_arm_body(), [10]);
        assert_eq!(digging_body(0, BlockPosition {x: 0,y:64,z:0}, 1).unwrap().len(), 11);
        assert!(player_look_body(f32::NAN, 0.0, false).is_err());
    }

}
