use serde::{Deserialize, Serialize};
use ulid::{serde::ulid_as_u128, Ulid};

use super::{Message, Peer};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Ping,
    Connect {
        peer: Peer,
        token: String,
    },
    PunchTo {
        from: Peer,
        to: Peer,
        token: String,
    },
    ConnectBroker {
        #[serde(with = "ulid_as_u128")]
        task_id: Ulid,
        peer: Peer,
        token: String,
    },
}

impl Request {
    pub fn into_message(self) -> Message {
        Message::Request(self)
    }
}
