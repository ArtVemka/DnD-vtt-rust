//! Socket event handlers, grouped by domain.
//!
//! Every module registers its own events in `register`; `table.rs` calls
//! `register` once per connection after `joinRoom` handlers are installed.

pub mod combat;
pub mod journal;
pub mod maps;
pub mod misc;
pub mod sheets;
pub mod tokens;

use std::sync::Arc;

use dnd_core::room::{ROLE_DM, Viewer};
use serde_json::{Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::SocketRef;

use crate::table::{Session, Table};

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    combat::register(socket, table.clone());
    journal::register(socket, table.clone());
    maps::register(socket, table.clone());
    misc::register(socket, table.clone());
    sheets::register(socket, table.clone());
    tokens::register(socket, table);
}

pub fn session_of(table: &Table, socket: &SocketRef) -> Option<Session> {
    table
        .sessions
        .lock()
        .unwrap()
        .get(&socket.id.to_string())
        .cloned()
}

pub fn require_joined(socket: &SocketRef, table: &Table) -> Option<Session> {
    let session = session_of(table, socket);
    match session {
        Some(session) if !session.role.is_empty() => Some(session),
        _ => {
            let _ = socket.emit(
                "errorMessage",
                &json!({ "message": "Сначала подключитесь к столу" }),
            );
            None
        }
    }
}

pub fn require_dm(socket: &SocketRef, table: &Table) -> Option<Session> {
    let session = require_joined(socket, table)?;
    if session.role != ROLE_DM {
        let _ = socket.emit(
            "errorMessage",
            &json!({ "message": "Это действие доступно только ДМ" }),
        );
        return None;
    }
    Some(session)
}

pub fn viewer_of(session: &Session) -> Viewer {
    Viewer {
        player_id: session.player_id.clone(),
        role: session.role.clone(),
    }
}

pub async fn broadcast_players(io: &SocketIo, table: &Arc<Table>) {
    crate::table::broadcast_players(io, table).await;
}

pub fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

pub fn error_text(data: &Value, key: &str) -> String {
    data.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}
