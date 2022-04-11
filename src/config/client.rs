use std::fs::File;

use anyhow::Result;
use serde::Deserialize;

use crate::message::Peer;

#[derive(Deserialize)]
pub struct Config {
    pub server_address: String,
    pub listen: bool,
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
    pub peer_id: String,
    pub persistent_keepalive: u8,
}

impl NetWork {
    pub fn me(&self) -> Peer {
        Peer {
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
