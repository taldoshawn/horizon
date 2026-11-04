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
        async_session::{LegacyWireWorker, LegacyInbound},
        physics::ClassicPlayer,
        session::ClientAction,
        state::{LegacyGame, UpdateKind},
    },
};

pub(super) struct NativeLegacy {
    connecting: Option<Receiver<std::result::Result<LegacyWireWorker, String>>>,
    worker: Option<LegacyWireWorker>,
    state: LegacyGame,
    renderer: LegacyRenderSync,
    motion: ClassicPlayer,
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
        }
    }
}
impl Game<'_> {
    /// Developer-only switch. A plain Android version selector remains on
    /// the existing public implementation until native 1.8.9 is actually
    /// playable and benchmarked.
    pub(super) fn native_189_requested()->bool {
        std::env::var("HORIZON_NATIVE_189_DEV").ok().as_deref()==Some("1")
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
                if packet.kind==UpdateKind::Joined {
                    self.reset_legacy_world();
                    self.player=None;
                    if let Some(j)=&legacy.state.join {
                        self.dim=match j.dimension {
                            -1=>Dim::Nether,1=>Dim::End,_=>Dim::Overworld,
                        };
                        self.set_mode_id(j.game_mode as i32);
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
    pub(super) fn tick_native_189(&mut self)->Result<()> {
        self.ticks=self.ticks.saturating_add(1);
        let Some(legacy)=self.legacy_remote.as_mut() else{return Ok(())};
        if self.player.is_none(){return Ok(())}
        let Some(worker)=legacy.worker.as_ref() else{return Ok(())};
        let p=self.player.as_ref().expect("checked");
        worker.send_action(ClientAction::PositionLook {
            x:p.position.x,y:p.position.y,z:p.position.z,
            yaw:self.yaw,pitch:self.pitch,
            on_ground:p.on_ground,
        })?;
        Ok(())
    }
}
