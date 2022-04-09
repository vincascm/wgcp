use std::net::SocketAddr;

use anyhow::{anyhow, Result};
use log::info;
use tokio::process::Command;

use crate::message::Peer;

async fn get_endpoint(peer: Peer) -> Result<Option<String>> {
      let output = Command::new("wg")
        .arg("show")
        .arg("wg0")
        .arg("endpoints")
        .output().await?;
      if output.status.success() {
          let output = String::from_utf8_lossy(&output.stdout);
          for i in output.split('\n') {
              let row: Vec<_> = i.split('\t').collect();
              if row.len() != 2 {
                  continue;
              }
              if row[0] == peer.id {
                  return Ok(Some(row[1].to_string()))
              }
          }
          Ok(None)
      } else {
          Err(anyhow!("execute wg command failure"))
      }
}

pub async fn set_endpoint(peer: Peer, addr: SocketAddr, persistent_keepalive: u8) -> Result<()> {
    info!(
        "set_endpoint peer {peer:?} address {addr} , persistent-keepalive {persistent_keepalive}"
    );
    let status = Command::new("wg")
        .arg("set")
        .arg("wg0")
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
