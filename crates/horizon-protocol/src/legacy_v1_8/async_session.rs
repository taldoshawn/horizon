//! Non-blocking protocol-47 networking for a native Android game thread.
//!
//! Receive and send are independent workers. The game's render thread drains
//! a *bounded* packet channel each frame; keepalives are acknowledged on the
//! reader thread, without waiting for GPU frames. No Java or ViaProxy involved.
//!
//! This is transport infrastructure, not a finished Minecraft 1.8.9 client.
//! Callers must process all PLAY packets, renderer, physics and UI themselves.
use std::{
    sync::mpsc::{self, Receiver, SyncSender, TryRecvError},
    thread,
    time::Duration,
};

use crate::{Error,Result,connection::{Closer,Connection}};
use super::{
    connect_login,keep_alive_id,keep_alive_response_body,
    play,plugin,
    session::{ClientAction,ReceivedLegacy},
    state::LegacyGame,
};

const QUEUED_INBOUND:usize=64;
const QUEUED_OUTBOUND:usize=128;
const MAX_EVENTS_PER_POLL:usize=64;

#[derive(Debug)]
pub enum LegacyInbound {
    /// One packet from the 1.8 play state, preserving old wire IDs.
    Packet {id:i32,payload:Vec<u8>},
    Disconnected {reason:String},
}

/// Nonblocking gameplay transport. The UI *must not* call a blocking TCP
/// receive; use poll() in the frame loop, then apply() on the game thread.
/// Separate read/write workers avoid stalling touch input when TCP is idle.
pub struct LegacyWireWorker {
    commands:SyncSender<Vec<u8>>,
    inbound:Receiver<LegacyInbound>,
    close:Closer,
}
impl LegacyWireWorker {
    /// Login with 1.8 protocol 47, including optional official Microsoft
    /// session join callback for online-mode servers.
    pub fn connect(
        address:&str,name:&str,timeout:Duration,
        authenticate:Option<&mut dyn FnMut(&str)->Result<()>>,
    )->Result<(Self,String)> {
        let (connection,uuid)=connect_login(address,name,timeout,authenticate)?;
        Ok((Self::after_login(connection)?,uuid))
    }
    /// For tested or already authenticated protocol-47 connections.
    pub fn after_login(connection:Connection)->Result<Self>{
        // Vanilla client settings/brand are sent only after S01 JoinGame.
        // Sending PLAY packets during the login→play transition can cause
        // an unmodified 1.8.9 server to close the connection.
        let settings=play::client_settings_body("en_US",8,0x7f)?;
        let brand=plugin::client_brand_body("horizon-rust")?;
        let (mut reader,mut writer)=connection.split();
        reader.set_read_timeout(Some(Duration::from_secs(30)))?;
        let close=reader.closer()?;
        let (commands_tx,commands_rx)=mpsc::sync_channel::<Vec<u8>>(QUEUED_OUTBOUND);
        let (events_tx,events_rx)=mpsc::sync_channel::<LegacyInbound>(QUEUED_INBOUND);
        let writer_events=events_tx.clone();

        thread::Builder::new().name("legacy18-tx".into()).spawn(move || {
            while let Ok(packet)=commands_rx.recv(){
                if let Err(error)=writer.send(&packet){
                    let _=writer_events.try_send(LegacyInbound::Disconnected {
                        reason:format!("1.8 send failed: {error}")
                    });
                    break;
                }
            }
            writer.shutdown();
        }).map_err(|e|Error::new(format!("spawn 1.8 writer: {e}")))?;

        let rx_commands=commands_tx.clone();
        thread::Builder::new().name("legacy18-rx".into()).spawn(move || {
            loop {
                let (id,payload)=match reader.recv(){
                    Ok(packet)=>packet,
                    Err(error)=>{
                        let _=events_tx.try_send(LegacyInbound::Disconnected {
                            reason:format!("1.8 connection ended: {error}")
                        });
                        break;
                    }
                };
                if id==0x01 {
                    // JoinGame is the point at which vanilla sends these.
                    if rx_commands.send(settings.clone()).is_err(){break;}
                    if rx_commands.send(brand.clone()).is_err(){break;}
                }
                // Keepalives must not wait for the renderer or Android GC.
                match keep_alive_id(id,&payload){
                    Ok(Some(value))=>{
                        if rx_commands.send(keep_alive_response_body(value)).is_err(){break;}
                    },
                    Ok(None)=>{},
                    Err(error)=>{
                        let _=events_tx.try_send(LegacyInbound::Disconnected {
                            reason:format!("invalid 1.8 keepalive: {error}")
                        });
                        break;
                    }
                }
                // Bounded: when the device cannot keep up, apply TCP
                // backpressure instead of growing memory without limit.
                if events_tx.send(LegacyInbound::Packet{id,payload}).is_err(){break;}
                if id==0x40 {break;}
            }
        }).map_err(|e|Error::new(format!("spawn 1.8 reader: {e}")))?;

        Ok(Self{commands:commands_tx,inbound:events_rx,close})
    }

    /// Try to queue one user action without blocking the Android UI thread.
    pub fn send_action(&self,action:ClientAction<'_>)->Result<()> {
        self.send_body(action.encode()?)
    }
    pub fn send_body(&self,body:Vec<u8>)->Result<()> {
        self.commands.try_send(body).map_err(|e|
            Error::new(format!("1.8 outgoing packet queue full or closed: {e}")))
    }
    /// Bounded frame drain. Raw packets remain in protocol 47 format.
    pub fn poll(&self)->Vec<LegacyInbound>{
        let mut out=Vec::new();
        for _ in 0..MAX_EVENTS_PER_POLL {
            match self.inbound.try_recv(){
                Ok(packet)=>out.push(packet),
                Err(TryRecvError::Empty|TryRecvError::Disconnected)=>break,
            }
        }
        out
    }

    /// Apply a packet to the native 1.8 state on the game thread, sending
    /// necessary teleport / inventory transaction acknowledgments back to
    /// the write worker. Keepalive was already sent by reader worker.
    /// Returned packet is usable directly by \`LegacyRenderSync\`.
    pub fn apply(
        &self,
        packet:LegacyInbound,
        state:&mut LegacyGame,
    )->Result<ReceivedLegacy>{
        match packet {
            LegacyInbound::Disconnected{reason}=>Err(Error::new(reason)),
            LegacyInbound::Packet{id,payload}=>{
                if id==0x40 {
                    return Err(Error::new(format!("1.8 server disconnect: {}",
                        crate::buf::Reader::new(&payload).string(32767)?)));
                }
                let applied=state.apply(id,&payload)?;
                if id!=0x00 {
                    for response in applied.replies {
                        self.send_body(response)?;
                    }
                }
                Ok(ReceivedLegacy{id,payload,kind:applied.kind})
            }
        }
    }
}
impl Drop for LegacyWireWorker {
    fn drop(&mut self){
        // Read/write are detached: a full inbound queue must not deadlock
        // an Android close() waiting for the reader thread to join.
        self.close.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{buf::{Reader,Writer},connection::Connection};
    use std::{net::TcpListener,thread,time::Instant};

    #[test]
    fn keepalive_writes_without_a_render_frame_and_actions_are_nonblocking() {
        let listener=TcpListener::bind("127.0.0.1:0").unwrap();
        let addr=listener.local_addr().unwrap();
        let server=thread::spawn(move||{
            let (socket,_)=listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(4))).unwrap();
            let mut peer=Connection::from_stream(socket).unwrap();
            // Before JoinGame the client must not send settings/brand,
            // yet a keepalive still needs an immediate reply without poll().
            let mut keep=Writer::new();keep.varint(0);keep.varint(777);
            peer.send(&keep.data).unwrap();
            let (id,data)=peer.recv().unwrap();
            assert_eq!(id,0);
            assert_eq!(Reader::new(&data).varint().unwrap(),777);
            let mut join=Writer::new();
            join.varint(1);join.i32(5);join.u8(0);join.i8(0);
            join.u8(2);join.u8(20);join.string("default");join.bool(false);
            peer.send(&join.data).unwrap();
            let (settings,_)=peer.recv().unwrap();
            let (brand,_)=peer.recv().unwrap();
            assert_eq!((settings,brand),(0x15,0x17));
            let (id,attack)=peer.recv().unwrap();
            assert_eq!(id,2);
            let mut r=Reader::new(&attack);
            assert_eq!(r.varint().unwrap(),10);
            assert_eq!(r.varint().unwrap(),1);
        });
        let socket=std::net::TcpStream::connect(addr).unwrap();
        let connection=Connection::from_stream(socket).unwrap();
        let client=LegacyWireWorker::after_login(connection).unwrap();
        // Do not enqueue an attack before the server has received
        // its keepalive reply: response ordering is intentional.
        let mut state=LegacyGame::default();
        let deadline=Instant::now()+Duration::from_secs(3);
        while Instant::now()<deadline&&state.join.is_none(){
            for packet in client.poll(){
                if let LegacyInbound::Packet{id:1,..}=packet {
                    client.apply(packet,&mut state).unwrap();
                }
            }
            thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(state.join.as_ref().unwrap().entity_id,5);
        client.send_action(ClientAction::Attack(10)).unwrap();
        // Wait for the write worker to finish; closing the TCP connection
        // before the worker drains its queue would race with the assertion.
        server.join().unwrap();
        drop(client);
    }
}
