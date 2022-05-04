use std::{
    mem::transmute,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use bytes::BufMut;
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use log::{error, info, warn};
use once_cell::sync::Lazy;
use tokio::{net::TcpStream, select, spawn, time::sleep};
use tokio_tungstenite::{
    connect_async, tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};
use ulid::Ulid;
use url::Url;

use wgcp::{
    config::client::{Config, NetWork},
    error::{Error, Result},
    message::{request::Request, response::Response, Message, Peer},
    socket_filter::broker_response_filter,
    wg,
};

static CONFIG: Lazy<Config> = Lazy::new(|| Config::from_env().unwrap());

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

async fn keepalive(tx: UnboundedSender<Message>) -> Result<()> {
    loop {
        Request::Ping.into_message().send(&tx)?;
        sleep(Duration::from_secs(CONFIG.keepalive.unwrap_or(90))).await;
    }
}

/// return local addr, peer id, peer addr
fn get_peer_with_broker(
    network: &NetWork,
    task_id: Ulid,
    broker: SocketAddr,
) -> Result<(Peer, SocketAddr)> {
    use socket2::{Domain, Protocol, Socket, Type};

    let broker_ip: Ipv4Addr = match broker.ip() {
        IpAddr::V4(v) => v,
        IpAddr::V6(_) => return Err(Error::new("invalid broker ip, must be ipv4")),
    };
    let broker_port = broker.port();
    let wg_port = wg::get_listen_port(&network.interface)?;

    let sock = Socket::new(Domain::IPV4, Type::RAW, Some(Protocol::UDP))?;
    sock.set_reuse_port(true)?;
    sock.set_reuse_address(true)?;
    sock.attach_filter(&broker_response_filter(broker_ip, broker_port, wg_port))?;

    let udp = Udp::new(wg_port, broker_port);

    let req = Request::ConnectBroker {
        task_id,
        peer: network.me(),
        token: CONFIG.token.clone(),
    }
    .into_message()
    .se()?;
    sock.send_to(&udp.packet(&req), &broker.into())?;
    let mut buf = vec![0u8; 1024];
    loop {
        let (_, sock_addr) = sock.recv_from(unsafe { transmute(buf.as_mut_slice()) })?;
        // skip ip header and udp header, total 28 bytes
        let msg = Message::de(&buf[28..])?;
        match msg {
            Message::Request(_) => (),
            Message::Response(response) => match response {
                Response::Addr { peer, addr } => {
                    let resp = Response::Wait.into_message();
                    let resp = resp.se()?;
                    sock.send_to(&udp.packet(&resp), &sock_addr)?;

                    // send 3 ping packet to peer, avoid NAT release port map
                    let udp = Udp::new(wg_port, addr.port());
                    let resp = Request::Ping.into_message();
                    let resp = resp.se()?;
                    sock.send_to(&udp.packet(&resp), &addr.into())?;
                    std::thread::sleep(Duration::from_secs(1));
                    sock.send_to(&udp.packet(&resp), &addr.into())?;
                    std::thread::sleep(Duration::from_secs(1));
                    sock.send_to(&udp.packet(&resp), &addr.into())?;
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
    let msg = match msg.try_into() {
        Ok(v) => v,
        Err(e) => {
            error!("deserialize command failure: {e}");
            return Ok(());
        }
    };
    match msg {
        Message::Request(request) => match request {
            Request::Ping => Response::Pong.into_message().send(&tx)?,
            _ => (),
        },
        Message::Response(response) => match response {
            Response::Connected => {
                if CONFIG.listen {
                    spawn(async move {
                        if let Err(e) = keepalive(tx).await {
                            error!("keepalive {e}");
                        }
                    });
                }
            }
            Response::Broker {
                task_id,
                broker_addr,
                to,
            } => {
                tokio::task::spawn_blocking(move || {
                    let network = match CONFIG.network.iter().find(|i| i.network == to.network) {
                        Some(v) => v,
                        None => {
                            error!("invalid network in Response::Broker");
                            return;
                        }
                    };
                    let config_peer = match network.peers.iter().find(|i| i.id == to.id) {
                        Some(v) => v,
                        None => {
                            error!("invalid peer id in Response::Broker");
                            return;
                        }
                    };
                    match get_peer_with_broker(network, task_id, broker_addr) {
                        Ok((peer, addr)) => {
                            if !(peer.network == network.network && peer.id == config_peer.id) {
                                error!("get_peer_with_broker error: peer is unable to verify");
                                return;
                            }
                            if let Err(e) = wg::set(&network.interface, config_peer, addr) {
                                error!("wg set error: {e}");
                            }
                        }
                        Err(e) => error!("get_peer_with_broker error: {e}"),
                    }
                    let r = Response::Complete {
                        task_id,
                        peer: network.me(),
                    }
                    .into_message()
                    .send(&tx);
                    match r {
                        Ok(_) => info!("notify server task {task_id} complete"),
                        Err(e) => error!("send Complete message error: {e}"),
                    }
                    wg_tx.unbounded_send(()).ok();
                });
            }
            Response::Wait => {
                if !CONFIG.listen {
                    info!("peer is offline");
                    wg_tx.unbounded_send(()).ok();
                }
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

async fn run(server_address: Url) -> Result<()> {
    let (ws, _) = connect_async(server_address).await?;
    let (write, mut read) = ws.split();
    let (tx, rx) = unbounded();

    spawn(async move {
        if let Err(e) = send_ws_message(write, rx).await {
            error!("send websocket message: {e}");
        };
    });

    if CONFIG.listen {
        for n in &CONFIG.network {
            Request::Connect {
                peer: n.me(),
                token: CONFIG.token.clone(),
            }
            .into_message()
            .send(&tx)?;
        }
    } else {
        for n in &CONFIG.network {
            if n.skip {
                continue;
            }

            let from = Peer {
                network: n.network.clone(),
                id: n.id.clone(),
            };

            for p in &n.peers {
                if p.skip {
                    continue;
                }

                let to = Peer {
                    network: n.network.clone(),
                    id: p.id.to_string(),
                };
                Request::PunchTo {
                    from: from.clone(),
                    to,
                    token: CONFIG.token.clone(),
                }
                .into_message()
                .send(&tx)?;
            }
        }
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

#[tokio::main]
async fn main() -> Result<()> {
    std::env::set_var("RUST_LOG", "info");
    env_logger::init();

    if std::env::var("CONFIG_FILE").is_err() {
        let filename = std::env::args()
            .nth(1)
            .unwrap_or_else(|| "config.yaml".to_owned());
        std::env::set_var("CONFIG_FILE", filename);
    }

    for n in &CONFIG.network {
        if !(n.network.len() <= 64 && n.id.len() <= 64) {
            return Err(Error::new("netork or id is too long in config"));
        }

        for p in &n.peers {
            if p.id.len() > 64 {
                return Err(Error::new("peer id is too long in config"));
            }
            if p.persistent_keepalive.unwrap_or(25) > 25 {
                warn!("persistent-keepalive is large than 25s, may result in NAT close rule map");
            }
        }
    }
    let server_address: Url = CONFIG.server_address.parse()?;
    use std::io::ErrorKind as IoErr;
    use tokio_tungstenite::tungstenite::Error as WsErr;
    loop {
        if let Err(e) = run(server_address.clone()).await {
            if let Error::Tungstenite(e) = e {
                match e {
                    WsErr::Protocol(e) => {
                        if e == tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake {
                            error!("websocket reset, will reconnect");
                            continue;
                        }
                    },
                    WsErr::Io(e) => {
                        let k = e.kind();
                        if k == IoErr::ConnectionReset || k == IoErr::TimedOut {
                            error!("tcp {k}, will reconnect");
                            continue;
                        }
                    },
                    WsErr::Http(resp) => {
                        if resp.status() == http::StatusCode::BAD_GATEWAY {
                            error!("server is offline, will reconnect");
                            continue;
                        }
                    },
                    x => error!("websocket error: {x}"),
                }
            } else {
                error!("{e}");
            }
        }
        break;
    }
    Ok(())
}
