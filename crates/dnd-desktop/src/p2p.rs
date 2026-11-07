//! P2P transport over iroh.
//!
//! The host publishes a ticket containing its endpoint address. A player binds
//! a local TCP port and proxies every connection to the host's socket.io port
//! over a QUIC bi-directional stream, so the renderer keeps using plain HTTP
//! against 127.0.0.1.

use base64::Engine;
use iroh::{Endpoint, EndpointAddr, endpoint::presets};
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const ALPN: &[u8] = b"dnd-table/1";
const TICKET_PREFIX: &str = "dnd1:";
const B64: base64::engine::general_purpose::GeneralPurpose =
    base64::engine::general_purpose::URL_SAFE_NO_PAD;

pub struct Host {
    pub endpoint: Endpoint,
    pub ticket: String,
    pub task: JoinHandle<()>,
}

pub struct Tunnel {
    pub endpoint: Endpoint,
    pub local_port: u16,
    pub task: JoinHandle<()>,
}

pub async fn start_host(port: u16) -> Result<Host, String> {
    let endpoint = Endpoint::builder(presets::N0)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|error| error.to_string())?;
    let ticket = ticket_for(&endpoint)?;
    let accept_endpoint = endpoint.clone();
    let task = tokio::spawn(async move {
        while let Some(incoming) = accept_endpoint.accept().await {
            let Ok(connection) = incoming.await else {
                continue;
            };
            tokio::spawn(async move {
                loop {
                    let Ok((send, recv)) = connection.accept_bi().await else {
                        break;
                    };
                    let Ok(tcp) = TcpStream::connect(("127.0.0.1", port)).await else {
                        continue;
                    };
                    tokio::spawn(proxy(tcp, send, recv));
                }
            });
        }
    });
    Ok(Host {
        endpoint,
        ticket,
        task,
    })
}

pub async fn start_tunnel(ticket: &str) -> Result<Tunnel, String> {
    let address = address_from_ticket(ticket)?;
    let endpoint = Endpoint::bind(presets::N0)
        .await
        .map_err(|error| error.to_string())?;
    let connection = endpoint
        .connect(address, ALPN)
        .await
        .map_err(|error| error.to_string())?;
    let listener = TcpListener::bind(("127.0.0.1", 0))
        .await
        .map_err(|error| error.to_string())?;
    let local_port = listener
        .local_addr()
        .map_err(|error| error.to_string())?
        .port();
    let task = tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let Ok((send, recv)) = connection.open_bi().await else {
                continue;
            };
            tokio::spawn(proxy(tcp, send, recv));
        }
    });
    Ok(Tunnel {
        endpoint,
        local_port,
        task,
    })
}

async fn proxy(
    tcp: TcpStream,
    mut send: iroh::endpoint::SendStream,
    mut recv: iroh::endpoint::RecvStream,
) {
    let (mut read, mut write) = tcp.into_split();
    let up = async {
        if tokio::io::copy(&mut read, &mut send).await.is_ok() {
            let _ = send.finish();
        }
    };
    let down = async {
        let _ = tokio::io::copy(&mut recv, &mut write).await;
        let _ = write.shutdown().await;
    };
    tokio::join!(up, down);
}

fn ticket_for(endpoint: &Endpoint) -> Result<String, String> {
    let address = endpoint.addr();
    let text = serde_json::to_vec(&address).map_err(|error| error.to_string())?;
    Ok(format!("{TICKET_PREFIX}{}", B64.encode(text)))
}

fn address_from_ticket(ticket: &str) -> Result<EndpointAddr, String> {
    let text = ticket
        .trim()
        .strip_prefix(TICKET_PREFIX)
        .ok_or_else(|| "P2P-билет повреждён".to_string())?;
    let bytes = B64
        .decode(text)
        .map_err(|_| "P2P-билет повреждён".to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "P2P-билет повреждён".to_string())
}

pub fn qr_data_url(ticket: &str) -> Result<String, String> {
    let code = qrcode::QrCode::new(ticket.as_bytes()).map_err(|error| error.to_string())?;
    let svg = code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .build();
    Ok(format!(
        "data:image/svg+xml;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(svg)
    ))
}

pub async fn close_host(host: Host) {
    host.task.abort();
    let _ = host.endpoint.close().await;
}

pub async fn close_tunnel(tunnel: Tunnel) {
    tunnel.task.abort();
    let _ = tunnel.endpoint.close().await;
}

pub fn endpoint_id(endpoint: &Endpoint) -> String {
    endpoint.id().to_string()
}
