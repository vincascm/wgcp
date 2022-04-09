use std::net::SocketAddr;

use anyhow::{anyhow, Result};
use tokio::process::Command;

use crate::message::Peer;

pub async fn set(
    local_addr: SocketAddr,
    peer: Peer,
    addr: SocketAddr,
    persistent_keepalive: u8,
) -> Result<()> {
    let status = Command::new("wg")
        .arg("set")
        .arg("wg0")
        .arg("listen-port")
        .arg(local_addr.port().to_string())
        .arg("peer")
        .arg(peer.id)
        .arg("endpoint")
        .arg(addr.to_string())
        .arg("persistent-keepalive")
        .arg(persistent_keepalive.to_string())
        .status()
        .await?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("execute wg command failure"))
    }
}
