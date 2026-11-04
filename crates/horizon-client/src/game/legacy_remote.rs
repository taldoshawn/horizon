//! The *development-only* native protocol-47 entry into Horizon's actual
//! game loop. This does NOT reuse the modern protocol or a proxy.
//!
//! 1.8 game rules are still incomplete: input-to-motion, biome/light parity,
//! worldgen, entities, inventory UI and Forge content require further work.
//! Disabled in ordinary launcher builds until the gameplay release gate passes.
use super::*;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use horizon_client::{
    legacy_chunk::LegacyBlockMap,
    legacy_world::{LegacyRenderSync, NativeChunkSink},
};
use horizon_protocol::{
    Result as ProtocolResult, Error as ProtocolError,
    legacy_v1_8::{
        async_session::LegacyWireWorker,
        physics::{ClassicPlayer, CollisionWorld, Controls, Aabb, Vec3 as OldVec3},
        play::{self,PlayerAction},spawn,
        session::ClientAction,
        state::{LegacyGame, UpdateKind},
    },
};

/// Android launcher owns this setting; it is cleared when returning to the
/// menu. Never enable the experimental game path through a public metadata tag.
#[cfg(target_os="android")]
static ANDROID_NATIVE_189:std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os="android")]
pub(super) fn set_android_native_189(value:bool) {
    ANDROID_NATIVE_189.store(value,std::sync::atomic::Ordering::Release);
}

/// A block-breaking request already sent to the server. Progress is local
/// feedback; the server alone decides whether a block is destroyed.
#[derive(Clone,Copy,Debug)]
struct LegacyMining { pos:play::BlockPosition,face:u8,progress:f32 }

/// 1.8 face ordering for a raycast's outward face normal.
fn old_face(face:[i32;3])->Option<u8> {
    match face {
        [0,-1,0]=>Some(0),[0,1,0]=>Some(1),
        [0,0,-1]=>Some(2),[0,0,1]=>Some(3),
        [-1,0,0]=>Some(4),[1,0,0]=>Some(5),
        _=>None,
    }
}

pub(super) struct NativeLegacy {
    connecting: Option<Receiver<std::result::Result<LegacyWireWorker, String>>>,
    worker: Option<LegacyWireWorker>,
    state: LegacyGame,
    renderer: LegacyRenderSync,
    motion: ClassicPlayer,
    last_sprint:bool,
    last_sneak:bool,
    last_selected:u8,
    mining:Option<LegacyMining>,
}
impl NativeLegacy {
    fn biome_map(biomes:&horizon_client::biome::Biomes)->[u16;256] {
        let plains=biomes.id("minecraft:plains");
        let mut lookup=[plains;256];
        // 1.8 numeric IDs, *not* the post-flattening ordinal order.
        let vanilla = [
            (0,"ocean"),(1,"plains"),(2,"desert"),(3,"windswept_hills"),
            (4,"forest"),(5,"taiga"),(6,"swamp"),(7,"river"),
            (8,"nether_wastes"),(9,"the_end"),(10,"frozen_ocean"),
            (11,"frozen_river"),(12,"snowy_plains"),(13,"snowy_mountains"),
            (14,"mushroom_fields"),(15,"mushroom_field_shore"),
            (16,"beach"),(17,"desert_hills"),(18,"wooded_hills"),
            (19,"taiga_hills"),(20,"mountain_edge"),(21,"jungle"),
            (22,"jungle_hills"),(23,"sparse_jungle"),(24,"deep_ocean"),
            (25,"stony_shore"),(26,"snowy_beach"),(27,"birch_forest"),
            (28,"birch_forest_hills"),(29,"dark_forest"),
            (30,"snowy_taiga"),(31,"snowy_taiga_hills"),
            (32,"old_growth_pine_taiga"),(33,"giant_tree_taiga_hills"),
            (34,"wooded_mountains"),(35,"savanna"),(36,"savanna_plateau"),
            (37,"badlands"),(38,"wooded_badlands"),(39,"badlands_plateau"),
            (129,"sunflower_plains"),(130,"desert_lakes"),
            (131,"gravelly_mountains"),(132,"flower_forest"),
            (133,"taiga_mountains"),(134,"swamp_hills"),
            (140,"ice_spikes"),(149,"modified_jungle"),(151,"modified_jungle_edge"),
            (155,"tall_birch_forest"),(156,"tall_birch_hills"),
            (157,"dark_forest_hills"),(158,"snowy_taiga_mountains"),
            (160,"giant_spruce_taiga"),(161,"giant_spruce_taiga_hills"),
            (162,"modified_gravelly_mountains"),(163,"shattered_savanna"),
            (164,"shattered_savanna_plateau"),(165,"eroded_badlands"),
            (166,"modified_wooded_badlands_plateau"),(167,"modified_badlands_plateau"),
        ];
        for (old,name) in vanilla {
            lookup[old]=biomes.id(&format!("minecraft:{name}"));
        }
        lookup
    }
    fn new(
        registry:&horizon_world::registry::BlockRegistry,
        biomes:&horizon_client::biome::Biomes,
        receiver:Receiver<std::result::Result<LegacyWireWorker,String>>,
    )->Self {
        Self {
            connecting:Some(receiver),worker:None,
            state:LegacyGame::default(),
            renderer:LegacyRenderSync::new(
                LegacyBlockMap::from_registry(registry),
                Self::biome_map(biomes)
            ),
            motion:ClassicPlayer::new(0.0,64.0,0.0),
            last_sprint:false,
            last_sneak:false,
            last_selected:0,
            mining:None,
        }
    }
}
/// Collision inputs from the 1.8 numeric chunk state. Unlike a modern
/// BlockStateId, the ID and four metadata bits are read from the *old* world.
/// Full parity needs legacy neighbor-dependent shapes, piston motion, entity
/// collisions, fluids and on-block speed effects.
struct OldWorldCollision<'a> { game:&'a LegacyGame }
impl CollisionWorld for OldWorldCollision<'_> {
    fn boxes(&self,swept:Aabb)->Vec<Aabb>{
        let (min_x,max_x)=(swept.min.x.floor() as i32,swept.max.x.ceil() as i32);
        let (min_y,max_y)=(swept.min.y.floor() as i32,swept.max.y.ceil() as i32);
        let (min_z,max_z)=(swept.min.z.floor() as i32,swept.max.z.ceil() as i32);
        let mut out=Vec::new();
        for y in min_y.max(0)..max_y.min(256) {
            for z in min_z..max_z {
                for x in min_x..max_x {
                    let Some((id,meta))=self.game.block(x,y,z) else{continue;};
                    // Transparent, fluid, decorative and climbing blocks
                    // must not become solid cubes.
                    if matches!(id,0|6|8|9|10|11|27|28|30|31|32|37|38|39|40|50|
                        51|55|59|63|65|66|68|69|70|72|75|76|77|83|90|93|94|
                        106|111|115|127|131|132|140|141|142|143|147|148|
                        149|150|151|157|175) {
                        continue;
                    }
                    let (bottom,top) = match id {
                        // Slab metadata bit 3: upper half.
                        44|126|182|205=>if meta&8!=0 {(0.5,1.0)}else{(0.0,0.5)},
                        26=>(0.0,0.5625), // bed
                        60|208=>(0.0,0.9375), // farmland / grass path
                        78=>(0.0,(f64::from(meta)+1.0)/8.0), // snow layers
                        88=>(0.0,0.875), // soul sand
                        171=>(0.0,0.0625), // carpet
                        85|113|139|188|189|190|191|192=>(0.0,1.5),
                        _=>(0.0,1.0),
                    };
                    let block= Aabb {
                        min:OldVec3::new(f64::from(x),f64::from(y)+bottom,f64::from(z)),
                        max:OldVec3::new(f64::from(x)+1.0,f64::from(y)+top,f64::from(z)+1.0),
                    };
                    out.push(block);
                }
            }
        }
        out
    }
}


/// Legacy 1.8 numeric entity types; this mapping is visual only.
/// It does not imply compatibility of 26.3 entity AI/data watchers.
fn legacy_entity_model(kind:spawn::EntityKind)->Option<&'static str>{
    use spawn::EntityKind;
    match kind {
        EntityKind::Player{..}=>Some("minecraft:player"),
        EntityKind::Mob{type_id}=>Some(match type_id {
            50=>"minecraft:creeper",51=>"minecraft:skeleton",
            52=>"minecraft:spider",53=>"minecraft:giant",
            54=>"minecraft:zombie",55=>"minecraft:slime",
            56=>"minecraft:ghast",57=>"minecraft:zombified_piglin",
            58=>"minecraft:enderman",59=>"minecraft:cave_spider",
            60=>"minecraft:silverfish",61=>"minecraft:blaze",
            62=>"minecraft:magma_cube",63=>"minecraft:ender_dragon",
            64=>"minecraft:wither",65=>"minecraft:bat",
            66=>"minecraft:witch",67=>"minecraft:endermite",
            68=>"minecraft:guardian",90=>"minecraft:pig",
            91=>"minecraft:sheep",92=>"minecraft:cow",
            93=>"minecraft:chicken",94=>"minecraft:squid",
            95=>"minecraft:wolf",96=>"minecraft:mooshroom",
            97=>"minecraft:snow_golem",98=>"minecraft:ocelot",
            99=>"minecraft:iron_golem",100=>"minecraft:horse",
            101=>"minecraft:rabbit",120=>"minecraft:villager",
            _=>return None,
        }),
    }
}

impl Game<'_> {
    /// Developer-only switch. A plain Android version selector remains on
    /// the existing public implementation until native 1.8.9 is actually
    /// playable and benchmarked.
    pub(super) fn native_189_requested(&self)->bool {
        if self.options.native_189 { return true; }
        #[cfg(target_os="android")]
        { ANDROID_NATIVE_189.load(std::sync::atomic::Ordering::Acquire) }
        #[cfg(not(target_os="android"))]
        { std::env::var("HORIZON_NATIVE_189_DEV").ok().as_deref()==Some("1") }
    }
    pub(super) fn connect_native_189(&mut self,address:&str,name:&str)->Result<()> {
        if address.trim().is_empty() {
            return Err("1.8 server address is empty".into());
        }
        let registry=self.pack.assets.registry;
        let account=horizon_client::auth::current();
        let username=account.as_ref().map_or_else(||name.to_owned(),|a|a.name.clone());
        let server=address.to_owned();
        let (tx,rx)=mpsc::sync_channel(1);
        std::thread::Builder::new().name("legacy18-connect".into()).spawn(move||{
            let mut authenticate=|hash:&str|->ProtocolResult<()> {
                let account=account.as_ref().ok_or_else(||ProtocolError::new(
                    "Online 1.8.9 server: sign in with Minecraft Microsoft account"
                ))?;
                horizon_client::auth::join_server(account,hash)
                    .map_err(|e|ProtocolError::new(format!("1.8 session authentication: {e:?}")))
            };
            let result=LegacyWireWorker::connect(
                &server,&username,Duration::from_secs(15),Some(&mut authenticate)
            ).map(|(client,_uuid)|client).map_err(|e|e.to_string());
            let _=tx.send(result);
        })?;
        self.legacy_remote=Some(NativeLegacy::new(
            registry,self.res.biomes(),rx
        ));
        self.reset_tutorial();
        self.perf.world_opening();
        self.world_biomes=None;
        self.clear_lod();
        self.pending_uploads.clear();
        self.reset_legacy_world();
        self.player=None;
        self.inventory=vec![None;36];
        self.armor=[None,None,None,None];
        self.offhand=None;
        self.carried=None;
        self.container=None;
        self.mobs.clear();
        self.dropped.clear();
        self.particles.clear();
        self.title=None;
        self.screen=Screen::Playing;
        self.clock=FixedStep::client();
        self.apply_settings();
        Ok(())
    }
    fn reset_legacy_world(&mut self) {
        let mut streaming=Streaming::new(
            Arc::new(remote::NoTerrain),self.pack.context.clone(),
            self.res.threads(),
        );
        streaming.remote=true;
        streaming.radius=self.settings.render_distance;
        self.streaming=streaming;
        self.world=World::new(self.res.facts().air,0);
        if let Some(r)=self.renderer.as_mut(){
            r.clear_sections(self.world.layout());
        }
    }
    fn sync_native_189_entities(
        &mut self, packet_id:i32, payload:&[u8],state:&LegacyGame
    )->Result<()> {
        match packet_id {
            0x13=>{
                for id in play::decode_destroy_entities(payload)? {
                    self.mobs.retain(|m|m.id != id as u32);
                }
            },
            0x0c|0x0f|0x15|0x16|0x17|0x18|0x19|0x12=>{
                let id=horizon_protocol::buf::Reader::new(payload).varint()?;
                let Some(entity)=state.entities.get(&id) else {return Ok(())};
                let position=[entity.x,entity.y,entity.z];
                let rotation=[entity.yaw,entity.pitch,entity.yaw];
                if packet_id==0x0c||packet_id==0x0f {
                    if let Some(kind)=entity.kind.and_then(legacy_entity_model) {
                        self.spawn_entity(id,kind,position,rotation);
                    }
                }else if let Some(mob)=self.mobs.iter_mut().find(|m|m.id==id as u32){
                    mob.previous=mob.position;
                    mob.position=Vec3::new(entity.x,entity.y,entity.z);
                    (mob.previous_yaw,mob.yaw)=(mob.yaw,entity.yaw);
                    mob.previous_head_pitch=mob.head_pitch;
                    mob.head_pitch=entity.pitch;
                    mob.previous_head_yaw=mob.head_yaw;
                    mob.head_yaw=entity.head_yaw;
                }
            },
            _=>{}
        }
        Ok(())
    }
    pub(super) fn poll_native_189(&mut self)->Result<()> {
        let Some(mut legacy)=self.legacy_remote.take() else{return Ok(());};
        let result=(||->Result<()>{
            if let Some(rx)=legacy.connecting.as_ref() {
                match rx.try_recv(){
                    Ok(Ok(worker))=>{
                        legacy.connecting=None;
                        legacy.worker=Some(worker);
                        log::info!("Native protocol-47 connection established");
                    }
                    Ok(Err(error))=>{
                        return Err(format!("Native Minecraft 1.8.9 connection: {error}").into());
                    }
                    Err(TryRecvError::Empty)=>{},
                    Err(TryRecvError::Disconnected)=>{
                        return Err("Native Minecraft 1.8.9 connector exited".into());
                    }
                }
            }
            let Some(worker)=&legacy.worker else{return Ok(())};
            for event in worker.poll(){
                let packet=worker.apply(event,&mut legacy.state)?;
                if matches!(packet.kind,UpdateKind::Joined|UpdateKind::Respawn) {
                    self.reset_legacy_world();
                    self.player=None;
                    if let Some(j)=&legacy.state.join {
                        self.dim=match j.dimension {
                            -1=>Dim::Nether,1=>Dim::End,_=>Dim::Overworld,
                        };
                        self.set_mode_id(j.game_mode as i32);
                    }
                }
                if packet.kind==UpdateKind::Health {
                    let h=legacy.state.health;
                    if let Some(player)=self.player.as_mut() {
                        player.set_health(h.health.max(0.0),false)?;
                        player.health.food.food=h.food;
                        player.health.food.saturation=h.saturation;
                    }
                    if h.health<=0.0&&self.player.is_some() {
                        self.screen=Screen::Dead;
                        self.capture(false);
                    }
                }
                if packet.kind==UpdateKind::Hotbar {
                    self.selected=usize::from(legacy.state.selected_slot);
                    legacy.last_selected=legacy.state.selected_slot;
                }
                if packet.id==0x12 {
                    let velocity=play::decode_entity_velocity(&packet.payload)?;
                    if legacy.state.join.as_ref().is_some_and(|j|j.entity_id==velocity.entity_id) {
                        legacy.motion.apply_server_velocity(velocity.x,velocity.y,velocity.z)?;
                        if let Some(p)=self.player.as_mut() {
                            p.velocity=Vec3::new(velocity.x,velocity.y,velocity.z);
                        }
                    }
                }
                if packet.kind==UpdateKind::PlayerCorrected {
                    let p=legacy.state.player;
                    legacy.motion.apply_server_position(
                        p.x,p.y,p.z,f64::from(p.yaw),f64::from(p.pitch)
                    )?;
                    let feet=Vec3::new(p.x,p.y,p.z);
                    self.previous_position=feet;
                    self.yaw=p.yaw;
                    self.pitch=p.pitch;
                    if let Some(player)=self.player.as_mut(){
                        player.position=feet;
                        player.velocity=Vec3::ZERO;
                        player.yaw=self.yaw;
                        player.pitch=self.pitch;
                    }else{
                        let mut player=PlayerMotion::new(self.motion,feet)?;
                        player.yaw=self.yaw;
                        player.pitch=self.pitch;
                        self.player=Some(player);
                        self.pad.player_replaced();
                        self.eye=1.62;
                        self.previous_eye=1.62;
                    }
                }
                if packet.kind==UpdateKind::Chat {
                    if let Some(chat)=legacy.state.last_chat.as_ref() {
                        let data=serde_json::from_str::<serde_json::Value>(&chat.json)
                            .unwrap_or_else(|_|serde_json::json!({"text":chat.json}));
                        let message=horizon_client::scoreboard::comp_text(&data).resolve_legacy();
                        let source=horizon_client::chat::ChatSource::Server;
                        if self.ui.chat.shows(source) {
                            self.ui.chat.add(
                                &self.gui.atlas,message,
                                Some(horizon_client::chat::SYSTEM_INDICATOR),
                                source,self.ticks,self.screen==Screen::Chat,
                            );
                        }
                    }
                }
                if packet.kind==UpdateKind::Entity {
                    self.sync_native_189_entities(packet.id,&packet.payload,&legacy.state)?;
                }
                let mut sink=NativeChunkSink {
                    world:&mut self.world,streaming:&mut self.streaming,
                };
                legacy.renderer.apply(
                    &packet,&legacy.state,&mut sink
                ).map_err(|e|format!("1.8 native chunk sync: {e}"))?;
            }
            Ok(())
        })();
        self.legacy_remote=Some(legacy);
        result
    }
    /// Tick only protocol-47 inputs when in 1.8 mode. Never run 26.3 world
    /// simulation or autosave for these worlds.
    /// User-triggered attack from the native touch/mouse button. Entity
    /// targeting is only an *assist*; protocol-47 server validates the hit,
    /// knockback and actual target. Never modify remote entity HP locally.
    pub(super) fn native_189_attack(&mut self)->bool {
        if self.legacy_remote.is_none(){return false;}
        let best=if let Some(player)=self.player.as_ref() {
            let eye=[player.position.x,player.position.y+1.62,player.position.z];
            let dir=self.interaction_direction();
            let mut best=None::<(i32,f64)>;
            if let Some(legacy)=self.legacy_remote.as_ref() {
                for (id,entity) in &legacy.state.entities {
                    if legacy.state.join.as_ref().is_some_and(|j|j.entity_id==*id) {
                        continue;
                    }
                    let min=[entity.x-0.3,entity.y,entity.z-0.3];
                    let max=[entity.x+0.3,entity.y+1.8,entity.z+0.3];
                    let (mut near,mut far)=(0.0f64,3.0f64);
                    for axis in 0..3 {
                        if dir[axis].abs()<1e-8 {
                            if eye[axis]<min[axis]||eye[axis]>max[axis] {
                                far=-1.0;break;
                            }
                        }else{
                            let x=(min[axis]-eye[axis])/dir[axis];
                            let y=(max[axis]-eye[axis])/dir[axis];
                            near=near.max(x.min(y));
                            far=far.min(x.max(y));
                        }
                    }
                    if far>=near && far>=0.0
                        && best.is_none_or(|(_,d)|near<d) {
                        best=Some((*id,near));
                    }
                }
            }
            best.filter(|(_,distance)|self.before_block(*distance))
        }else{None};
        let block_hit=if best.is_none(){self.pick()}else{None};
        let Some(legacy)=self.legacy_remote.as_mut() else{return false};
        let Some(worker)=legacy.worker.as_ref() else{return false};
        if let Err(error)=worker.send_action(ClientAction::SwingArm) {
            log::warn!("1.8 swing: {error}");
            return false;
        }
        if let Some((entity_id,_distance))=best {
            if let Err(error)=worker.send_action(ClientAction::Attack(entity_id)) {
                log::warn!("1.8 attack: {error}");
            }
            legacy.motion.sprinting=false;
        }else if let Some(hit)=block_hit
            && let Some(face)=old_face(hit.face)
        {
            let position=play::BlockPosition{
                x:hit.pos[0],y:hit.pos[1],z:hit.pos[2]
            };
            let next=LegacyMining{pos:position,face,progress:0.0};
            if let Some(previous)=legacy.mining.take() {
                if previous.pos!=position {
                    let _=worker.send_action(ClientAction::Dig{
                        status:1,position:previous.pos,face:previous.face
                    });
                }
            }
            if let Err(error)=worker.send_action(ClientAction::Dig{
                status:0,position,face
            }) {
                log::warn!("1.8 start block digging: {error}");
            }else if self.creative {
                let _=worker.send_action(ClientAction::Dig{
                    status:2,position,face
                });
            }else{
                legacy.mining=Some(next);
            }
        }
        true
    }

    /// One classic right-click from the existing touch/mouse interaction
    /// surface. Block changes and container opens must come from the server;
    /// never run modern local placement/loot on a 1.8 world.
    pub(super) fn native_189_use_hand(&mut self,hit:Option<&Hit>)->Result<bool>{
        let Some(legacy)=self.legacy_remote.as_ref() else{return Ok(false)};
        let Some(worker)=legacy.worker.as_ref() else{return Ok(false)};
        let selected=self.selected.min(8);
        let held=legacy.state.windows.get(&0).and_then(|w|
            w.slots.get(36+selected)).and_then(|s|s.as_ref());
        // Sword blocking is a *held* use action; the 20 TPS input loop
        // sends its C08 start and C07 release without duplicate clicks.
        if held.is_some_and(|item|
            matches!(item.numeric_id,267|268|272|276|283)) {
            return Ok(true);
        }
        if let Some(hit)=hit {
            let Some(face)=old_face(hit.face) else {return Ok(false)};
            let point=std::array::from_fn(|i|{
                ((hit.point[i]-f64::from(hit.pos[i])).clamp(0.0,0.999)*16.0)
                    .floor() as u8
            });
            worker.send_action(ClientAction::Place{
                position:play::BlockPosition{
                    x:hit.pos[0],y:hit.pos[1],z:hit.pos[2]
                },face,item:held,cursor:point,
            })?;
        }else if held.is_some(){
            // Right-click the air (bows, consumables and throwable items).
            worker.send_action(ClientAction::BlockSword(held))?;
        }else{return Ok(false);}
        Ok(true)
    }

    pub(super) fn native_send_chat(&self,line:&str)->Result<()> {
        if let Some(worker)=self.legacy_remote.as_ref().and_then(|l|l.worker.as_ref()) {
            worker.send_action(ClientAction::Chat(line))?;
        }
        Ok(())
    }
    pub(super) fn tick_native_189(&mut self)->Result<()> {
        self.ticks=self.ticks.saturating_add(1);
        let playing=self.screen==Screen::Playing && !self.dialog_open();
        let stick=self.pad_move();
        let held=|key|playing && self.keys.contains(&key);
        let keys=Keys {
            forward:held(KeyCode::KeyW),backward:held(KeyCode::KeyS),
            left:held(KeyCode::KeyA),right:held(KeyCode::KeyD),
            jump:held(KeyCode::Space),shift:held(KeyCode::ShiftLeft),
            sprint:held(KeyCode::ControlLeft),analog:None,
        }.with_stick(stick.filter(|_|playing));
        let Some(legacy)=self.legacy_remote.as_mut() else{return Ok(())};
        if self.player.is_none(){return Ok(())}
        let Some(worker)=legacy.worker.as_ref() else{return Ok(())};
        if let Ok(slot)=u8::try_from(self.selected) {
            if slot<=8 && slot!=legacy.last_selected {
                worker.send_action(ClientAction::SelectHotbar(slot))?;
                legacy.last_selected=slot;
            }
        }
        // Holding right-click with a 1.8 sword blocks instead of activating
        // 26.3 dual-wield/shield behavior. Forge/old items require an adapter.
        let chosen=legacy.state.windows.get(&0).and_then(|w|
            w.slots.get(36+self.selected)).and_then(|s|s.as_ref());
        let has_sword=chosen.is_some_and(|s|
            matches!(s.numeric_id,267|268|272|276|283));
        let should_block=playing&&self.right&&has_sword;
        if should_block!=legacy.motion.using_sword {
            if should_block {worker.send_action(ClientAction::BlockSword(chosen))?;}
            else {worker.send_action(ClientAction::ReleaseSword)?;}
            legacy.motion.using_sword=should_block;
        }
        let movement=keys.direction();
        // The old network state, not a modern 26.3 registry, supplies
        // collision blocks. The exact historic block-shape table still needs
        // independent 1.8 parity testing before any public release.
        let at=legacy.motion.feet;
        if !legacy.state.columns.contains_key(&(
            (at.x.floor() as i32).div_euclid(16),
            (at.z.floor() as i32).div_euclid(16)
        )) {
            worker.send_action(ClientAction::OnGround(false))?;
            return Ok(());
        }
        legacy.motion.yaw=f64::from(self.yaw);
        legacy.motion.pitch=f64::from(self.pitch);
        legacy.motion.tick(Controls {
            forward:f64::from(movement[1]),
            strafe:f64::from(movement[0]),
            jump:keys.jump,sprint:keys.sprint,sneak:keys.shift,
        },&OldWorldCollision{game:&legacy.state})?;
        let id=legacy.state.join.as_ref().map(|j|j.entity_id);
        if let Some(id)=id {
            if legacy.motion.sprinting!=legacy.last_sprint {
                let action=if legacy.motion.sprinting {
                    PlayerAction::StartSprinting
                }else{PlayerAction::StopSprinting};
                worker.send_body(play::player_action_body(id,action,0)?)?;
                legacy.last_sprint=legacy.motion.sprinting;
            }
            if keys.shift!=legacy.last_sneak {
                let action=if keys.shift {
                    PlayerAction::StartSneaking
                }else{PlayerAction::StopSneaking};
                worker.send_body(play::player_action_body(id,action,0)?)?;
                legacy.last_sneak=keys.shift;
            }
        }
        let feet=legacy.motion.feet;
        self.previous_position=self.player.as_ref().expect("checked").position;
        if let Some(p)=self.player.as_mut() {
            p.position=Vec3::new(feet.x,feet.y,feet.z);
            p.velocity=Vec3::new(
                legacy.motion.velocity.x,legacy.motion.velocity.y,legacy.motion.velocity.z
            );
            p.yaw=self.yaw;p.pitch=self.pitch;
        }
        worker.send_action(ClientAction::PositionLook {
            x:feet.x,y:feet.y,z:feet.z,yaw:self.yaw,pitch:self.pitch,
            on_ground:legacy.motion.on_ground,
        })?;
        Ok(())
    }
}
