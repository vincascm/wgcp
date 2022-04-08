use serde::{Deserialize, Serialize};

use super::Message;

#[derive(Serialize, Deserialize, Debug)]
pub enum Error {
    UnSupportedMessage,
    NotFound,
    PeerOffline,
}

impl Error {
    pub fn into_message(self) -> Message {
        Message::Err(self)
    }
}
