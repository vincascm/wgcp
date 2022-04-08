use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use super::{Message, Peer};

#[derive(Serialize, Deserialize, Debug)]
pub enum Response {
    Pong,
    Connected,
    Broker(SocketAddr),
    Wait,
    Addr { peer: Peer, addr: SocketAddr },
    Complete,
}

impl Response {
    pub fn into_message(self) -> Message {
        Message::Response(self)
    }
}
