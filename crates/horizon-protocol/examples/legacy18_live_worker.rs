//! Live, headless Android-equivalent protocol-47 pipeline smoke test.
//! This runs the same bounded async TCP workers and state event pump as the
//! experimental Android Game. It is NOT a graphical or playable release.
//! Intended for an official unmodified vanilla 1.8.9 test server, offline mode.
use std::{thread,time::{Duration,Instant}};
use horizon_protocol::legacy_v1_8::{
    async_session::{LegacyInbound,LegacyWireWorker},
    session::ClientAction,
    state::{LegacyGame,UpdateKind},
};

fn main()->Result<(),Box<dyn std::error::Error>>{
    let mut args=std::env::args().skip(1);
    let address=args.next().ok_or("usage: legacy18_live_worker <host:port> <username>")?;
    let name=args.next().ok_or("missing username")?;
    let (client,uuid)=LegacyWireWorker::connect(
        &address,&name,Duration::from_secs(15),None
    )?;
    eprintln!("Native Android-equivalent protocol 47 connected, UUID {uuid}");
    let mut game=LegacyGame::default();
    let mut seen_join=false;
    let mut seen_correction=false;
    let mut chunks=0usize;
    let mut packets=0u64;
    let deadline=Instant::now()+Duration::from_secs(25);
    let mut next_movement=Instant::now();
    let mut finished_at=None;
    loop {
        for event in client.poll(){
            match event {
                LegacyInbound::Disconnected{reason}=>{
                    return Err(format!("1.8 async TCP worker disconnected: {reason}").into());
                }
                LegacyInbound::Packet{id,payload}=>{
                    let packet=client.apply(
                        LegacyInbound::Packet{id,payload},&mut game
                    )?;
                    packets+=1;
                    match packet.kind{
                        UpdateKind::Joined=>seen_join=true,
                        UpdateKind::PlayerCorrected=>seen_correction=true,
                        UpdateKind::Chunk|UpdateKind::ChunkBulk=>{
                            chunks=game.columns.len();
                        }
                        _=>{}
                    }
                }
            }
        }
        let now=Instant::now();
        if seen_correction&&now>=next_movement{
            let pos=game.player;
            client.send_action(ClientAction::PositionLook{
                x:pos.x,y:pos.y,z:pos.z,
                yaw:pos.yaw,pitch:pos.pitch,on_ground:game.on_ground
            })?;
            next_movement=now+Duration::from_millis(100);
        }
        if seen_join&&seen_correction&&chunks>0{
            // Stay alive across multiple frames, not just the first chunk.
            let first=finished_at.get_or_insert(now);
            if now.duration_since(*first)>Duration::from_millis(500){break;}
        }
        if now>=deadline{
            return Err(format!(
                "1.8 async world timeout: joined={seen_join},position={seen_correction},chunks={chunks},packets={packets}"
            ).into());
        }
        thread::sleep(Duration::from_millis(8));
    }
    println!(
        "NATIVE_189_WIRE_OK: joined={seen_join} corrected={seen_correction} chunks={chunks} packets={packets}"
    );
    Ok(())
}
