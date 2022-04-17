use std::fs::File;

use serde::Deserialize;

use crate::{error::Result, message::Peer as MessagePeer};

#[derive(Deserialize)]
pub struct Config {
    pub server_address: String,
    pub listen: bool,
    pub token: String,
    pub keepalive: Option<u64>,
    pub network: Vec<NetWork>,
}

#[derive(Deserialize)]
pub struct NetWork {
    pub network: String,
    #[serde(default)]
    pub skip: bool,
    /// WireGuard interface
    pub interface: String,
    pub id: String,
    pub peers: Vec<Peer>,
}

#[derive(Deserialize)]
pub struct Peer {
    #[serde(default)]
    pub skip: bool,
    pub id: String,
    pub persistent_keepalive: Option<u8>,
}

impl NetWork {
    pub fn me(&self) -> MessagePeer {
        MessagePeer {
            network: self.network.clone(),
            id: self.id.clone(),
        }
    }
}

impl Config {
    pub fn from_env() -> Result<Config> {
        let file = std::env::var("CONFIG_FILE")?;
        let file = File::open(&file)?;
        let config = serde_yaml::from_reader(file)?;
        Ok(config)
    }
}
