//! Headless protocol-47 state-sync diagnostic. NOT a playable/rendered client.
//! Logs into an offline 1.8.9 test server, handles join, keepalive, server
//! corrections, blocks, entities, inventory, chat and plugin channels.
//! Online-mode servers need a proper Microsoft session-auth callback.
use std::time::Duration;
use horizon_protocol::legacy_v1_8::{
    connect_login,
    play,
    plugin,
    state::{LegacyGame,UpdateKind},
};

fn main()->Result<(),Box<dyn std::error::Error>> {
    let mut args=std::env::args().skip(1);
    let address=args.next().ok_or(
        "usage: cargo run -p horizon-protocol --example legacy18_probe -- <host:port> <username>"
    )?;
    let username=args.next().ok_or("missing username")?;
    let expect_world=matches!(args.next().as_deref(),Some("--expect-world"));
    let (mut conn,uuid)=connect_login(&address,&username,Duration::from_secs(10),None)?;
    conn.reader.set_read_timeout(Some(Duration::from_secs(40)))?;
    eprintln!("Connected via protocol 47: {username} UUID {uuid}");
    eprintln!("HEADLESS diagnostic only: gameplay renderer, native 1.8 server and Forge mods are absent.");
    // Vanilla 1.8 sends ClientSettings/Brand only *after* JoinGame (S01).
    let mut game=LegacyGame::default();
    let mut seen=[0u32;14];
    for _ in 0..512 {
        let (id,payload)=conn.recv().map_err(|error| {
            eprintln!("1.8 stream ended: events={seen:?}, columns={}, entities={}",
                game.columns.len(),game.entities.len());
            error
        })?;
        if seen.iter().sum::<u32>() < 32 {
            eprintln!("1.8 clientbound packet: 0x{id:02x}, bytes={}",payload.len());
        }
        let output=game.apply(id,&payload)?;
        for reply in output.replies {
            conn.send(&reply)?;
        }
        let kind=output.kind;
        let index=match kind {
            UpdateKind::Joined=>0,
            UpdateKind::PlayerCorrected=>1,
            UpdateKind::Chunk=>2,
            UpdateKind::ChunkBulk=>3,
            UpdateKind::Block=>4,
            UpdateKind::Entity=>5,
            UpdateKind::Inventory=>6,
            UpdateKind::Chat=>7,
            UpdateKind::Plugin=>8,
            UpdateKind::KeepAlive=>9,
            UpdateKind::Ignored=>10,
            UpdateKind::Respawn=>11,
            UpdateKind::Health=>12,
            UpdateKind::Hotbar=>13,
        };
        seen[index]+=1;
        match kind {
            UpdateKind::Joined=>{
                conn.send(&play::client_settings_body("en_US",8,0x7f)?)?;
                conn.send(&plugin::client_brand_body("horizon-rust")?)?;
                let join=game.join.as_ref().expect("join processed");
                println!("Joined: entity {}, dimension {}, gamemode {}",
                    join.entity_id,join.dimension,join.game_mode);
            },
            UpdateKind::PlayerCorrected=>println!(
                "Position confirmed: {:.2} {:.2} {:.2}",
                game.player.x,game.player.y,game.player.z,
            ),
            UpdateKind::Plugin if game.forge_required=>{
                eprintln!("FML|HS detected. Forge Java client mods are NOT supported by this test client.");
            },
            _=>{},
        }
        if expect_world && seen[0]>0 && seen[1]>0 && !game.columns.is_empty() {
            break;
        }
    }
    println!("Diagnostics: chunks={} entities={} windows={} packets={seen:?}",
        game.columns.len(),game.entities.len(),game.windows.len());
    if expect_world && (seen[0]==0 || seen[1]==0 || game.columns.is_empty()) {
        return Err(format!(
            "real Minecraft 1.8.9 world check failed: join={}, corrections={}, columns={}",
            seen[0],seen[1],game.columns.len()
        ).into());
    }
    Ok(())
}
