use serde::{Deserialize, Serialize};
use ulid::{serde::ulid_as_u128, Ulid};

use super::{Message, Peer};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Ping,
    Connect(Peer),
    PunchTo {
        from: Peer,
        to: Peer,
    },
    ConnectBroker {
        #[serde(with = "ulid_as_u128")]
        task_id: Ulid,
        peer: Peer,
    },
}

impl Request {
    pub fn into_message(self) -> Message {
        Message::Request(self)
    }
}
