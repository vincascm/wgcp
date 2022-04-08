use std::convert::{TryFrom, TryInto};

use anyhow::{Error, Result};
use futures_channel::mpsc::UnboundedSender;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::{request::Request, response::Response};

#[derive(Serialize, Deserialize, Debug)]
pub enum Message {
    Request(Request),
    Response(Response),
    Err(super::error::Error),
}

impl Message {
    pub fn send(self, tx: &UnboundedSender<Message>) -> Result<()> {
        tx.unbounded_send(self)?;
        Ok(())
    }

    pub fn de(v: &[u8]) -> Result<Self> {
        Ok(bincode::deserialize(v)?)
    }

    pub fn se(&self) -> Result<Vec<u8>> {
        Ok(bincode::serialize(self)?)
    }
}

impl TryFrom<WsMessage> for Message {
    type Error = Error;

    fn try_from(m: WsMessage) -> Result<Self, Self::Error> {
        Self::de(&m.into_data())
    }
}

impl TryInto<WsMessage> for Message {
    type Error = Error;

    fn try_into(self) -> Result<WsMessage, Self::Error> {
        Ok(WsMessage::binary(self.se()?))
    }
}

#[derive(PartialEq, Eq, Hash, Clone, Serialize, Deserialize, Debug)]
pub struct Peer {
    pub network: String,
    pub id: String,
}
