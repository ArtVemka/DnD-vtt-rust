//! Realtime table server: axum + Socket.IO.
//!
//! Phase 1 is a protocol spike: serve the existing renderer and answer `ping`.
//! Parity with `E:\DnD\server\socket.js` is checked by `tools/parity.js`.

mod auth;
mod net;

use axum::Router;
use serde_json::{json, Value};
use socketioxide::{
    extract::{Data, SocketRef},
    SocketIo,
};
use tower_http::{cors::CorsLayer, services::ServeDir};

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

async fn on_connect(socket: SocketRef, Data(handshake): Data<Value>) {
    println!("connect {} {:?}", socket.id, handshake);

    socket.on("ping", async |socket: SocketRef, Data(payload): Data<Value>| {
        let echo = payload.get("echo").cloned().unwrap_or(Value::Null);
        let _ = socket.emit("pong", &json!({ "time": now_ms(), "echo": echo }));
    });

    socket.on_disconnect(async |socket: SocketRef| {
        println!("disconnect {}", socket.id);
    });
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().with_max_level(tracing::Level::INFO).init();

    let (layer, io) = SocketIo::new_layer();
    io.ns("/", on_connect);

    let static_dir = std::env::var("DND_RENDERER").unwrap_or_else(|_| "renderer".to_string());
    let app = Router::new()
        .fallback_service(ServeDir::new(&static_dir))
        .layer(CorsLayer::permissive())
        .layer(layer);

    let port = match std::env::var("DND_PORT").ok().and_then(|value| value.parse::<u16>().ok()) {
        Some(port) => port,
        None => net::find_free_port(32100, 64).await,
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("LISTENING {}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}
