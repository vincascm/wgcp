use std::fs::File;

use serde::Deserialize;

use crate::error::Result;

#[derive(Deserialize)]
pub struct Config {
    #[serde(default = "Config::default_listen_address")]
    pub listen_address: String,
    #[serde(default = "Config::default_broker")]
    pub broker: String,
    pub token: String,
}

impl Config {
    fn default_listen_address() -> String {
        "127.0.0.1:5465".to_string()
    }

    fn default_broker() -> String {
        "127.0.0.1:5466".to_string()
    }

    pub fn from_env() -> Result<Config> {
        let file = std::env::var("CONFIG_FILE")?;
        let file = File::open(&file)?;
        let config = serde_yaml::from_reader(file)?;
        Ok(config)
    }
}
