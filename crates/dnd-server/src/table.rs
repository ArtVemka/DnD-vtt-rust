//! Realtime table: identity resolution, rooms, the join handshake.
//!
//! The server binds loopback only, so the host key is the real gate for the DM
//! role; the reference additionally checked the handshake address, which has no
//! extra value here. Player identity follows the reference: a known player id
//! is accepted only with its secret `playerKey`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use dnd_core::campaign::CampaignStore;
use dnd_core::room::{RoomState, Viewer, DM_PLAYER_ID, ROLE_DM, ROLE_PLAYER};
use serde_json::{json, Value};
use socketioxide::extract::{AckSender, Data, SocketRef};
use socketioxide::SocketIo;
use uuid::Uuid;

use crate::auth::{safe_equal, sanitize_name};

#[derive(Debug, PartialEq, Eq)]
pub enum JoinError {
    EmptyName,
    IdentityDenied(&'static str),
}

#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    Dm { name: String },
    Player {
        name: String,
        player_id: String,
        player_key: String,
    },
}

pub fn valid_player_id(player_id: &str) -> bool {
    (4..=64).contains(&player_id.len())
        && player_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

fn random_player_key() -> String {
    let first = Uuid::new_v4().simple().to_string();
    let second = Uuid::new_v4().simple().to_string();
    format!("{}{}", &first[..32], &second[..16])
}

/// Decides who is joining, before any state is mutated.
pub fn resolve_join(room: &RoomState, data: &Value, host_key: &str) -> Result<JoinOutcome, JoinError> {
    let name = sanitize_name(data.get("name").unwrap_or(&Value::Null));
    if name.is_empty() {
        return Err(JoinError::EmptyName);
    }

    let host_ok = safe_equal(
        data.get("hostKey").and_then(Value::as_str).unwrap_or(""),
        host_key,
    );
    if host_ok {
        return Ok(JoinOutcome::Dm { name });
    }

    let requested = data
        .get("playerId")
        .map(|value| match value {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        })
        .unwrap_or_default();
    let provided_key: String = data
        .get("playerKey")
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(128)
        .collect();

    if requested == DM_PLAYER_ID {
        return Err(JoinError::IdentityDenied(
            "Это код мастера. Войдём заново под своим кодом.",
        ));
    }

    let wanted = if valid_player_id(&requested) {
        requested
    } else {
        String::new()
    };
    if let Some(existing) = room.players.get(&wanted) {
        let existing_key = existing.get("playerKey").and_then(Value::as_str).unwrap_or("");
        if !existing_key.is_empty() {
            if !safe_equal(&provided_key, existing_key) {
                return Err(JoinError::IdentityDenied(
                    "Игрок с таким кодом уже за столом. Войдём заново под новым кодом.",
                ));
            }
            return Ok(JoinOutcome::Player {
                name,
                player_id: wanted,
                player_key: existing_key.to_string(),
            });
        }
        if room.is_online(&wanted) {
            return Err(JoinError::IdentityDenied(
                "Игрок с таким кодом уже за столом. Войдём заново под новым кодом.",
            ));
        }
    }

    Ok(JoinOutcome::Player {
        name,
        player_id: if wanted.is_empty() {
            format!("p-{}", Uuid::new_v4())
        } else {
            wanted
        },
        player_key: random_player_key(),
    })
}

#[derive(Debug, Clone)]
pub struct Session {
    pub role: String,
    pub player_id: String,
    pub name: String,
}

pub struct Table {
    pub room: Mutex<RoomState>,
    pub host_key: String,
    pub store: Option<Arc<CampaignStore>>,
    pub sessions: Mutex<std::collections::HashMap<String, Session>>,
    pub dirty: AtomicBool,
}

impl Table {
    pub fn new(room: RoomState, host_key: String, store: Option<Arc<CampaignStore>>) -> Arc<Self> {
        Arc::new(Self {
            room: Mutex::new(room),
            host_key,
            store,
            sessions: Mutex::new(std::collections::HashMap::new()),
            dirty: AtomicBool::new(false),
        })
    }

    pub fn mark_dirty(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    pub fn campaign_id(&self) -> Option<String> {
        self.room
            .lock()
            .unwrap()
            .campaign
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    pub fn player_list(&self, dm_view: bool) -> Value {
        let room = self.room.lock().unwrap();
        Value::Array(room.player_list(dnd_core::room::PlayerListOptions {
            include_tokens: true,
            include_dm_notes: dm_view,
            include_hidden: dm_view,
        }))
    }

    pub fn snapshot_for(&self, viewer: &Viewer) -> Value {
        self.room.lock().unwrap().snapshot_for(viewer)
    }

    /// Persists the campaign if anything changed since the last call.
    pub fn save_if_dirty(&self) -> Result<bool, dnd_core::campaign::CampaignError> {
        if !self.dirty.swap(false, Ordering::Relaxed) {
            return Ok(false);
        }
        let Some(store) = &self.store else {
            return Ok(false);
        };
        let mut campaign = {
            let mut room = self.room.lock().unwrap();
            let state = room.serialize();
            room.campaign["state"] = state;
            room.campaign.clone()
        };
        store.save(&mut campaign)?;
        self.room.lock().unwrap().campaign = campaign;
        Ok(true)
    }
}

/// Builds the Socket.IO layer and registers the handshake.
pub fn create_io(table: Arc<Table>) -> (socketioxide::layer::SocketIoLayer, SocketIo) {
    let (layer, io) = SocketIo::builder()
        .max_payload(20 * 1024 * 1024)
        .build_layer();

    let connect_table = table.clone();
    io.ns(
        "/",
        move |socket: SocketRef, Data(_auth): Data<Value>| {
            let table = connect_table.clone();
            async move { on_connect(socket, table).await }
        },
    );
    (layer, io)
}

async fn on_connect(socket: SocketRef, table: Arc<Table>) {
    {
        let join_table = table.clone();
        socket.on(
            "joinRoom",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = join_table.clone();
                async move { handle_join_room(socket, io, table, data, ack).await }
            },
        );
    }
    {
        socket.on(
            "ping",
            |socket: SocketRef, Data::<Value>(payload)| async move {
                let echo = payload.get("echo").cloned().unwrap_or(Value::Null);
                let _ = socket.emit("pong", &json!({ "time": now_ms(), "echo": echo }));
            },
        );
    }
    {
        let leave_table = table.clone();
        socket.on_disconnect(move |socket: SocketRef, io: SocketIo| {
            let table = leave_table.clone();
            async move { handle_disconnect(socket, io, table).await }
        });
    }
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

async fn handle_join_room(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
    ack: AckSender,
) {
    let outcome = {
        let room = table.room.lock().unwrap();
        resolve_join(&room, &data, &table.host_key)
    };

    let (role, player_id, player_key, name) = match outcome {
        Ok(JoinOutcome::Dm { name }) => {
            let mut room = table.room.lock().unwrap();
            room.add_dm(&name);
            room.attach_socket(DM_PLAYER_ID, &socket.id.to_string());
            (ROLE_DM.to_string(), DM_PLAYER_ID.to_string(), String::new(), name)
        }
        Ok(JoinOutcome::Player {
            name,
            player_id,
            player_key,
        }) => {
            let mut room = table.room.lock().unwrap();
            room.add_player(&player_id, &name, &player_key);
            room.attach_socket(&player_id, &socket.id.to_string());
            (ROLE_PLAYER.to_string(), player_id, player_key, name)
        }
        Err(JoinError::EmptyName) => {
            let _ = ack.send(&json!({ "ok": false, "error": "Имя не может быть пустым" }));
            let _ = socket.emit("errorMessage", &json!({ "message": "Имя не может быть пустым" }));
            return;
        }
        Err(JoinError::IdentityDenied(message)) => {
            let _ = ack.send(&json!({ "ok": false, "code": "identity-denied", "error": message }));
            return;
        }
    };

    socket.join("table");
    if role == ROLE_DM {
        socket.join("dm-sockets");
    }
    socket.join(player_id.clone());

    table.sessions.lock().unwrap().insert(
        socket.id.to_string(),
        Session {
            role: role.clone(),
            player_id: player_id.clone(),
            name: name.clone(),
        },
    );

    let you = json!({
        "playerId": player_id,
        "name": name,
        "role": role,
        "playerKey": player_key
    });
    let state = table.snapshot_for(&Viewer {
        player_id: player_id.clone(),
        role: role.clone(),
    });
    let _ = socket.emit("joined", &json!({ "you": you, "state": state }));
    let _ = ack.send(&json!({ "ok": true, "you": you }));
    table.mark_dirty();
    broadcast_players(&io, &table).await;
}

async fn handle_disconnect(socket: SocketRef, io: SocketIo, table: Arc<Table>) {
    let session = table.sessions.lock().unwrap().remove(&socket.id.to_string());
    let Some(session) = session else {
        return;
    };
    table
        .room
        .lock()
        .unwrap()
        .detach_socket(&session.player_id, &socket.id.to_string());
    table.mark_dirty();
    broadcast_players(&io, &table).await;
}

async fn broadcast_players(io: &SocketIo, table: &Arc<Table>) {
    let players_view = table.player_list(false);
    let _ = io
        .within("table")
        .except("dm-sockets")
        .emit("playersUpdate", &players_view)
        .await;
    let dm_view = table.player_list(true);
    let _ = io
        .within("dm-sockets")
        .emit("playersUpdate", &dm_view)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn campaign() -> Value {
        json!({ "id": "abcd", "name": "Тест", "state": {} })
    }

    #[test]
    fn host_key_opens_the_dm_role() {
        let room = RoomState::new(campaign());
        let outcome = resolve_join(&room, &json!({ "name": "Мастер", "hostKey": "secret" }), "secret");
        assert_eq!(outcome, Ok(JoinOutcome::Dm { name: "Мастер".into() }));
    }

    #[test]
    fn missing_name_is_rejected() {
        let room = RoomState::new(campaign());
        assert_eq!(
            resolve_join(&room, &json!({ "name": "   " }), "secret"),
            Err(JoinError::EmptyName)
        );
    }

    #[test]
    fn wrong_key_makes_a_fresh_player() {
        let room = RoomState::new(campaign());
        match resolve_join(&room, &json!({ "name": "Гость", "hostKey": "bad" }), "secret") {
            Ok(JoinOutcome::Player { name, player_id, player_key }) => {
                assert_eq!(name, "Гость");
                assert!(player_id.starts_with("p-"));
                assert_eq!(player_key.len(), 48);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn reserved_dm_id_is_denied() {
        let room = RoomState::new(campaign());
        let error = resolve_join(&room, &json!({ "name": "Хитрец", "playerId": "dm" }), "secret")
            .unwrap_err();
        assert_eq!(
            error,
            JoinError::IdentityDenied("Это код мастера. Войдём заново под своим кодом.")
        );
    }

    #[test]
    fn known_player_needs_the_right_secret() {
        let mut room = RoomState::new(campaign());
        room.add_player("p-hero", "Герой", "aabbcc");
        assert!(matches!(
            resolve_join(&room, &json!({ "name": "Герой", "playerId": "p-hero", "playerKey": "wrong" }), "secret"),
            Err(JoinError::IdentityDenied(_))
        ));
        let outcome = resolve_join(
            &room,
            &json!({ "name": "Герой", "playerId": "p-hero", "playerKey": "aabbcc" }),
            "secret",
        )
        .unwrap();
        assert_eq!(
            outcome,
            JoinOutcome::Player {
                name: "Герой".into(),
                player_id: "p-hero".into(),
                player_key: "aabbcc".into()
            }
        );
    }

    #[test]
    fn legacy_player_without_key_cannot_be_taken_while_online() {
        let mut room = RoomState::new(campaign());
        room.add_player("p-hero", "Герой", "");
        room.attach_socket("p-hero", "s1");
        assert!(matches!(
            resolve_join(&room, &json!({ "name": "Вор", "playerId": "p-hero" }), "secret"),
            Err(JoinError::IdentityDenied(_))
        ));
        room.detach_socket("p-hero", "s1");
        assert!(matches!(
            resolve_join(&room, &json!({ "name": "Герой", "playerId": "p-hero" }), "secret"),
            Ok(JoinOutcome::Player { .. })
        ));
    }

    #[test]
    fn invalid_ids_are_replaced() {
        let room = RoomState::new(campaign());
        let outcome = resolve_join(&room, &json!({ "name": "Игрок", "playerId": "x" }), "secret").unwrap();
        assert!(matches!(outcome, JoinOutcome::Player { .. }));
        if let JoinOutcome::Player { player_id, .. } = outcome {
            assert!(player_id.starts_with("p-"));
        }
    }

    #[test]
    fn player_ids_follow_the_reference_pattern() {
        assert!(valid_player_id("p-hero_1"));
        assert!(!valid_player_id("abc"));
        assert!(!valid_player_id("плохой"));
        assert!(!valid_player_id(&"a".repeat(65)));
    }
}

