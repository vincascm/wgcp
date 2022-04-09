use std::net::SocketAddr;

use anyhow::Result;
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use log::{error, warn};
use tokio::{
    net::{TcpStream, UdpSocket},
    spawn,
    select,
};
use tokio_tungstenite::{
    connect_async, tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};
use url::Url;

use wgcp::{
    config::client::CONFIG,
    message::{request::Request, response::Response, Message, Peer},
    wg,
};

/// return local addr, peer id, peer addr 
async fn get_peer_with_broker(broker: SocketAddr) -> Result<(SocketAddr, Peer, SocketAddr)> {
    use socket2::{Domain, Protocol, Socket, Type};
    let bind_addr: SocketAddr = "0.0.0.0:0".parse()?;
    let sock = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    sock.bind(&bind_addr.into())?;
    sock.set_reuse_port(true)?;
    sock.set_reuse_address(true)?;
    let sock: std::net::UdpSocket = sock.into();
    let sock = UdpSocket::from_std(sock)?;
    let local_addr = sock.local_addr()?;
    let req = Request::Connect(Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.id.to_string(),
    })
    .into_message();
    sock.send_to(&req.se()?, broker).await?;
    let mut buf = [0; 1024];
    loop {
        let (_, sock_addr) = sock.recv_from(&mut buf).await?;
        let msg = Message::de(&buf)?;
        match msg {
            Message::Request(request) => match request {
                Request::Ping => {
                    let resp = Response::Pong.into_message();
                    sock.send_to(&resp.se()?, sock_addr).await?;
                }
                _ => (),
            },
            Message::Response(response) => match response {
                Response::Addr { peer, addr } => {
                    let resp = Response::Complete.into_message();
                    sock.send_to(&resp.se()?, sock_addr).await?;
                    return Ok((local_addr, peer, addr));
                }
                _ => (),
            },
            Message::Err(e) => error!("broker error: {e:?}"),
        }
    }
}

/// return true: success
async fn handle_message(msg: WsMessage, tx: UnboundedSender<Message>, wg_tx: UnboundedSender<()>) -> Result<bool> {
    let msg = msg.try_into()?;
    match msg {
        Message::Request(request) => match request {
            Request::Ping => Response::Pong.into_message().send(&tx)?,
            _ => (),
        },
        Message::Response(response) => match response {
            Response::Broker(broker) => {
                spawn(async move {
                    match get_peer_with_broker(broker).await {
                        Ok((local_addr, peer, addr)) => {
                            if let Err(e) = wg::set(local_addr, peer, addr, CONFIG.persistent_keepalive).await {
                                error!("wg set error: {e}");
                            }
                            wg_tx.unbounded_send(()).ok();
                        },
                        Err(e) => error!("get_peer_with_broker error: {e}"),
                    }
                });
                return Ok(true);
            }
            _ => (),
        },
        Message::Err(e) => error!("ws server error: {e:?}"),
    }
    Ok(false)
}

async fn send_ws_message(
    mut ws_write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, WsMessage>,
    mut rx: UnboundedReceiver<Message>,
) -> Result<()> {
    while let Some(msg) = rx.next().await {
        let msg = msg.try_into()?;
        ws_write.send(msg).await?;
    }
    Ok(())
}

fn punch_to(tx: &UnboundedSender<Message>) -> Result<()> {
    let from = Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.id.to_string(),
    };
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

    let server_address: Url = CONFIG.server_address.parse()?;
    let (ws, _) = connect_async(server_address).await?;
    let (write, mut read) = ws.split();
    let (tx, rx) = unbounded();

    spawn(send_ws_message(write, rx));

    Request::Connect(Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.id.to_string(),
    })
    .into_message()
    .send(&tx)?;

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
                break;
            },
        }
    }
    Ok(())
}
