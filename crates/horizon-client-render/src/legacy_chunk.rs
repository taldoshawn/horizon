//! Convert Minecraft Java 1.8.x numeric chunk storage to the Horizon native
//! **rendering storage format**. This is not a 1.8 gameplay compatibility shim:
//! block mechanics, models, item registries and textures still require ports.
//!
//! The source is protocol-47 ChunkData (or a pre-flattening Anvil file).
//! Output is a *native* renderer Chunk, with legacy light, biomes and blocks
//! mapped into the installed game's block/biome registry (not ViaProxy).
use std::sync::Arc;

use horizon_core::paletted::PalettedContainer;
use horizon_protocol::legacy_v1_8::state::Column as OldColumn;
use horizon_world::{
    block_state_properties::BlockProperties,
    chunk::BlockState,
    datafix,
    nbt::Tag,
    registry::BlockRegistry,
};

use crate::chunk::{BIOME_STRATEGY, Chunk, ColumnLayout, Light, Section};

const LEGACY_STATE_CAPACITY:usize=65_536; // 12-bit numeric block + 4-bit metadata
const OLD_SECTION_VOLUME:usize=4096;
const OLD_LIGHT_BYTES:usize=2048;

/// Quality counters matter: an unknown legacy state must not silently be
/// declared "compatible" just because it was drawn as air or a default state.
#[derive(Clone,Copy,Debug,Default,PartialEq,Eq)]
pub struct MappingQuality {
    pub exact:u32,
    pub defaulted:u32,
    pub absent:u32,
}
/// Precomputed map old state → renderer runtime state; 256 KiB per renderer.
///
/// The 1.8 pre-flattened 0..255 block table is reused from the original
/// DataFixer export, but 1.8→current semantics still need parity checks.
/// Forge numeric IDs >255 are intentionally *unmapped* unless configured.
#[derive(Clone)]
pub struct LegacyBlockMap {
    ids:Box<[u32;LEGACY_STATE_CAPACITY]>,
    air:u32,
    pub quality:MappingQuality,
}
fn parse_old_state(tag:&Tag)->Option<BlockState> {
    let Tag::String(name)=tag.get("Name")? else {return None;};
    let mut properties=BlockProperties::new();
    if let Some(props)=tag.get("Properties") {
        let Tag::Compound(props)=props else{return None;};
        for (key,value) in props {
            let Tag::String(value)=value else {return None;};
            properties.insert(key.to_string_lossy(),value.to_string_lossy());
        }
    }
    Some(BlockState{name:name.to_string_lossy(),properties})
}
impl LegacyBlockMap {
    /// Precompute a rendering map. Exact property matches take precedence;
    /// when the *name* still exists but properties changed, use its default
    /// for visualization only. Record such degradations in the quality stats.
    pub fn from_registry(registry:&BlockRegistry)->Self {
        let air=registry.default_state("minecraft:air").map_or(0,|id|id.0);
        let mut ids=Box::new([air;LEGACY_STATE_CAPACITY]);
        let mut quality=MappingQuality::default();
        for old in 0..4096usize {
            let Some(tag)=datafix::legacy_block_state_tag(old as u16) else {
                quality.absent+=1;
                continue;
            };
            let Some(state)=parse_old_state(&tag) else {
                quality.absent+=1;
                continue;
            };
            let name=state.name.as_str();
            if let Ok(id)=registry.resolve(&state) {
                ids[old]=id.0;
                quality.exact+=1;
            } else if let Some(id)=registry.default_state(name) {
                ids[old]=id.0;
                quality.defaulted+=1;
            } else {
                quality.absent+=1;
            }
        }
        Self{ids,air,quality}
    }
    /// The legacy numeric block state is kept as u16 throughout the old
    /// network code. This returns a *runtime renderer* registry ID.
    pub fn renderer_state(&self,legacy:u16)->u32 {self.ids[legacy as usize]}
    pub fn air_state(&self)->u32 {self.air}

    /// Hook for a validated server registry extension or a dedicated
    /// 1.8.9 resource exporter. Forge custom IDs must not be fabricated.
    pub fn override_state(&mut self,old:u16,renderer:u32) {
        self.ids[old as usize]=renderer;
    }
    #[cfg(test)]
    pub(crate) fn from_registry_test(pairs:&[(u16,u32)])->Self {
        let mut ids=Box::new([0u32;LEGACY_STATE_CAPACITY]);
        for (old,new_id) in pairs {ids[*old as usize]=*new_id;}
        Self {ids,air:0,quality:MappingQuality::default()}
    }

}
fn nibble(bytes:&[u8],index:usize)->u8 {
    if index&1==0 {bytes[index>>1]&15} else {bytes[index>>1]>>4}
}
fn biomes(old:Option<&[u8]>,maps:&[u16;256])->Result<[u32;64],String> {
    if let Some(bytes)=old {
        if bytes.len()!=256{return Err("bad 1.8 biome column length".into());}
    }
    Ok(std::array::from_fn(|quart|{
        let z=(quart>>2)&3;
        let x=quart&3;
        let idx=(z*4+2)*16+x*4+2;
        let old_biome=old.map_or(1,|b|b[idx]) as usize;
        u32::from(maps[old_biome])
    }))
}
/// Build a renderer-friendly chunk. Current renderer's VANILLA layout
/// spans -64..320; 1.8 blocks occupy exactly y=0..255. Lighting information
/// from the old server is *not relit* using modern rules.
///
/// The result is suitable for chunk meshing, but not direct insertion into
/// an in-progress 26.3 simulation without a separate legacy game state.
pub fn render_old_chunk(
    x:i32,z:i32,
    old:&OldColumn,
    blocks:&LegacyBlockMap,
    biome_map:&[u16;256],
    has_skylight:bool,
)->Result<Chunk,String> {
    let old_biomes=biomes(old.biomes.as_deref(),biome_map)?;
    let mut native=Chunk::with_layout(x,z,ColumnLayout::VANILLA);
    native.lit=true;
    native.light_loaded=true;
    // Always initialize Nether light to zero instead of inheriting modern
    // Overworld's missing-section full skylight.
    for section in &mut native.sections {
        let s=Arc::make_mut(section);
        s.light=Light::Uniform(if has_skylight{0xf0}else{0x00});
        s.biomes=PalettedContainer::from_values(BIOME_STRATEGY,&old_biomes);
    }
    for (section_y,section) in old.sections.iter().enumerate() {
        let Some(source)=section else {continue;};
        if source.y as usize!=section_y {
            return Err(format!("1.8 section slot mismatch at {}",section_y));
        }
        if source.states.len()!=OLD_SECTION_VOLUME
            || source.block_light.len()!=OLD_LIGHT_BYTES
            || source.sky_light.as_ref().is_some_and(|a|a.len()!=OLD_LIGHT_BYTES)
        {
            return Err(format!("invalid 1.8 section arrays at {}",section_y));
        }
        if has_skylight&&source.sky_light.is_none() {
            return Err(format!("1.8 Overworld section {} lacks skylight",section_y));
        }
        let mut renderer_blocks=[blocks.air;OLD_SECTION_VOLUME];
        let mut renderer_light=[0u8;OLD_SECTION_VOLUME];
        for i in 0..OLD_SECTION_VOLUME {
            renderer_blocks[i]=blocks.renderer_state(source.states[i]);
            let block=nibble(&source.block_light,i);
            let sky=if has_skylight {
                nibble(source.sky_light.as_ref().expect("validated"),i)
            } else {0};
            renderer_light[i]=(sky<<4)|block;
        }
        let target=native.section_index((section_y as i32)*16)
            .ok_or_else(||"1.8 section outside native renderer range".to_owned())?;
        let mut translated=Section {
            blocks:Section::from_blocks(&renderer_blocks),
            biomes:PalettedContainer::from_values(BIOME_STRATEGY,&old_biomes),
            light:Light::Full(Box::new(renderer_light)),
        };
        translated.compact();
        native.sections[target]=Arc::new(translated);
    }
    Ok(native)
}

#[cfg(test)]
mod tests {
    use super::*;
    use horizon_protocol::legacy_v1_8::state::Section as OldSection;

    #[test]
    fn legacy_packed_blocks_lights_and_biomes_reach_native_chunk() {
        let mut ids=Box::new([0u32;LEGACY_STATE_CAPACITY]);
        ids[16]=912; // Old block ID=1, data=0
        let block_map=LegacyBlockMap{
            ids,air:0,quality:MappingQuality::default()
        };
        let mut states=vec![0u16;4096];
        let idx=3*256+4*16+2;
        states[idx]=16;
        let mut block_light=vec![0u8;2048];
        let mut sky_light=vec![0u8;2048];
        block_light[idx/2]=0x0A;
        sky_light[idx/2]=0x0B;
        let old=OldColumn{
            sections:(0..16).map(|sy|if sy==0 {
                Some(OldSection {
                    y:0,states:states.clone(),
                    block_light:block_light.clone(),
                    sky_light:Some(sky_light.clone()),
                })
            }else{None}).collect(),
            biomes:Some(vec![1;256]),
        };
        let mut biome_map=[0u16;256];biome_map[1]=7;
        let chunk=render_old_chunk(-2,9,&old,&block_map,&biome_map,true).unwrap();
        assert_eq!((chunk.x,chunk.z),(-2,9));
        assert_eq!(chunk.get(2,3,4),912);
        assert_eq!(chunk.light(2,3,4),0xBA);
        assert_eq!(chunk.biome(2,3,4),7);
        assert_eq!(chunk.get(0,255,0),0);
        assert!(chunk.light_loaded);
    }
    #[test]
    fn nether_chunk_has_no_skylight_and_rejects_bad_arrays() {
        let block_map=LegacyBlockMap {
            ids:Box::new([0;LEGACY_STATE_CAPACITY]),air:0,
            quality:MappingQuality::default(),
        };
        let biome_map=[0u16;256];
        let mut old=OldColumn {
            sections:vec![None;16],
            biomes:None,
        };
        let chunk=render_old_chunk(0,0,&old,&block_map,&biome_map,false).unwrap();
        assert_eq!(chunk.light(2,67,4),0);
        old.sections[3]=Some(OldSection{
            y:3,states:vec![0;4096],
            block_light:vec![0;2047],sky_light:None,
        });
        assert!(render_old_chunk(0,0,&old,&block_map,&biome_map,false).is_err());
    }
}
