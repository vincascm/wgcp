use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use log::error;
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    sync::Mutex,
    time::sleep,
};
use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage, WebSocketStream};

use wgcp::{
    config::server::CONFIG,
    message::{
        error::Error as MessageError, request::Request, response::Response, Message, Peer as PeerId,
    },
};

#[derive(Default, Debug)]
struct NetWorks {
    inner: HashMap<String, NetWork>,
}

impl NetWorks {
    fn get(&self, network: &str, peer_id: &str) -> Result<Option<&Peer>> {
        let endpoint = self.inner.get(network).and_then(|i| i.peers.get(peer_id));
        Ok(endpoint)
    }

    fn update(&mut self, network: &str, peer_id: &str, tx: UnboundedSender<Message>) -> Result<()> {
        self.inner
            .entry(network.to_string())
            .and_modify(|i| {
                i.peers
                    .entry(peer_id.to_string())
                    .and_modify(|i| {
                        i.tx = Some(tx.clone());
                    })
                    .or_insert(Peer {
                        tx: Some(tx.clone()),
                    });
            })
            .or_insert_with(|| {
                let mut peers = HashMap::new();
                peers.insert(peer_id.to_string(), Peer { tx: Some(tx) });
                NetWork { peers }
            });
        Ok(())
    }
}

#[derive(Default, Debug)]
struct NetWork {
    peers: HashMap<String, Peer>,
}

#[derive(Debug)]
struct Peer {
    tx: Option<UnboundedSender<Message>>,
}

async fn broker_handler(mut broker: Broker) -> Result<()> {
    let sock = UdpSocket::bind(broker.listen_addr).await?;
    let sock = Arc::new(sock);
    let mut buf = [0; 1024];
    loop {
        let (_, addr) = sock.recv_from(&mut buf).await?;
        let msg = Message::de(&buf)?;
        dbg!(&msg);
        match msg {
            Message::Request(request) => match request {
                Request::Ping => {
                    let resp = Response::Pong.into_message();
                    sock.send_to(&resp.se()?, addr).await?;
                }
                Request::Connect(peer) => {
                    broker.networks.insert(peer.clone(), addr);
                    match broker.task.get(&peer) {
                        Some(remote_peer_id) => {
                            match broker.networks.get(remote_peer_id) {
                                Some(remote_addr) => {
                                    let resp = Response::Addr {
                                        peer: remote_peer_id.clone(),
                                        addr: *remote_addr,
                                    }
                                    .into_message();
                                    sock.send_to(&resp.se()?, addr).await?;
                                    let remote_resp = Response::Addr {
                                        peer: peer.clone(),
                                        addr,
                                    }
                                    .into_message();
                                    sock.send_to(&remote_resp.se()?, remote_addr).await?;
                                }
                                None => {
                                    let resp = Response::Wait.into_message();
                                    sock.send_to(&resp.se()?, addr).await?;
                                    //tokio::spawn(send_ping(sock.clone(), addr));
                                }
                            }
                        }
                        None => {
                            let resp = MessageError::UnSupportedMessage.into_message();
                            sock.send_to(&resp.se()?, addr).await?;
                        }
                    }
                }
                Request::PunchTo { to, .. } => match broker.networks.get(&to) {
                    Some(addr) => {
                        let resp = Response::Addr {
                            peer: to,
                            addr: *addr,
                        }
                        .into_message();
                        sock.send_to(&resp.se()?, addr).await?;
                    }
                    None => {
                        let resp = MessageError::PeerOffline.into_message();
                        sock.send_to(&resp.se()?, addr).await?;
                    }
                },
            },
            Message::Response(response) => match response {
                Response::Complete => {
                    sleep(Duration::from_secs(60)).await;
                    break;
                }
                _ => (),
            },
            Message::Err(e) => error!("client error: {e:?}"),
        }
    }
    Ok(())
}

async fn broker_listen(broker: Broker) {
    if let Err(e) = broker_handler(broker).await {
        error!("broker error: {e}");
    }
}

struct Broker {
    listen_addr: SocketAddr,
    /// from -> to
    task: HashMap<PeerId, PeerId>,
    networks: HashMap<PeerId, SocketAddr>,
}

async fn handle_message(
    networks: Arc<Mutex<NetWorks>>,
    msg: WsMessage,
    tx: UnboundedSender<Message>,
) -> Result<()> {
    let msg = msg.try_into()?;
    dbg!(&msg);
    match msg {
        Message::Request(request) => match request {
            Request::Ping => Response::Pong.into_message().send(&tx)?,
            Request::Connect(peer) => {
                let mut n = networks.lock().await;
                n.update(&peer.network, &peer.id, tx.clone())?;
                Response::Connected.into_message().send(&tx)?;
            }
            Request::PunchTo { from, to } => {
                let n = networks.lock().await;
                let remote_peer = n.get(&to.network, &to.id)?;
                match remote_peer.and_then(|i| i.tx.as_ref()) {
                    Some(remote_tx) => {
                        let listen_addr: SocketAddr = CONFIG.broker.parse()?;
                        let mut task = HashMap::new();
                        task.insert(from.clone(), to.clone());
                        task.insert(to, from);
                        let broker = Broker {
                            listen_addr,
                            task,
                            networks: HashMap::new(),
                        };
                        tokio::spawn(broker_listen(broker));
                        Response::Broker(listen_addr).into_message().send(&tx)?;
                        Response::Broker(listen_addr)
                            .into_message()
                            .send(remote_tx)?;
                    }
                    None => MessageError::PeerOffline.into_message().send(&tx)?,
                }
            }
        },
        Message::Response(_) => (),
        Message::Err(e) => {
            error!("client error: {e:?}");
        }
    }
    Ok(())
}

async fn send_ws_message(
    mut ws_write: SplitSink<WebSocketStream<TcpStream>, WsMessage>,
    mut rx: UnboundedReceiver<Message>,
) -> Result<()> {
    while let Some(msg) = rx.next().await {
        let msg = msg.try_into()?;
        ws_write.send(msg).await?;
    }
    Ok(())
}

async fn handle_connection(networks: Arc<Mutex<NetWorks>>, stream: TcpStream) -> Result<()> {
    let ws = accept_async(stream).await?;
    let (write, mut read) = ws.split();
    let (tx, rx) = unbounded();

    tokio::spawn(send_ws_message(write, rx));

    while let Some(msg) = read.next().await {
        handle_message(networks.clone(), msg?, tx.clone()).await?;
    }
    Ok(())
}

async fn spawn_handler(networks: Arc<Mutex<NetWorks>>, stream: TcpStream) {
    if let Err(e) = handle_connection(networks.clone(), stream).await {
        error!("{e}");
    }
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

    let networks = Arc::new(Mutex::new(NetWorks::default()));

    let listener = TcpListener::bind(&CONFIG.listen_address).await?;
    while let Ok((stream, _)) = listener.accept().await {
        tokio::spawn(spawn_handler(networks.clone(), stream));
    }
    Ok(())
}
