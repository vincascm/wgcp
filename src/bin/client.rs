use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use futures_channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures_util::{stream::SplitSink, SinkExt, StreamExt};
use log::error;
use tokio::{
    net::{TcpStream, UdpSocket},
    time::sleep,
};
use tokio_tungstenite::{
    connect_async, tungstenite::Message as WsMessage, MaybeTlsStream, WebSocketStream,
};
use url::Url;

use wgcp::{
    config::client::CONFIG,
    message::{error::Error as MessageError, request::Request, response::Response, Message, Peer},
    wg,
};

async fn broker_handler(broker: SocketAddr) -> Result<()> {
    let sock = UdpSocket::bind("0.0.0.0:0").await?;
    let sock = Arc::new(sock);
    sock.connect(broker).await?;
    let req = Request::Connect(Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.id.to_string(),
    })
    .into_message();
    sock.send(&req.se()?).await?;
    let mut buf = [0; 1024];
    loop {
        sock.recv(&mut buf).await?;
        let msg = Message::de(&buf)?;
        dbg!(&msg);
        match msg {
            Message::Request(request) => match request {
                Request::Ping => {
                    let resp = Response::Pong.into_message();
                    sock.send(&resp.se()?).await?;
                }
                _ => (),
            },
            Message::Response(response) => match response {
                Response::Addr { peer, addr } => {
                    wg::set_endpoint(peer, addr, CONFIG.persistent_keepalive).await?;
                    let resp = Response::Complete.into_message();
                    sock.send(&resp.se()?).await?;
                }
                _ => (),
            },
            Message::Err(e) => error!("broker error: {e:?}"),
        }
    }
}

async fn broker_listen(broker: SocketAddr) {
    if let Err(e) = broker_handler(broker).await {
        error!("broker error: {e}");
    }
}

async fn handle_message(msg: WsMessage, tx: UnboundedSender<Message>) -> Result<()> {
    let msg = msg.try_into()?;
    dbg!(&msg);
    match msg {
        Message::Request(request) => match request {
            Request::Ping => Response::Pong.into_message().send(&tx)?,
            _ => (),
        },
        Message::Response(response) => match response {
            Response::Broker(broker) => {
                tokio::spawn(broker_listen(broker));
            }
            _ => (),
        },
        Message::Err(e) => match e {
            MessageError::PeerOffline => {
                sleep(Duration::from_secs(3)).await;
                punch_to(&tx)?;
            }
            e => error!("server error: {e:?}"),
        },
    }
    Ok(())
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

    let server_address: Url = CONFIG.server_address.parse()?;
    let (ws, _) = connect_async(server_address).await?;
    let (write, mut read) = ws.split();
    let (tx, rx) = unbounded();

    tokio::spawn(send_ws_message(write, rx));

    Request::Connect(Peer {
        network: CONFIG.network.to_string(),
        id: CONFIG.id.to_string(),
    })
    .into_message()
    .send(&tx)?;

    if !CONFIG.listen {
        punch_to(&tx)?;
    }
    while let Some(msg) = read.next().await {
        handle_message(msg?, tx.clone()).await?;
    }
    Ok(())
}
