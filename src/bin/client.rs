use std::{
    mem::transmute,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::{bail, Result};
use bytes::BufMut;
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use log::{error, warn};
use tokio::{net::TcpStream, select, spawn, time::sleep};
use tokio_tungstenite::{
    connect_async, tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};
use url::Url;

use wgcp::{
    config::client::CONFIG,
    message::{request::Request, response::Response, Message, Peer},
    socket_filter::broker_response_filter,
    wg,
};

fn me() -> Peer {
    Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.id.to_string(),
    }
}

struct Udp {
    src: u16,
    dest: u16,
}

impl Udp {
    fn new(src: u16, dest: u16) -> Self {
        Udp { src, dest }
    }

    fn packet(&self, v: &[u8]) -> Vec<u8> {
        let len = 8 + v.len();
        let mut b = Vec::new();
        b.put_u16(self.src);
        b.put_u16(self.dest);
        b.put_u16(len as u16);
        b.put_u16(0);
        b.put(v);
        b
    }
}

/// return local addr, peer id, peer addr
fn get_peer_with_broker(broker: SocketAddr) -> Result<(Peer, SocketAddr)> {
    use socket2::{Domain, Protocol, Socket, Type};

    let broker_ip: Ipv4Addr = match broker.ip() {
        IpAddr::V4(v) => v,
        IpAddr::V6(_) => bail!("invalid broker ip, must be ipv4"),
    };
    let broker_port = broker.port();
    let wg_port = wg::get_listen_port(me())?;

    let sock = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::UDP))?;
    sock.set_reuse_port(true)?;
    sock.set_reuse_address(true)?;
    sock.attach_filter(&broker_response_filter(broker_ip, broker_port, wg_port))?;

    let udp = Udp::new(wg_port, broker_port);

    let req = Request::Connect(me()).into_message().se()?;
    sock.send_to(&udp.packet(&req), &broker.into())?;
    let mut buf = vec![0u8; 1024];
    loop {
        let (_, sock_addr) = sock.recv_from(unsafe { transmute(buf.as_mut_slice()) })?;
        // skip ip header and udp header, total 28 bytes
        let msg = Message::de(&buf[28..])?;
        dbg!(&msg);
        match msg {
            Message::Request(_) => (),
            Message::Response(response) => match response {
                Response::Addr { peer, addr } => {
                    let resp = Response::Complete.into_message();
                    sock.send_to(&udp.packet(&resp.se()?), &sock_addr)?;
                    return Ok((peer, addr));
                }
                _ => (),
            },
            Message::Close => (),
            Message::Err(e) => error!("broker error: {e:?}"),
        }
    }
}

/// return true: success
async fn handle_message(
    msg: WsMessage,
    tx: UnboundedSender<Message>,
    wg_tx: UnboundedSender<()>,
) -> Result<()> {
    let msg = msg.try_into()?;
    dbg!(&msg);
    match msg {
        Message::Request(request) => match request {
            Request::Ping => Response::Pong.into_message().send(&tx)?,
            _ => (),
        },
        Message::Response(response) => match response {
            Response::Broker(broker) => {
                tokio::task::spawn_blocking(move || {
                    match get_peer_with_broker(broker) {
                        Ok((peer, addr)) => {
                            if let Err(e) = wg::set(peer, addr, CONFIG.persistent_keepalive) {
                                error!("wg set error: {e}");
                            }
                        }
                        Err(e) => error!("get_peer_with_broker error: {e}"),
                    }
                    wg_tx.unbounded_send(()).ok();
                });
            }
            _ => (),
        },
        Message::Close => (),
        Message::Err(e) => error!("ws server error: {e:?}"),
    }
    Ok(())
}

async fn send_ws_message(
    mut ws_write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>,
    mut rx: UnboundedReceiver<Message>,
) -> Result<()> {
    while let Some(msg) = rx.next().await {
        let msg = match msg {
            Message::Close => WsMessage::Close(None),
            x => x.try_into()?,
        };
        ws_write.send(msg).await?;
    }
    Ok(())
}

fn punch_to(tx: &UnboundedSender<Message>) -> Result<()> {
    let from = me();
    let to = Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.peer_id.to_string(),
    };
    Request::PunchTo { from, to }.into_message().send(tx)?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::init();

    if std::env::var("CONFIG_FILE").is_err() {
        let filename = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "config.yaml".to_owned());
        std::env::set_var("CONFIG_FILE", filename);
    }

    if CONFIG.persistent_keepalive > 25 {
        warn!("persistent-keepalive is large than 25s, may result in NAT close rule map");
    }
    if !(CONFIG.network.len() <= 64 && CONFIG.id.len() <= 64 && CONFIG.peer_id.len() <= 64) {
        bail!("netork, id, or peer_id is too long in config");
    }

    let server_address: Url = CONFIG.server_address.parse()?;
    let (ws, _) = connect_async(server_address).await?;
    let (write, mut read) = ws.split();
    let (tx, rx) = unbounded();

    spawn(send_ws_message(write, rx));

    Request::Connect(me()).into_message().send(&tx)?;

    if !CONFIG.listen {
        punch_to(&tx)?;
    }
    let (wg_tx, mut wg_rx) = unbounded();
    loop {
        select! {
            msg = read.next() => {
                if let Some(msg) = msg {
                    handle_message(msg?, tx.clone(), wg_tx.clone()).await?;
                }
            },
            _ = wg_rx.next() => {
                if !CONFIG.listen {
                    tx.unbounded_send(Message::Close)?;
                    sleep(Duration::from_secs(1)).await;
                    break;
                }
            },
        }
    }
    Ok(())
}
