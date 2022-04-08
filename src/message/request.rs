use serde::{Deserialize, Serialize};

use super::{Message, Peer};

#[derive(Serialize, Deserialize, Debug)]
pub enum Request {
    Ping,
    Connect(Peer),
    PunchTo { from: Peer, to: Peer },
}

impl Request {
    pub fn into_message(self) -> Message {
        Message::Request(self)
    }
}
