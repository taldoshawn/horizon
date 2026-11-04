//! Apply *already validated* protocol-47 world packets to the native Rust
//! renderer's chunk storage. The 1.8 session and the renderer must remain
//! separate: a network worker decodes packets; the game thread owns GPU work.
//!
//! The sink abstraction makes the translation testable without GPU hardware.
//! \`NativeChunkSink\` is a real adapter to \`World\` + \`Streaming\`, reusing
//! Horizon's existing chunk insertion, mesh scheduling and chunk eviction.
use horizon_protocol::legacy_v1_8::{
    play,
    session::ReceivedLegacy,
    state::{LegacyGame, UpdateKind},
};
use horizon_world::registry::BlockStateId;

use crate::{
    chunk::Chunk,
    streaming::Streaming,
    world::World,
    legacy_chunk::{LegacyBlockMap,render_old_chunk},
};

pub trait ChunkSink {
    /// Inserts a fully translated 1.8 column, with block/sky light.
    fn loaded(&mut self,chunk:Chunk)->Result<(),String>;
    /// Removes a column the server explicitly unloaded.
    fn unloaded(&mut self,x:i32,z:i32)->Result<(),String>;
    /// A change is authoritative; don't fabricate an item drop or block tick.
    fn block_changed(&mut self,x:i32,y:i32,z:i32,renderer_state:u32)
        ->Result<(),String>;
}
/// Direct integration with the renderer world. Inserted chunks join the same
/// native meshing/streaming path as current-version remote chunks.
/// The caller must NOT use this with a 26.3 simulation, which has different
/// physics, block behaviors and registries.
pub struct NativeChunkSink<'a> {
    pub world:&'a mut World,
    pub streaming:&'a mut Streaming,
}
impl ChunkSink for NativeChunkSink<'_> {
    fn loaded(&mut self,chunk:Chunk)->Result<(),String>{
        self.streaming.insert_remote(self.world,chunk);
        Ok(())
    }
    fn unloaded(&mut self,x:i32,z:i32)->Result<(),String>{
        self.streaming.forget(self.world,x,z);
        Ok(())
    }
    fn block_changed(&mut self,x:i32,y:i32,z:i32,state:u32)->Result<(),String>{
        // A late block update for an unloaded chunk is ignored, as a vanilla
        // client ignores an out-of-view delta.
        if !self.world.chunks.contains_key(&(x.div_euclid(16),z.div_euclid(16))) {
            return Ok(());
        }
        self.world.set(x,y,z,BlockStateId(state)).map_err(|e|e.to_string())
    }
}
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub struct RenderSyncResult {
    pub loaded:usize,
    pub unloaded:usize,
    pub blocks:usize,
    /// A fresh dimension needs a fresh World and Streaming before chunks
    /// arrive. A modern 26.3 player state must not be kept here.
    pub reset_required:bool,
}

/// Protocol-47 → renderer update, using old numeric ID/metadata and lighting.
/// May be used only after \`LegacySession::receive_one_packet\` has applied
/// the payload to its own \`LegacyGame\` state.
pub struct LegacyRenderSync {
    pub block_map:LegacyBlockMap,
    /// 1.8 byte biome ID to index in the installed renderer's biome registry.
    pub biome_map:[u16;256],
    has_skylight:bool,
}
impl LegacyRenderSync {
    pub fn new(block_map:LegacyBlockMap,biome_map:[u16;256])->Self {
        Self{block_map,biome_map,has_skylight:true}
    }
    fn sync_chunk(
        &self,
        state:&LegacyGame,
        sink:&mut impl ChunkSink,
        x:i32,z:i32
    )->Result<bool,String>{
        if let Some(column)=state.columns.get(&(x,z)) {
            let chunk=render_old_chunk(x,z,column,&self.block_map,&self.biome_map,self.has_skylight)?;
            sink.loaded(chunk)?;
            Ok(true)
        } else {
            sink.unloaded(x,z)?;
            Ok(false)
        }
    }
    pub fn apply(
        &mut self, packet:&ReceivedLegacy,
        state:&LegacyGame,
        sink:&mut impl ChunkSink,
    )->Result<RenderSyncResult,String>{
        let mut result=RenderSyncResult::default();
        match (packet.kind,packet.id) {
            (UpdateKind::Joined,0x01)=>{
                self.has_skylight=state.join.as_ref().is_none_or(|j|j.dimension==0);
                result.reset_required=true;
            }
            (UpdateKind::Chunk,0x21)=>{
                let data=play::decode_chunk_data(&packet.payload,self.has_skylight)
                    .map_err(|e|e.to_string())?;
                if self.sync_chunk(state,sink,data.x,data.z)? {
                    result.loaded+=1;
                } else {
                    result.unloaded+=1;
                }
            }
            (UpdateKind::ChunkBulk,0x26)=>{
                for data in play::decode_chunk_bulk(&packet.payload)
                    .map_err(|e|e.to_string())? {
                    if self.sync_chunk(state,sink,data.x,data.z)? {
                        result.loaded+=1;
                    } else {result.unloaded+=1;}
                }
            }
            (UpdateKind::Block,0x23)=>{
                let p=play::decode_block_change(&packet.payload)
                    .map_err(|e|e.to_string())?;
                let pos=p.position;
                sink.block_changed(pos.x,pos.y,pos.z,
                    self.block_map.renderer_state(p.state_id as u16))?;
                result.blocks+=1;
            }
            (UpdateKind::Block,0x22)=>{
                for p in play::decode_multi_block_change(&packet.payload)
                    .map_err(|e|e.to_string())? {
                    let pos=p.position;
                    sink.block_changed(pos.x,pos.y,pos.z,
                        self.block_map.renderer_state(p.state_id as u16))?;
                    result.blocks+=1;
                }
            }
            _=>{}
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::{buf::Writer,legacy_v1_8::play::BlockPosition};

    #[derive(Default)]
    struct MockSink {
        columns:Vec<Chunk>,
        blocks:Vec<(i32,i32,i32,u32)>,
        unload:Vec<(i32,i32)>,
    }
    impl ChunkSink for MockSink {
        fn loaded(&mut self,chunk:Chunk)->Result<(),String>{
            self.columns.push(chunk);
            Ok(())
        }
        fn unloaded(&mut self,x:i32,z:i32)->Result<(),String>{
            self.unload.push((x,z));Ok(())
        }
        fn block_changed(&mut self,x:i32,y:i32,z:i32,id:u32)->Result<(),String>{
            self.blocks.push((x,y,z,id));Ok(())
        }
    }

    #[test]
    fn packet_47_chunk_data_is_mapped_to_native_renderer_and_block_deltas() {
        let palette=LegacyBlockMap::from_registry_test(&[(16,413),(32,429)]);
        let mut biomes=[0u16;256];biomes[1]=9;
        let mut sync=LegacyRenderSync::new(palette,biomes);
        let mut state=LegacyGame::default();
        let mut sink=MockSink::default();
        let mut join=Writer::new();
        join.i32(1);join.u8(0);join.i8(0);join.u8(2);
        join.u8(20);join.string("default");join.bool(false);
        let outcome=state.apply(1,&join.data).unwrap();
        let report=sync.apply(&ReceivedLegacy{
            id:1,payload:join.data,kind:outcome.kind
        },&state,&mut sink).unwrap();
        assert!(report.reset_required);
        let mut data=vec![0u8;8192+2048+2048+256];
        data[0]=16;
        data[8192+2048]=0x0f;
        data[8192+2048+2048..].fill(1);
        let mut packet=Writer::new();
        packet.i32(-1);packet.i32(3);packet.bool(true);packet.u16(1);
        packet.byte_array(&data);
        let kind=state.apply(0x21,&packet.data).unwrap().kind;
        let effect=sync.apply(&ReceivedLegacy{
            id:0x21,payload:packet.data,kind
        },&state,&mut sink).unwrap();
        assert_eq!(effect.loaded,1);
        assert_eq!(sink.columns[0].get(0,0,0),413);
        assert_eq!(sink.columns[0].biome(0,0,0),9);
        assert_eq!(sink.columns[0].light(0,0,0),0xf0);

        let mut update=Writer::new();
        update.i64(BlockPosition{x:-16,y:0,z:48}.to_wire().unwrap());
        update.varint(32);
        let kind=state.apply(0x23,&update.data).unwrap().kind;
        let effect=sync.apply(&ReceivedLegacy{
            id:0x23,payload:update.data,kind
        },&state,&mut sink).unwrap();
        assert_eq!(effect.blocks,1);
        assert_eq!(sink.blocks,vec![(-16,0,48,429)]);

        let mut unload=Writer::new();
        unload.i32(-1);unload.i32(3);unload.bool(true);unload.u16(0);
        unload.byte_array(&[]);
        let kind=state.apply(0x21,&unload.data).unwrap().kind;
        let effect=sync.apply(&ReceivedLegacy{
            id:0x21,payload:unload.data,kind
        },&state,&mut sink).unwrap();
        assert_eq!(effect.unloaded,1);
        assert_eq!(sink.unload,vec![(-1,3)]);
    }
}
