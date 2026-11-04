//! Protocol 47 diagnostic (not a gameplay client): joins a 1.8 offline-mode
//! server, acknowledges 1.8 movement corrections and validates chunk framing.
//! Does not render the world, load entities, or authenticate premium servers.
use std::time::Duration;
use horizon_protocol::legacy_v1_8::{
    connect_login, keep_alive_id, keep_alive_response_body,
    play::{self, PositionLook},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let address = args.next().ok_or("usage: cargo run -p horizon-protocol --example legacy18_probe -- <server:port> <username>")?;
    let username = args.next().ok_or("missing username")?;
    let (mut conn, uuid) = connect_login(&address, &username, Duration::from_secs(10), None)?;
    println!("Connected via protocol 47: {username} UUID {uuid}");
    println!("Diagnostic only: rendering and playable 1.8 translation are not implemented.");
    conn.send(&play::client_settings_body("en_US", 8, 0x7f)?)?;
    let mut has_sky_light = true;
    let mut current_position = PositionLook { x: 0.0, y: 64.0, z: 0.0,
        yaw: 0.0, pitch: 0.0, flags: 0 };
    let mut received_chunks = 0;
    for _ in 0..128 {
        let (id, payload) = conn.recv()?;
        if let Some(value) = keep_alive_id(id, &payload)? {
            conn.send(&keep_alive_response_body(value))?;
            continue;
        }
        match id {
            0x01 => {
                let join = play::decode_join_game(&payload)?;
                has_sky_light = join.dimension == 0;
                println!("Joined entity {} dimension {} gamemode {}", join.entity_id, join.dimension, join.game_mode);
            }
            0x08 => {
                current_position = play::decode_position_look(&payload)?.absolute(current_position);
                conn.send(&play::position_look_response_body(current_position, false))?;
                println!("Initial position ({:.1}, {:.1}, {:.1})", current_position.x, current_position.y, current_position.z);
            }
            0x21 => {
                let chunk = play::decode_chunk_data(&payload, has_sky_light)?;
                received_chunks += 1;
                println!("Chunk ({}, {}) sections {}", chunk.x, chunk.z, chunk.sections.len());
            }
            0x26 => {
                let chunks = play::decode_chunk_bulk(&payload)?;
                received_chunks += chunks.len();
                println!("Chunk bulk: {} columns", chunks.len());
            }
            _ => println!("Untranslated PLAY ID {id:#04x} ({} bytes)", payload.len()),
        }
    }
    println!("Finished diagnostic: parsed {received_chunks} chunk columns");
    Ok(())
}
