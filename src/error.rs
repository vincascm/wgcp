use thiserror::Error;

use crate::message::Message;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Error, Debug)]
pub enum Error {
    #[error("{0}")]
    Common(String),

    #[error(transparent)]
    Bincode(#[from] bincode::Error),
    #[error(transparent)]
    SerdeYaml(#[from] serde_yaml::Error),
    #[error(transparent)]
    FuturesChannelTrySend(#[from] futures_channel::mpsc::TrySendError<Message>),
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),
    #[error(transparent)]
    StdEnvVar(#[from] std::env::VarError),
    #[error(transparent)]
    StdAddrParse(#[from] std::net::AddrParseError),
    #[error(transparent)]
    StdIo(#[from] std::io::Error),
    #[error(transparent)]
    Tungstenite(#[from] tokio_tungstenite::tungstenite::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
}

impl Error {
    pub fn new(s: &str) -> Self {
        Error::Common(s.to_string())
    }
}
