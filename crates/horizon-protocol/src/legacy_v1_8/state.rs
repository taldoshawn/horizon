//! Server-authoritative world, entity and inventory state for protocol 47.
//!
//! 1.8 packet fields are intentionally not translated into 26.3 registries.
//! This is a bounded, reusable state model. A native 1.8 renderer and game
//! simulation must consume this model before the Android client is playable.
use std::collections::HashMap;
use crate::{Error, Result};
use super::{keep_alive_id, keep_alive_response_body, inventory, play, plugin, spawn};

const MAX_COLUMNS: usize = 1024;
const MAX_ENTITIES: usize = 16_384;
const MAX_WINDOWS: usize = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub y: u8,
    /// Legacy state = (numeric_block_id << 4) | metadata.
    pub states: Vec<u16>,
    pub block_light: Vec<u8>,
    pub sky_light: Option<Vec<u8>>,
}
impl Section {
    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<(u16,u8)> {
        if x>=16 || y>=16 || z>=16 {return None;}
        let state=*self.states.get(y*256+z*16+x)?;
        Some((state >> 4, (state & 15) as u8))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub sections: Vec<Option<Section>>,
    pub biomes: Option<Vec<u8>>,
}
impl Default for Column {
    fn default() -> Self { Self { sections: vec![None;16], biomes: None } }
}
impl Column {
    pub fn block(&self, x: usize,y:usize,z:usize)->Option<(u16,u8)> {
        if y>255{return None;}
        self.sections.get(y/16)?.as_ref()?.block(x,y%16,z)
    }
    fn apply_chunk(&mut self, chunk: &play::ChunkData<'_>) {
        if chunk.full {self.sections=vec![None;16];}
        for chunk_section in &chunk.sections {
            let mut states=Vec::with_capacity(4096);
            for packed in chunk_section.packed_blocks.chunks_exact(2) {
                states.push(u16::from_le_bytes([packed[0],packed[1]]));
            }
            self.sections[chunk_section.y as usize]=Some(Section {
                y:chunk_section.y,
                states,
                block_light:chunk_section.block_light.to_vec(),
                sky_light:chunk_section.sky_light.map(|s|s.to_vec()),
            });
        }
        if let Some(biomes)=chunk.biomes {self.biomes=Some(biomes.to_vec());}
    }
    fn set_block(&mut self, x:usize,y:usize,z:usize,state_id:i32)->Result<bool> {
        if x>=16||z>=16||y>255||!(0..=65535).contains(&state_id) {
            return Err(Error::new("legacy block update outside bounds"));
        }
        let Some(s)=self.sections[y/16].as_mut() else {return Ok(false);};
        s.states[(y%16)*256+z*16+x]=state_id as u16;
        Ok(true)
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackedEntity {
    pub id:i32,
    pub kind:Option<spawn::EntityKind>,
    pub x:f64,pub y:f64,pub z:f64,
    pub yaw:f32,pub pitch:f32,
    pub vx:f64,pub vy:f64,pub vz:f64,
    pub on_ground:bool,
}
impl TrackedEntity {
    fn new(id:i32)->Self {
        Self{id,kind:None,x:0.0,y:0.0,z:0.0,yaw:0.0,pitch:0.0,vx:0.0,vy:0.0,vz:0.0,on_ground:false}
    }
}
#[derive(Debug,Clone,PartialEq,Eq)]
pub struct InventoryWindow {
    pub slots:Vec<Option<inventory::ItemStack>>,
}

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum UpdateKind {
    Joined,
    PlayerCorrected,
    Chunk,
    ChunkBulk,
    Block,
    Entity,
    Inventory,
    Chat,
    Plugin,
    KeepAlive,
    Ignored,
}
/// Result of applying an inbound packet. Any response bytes are a complete
/// packet body (VarInt packet ID plus the old payload); Connection::send frames it.
#[derive(Debug)]
pub struct Applied {
    pub kind:UpdateKind,
    pub replies:Vec<Vec<u8>>,
}
impl Applied {
    fn new(kind:UpdateKind)->Self {Self{kind,replies:Vec::new()}}
}
#[derive(Debug)]
pub struct LegacyGame {
    pub join:Option<play::JoinGame>,
    pub player:play::PositionLook,
    pub on_ground:bool,
    pub columns:HashMap<(i32,i32),Column>,
    pub entities:HashMap<i32,TrackedEntity>,
    pub windows:HashMap<u8,InventoryWindow>,
    pub cursor:Option<inventory::ItemStack>,
    pub last_chat:Option<play::ChatMessage>,
    pub plugin_channels:Vec<String>,
    /// Recognize servers requiring FML, without spoofing Forge client support.
    pub forge_required:bool,
}
impl Default for LegacyGame {
    fn default()->Self {Self {
        join:None,
        player:play::PositionLook{x:0.0,y:64.0,z:0.0,yaw:0.0,pitch:0.0,flags:0},
        on_ground:false,
        columns:HashMap::new(),entities:HashMap::new(),windows:HashMap::new(),
        cursor:None,last_chat:None,plugin_channels:Vec::new(),forge_required:false,
    }}
}
impl LegacyGame {
    fn insert_chunk(&mut self,chunk:&play::ChunkData<'_>)->Result<()> {
        let key=(chunk.x,chunk.z);
        // Vanilla 1.8 unload: ground-up continuous with no sections and no
        // biome data. Keep the 256-byte empty full chunk if the server sent it.
        if chunk.full&&chunk.sections.is_empty()&&chunk.biomes.is_none(){
            self.columns.remove(&key);
            return Ok(());
        }
        if !self.columns.contains_key(&key) && self.columns.len() >= MAX_COLUMNS {
            // A view-distance cap limits persistent memory on Android. Remove
            // the most distant column, never evict a chunk being overwritten.
            let px=self.player.x.floor() as i32 / 16;
            let pz=self.player.z.floor() as i32 / 16;
            if let Some(farthest)=self.columns.keys().copied().max_by_key(|&(x,z)|{
                i64::from(x.saturating_sub(px)).pow(2)+i64::from(z.saturating_sub(pz)).pow(2)
            }) {self.columns.remove(&farthest);}
        }
        self.columns.entry(key).or_default().apply_chunk(chunk);
        Ok(())
    }
    pub fn block(&self,x:i32,y:i32,z:i32)->Option<(u16,u8)> {
        if !(0..=255).contains(&y){return None;}
        let chunk_x=x.div_euclid(16);
        let chunk_z=z.div_euclid(16);
        self.columns.get(&(chunk_x,chunk_z))?.block(
            x.rem_euclid(16) as usize,y as usize,z.rem_euclid(16) as usize)
    }
    fn set_block(&mut self, change:play::BlockChange)->Result<()> {
        let p=change.position;
        if !(0..=255).contains(&p.y){return Err(Error::new("block change Y out of bounds"));}
        if let Some(column)=self.columns.get_mut(&(p.x.div_euclid(16),p.z.div_euclid(16))) {
            column.set_block(p.x.rem_euclid(16) as usize,p.y as usize,
                p.z.rem_euclid(16) as usize,change.state_id)?;
        }
        Ok(())
    }
    fn update_entity<F:FnOnce(&mut TrackedEntity)>(&mut self,id:i32, update:F) {
        if id < 0 {return;}
        if self.entities.contains_key(&id) {
            if let Some(entity)=self.entities.get_mut(&id) {update(entity);}
        } else if self.entities.len() < MAX_ENTITIES {
            let mut entity=TrackedEntity::new(id);
            update(&mut entity);
            self.entities.insert(id,entity);
        }
    }
    /// Apply one PLAY packet from an authenticated 1.8 server.
    /// This is NOT a replacement for collision, game ticking or rendering.
    pub fn apply(&mut self,id:i32,payload:&[u8])->Result<Applied> {
        if let Some(value)=keep_alive_id(id,payload)? {
            return Ok(Applied{kind:UpdateKind::KeepAlive,replies:vec![keep_alive_response_body(value)]});
        }
        let kind=match id {
            0x01 => {
                let join=play::decode_join_game(payload)?;
                self.join=Some(join);
                self.entities.clear();self.columns.clear();self.windows.clear();
                self.cursor=None;
                UpdateKind::Joined
            },
            0x08 => {
                let correction=play::decode_position_look(payload)?;
                self.player=correction.absolute(self.player);
                return Ok(Applied{kind:UpdateKind::PlayerCorrected,
                    replies:vec![play::position_look_response_body(self.player,self.on_ground)]});
            },
            0x21 => {
                let sky=self.join.as_ref().map_or(true,|j|j.dimension==0);
                let chunk=play::decode_chunk_data(payload,sky)?;
                self.insert_chunk(&chunk)?;
                UpdateKind::Chunk
            },
            0x26 => {
                let chunks=play::decode_chunk_bulk(payload)?;
                for chunk in chunks {self.insert_chunk(&chunk)?;}
                UpdateKind::ChunkBulk
            },
            0x23 => {
                let change=play::decode_block_change(payload)?;
                self.set_block(change)?;
                UpdateKind::Block
            },
            0x22 => {
                for change in play::decode_multi_block_change(payload)? {self.set_block(change)?;}
                UpdateKind::Block
            },
            // Explicit spawn packets must anchor entity position before
            // relative moves. Numeric types remain legacy-only.
            0x0c | 0x0f => {
                let spawned=if id==0x0c {spawn::decode_player(payload)?}
                    else {spawn::decode_mob(payload)?};
                self.update_entity(spawned.entity_id,|entity|{
                    entity.kind=Some(spawned.kind);
                    entity.x=spawned.x;entity.y=spawned.y;entity.z=spawned.z;
                    entity.yaw=spawned.yaw;entity.pitch=spawned.pitch;
                    entity.vx=spawned.velocity_x;
                    entity.vy=spawned.velocity_y;
                    entity.vz=spawned.velocity_z;
                });
                UpdateKind::Entity
            },
            0x12 => {
                let e=play::decode_entity_velocity(payload)?;
                self.update_entity(e.entity_id,|entity|{
                    entity.vx=e.x;entity.vy=e.y;entity.vz=e.z;
                });
                UpdateKind::Entity
            },
            0x15 => {
                let m=play::decode_entity_relative_move(payload)?;
                self.update_entity(m.entity_id,|entity|{
                    entity.x+=m.dx;entity.y+=m.dy;entity.z+=m.dz;
                    entity.on_ground=m.on_ground;
                });
                UpdateKind::Entity
            },
            0x18 => {
                let m=play::decode_entity_teleport(payload)?;
                self.update_entity(m.entity_id,|entity|{
                    entity.x=m.x;entity.y=m.y;entity.z=m.z;
                    entity.yaw=m.yaw;entity.pitch=m.pitch;entity.on_ground=m.on_ground;
                });
                UpdateKind::Entity
            },
            0x13 => {
                for entity in play::decode_destroy_entities(payload)? {self.entities.remove(&entity);}
                UpdateKind::Entity
            },
            0x2f => {
                let update=inventory::decode_set_slot(payload)?;
                if update.window_id==-1 && update.slot==-1 {
                    self.cursor=update.item;
                } else if update.window_id>=0 && update.slot>=0 {
                    let window=update.window_id as u8;
                    if let Some(slots)=self.windows.get_mut(&window) {
                        if let Some(slot)=slots.slots.get_mut(update.slot as usize) {
                            *slot=update.item;
                        }
                    }
                }
                UpdateKind::Inventory
            },
            0x30 => {
                let update=inventory::decode_window_items(payload)?;
                if !self.windows.contains_key(&update.window_id)&&self.windows.len()>=MAX_WINDOWS {
                    return Err(Error::new("legacy inventory windows exceed limit"));
                }
                self.windows.insert(update.window_id,InventoryWindow{slots:update.items});
                UpdateKind::Inventory
            },
            0x32 => {
                let tx=inventory::decode_confirm_transaction(payload)?;
                if !tx.accepted {
                    return Ok(Applied{kind:UpdateKind::Inventory,
                        replies:vec![inventory::confirm_transaction_body(tx)]});
                }
                UpdateKind::Inventory
            },
            0x02 => {
                self.last_chat=Some(play::decode_chat_message(payload)?);
                UpdateKind::Chat
            },
            0x3f => {
                let msg=plugin::decode_plugin_message(payload)?;
                if msg.channel=="REGISTER" {
                    self.plugin_channels=plugin::decode_registered_channels(&msg.data)?;
                }
                if plugin::inspect_forge_handshake(&msg)?.is_some() {
                    self.forge_required=true;
                }
                UpdateKind::Plugin
            },
            _ => UpdateKind::Ignored,
        };
        Ok(Applied::new(kind))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;

    #[test]
    fn syncs_chunks_block_updates_and_negative_coordinates() {
        let mut world=LegacyGame::default();
        let mut raw=vec![0u8;8192+2048+2048+256];
        let i=(3*256+4*16+2)*2;
        raw[i..i+2].copy_from_slice(&(35u16<<4|14).to_le_bytes());
        let mut p=Writer::new();
        p.i32(-2);p.i32(9);p.bool(true);p.u16(1);p.byte_array(&raw);
        assert_eq!(world.apply(0x21,&p.data).unwrap().kind,UpdateKind::Chunk);
        assert_eq!(world.block(-30,3,148),Some((35,14)));
        let mut update=Writer::new();
        update.i64(play::BlockPosition{x:-30,y:3,z:148}.to_wire().unwrap());
        update.varint(1 << 4);
        world.apply(0x23,&update.data).unwrap();
        assert_eq!(world.block(-30,3,148),Some((1,0)));
        assert_eq!(world.block(-17,3,148),Some((0,0)));
        assert_eq!(world.block(-30,-1,148),None);
    }
    #[test]
    fn joins_updates_player_and_acks_transactions() {
        let mut state=LegacyGame::default();
        let mut w=Writer::new();
        w.i32(7);w.u8(0);w.i8(0);w.u8(2);w.u8(20);w.string("default");w.bool(false);
        assert_eq!(state.apply(1,&w.data).unwrap().kind,UpdateKind::Joined);
        let mut position=Writer::new();
        position.f64(1.0);position.f64(65.0);position.f64(-2.0);
        position.f32(45.0);position.f32(5.0);position.u8(0);
        let result=state.apply(8,&position.data).unwrap();
        assert_eq!(result.kind,UpdateKind::PlayerCorrected);
        assert_eq!(result.replies[0][0],6);
        assert_eq!((state.player.x,state.player.y,state.player.z),(1.0,65.0,-2.0));
        let ack=state.apply(0x32,&[1,0,4,0]).unwrap();
        assert_eq!(ack.replies[0],[15,1,0,4,0]);
        let keep=state.apply(0,&[0x2c]).unwrap();
        assert_eq!(keep.replies[0],[0,0x2c]);
    }
    #[test]
    fn spawn_player_then_relative_move_updates_absolute_coordinates() {
        let mut game=LegacyGame::default();
        let mut data=Writer::new();
        data.varint(99);data.uuid(0x123456);
        data.i32(320);data.i32(2048);data.i32(-64);
        data.u8(0);data.u8(64);data.i16(276);data.u8(0x7f);
        assert_eq!(game.apply(0x0c,&data.data).unwrap().kind,UpdateKind::Entity);
        assert_eq!(game.entities[&99].kind,
            Some(spawn::EntityKind::Player{uuid:0x123456,held_item_id:276}));
        assert_eq!((game.entities[&99].x,game.entities[&99].y,game.entities[&99].z),
            (10.0,64.0,-2.0));
        let mut rel=Writer::new();
        rel.varint(99);rel.i8(16);rel.i8(0);rel.i8(0);rel.bool(true);
        game.apply(0x15,&rel.data).unwrap();
        assert_eq!(game.entities[&99].x,10.5);
    }

    #[test]
    fn syncs_inventory_entities_plugins_and_removals() {
        let mut state=LegacyGame::default();
        let mut items=Writer::new();
        items.u8(0);items.i16(1);inventory::write_slot(&mut items,None).unwrap();
        assert_eq!(state.apply(0x30,&items.data).unwrap().kind,UpdateKind::Inventory);
        assert_eq!(state.windows.get(&0).unwrap().slots.len(),1);
        let mut set=Writer::new();
        set.i8(0);set.i16(0);
        inventory::write_slot(&mut set,Some(&inventory::ItemStack{
            numeric_id:276,count:1,damage:0,tag:None,
        })).unwrap();
        state.apply(0x2f,&set.data).unwrap();
        assert_eq!(state.windows[&0].slots[0].as_ref().unwrap().numeric_id,276);
        let mut t=Writer::new();
        t.varint(12);t.i32(32);t.i32(2048);t.i32(-64);
        t.u8(0);t.u8(0);t.bool(true);
        state.apply(0x18,&t.data).unwrap();
        assert_eq!(state.entities[&12].y,64.0);
        let mut d=Writer::new();d.varint(1);d.varint(12);
        state.apply(0x13,&d.data).unwrap();
        assert!(!state.entities.contains_key(&12));
        let register=plugin::register_channels_body(&["BungeeCord"]).unwrap();
        state.apply(0x3f,&register[1..]).unwrap();
        assert_eq!(state.plugin_channels,["BungeeCord"]);
    }
}
