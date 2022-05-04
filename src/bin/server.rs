use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use log::{error, info};
use once_cell::sync::Lazy;
use tokio::{
    net::{TcpListener, TcpStream, UdpSocket},
    select, spawn,
    sync::Mutex,
    time::sleep,
};
use tokio_tungstenite::{accept_async, tungstenite::Message as WsMessage, WebSocketStream};
use ulid::Ulid;

use wgcp::{
    config::server::Config,
    error::{Error, Result},
    message::{
        error::Error as MessageError, request::Request, response::Response, Message, Peer as PeerId,
    },
};

static CONFIG: Lazy<Config> = Lazy::new(|| Config::from_env().unwrap());

async fn create_broker() -> Result<Arc<Mutex<Broker>>> {
    //let mut task = HashMap::new();
    //task.insert(Ulid::new(), t);
    let broker = Broker {
        listen_addr: CONFIG.broker.parse()?,
        task: HashMap::new(),
        networks: HashMap::new(),
    };
    let broker = Arc::new(Mutex::new(broker));
    let broker_clone = broker.clone();
    spawn(async move {
        if let Err(e) = broker_handler(broker_clone.clone()).await {
            error!("broker error: {e}");
        }
    });
    Ok(broker)
}

struct NetWorks {
    peers: HashMap<PeerId, Peer>,
    broker: Option<Arc<Mutex<Broker>>>,
}

impl NetWorks {
    fn get(&self, peer_id: &PeerId) -> Option<&Peer> {
        self.peers.get(peer_id)
    }

    fn update(&mut self, peer_id: &PeerId, tx: UnboundedSender<Message>) -> Result<()> {
        self.peers
            .entry(peer_id.clone())
            .and_modify(|i| {
                i.tx = Some(tx.clone());
            })
            .or_insert_with(|| Peer { tx: Some(tx) });
        Ok(())
    }

    async fn punch_to(&mut self, from: &PeerId, to: &PeerId) -> Result<(Ulid, SocketAddr)> {
        let task_id = Ulid::new();
        let t = Task {
            from: from.clone(),
            to: to.clone(),
            from_complete: false,
            to_complete: false,
        };
        match &self.broker {
            Some(broker) => {
                let mut broker = broker.lock().await;
                broker.task.insert(task_id, t);
                Ok((task_id, broker.listen_addr))
            }
            None => {
                let broker = create_broker().await?;
                let listen_addr = {
                    let mut b = broker.lock().await;
                    b.task.insert(task_id, t);
                    b.listen_addr
                };
                self.broker = Some(broker);
                Ok((task_id, listen_addr))
            }
        }
    }
}

#[derive(Debug)]
struct Peer {
    tx: Option<UnboundedSender<Message>>,
}

async fn broker_handler(broker: Arc<Mutex<Broker>>) -> Result<()> {
    let sock = {
        let b = broker.lock().await;
        UdpSocket::bind(b.listen_addr).await?
    };
    let sock = Arc::new(sock);
    let mut buf = [0; 1024];
    loop {
        select! {
            r = sock.recv_from(&mut buf) => {
                let (_, addr) = r?;
                let msg = Message::de(&buf)?;
                match msg {
                    Message::Request(request) => match request {
                        Request::Ping => {
                            let resp = Response::Pong.into_message();
                            sock.send_to(&resp.se()?, addr).await?;
                        }
                        Request::ConnectBroker {task_id, peer, token } => {
                            if token != CONFIG.token {
                                let resp = Response::AuthFailed.into_message();
                                sock.send_to(&resp.se()?, addr).await?;
                                continue;
                            }
                            let mut b = broker.lock().await;
                            b.networks.insert(peer.clone(), addr);
                            match b.task.get(&task_id) {
                                Some(task) => {
                                    let remote_peer_id = task.remote_peer_id(&peer)?;
                                    match b.networks.get(remote_peer_id) {
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
                                        }
                                    }
                                },
                                None => {
                                    let resp = MessageError::UnSupportedMessage.into_message();
                                    sock.send_to(&resp.se()?, addr).await?;
                                }
                            }
                        }
                        Request::PunchTo { to, token, .. } => {
                            if token != CONFIG.token {
                                let resp = Response::AuthFailed.into_message();
                                sock.send_to(&resp.se()?, addr).await?;
                                continue;
                            }
                            let broker = broker.lock().await;
                            match broker.networks.get(&to) {
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
                            }
                        },
                        _ => (),
                    },
                    Message::Response(_) => (),
                    Message::Close => (),
                    Message::Err(e) => error!("client error: {e:?}"),
                }
            },
            _ = sleep(Duration::from_secs(300)) => {
                //break;
            },
        }
    }
}

struct Broker {
    listen_addr: SocketAddr,
    task: HashMap<Ulid, Task>,
    networks: HashMap<PeerId, SocketAddr>,
}

impl Broker {
    fn complete(&mut self, task_id: Ulid, peer_id: &PeerId) {
        let mut is_complete = false;
        self.task.entry(task_id).and_modify(|i| {
            is_complete = i.complete(peer_id);
        });
        if is_complete {
            info!("task {task_id} complete");
            self.task.remove(&task_id);
            self.networks.remove(peer_id);
        }
    }
}

struct Task {
    from: PeerId,
    to: PeerId,
    from_complete: bool,
    to_complete: bool,
}

impl Task {
    fn remote_peer_id(&self, peer_id: &PeerId) -> Result<&PeerId> {
        if peer_id == &self.from {
            Ok(&self.to)
        } else if peer_id == &self.to {
            Ok(&self.from)
        } else {
            Err(Error::new("current task has not this peer"))
        }
    }

    fn complete(&mut self, peer_id: &PeerId) -> bool {
        if peer_id == &self.from {
            self.from_complete = true;
        } else if peer_id == &self.to {
            self.to_complete = true;
        }
        self.from_complete && self.to_complete
    }
}

async fn handle_message(
    networks: Arc<Mutex<NetWorks>>,
    msg: WsMessage,
    tx: UnboundedSender<Message>,
) -> Result<()> {
    let msg = match msg {
        WsMessage::Close(_) => return Ok(()),
        x => x.try_into()?,
    };
    match msg {
        Message::Request(request) => match request {
            Request::Ping => Response::Pong.into_message().send(&tx)?,
            Request::Connect { peer, token } => {
                if token == CONFIG.token {
                    let mut n = networks.lock().await;
                    n.update(&peer, tx.clone())?;
                    Response::Connected.into_message().send(&tx)?;
                } else {
                    Response::AuthFailed.into_message().send(&tx)?;
                }
            }
            Request::PunchTo { from, to, token } => {
                if token != CONFIG.token {
                    Response::AuthFailed.into_message().send(&tx)?;
                    return Ok(());
                }
                let mut n = networks.lock().await;
                n.update(&from, tx.clone())?;
                let (task_id, broker_addr) = n.punch_to(&from, &to).await?;
                match n.get(&to).and_then(|i| i.tx.as_ref()) {
                    Some(remote_tx) => {
                        info!("ws punch from {from:?} to {to:?}");
                        let resp = Response::Broker {
                            task_id,
                            broker_addr,
                            to,
                        };
                        resp.into_message().send(&tx)?;
                        let resp = Response::Broker {
                            task_id,
                            broker_addr,
                            to: from,
                        };
                        resp.into_message().send(remote_tx)?;
                    }
                    None => Response::Wait.into_message().send(&tx)?,
                }
            }
            _ => (),
        },
        Message::Response(response) => match response {
            Response::Complete { task_id, peer } => {
                let mut n = networks.lock().await;
                if let Some(broker) = &mut n.broker {
                    let mut broker = broker.lock().await;
                    broker.complete(task_id, &peer);
                }
            }
            _ => (),
        },
        Message::Err(e) => {
            error!("client error: {e:?}");
        }
        _ => (),
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

    spawn(send_ws_message(write, rx));

    while let Some(msg) = read.next().await {
        handle_message(networks.clone(), msg?, tx.clone()).await?;
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

    let networks = Arc::new(Mutex::new(NetWorks {
        peers: HashMap::new(),
        broker: None,
    }));
    let listener = TcpListener::bind(&CONFIG.listen_address).await?;
    while let Ok((stream, _)) = listener.accept().await {
        let networks = networks.clone();
        spawn(async move {
            if let Err(e) = handle_connection(networks.clone(), stream).await {
                error!("{e}");
            }
        });
    }
    Ok(())
}
