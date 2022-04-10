use std::{net::SocketAddr, process::Command};

use anyhow::{anyhow, Result};

use crate::message::Peer;

pub fn get_listen_port(peer: Peer) -> Result<u16> {
    let output = Command::new("wg")
        .arg("show")
        .arg("wg0")
        .arg("listen-port")
        .output()?;
    if output.status.success() {
        let output = String::from_utf8_lossy(&output.stdout);
        Ok(output.trim().parse()?)
    } else {
        Err(anyhow!("execute wg command failure"))
    }
}

pub fn set(peer: Peer, addr: SocketAddr, persistent_keepalive: u8) -> Result<()> {
    let status = Command::new("wg")
        .arg("set")
        .arg("wg0")
        .arg("peer")
        .arg(peer.id)
        .arg("endpoint")
        .arg(addr.to_string())
        .arg("persistent-keepalive")
        .arg(persistent_keepalive.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!("execute wg command failure"))
    }
}
