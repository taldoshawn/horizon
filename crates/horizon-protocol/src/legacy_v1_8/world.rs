//! Read-only Minecraft Java 1.8.9 Anvil chunk decoder.
//! This does NOT launch a 1.8 world or write back to player saves.
use std::io::Read;
use flate2::read::{GzDecoder, ZlibDecoder};
use horizon_world::nbt::{self, Tag};
use crate::{Error, Result};

const SECTOR: usize = 4096;
const MAX_INFLATED: usize = 16 * 1024 * 1024;
const MAX_COMPRESSED: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldSection {
    pub y: u8,
    /// Each value holds numeric legacy block ID in bits 4..15 and metadata in bits 0..3.
    pub state: Vec<u16>,
    pub block_light: Vec<u8>,
    pub sky_light: Option<Vec<u8>>,
}
impl OldSection {
    pub fn block(&self, x: u8, y: u8, z: u8) -> Option<(u16, u8)> {
        if x > 15 || y > 15 || z > 15 { return None; }
        let packed = self.state[usize::from(y)*256+usize::from(z)*16+usize::from(x)];
        Some((packed >> 4, (packed & 15) as u8))
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OldColumn {
    pub chunk_x: i32,
    pub chunk_z: i32,
    pub sections: Vec<OldSection>,
    pub biomes: Option<Vec<u8>>,
}
fn bytes<'a>(tag: &'a Tag, key: &str, n: usize) -> Result<&'a [u8]> {
    match tag.get(key) {
        Some(Tag::ByteArray(data)) if data.len()==n => Ok(data),
        _ => Err(Error::new(format!("1.8 chunk invalid {key} array"))),
    }
}
fn optional<'a>(tag: &'a Tag, key: &str, n: usize) -> Result<Option<&'a [u8]>> {
    match tag.get(key) {
        Some(Tag::ByteArray(data)) if data.len()==n => Ok(Some(data)),
        None => Ok(None),
        _ => Err(Error::new(format!("1.8 chunk invalid optional {key} array"))),
    }
}
fn nibble(bytes: &[u8], i: usize) -> u8 {
    if i%2==0 { bytes[i/2]&15 } else { bytes[i/2]>>4 }
}
/// Decode a raw named-NBT root from an old Anvil chunk.
pub fn decode_nbt(data: &[u8], x: i32, z: i32) -> Result<OldColumn> {
    if data.len()>MAX_INFLATED { return Err(Error::new("1.8 NBT over limit")); }
    let limits=nbt::Limits { max_bytes:MAX_INFLATED, max_nodes:200_000, max_depth:80 };
    let (root,used)=nbt::read_named(data,limits)
        .map_err(|e| Error::new(format!("1.8 NBT: {e}")))?;
    if used!=data.len() { return Err(Error::new("trailing 1.8 NBT")); }
    let level=root.tag.get("Level").ok_or_else(|| Error::new("missing Level tag"))?;
    if !matches!((level.get("xPos"),level.get("zPos")),
        (Some(Tag::Int(a)),Some(Tag::Int(b))) if (*a,*b)==(x,z)) {
        return Err(Error::new("1.8 chunk coordinates mismatch"));
    }
    let source=match level.get("Sections") {
        Some(Tag::List(items)) if items.len()<=16 => items,
        _ => return Err(Error::new("invalid 1.8 section list")),
    };
    let mut sections=Vec::with_capacity(source.len());
    let mut seen=0u16;
    for section in source {
        let y=match section.get("Y") {
            Some(Tag::Byte(y)) if (0..16).contains(y) => *y as u8,
            _ => return Err(Error::new("invalid 1.8 section height")),
        };
        let bit=1u16 << y;
        if seen&bit!=0 { return Err(Error::new("duplicate 1.8 section height")); }
        seen|=bit;
        let blocks=bytes(section,"Blocks",4096)?;
        let data=bytes(section,"Data",2048)?;
        let add=optional(section,"Add",2048)?;
        let block_light=bytes(section,"BlockLight",2048)?.to_vec();
        let sky_light=optional(section,"SkyLight",2048)?.map(|v|v.to_vec());
        let mut state=Vec::with_capacity(4096);
        for i in 0..4096 {
            let id=u16::from(blocks[i])
                | (u16::from(add.map_or(0,|a|nibble(a,i)))<<8);
            state.push((id<<4)|u16::from(nibble(data,i)));
        }
        sections.push(OldSection {y,state,block_light,sky_light});
    }
    sections.sort_by_key(|s|s.y);
    let biomes=optional(level,"Biomes",256)?.map(|v|v.to_vec());
    Ok(OldColumn {chunk_x:x,chunk_z:z,sections,biomes})
}

/// Load one chunk from bytes of its correct .mca region file.
/// Returns None if the chunk is absent. Never writes to disk.
pub fn read_anvil_chunk(region: &[u8], x: i32, z: i32) -> Result<Option<OldColumn>> {
    if region.len()<SECTOR*2 {return Err(Error::new("truncated Anvil header"));}
    let idx=((z.rem_euclid(32) as usize)*32+(x.rem_euclid(32) as usize))*4;
    let offset=((region[idx] as usize)<<16)
        |((region[idx+1] as usize)<<8)|(region[idx+2] as usize);
    let count=region[idx+3] as usize;
    if offset==0&&count==0{return Ok(None);}
    if offset<2||count==0{return Err(Error::new("invalid Anvil sectors"));}
    let start=offset.checked_mul(SECTOR).ok_or_else(||Error::new("Anvil offset overflow"))?;
    let end=start.checked_add(count*SECTOR).ok_or_else(||Error::new("Anvil size overflow"))?;
    if end>region.len() {return Err(Error::new("Anvil chunk out of bounds"));}
    let length=u32::from_be_bytes(region[start..start+4].try_into().unwrap()) as usize;
    if !(2..=MAX_COMPRESSED+1).contains(&length)||length+4>count*SECTOR {
        return Err(Error::new("invalid Anvil compressed length"));
    }
    let payload=&region[start+5..start+4+length];
    let decoder:Box<dyn Read+'_>=match region[start+4] {
        1=>Box::new(GzDecoder::new(payload)),
        2=>Box::new(ZlibDecoder::new(payload)),
        _=>return Err(Error::new("unsupported Anvil compression")),
    };
    let mut expanded=Vec::new();
    decoder.take((MAX_INFLATED+1) as u64).read_to_end(&mut expanded)
        .map_err(|e|Error::new(format!("1.8 decompress: {e}")))?;
    if expanded.len()>MAX_INFLATED {return Err(Error::new("inflated 1.8 chunk too large"));}
    Ok(Some(decode_nbt(&expanded,x,z)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{write::ZlibEncoder, Compression};
    use horizon_world::nbt::{NamedTag,JavaString};
    use std::io::Write;

    #[test]
    fn decodes_named_legacy_block_arrays_from_region() {
        let mut blocks=vec![0;4096];blocks[0]=1;blocks[1]=44;
        let mut data=vec![0;2048];data[0]=0x70;
        let mut add=vec![0;2048];add[0]=0x10;
        let section=Tag::compound([
            ("Y",Tag::Byte(0)),("Blocks",Tag::ByteArray(blocks)),
            ("Data",Tag::ByteArray(data)),("Add",Tag::ByteArray(add)),
            ("BlockLight",Tag::ByteArray(vec![0;2048])),
        ]);
        let level=Tag::compound([
            ("xPos",Tag::Int(-1)),("zPos",Tag::Int(33)),
            ("Sections",Tag::List(vec![section])),
            ("Biomes",Tag::ByteArray(vec![1;256])),
        ]);
        let named=NamedTag {
            name:JavaString::from(""),tag:Tag::compound([("Level",level)])
        };
        let plain=nbt::write_named(&named,nbt::Limits::default()).unwrap();
        let column=decode_nbt(&plain,-1,33).unwrap();
        assert_eq!(column.sections[0].block(0,0,0),Some((1,0)));
        assert_eq!(column.sections[0].block(1,0,0),Some((300,7)));
        let mut encoder=ZlibEncoder::new(Vec::new(),Compression::fast());
        encoder.write_all(&plain).unwrap();
        let compressed=encoder.finish().unwrap();
        let n=(compressed.len()+5).div_ceil(SECTOR);
        let mut region=vec![0;n*SECTOR+2*SECTOR];
        let idx=(31+32)*4;
        region[idx+2]=2;region[idx+3]=n as u8;
        let at=2*SECTOR;
        region[at..at+4].copy_from_slice(&((compressed.len()+1) as u32).to_be_bytes());
        region[at+4]=2;
        region[at+5..at+5+compressed.len()].copy_from_slice(&compressed);
        assert!(read_anvil_chunk(&region,-1,33).unwrap().is_some());
        assert!(read_anvil_chunk(&region,0,33).unwrap().is_none());
        assert!(read_anvil_chunk(&region,-1,34).unwrap().is_none());
    }
}
