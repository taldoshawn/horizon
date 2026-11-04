//! Session API for Minecraft Java protocol 47 (1.8.0–1.8.9).
//!
//! Owns an authenticated connection, applies inbound packets to the *legacy*
//! world, acknowledges server corrections and encodes real client controls.
//! Intended to run in a networking worker, never on Android's UI thread.
//! Rendering, integrated singleplayer, modded Forge registries and many 1.8
//! packets remain unimplemented; an active session is not a playable client.
use std::time::Duration;

use crate::{Error, Result, connection::Connection, buf::Reader};
use super::{
    connect_login, inventory,
    physics::ClassicPlayer,
    play::{self, PositionLook, UseEntityAction},
    plugin,
    state::{LegacyGame, UpdateKind},
};

/// Commands issued by the native UI/touch input or simulation at 20 TPS.
#[derive(Debug, Clone, Copy)]
pub enum ClientAction<'a> {
    /// One idle tick when no movement or rotation needs to be sent.
    OnGround(bool),
    /// Absolute feet coordinates (not 26.3 player position / height).
    Position { x:f64, y:f64, z:f64, on_ground:bool },
    /// Euler rotation in degrees.
    Look { yaw:f32, pitch:f32, on_ground:bool },
    /// Complete movement + orientation (serverbound 0x06).
    PositionLook {
        x:f64, y:f64, z:f64, yaw:f32, pitch:f32, on_ground:bool,
    },
    Chat(&'a str),
    Attack(i32),
    Interact(i32),
    SwingArm,
    SelectHotbar(u8),
    Dig { status:u8, position:play::BlockPosition, face:u8 },
    BlockSword(Option<&'a inventory::ItemStack>),
    ReleaseSword,
    Click { click: inventory::Click, item:Option<&'a inventory::ItemStack> },
}
fn finite_position(x:f64,y:f64,z:f64,yaw:f32,pitch:f32)->Result<()> {
    if [x,y,z].into_iter().all(f64::is_finite)
        && [yaw,pitch].into_iter().all(f32::is_finite) {
        Ok(())
    } else {
        Err(Error::new("non-finite 1.8 movement or rotation"))
    }
}
impl ClientAction<'_> {
    /// Serialize the 1.8 wire *packet body* (including packet ID, excluding
    /// compression/encryption/frame); cannot reuse modern packet enum.
    pub fn encode(self)->Result<Vec<u8>> {
        match self {
            Self::OnGround(ground)=>Ok(play::player_ground_body(ground)),
            Self::Position{x,y,z,on_ground}=>play::player_position_body(x,y,z,on_ground),
            Self::Look{yaw,pitch,on_ground}=>play::player_look_body(yaw,pitch,on_ground),
            Self::PositionLook{x,y,z,yaw,pitch,on_ground}=>{
                finite_position(x,y,z,yaw,pitch)?;
                Ok(play::position_look_response_body(PositionLook{
                    x,y,z,yaw,pitch,flags:0
                },on_ground))
            },
            Self::Chat(line)=>play::chat_message_body(line),
            Self::Attack(id)=>play::use_entity_body(id,UseEntityAction::Attack),
            Self::Interact(id)=>play::use_entity_body(id,UseEntityAction::Interact),
            Self::SwingArm=>Ok(play::swing_arm_body()),
            Self::SelectHotbar(slot)=>inventory::held_item_change_body(slot),
            Self::Dig{status,position,face}=>play::digging_body(status,position,face),
            Self::BlockSword(item)=>play::sword_block_body(item),
            Self::ReleaseSword=>Ok(play::release_use_item_body()),
            Self::Click{click,item}=>inventory::click_window_body(click,item),
        }
    }
}


/// A gameplay packet after the 1.8 state model has processed it.
/// Retaining the exact 1.8 wire bytes allows a renderer adapter to update
/// the right chunks without re-decoding modern (26.3) packet formats.
#[derive(Debug)]
pub struct ReceivedLegacy {
    pub id:i32,
    pub payload:Vec<u8>,
    pub kind:UpdateKind,
}

/// A live protocol-47 session, intended for use by a networking worker.
///
/// Do not move its connection onto a UI/render thread: recv() blocks.
pub struct LegacySession {
    connection:Connection,
    pub game:LegacyGame,
    pub local_player:ClassicPlayer,
    pub username:String,
    pub uuid:String,
    pub received_packets:u64,
    pub sent_packets:u64,
}
impl LegacySession {
    /// Negotiate protocol 47 (with compression/encryption as requested).
    /// Authenticate using the caller's official Minecraft session callback.
    /// None means the server must have offline authentication.
    pub fn connect(
        address:&str,
        username:&str,
        timeout:Duration,
        authenticate:Option<&mut dyn FnMut(&str)->Result<()>>,
    )->Result<Self>{
        let (connection,uuid)=connect_login(address,username,timeout,authenticate)?;
        Self::from_logged_in_connection(connection,username.to_owned(),uuid)
    }

    /// For a connection that has *already* completed protocol-47 login,
    /// including in-memory socket integration tests.
    pub fn from_logged_in_connection(
        connection:Connection,
        username:String,
        uuid:String,
    )->Result<Self>{
        let mut session=Self {
            connection,
            game:LegacyGame::default(),
            local_player:ClassicPlayer::new(0.0,64.0,0.0),
            username,uuid,
            received_packets:0,sent_packets:0,
        };
        session.send_body(&play::client_settings_body("en_US",8,0x7f)?)?;
        session.send_body(&plugin::client_brand_body("horizon-rust")?)?;
        Ok(session)
    }
    fn send_body(&mut self,body:&[u8])->Result<()> {
        self.connection.send(body)?;
        self.sent_packets=self.sent_packets.saturating_add(1);
        Ok(())
    }
    pub fn send_action(&mut self,action:ClientAction<'_>)->Result<()> {
        let body=action.encode()?;
        self.send_body(&body)?;
        match action {
            ClientAction::Attack(_) => self.local_player.sprinting=false,
            ClientAction::BlockSword(_) => self.local_player.using_sword=true,
            ClientAction::ReleaseSword => self.local_player.using_sword=false,
            _ => {}
        }
        Ok(())
    }
    /// Read one packet, apply it to the authoritative world, and preserve
    /// its original protocol-47 bytes for the renderer/game loop. The
    /// caller owns the Vec and must bound its render/update queue on Android.
    pub fn receive_one_packet(&mut self)->Result<ReceivedLegacy>{
        let (id,payload)=self.connection.recv()?;
        self.received_packets=self.received_packets.saturating_add(1);
        if id==0x40 {
            let reason=Reader::new(&payload).string(32767)?;
            return Err(Error::new(format!("Minecraft 1.8 server disconnected: {reason}")));
        }
        let update=self.game.apply(id,&payload)?;
        for response in update.replies {self.send_body(&response)?;}
        if update.kind==UpdateKind::PlayerCorrected {
            let p=self.game.player;
            self.local_player.apply_server_position(
                p.x,p.y,p.z,f64::from(p.yaw),f64::from(p.pitch)
            )?;
        }
        if id==0x12 {
            let velocity=play::decode_entity_velocity(&payload)?;
            if self.game.join.as_ref().is_some_and(|j|j.entity_id==velocity.entity_id) {
                self.local_player.apply_server_velocity(
                    velocity.x,velocity.y,velocity.z
                )?;
            }
        }
        Ok(ReceivedLegacy { id, payload, kind:update.kind })
    }
    /// Convenience API when a diagnostic needs only the event type.
    /// The real native renderer must use receive_one_packet(), since its
    /// block/lighting adapter needs the exact original packet payload.
    pub fn receive_one(&mut self)->Result<UpdateKind>{
        Ok(self.receive_one_packet()?.kind)
    }
    /// Set the read timeout from the network worker to allow worker shutdown.
    pub fn set_read_timeout(&mut self,timeout:Duration)->Result<()> {
        self.connection.reader.set_read_timeout(Some(timeout))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buf::{Reader,Writer},connection::Connection};
    use std::{net::TcpListener,thread};

    #[test]
    fn client_actions_use_the_old_wire_packet_ids() {
        assert_eq!(ClientAction::OnGround(true).encode().unwrap(),[3,1]);
        assert_eq!(ClientAction::SwingArm.encode().unwrap(),[10]);
        assert_eq!(ClientAction::Attack(7).encode().unwrap(),[2,7,1]);
        assert_eq!(ClientAction::SelectHotbar(8).encode().unwrap(),[9,0,8]);
        assert!(ClientAction::SelectHotbar(9).encode().is_err());
        assert!(ClientAction::PositionLook{
            x:1.0,y:f64::NAN,z:0.0,yaw:0.0,pitch:0.0,on_ground:false
        }.encode().is_err());
        let bytes=ClientAction::PositionLook{
            x:1.0,y:64.0,z:-2.0,yaw:90.0,pitch:0.0,on_ground:true
        }.encode().unwrap();
        assert_eq!(bytes[0],6);
        assert_eq!(bytes.len(),34);
        let mut r=Reader::new(&bytes[1..]);
        assert_eq!(r.f64().unwrap(),1.0);
        assert_eq!(r.f64().unwrap(),64.0);
        assert_eq!(r.f64().unwrap(),-2.0);
        assert_eq!(r.f32().unwrap(),90.0);
        assert_eq!(r.f32().unwrap(),0.0);
        assert!(r.bool().unwrap());
    }

    #[test]
    fn server_disconnect_reason_is_reported() {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let address=listener.local_addr().unwrap();
        let server=thread::spawn(move||{
            let (socket,_)=listener.accept().unwrap();
            let mut connection=Connection::from_stream(socket).unwrap();
            connection.recv().unwrap(); // 1.8 client settings
            connection.recv().unwrap(); // MC|Brand
            let mut disconnect=Writer::new();
            disconnect.varint(0x40);
            disconnect.string(r#"{"text":"Server closed"}"#);
            connection.send(&disconnect.data).unwrap();
        });
        let socket=std::net::TcpStream::connect(address).unwrap();
        let connection=Connection::from_stream(socket).unwrap();
        let mut client=LegacySession::from_logged_in_connection(
            connection,"Steve".into(),"test-uuid".into()
        ).unwrap();
        let error=client.receive_one().unwrap_err();
        assert!(error.to_string().contains("Server closed"));
        server.join().unwrap();
    }

    #[test]
    fn network_worker_acks_server_and_sends_touch_actions() {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let address=listener.local_addr().unwrap();
        let server=thread::spawn(move|| {
            let (socket,_)=listener.accept().unwrap();
            let mut c=Connection::from_stream(socket).unwrap();
            // Initial settings and brand are sent by LegacySession itself.
            let (settings,_) = c.recv().unwrap();
            let (brand,_) = c.recv().unwrap();
            assert_eq!((settings,brand),(0x15,0x17));
            let mut join=Writer::new();
            join.i32(55);join.u8(0);join.i8(0);join.u8(2);
            join.u8(20);join.string("default");join.bool(false);
            let mut packet=Writer::new();packet.varint(1);packet.bytes(&join.data);
            c.send(&packet.data).unwrap();

            let mut position=Writer::new();
            position.varint(8);
            position.f64(10.0);position.f64(66.0);position.f64(-2.0);
            position.f32(45.0);position.f32(0.0);position.u8(0);
            c.send(&position.data).unwrap();
            let (id,payload)=c.recv().unwrap();
            assert_eq!(id,6);
            assert_eq!(Reader::new(&payload).f64().unwrap(),10.0);

            let mut keep=Writer::new();keep.varint(0);keep.varint(315);
            c.send(&keep.data).unwrap();
            let (id,payload)=c.recv().unwrap();
            assert_eq!(id,0);
            assert_eq!(Reader::new(&payload).varint().unwrap(),315);

            let (id,payload)=c.recv().unwrap();
            assert_eq!(id,2);
            let mut r=Reader::new(&payload);
            assert_eq!(r.varint().unwrap(),7);
            assert_eq!(r.varint().unwrap(),1);
        });
        let socket=std::net::TcpStream::connect(address).unwrap();
        let connection=Connection::from_stream(socket).unwrap();
        let mut client=LegacySession::from_logged_in_connection(
            connection,"Steve".into(),"test-uuid".into()
        ).unwrap();
        assert_eq!(client.receive_one().unwrap(),UpdateKind::Joined);
        assert_eq!(client.game.join.as_ref().unwrap().entity_id,55);
        assert_eq!(client.receive_one().unwrap(),UpdateKind::PlayerCorrected);
        assert_eq!(client.local_player.feet.y,66.0);
        assert_eq!(client.receive_one().unwrap(),UpdateKind::KeepAlive);
        client.send_action(ClientAction::Attack(7)).unwrap();
        assert_eq!(client.sent_packets,5);
        server.join().unwrap();
    }
}
