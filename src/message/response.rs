use std::net::SocketAddr;

use serde::{Deserialize, Serialize};
use ulid::{serde::ulid_as_u128, Ulid};

use super::{Message, Peer};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Response {
    Pong,
    Connected,
    Broker {
        #[serde(with = "ulid_as_u128")]
        task_id: Ulid,
        broker_addr: SocketAddr,
        to: Peer,
    },
    Wait,
    Addr {
        peer: Peer,
        addr: SocketAddr,
    },
    Complete {
        #[serde(with = "ulid_as_u128")]
        task_id: Ulid,
        peer: Peer,
    },
}

impl Response {
    pub fn into_message(self) -> Message {
        Message::Response(self)
    }
}
