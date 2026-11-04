//! Deterministic vanilla-style 1.8 superflat terrain seed and Anvil chunk bytes.
//!
//! This implements the default superflat *layers* only (bedrock, two dirt,
//! grass). It does not implement 1.8 overworld biome noise, caves, villages,
//! mobs, lighting propagation or integrated singleplayer server ticks.
use std::io::Write;
use flate2::{Compression, write::ZlibEncoder};
use horizon_world::nbt::{self, NamedTag, JavaString, Tag};
use crate::{Error, Result};
use super::world;

const SECTOR:usize=4096;
const BLOCKS_PER_SECTION:usize=4096;
const HALF_SECTION:usize=2048;
pub const FLAT_TOP_Y:i32=4;

/// Build a 1.8.9 NBT chunk holding the classic default superflat layers.
/// Blocks are integer IDs (7 bedrock, 3 dirt, 2 grass), NOT modern block states.
pub fn default_superflat_chunk_nbt(x:i32,z:i32)->Result<Vec<u8>>{
    let mut blocks=vec![0u8;BLOCKS_PER_SECTION];
    let data=vec![0u8;HALF_SECTION];
    let block_light=vec![0u8;HALF_SECTION];
    let mut sky_light=vec![0u8;HALF_SECTION];
    for y in 0..16 {
        let id=match y {0=>7,1|2=>3,3=>2,_=>0};
        for z in 0..16 {
            for x in 0..16 {
                let index=y*256+z*16+x;
                blocks[index]=id;
                // Upper 12 layers have sky light 15. Ground layers are opaque.
                if y>=4 {
                    if index&1==0 {sky_light[index/2]|=0x0f;}
                    else {sky_light[index/2]|=0xf0;}
                }
            }
        }
    }
    let section=Tag::compound([
        ("Y",Tag::Byte(0)),
        ("Blocks",Tag::ByteArray(blocks)),
        ("Data",Tag::ByteArray(data)),
        ("BlockLight",Tag::ByteArray(block_light)),
        ("SkyLight",Tag::ByteArray(sky_light)),
    ]);
    let level=Tag::compound([
        ("xPos",Tag::Int(x)),
        ("zPos",Tag::Int(z)),
        ("LastUpdate",Tag::Long(0)),
        ("InhabitedTime",Tag::Long(0)),
        ("TerrainPopulated",Tag::Byte(1)),
        ("LightPopulated",Tag::Byte(1)),
        ("Biomes",Tag::ByteArray(vec![1;256])),
        ("HeightMap",Tag::IntArray(vec![FLAT_TOP_Y;256])),
        ("Sections",Tag::List(vec![section])),
        ("Entities",Tag::List(vec![])),
        ("TileEntities",Tag::List(vec![])),
        ("TileTicks",Tag::List(vec![])),
    ]);
    let named=NamedTag {
        name:JavaString::from(""),
        tag:Tag::compound([("Level",level)]),
    };
    nbt::write_named(&named,nbt::Limits::default())
        .map_err(|e| Error::new(format!("serializing 1.8 superflat chunk: {e}")))
}

/// Create a brand-new region file containing one 1.8 chunk, for offline
/// inspection or later use by a real integrated server. Does not modify
/// existing save files. Region file path is r.(x div 32).(z div 32).mca.
pub fn new_superflat_region_file(chunk_x:i32,chunk_z:i32)->Result<Vec<u8>>{
    let plain=default_superflat_chunk_nbt(chunk_x,chunk_z)?;
    let mut encoder=ZlibEncoder::new(Vec::new(),Compression::fast());
    encoder.write_all(&plain).map_err(|e|Error::new(format!("compressing Anvil chunk: {e}")))?;
    let compressed=encoder.finish().map_err(|e|Error::new(format!("finishing Anvil chunk: {e}")))?;
    let length=compressed.len().checked_add(5)
        .ok_or_else(||Error::new("Anvil compressed chunk overflows"))?;
    let count=length.div_ceil(SECTOR);
    if count==0||count>255 {return Err(Error::new("Anvil chunk exceeds 255 sectors"));}
    let mut region=vec![0u8;(2+count)*SECTOR];
    let idx=((chunk_z.rem_euclid(32) as usize)*32
        +(chunk_x.rem_euclid(32) as usize))*4;
    region[idx+2]=2;region[idx+3]=count as u8;
    let start=2*SECTOR;
    let len=u32::try_from(compressed.len()+1)
        .map_err(|_|Error::new("Anvil chunk too large"))?;
    region[start..start+4].copy_from_slice(&len.to_be_bytes());
    region[start+4]=2;
    region[start+5..start+5+compressed.len()].copy_from_slice(&compressed);
    Ok(region)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_flat_layers_roundtrip_through_real_1_8_anvil_bytes() {
        for (x,z) in [(0,0),(-1,33),(64,-65)] {
            let region=new_superflat_region_file(x,z).unwrap();
            let chunk=world::read_anvil_chunk(&region,x,z).unwrap().unwrap();
            assert_eq!((chunk.chunk_x,chunk.chunk_z),(x,z));
            assert_eq!(chunk.sections.len(),1);
            let section=&chunk.sections[0];
            assert_eq!(section.block(0,0,0),Some((7,0)));
            assert_eq!(section.block(0,1,0),Some((3,0)));
            assert_eq!(section.block(0,2,0),Some((3,0)));
            assert_eq!(section.block(0,3,0),Some((2,0)));
            assert_eq!(section.block(0,4,0),Some((0,0)));
            assert!(chunk.biomes.unwrap().iter().all(|&biome|biome==1));
        }
    }
    #[test]
    fn different_coordinates_produce_different_legacy_headers() {
        let a=new_superflat_region_file(0,0).unwrap();
        let b=new_superflat_region_file(1,0).unwrap();
        assert_ne!(&a[..4096],&b[..4096]);
        assert!(world::read_anvil_chunk(&a,1,0).unwrap().is_none());
    }
}
