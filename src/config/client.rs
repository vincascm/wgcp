use std::fs::File;

use anyhow::Result;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub server_address: String,
    pub listen: bool,
    /// WireGuard interface
    pub interface: String,
    pub network: String,
    pub id: String,
    pub peer_id: String,
    pub persistent_keepalive: u8,
}

impl Config {
    pub fn from_env() -> Result<Config> {
        let file = std::env::var("CONFIG_FILE")?;
        let file = File::open(&file)?;
        let config = serde_yaml::from_reader(file)?;
        Ok(config)
    }
}
