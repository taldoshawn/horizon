//! Entity spawn packets from the *Minecraft 1.8.9* protocol 47.
//! Their numeric types and fixed-point positions must not enter 26.3 entity
//! registries without a complete legacy registry/renderer mapping.
use crate::{Error, Result,buf::Reader};

const MAX_METADATA:usize=64*1024;
const ROTATION_SCALE:f32=360.0/256.0;

#[derive(Debug,Clone,Copy,PartialEq,Eq)]
pub enum EntityKind {
    Player{uuid:u128,held_item_id:i16},
    Mob{type_id:u8},
}

#[derive(Debug,Clone,PartialEq)]
pub struct Spawn {
    pub entity_id:i32,
    pub kind:EntityKind,
    pub x:f64,pub y:f64,pub z:f64,
    pub yaw:f32,pub pitch:f32,pub head_yaw:f32,
    pub velocity_x:f64,pub velocity_y:f64,pub velocity_z:f64,
    /// Opaque 1.8 DataWatcher bytes, still requiring its terminator 0x7F.
    pub metadata:Vec<u8>,
}
fn metadata(reader:&mut Reader<'_>)->Result<Vec<u8>> {
    let rest=reader.rest();
    if rest.is_empty()||rest.len()>MAX_METADATA||rest.last()!=Some(&0x7f) {
        return Err(Error::new("legacy DataWatcher metadata missing terminator or too long"));
    }
    Ok(rest.to_vec())
}
fn fixed(raw:i32)->f64 {f64::from(raw)/32.0}
fn rotation(raw:u8)->f32 {f32::from(raw)*ROTATION_SCALE}

/// Clientbound SpawnPlayer 0x0C: entity ID, UUID, fixed point position,
/// yaw/pitch, held item ID and DataWatcher.
pub fn decode_player(payload:&[u8])->Result<Spawn> {
    let mut r=Reader::new(payload);
    let entity_id=r.varint()?;
    if entity_id<0 {return Err(Error::new("negative 1.8 spawned player entity ID"));}
    let uuid=r.uuid()?;
    let x=fixed(r.i32()?);
    let y=fixed(r.i32()?);
    let z=fixed(r.i32()?);
    let yaw=rotation(r.u8()?);
    let pitch=rotation(r.u8()?);
    let held_item_id=r.i16()?;
    let metadata=metadata(&mut r)?;
    Ok(Spawn {
        entity_id,kind:EntityKind::Player{uuid,held_item_id},
        x,y,z,yaw,pitch,head_yaw:yaw,
        velocity_x:0.0,velocity_y:0.0,velocity_z:0.0,metadata,
    })
}
/// Clientbound SpawnMob 0x0F: entity ID, numeric mob type, fixed position,
/// yaw/pitch/head yaw, velocity scaled by 8000 and DataWatcher.
pub fn decode_mob(payload:&[u8])->Result<Spawn> {
    let mut r=Reader::new(payload);
    let entity_id=r.varint()?;
    if entity_id<0 {return Err(Error::new("negative 1.8 mob entity ID"));}
    let type_id=r.u8()?;
    let x=fixed(r.i32()?);
    let y=fixed(r.i32()?);
    let z=fixed(r.i32()?);
    let yaw=rotation(r.u8()?);
    let pitch=rotation(r.u8()?);
    let head_yaw=rotation(r.u8()?);
    let velocity_x=f64::from(r.i16()?)/8000.0;
    let velocity_y=f64::from(r.i16()?)/8000.0;
    let velocity_z=f64::from(r.i16()?)/8000.0;
    let metadata=metadata(&mut r)?;
    Ok(Spawn {
        entity_id,kind:EntityKind::Mob{type_id},
        x,y,z,yaw,pitch,head_yaw,
        velocity_x,velocity_y,velocity_z,metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;
    #[test]
    fn player_spawn_wires_uuid_and_position() {
        let mut w=Writer::new();
        w.varint(123);w.uuid(0x1234);
        w.i32(-64);w.i32(2048);w.i32(96);
        w.u8(64);w.u8(0);w.i16(276);w.u8(0x7f);
        let p=decode_player(&w.data).unwrap();
        assert_eq!(p.entity_id,123);
        assert_eq!(p.kind,EntityKind::Player{uuid:0x1234,held_item_id:276});
        assert_eq!((p.x,p.y,p.z),(-2.0,64.0,3.0));
        assert_eq!(p.yaw,90.0);
        w.data.pop();
        assert!(decode_player(&w.data).is_err());
    }
    #[test]
    fn mob_spawn_wires_classic_numeric_type_and_velocity() {
        let mut w=Writer::new();
        w.varint(9);w.u8(54);w.i32(32);w.i32(2048);w.i32(64);
        w.u8(0);w.u8(0);w.u8(0);
        w.i16(8000);w.i16(-4000);w.i16(0);w.u8(0x7f);
        let m=decode_mob(&w.data).unwrap();
        assert_eq!(m.kind,EntityKind::Mob{type_id:54});
        assert_eq!((m.x,m.y,m.z),(1.0,64.0,2.0));
        assert_eq!((m.velocity_x,m.velocity_y,m.velocity_z),(1.0,-0.5,0.0));
    }
}
