//! Tokens: creation, movement, images, visibility, light, aura, vision.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use dnd_core::campaign::CampaignError;
use dnd_core::content;
use dnd_core::media::decode_image;
use dnd_core::model::MAP_TYPES;
use dnd_core::room::{ROLE_DM, ROLE_PLAYER, RoomState};
use dnd_core::rules;
use dnd_core::sheet::{self, LIMITS};
use serde_json::{Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{Data, SocketRef};

use crate::auth::sanitize_name;
use crate::handlers::{broadcast_players, now_ms, require_dm, require_joined};
use crate::table::Table;

const TOKEN_PALETTE: [&str; 8] = [
    "#c96f4a", "#4a8fc9", "#6fb45f", "#b45fa0", "#c9a34a", "#5f7fb4", "#a05f5f", "#4ab4a0",
];
const FOG_FOLLOW_RADIUS: f64 = 0.08;
const EMOTE_TTL_MS: u128 = 6000;

// ---------------------------------------------------------------------------
// Helpers (mirrors of the small JS socket helpers)
// ---------------------------------------------------------------------------

fn js_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(other) => other.to_string(),
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|n| n != 0.0 && !n.is_nan())
            .unwrap_or(false),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `Number(value)` for the loose reads in movement payloads.
fn number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(flag) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(raw) => raw.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        Value::Array(items) if items.len() == 1 => number(&items[0]),
        _ => f64::NAN,
    }
}

fn token_index(room: &RoomState, token_id: &str) -> Option<usize> {
    room.tokens
        .iter()
        .position(|token| js_string(token.get("id")) == token_id)
}

fn emit_error(socket: &SocketRef, message: impl AsRef<str>) {
    let _ = socket.emit("errorMessage", &json!({ "message": message.as_ref() }));
}

fn token_payloads(
    room: &RoomState,
    token: &Value,
    payload: Option<Value>,
) -> (Value, Option<Value>) {
    let dm = payload
        .clone()
        .unwrap_or_else(|| room.token_for_viewer(token, true));
    let players = if room.is_token_visible_to_players(token) {
        Some(payload.unwrap_or_else(|| room.token_for_viewer(token, false)))
    } else {
        None
    };
    (dm, players)
}

async fn emit_token(io: &SocketIo, event: &str, payloads: (Value, Option<Value>)) {
    let _ = io.to("dm-sockets").emit(event, &payloads.0).await;
    if let Some(players) = payloads.1 {
        let _ = io
            .to("table")
            .except("dm-sockets")
            .emit(event, &players)
            .await;
    }
}

fn push_undo(room: &mut RoomState) {
    let snapshot = json!({
        "maps": room.maps,
        "tokens": room.tokens,
        "initiative": room.initiative,
        "activeMap": room.active_map,
        "activeMapId": room.active_map_id,
        "journal": room.journal,
        "journalEntries": room.journal_entries,
        "inventory": room.inventory,
        "inventoryCoins": room.inventory_coins,
        "gameTime": room.game_time,
        "calendarMonths": room.calendar_months,
        "calendarEvents": room.calendar_events
    });
    room.undo_stack.push(snapshot);
    if room.undo_stack.len() > 20 {
        room.undo_stack.remove(0);
    }
}

fn reset_image_crop(token: &mut Value) {
    token["imageZoom"] = json!(1);
    token["imageX"] = json!(0);
    token["imageY"] = json!(0);
}

fn sync_sheet_tokens(room: &mut RoomState, player_id: &str) {
    let Some(sheet) = room.sheets.get(player_id).cloned() else {
        return;
    };
    let max_hp = rules::effective_max_hp(&sheet) as i64;
    let hp = sheet::clamp_int(sheet.get("hp").unwrap_or(&Value::Null), 0, max_hp, 0);
    for token in room.tokens.iter_mut() {
        if token.get("assignedPlayerId").and_then(Value::as_str) == Some(player_id) {
            token["maxHp"] = json!(max_hp);
            token["hp"] = json!(hp);
        }
    }
}

fn remove_initiative_entry(room: &mut RoomState, entry_id: &str) -> bool {
    let index = {
        let Some(entries) = room
            .initiative
            .get_mut("entries")
            .and_then(Value::as_array_mut)
        else {
            return false;
        };
        let Some(index) = entries
            .iter()
            .position(|entry| js_string(entry.get("id")) == entry_id)
        else {
            return false;
        };
        entries.remove(index);
        index
    };
    let turn_id = js_string(room.initiative.get("turnId"));
    if turn_id == entry_id {
        let next = room
            .initiative
            .get("entries")
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries
                    .get(index)
                    .or_else(|| index.checked_sub(1).and_then(|i| entries.get(i)))
            })
            .and_then(|entry| entry.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        room.initiative["turnId"] = next;
        for token in room.tokens.iter_mut() {
            token["movedCells"] = json!(0);
        }
    }
    true
}

fn drop_initiative_by_ref(room: &mut RoomState, ref_id: &str) -> usize {
    if ref_id.is_empty() {
        return 0;
    }
    let ids: Vec<String> = room
        .initiative
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter(|entry| entry.get("refId").and_then(Value::as_str) == Some(ref_id))
                .map(|entry| js_string(entry.get("id")))
                .collect()
        })
        .unwrap_or_default();
    let mut removed = 0;
    for id in ids {
        if remove_initiative_entry(room, &id) {
            removed += 1;
        }
    }
    removed
}

fn map_update_payloads(room: &RoomState, kind: &str) -> (Value, Value) {
    let slot = room.maps.get(kind);
    let dm = json!({
        "type": kind,
        "items": slot.and_then(|slot| slot.get("items")).cloned().unwrap_or_else(|| json!([])),
        "activeId": slot.and_then(|slot| slot.get("activeId")).cloned().unwrap_or(Value::Null),
        "activeMap": room.active_map,
        "activeMapId": room.active_map_id
    });
    let visible = room.map_slot_for_viewer(kind, false);
    let players = json!({
        "type": kind,
        "items": visible.get("items").cloned().unwrap_or_else(|| json!([])),
        "activeId": visible.get("activeId").cloned().unwrap_or(Value::Null),
        "activeMap": if room.active_map_id.is_some() { json!(room.active_map) } else { Value::Null },
        "activeMapId": room.active_map_id
    });
    (dm, players)
}

async fn emit_map_update(io: &SocketIo, payloads: (Value, Value)) {
    let _ = io.to("dm-sockets").emit("mapUpdated", &payloads.0).await;
    let _ = io
        .to("table")
        .except("dm-sockets")
        .emit("mapUpdated", &payloads.1)
        .await;
}

/// Opens the fog around a token that just finished its move. Returns whether
/// the map actually changed (its `fogFollow` toggle is on).
fn open_fog_follow(room: &mut RoomState, kind: &str, token_map_id: &str, x: f64, y: f64) -> bool {
    let Some(slot) = room.maps.get_mut(kind) else {
        return false;
    };
    let active_id = slot
        .get("activeId")
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(items) = slot.get_mut("items").and_then(Value::as_array_mut) else {
        return false;
    };
    let by_token = items.iter().position(|item| {
        !token_map_id.is_empty() && item.get("id").and_then(Value::as_str) == Some(token_map_id)
    });
    let index = by_token.or_else(|| {
        active_id.as_deref().and_then(|id| {
            items
                .iter()
                .position(|item| item.get("id").and_then(Value::as_str) == Some(id))
        })
    });
    let Some(index) = index else {
        return false;
    };
    if !truthy(items[index].get("fogFollow")) {
        return false;
    }
    let mask = content::normalize_fog(items[index].get("fogMask").unwrap_or(&Value::Null));
    let opened = rules::open_fog_at(&mask, &json!(x), &json!(y), &json!(FOG_FOLLOW_RADIUS));
    items[index]["fogMask"] = json!(opened);
    true
}

fn group_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn save_error(error: &CampaignError, fallback: &str) -> String {
    match error {
        CampaignError::Quota { message, .. } => message.clone(),
        _ => fallback.to_string(),
    }
}

fn requested_map_type(room: &RoomState, payload: Option<&Value>) -> String {
    let requested = payload.and_then(Value::as_str).unwrap_or("");
    if MAP_TYPES.contains(&requested) {
        return requested.to_string();
    }
    room.active_map
        .clone()
        .unwrap_or_else(|| "battle".to_string())
}

/// Applies a mutation to one token, then sends `tokenUpdated` to the DM and,
/// when the token is visible, to the players. `mutate` returns `false` when the
/// reference would have silently skipped the event (no change).
async fn update_token<F>(
    socket: &SocketRef,
    io: &SocketIo,
    table: &Arc<Table>,
    token_id: &str,
    undo: bool,
    mutate: F,
) where
    F: FnOnce(&mut Value) -> Result<bool, String>,
{
    let payloads = {
        let mut room = table.room.lock().unwrap();
        let Some(index) = token_index(&room, token_id) else {
            drop(room);
            emit_error(socket, "Фишка не найдена");
            return;
        };
        if undo {
            push_undo(&mut room);
        }
        match mutate(&mut room.tokens[index]) {
            Ok(true) => {}
            Ok(false) => return,
            Err(message) => {
                drop(room);
                emit_error(socket, message);
                return;
            }
        }
        let token = room.tokens[index].clone();
        token_payloads(&room, &token, None)
    };
    table.mark_dirty();
    emit_token(io, "tokenUpdated", payloads).await;
}

/// `emitTokenVisibility`: hide goes to everyone (players must drop the token),
/// showing goes to the DM plus `tokenCreated` for players.
async fn set_visibility(io: &SocketIo, table: &Arc<Table>, token_id: &str, hidden: bool) -> bool {
    let created = {
        let mut room = table.room.lock().unwrap();
        let Some(index) = token_index(&room, token_id) else {
            return false;
        };
        let was_hidden = truthy(room.tokens[index].get("hidden"));
        room.tokens[index]["hidden"] = json!(hidden);
        let token = room.tokens[index].clone();
        if !hidden && was_hidden {
            Some(token_payloads(&room, &token, None))
        } else {
            None
        }
    };
    table.mark_dirty();
    if hidden {
        let _ = io
            .emit(
                "tokenHidden",
                &json!({ "tokenId": token_id, "hidden": true }),
            )
            .await;
    } else {
        let _ = io
            .to("dm-sockets")
            .emit(
                "tokenHidden",
                &json!({ "tokenId": token_id, "hidden": false }),
            )
            .await;
        if let Some(payloads) = created {
            emit_token(io, "tokenCreated", payloads).await;
        }
    }
    broadcast_players(io, table).await;
    true
}

// ---------------------------------------------------------------------------
// Register
// ---------------------------------------------------------------------------

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    // --- creation ---------------------------------------------------------
    {
        let table = table.clone();
        socket.on(
            "createTokenFromSheet",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let player_id = js_string(data.get("playerId"));
                    let payloads = {
                        let mut room = table.room.lock().unwrap();
                        let Some(player) = room.players.get(&player_id).cloned() else {
                            drop(room);
                            emit_error(&socket, "Игрок не найден");
                            return;
                        };
                        if player.get("role").and_then(Value::as_str).unwrap_or(ROLE_PLAYER) == ROLE_DM {
                            drop(room);
                            emit_error(&socket, "У мастера нет фишки на поле");
                            return;
                        }
                        if let Some(existing) = room
                            .tokens
                            .iter()
                            .find(|token| token.get("assignedPlayerId").and_then(Value::as_str) == Some(player_id.as_str()))
                        {
                            let name = js_string(existing.get("name"));
                            drop(room);
                            emit_error(&socket, format!("У игрока уже есть фишка «{name}»"));
                            return;
                        }
                        if room.tokens.len() as i64 >= LIMITS.max_tokens {
                            drop(room);
                            emit_error(&socket, "Слишком много фишек на столе");
                            return;
                        }

                        let sheet = room.sheets.get(&player_id).cloned();
                        let name = {
                            let from_sheet = sanitize_name(sheet.as_ref().and_then(|sheet| sheet.get("characterName")).unwrap_or(&Value::Null));
                            if !from_sheet.is_empty() {
                                from_sheet
                            } else {
                                let from_player = sanitize_name(player.get("name").unwrap_or(&Value::Null));
                                if from_player.is_empty() { "Фишка".to_string() } else { from_player }
                            }
                        };
                        let map_type = requested_map_type(&room, data.get("mapType"));
                        let image_url = player
                            .get("avatar")
                            .filter(|value| truthy(Some(value)))
                            .cloned()
                            .unwrap_or(Value::Null);
                        let max_hp = sheet
                            .as_ref()
                            .map(|sheet| json!(rules::effective_max_hp(sheet) as i64))
                            .unwrap_or(Value::Null);
                        let hp = sheet
                            .as_ref()
                            .and_then(|sheet| sheet.get("hp").cloned())
                            .unwrap_or(Value::Null);
                        let color = TOKEN_PALETTE[room.tokens.len() % TOKEN_PALETTE.len()];
                        let raw = json!({
                            "id": content::new_token_id(),
                            "name": name,
                            "mapType": map_type,
                            "mapId": sheet::sanitize_text(data.get("mapId").unwrap_or(&Value::Null), 80),
                            "x": 0.5,
                            "y": 0.5,
                            "imageUrl": image_url,
                            "assignedPlayerId": player_id,
                            "side": "ally",
                            "avatarFromSheet": true,
                            "maxHp": max_hp,
                            "hp": hp,
                            "hidden": true,
                            "color": color,
                            "damageResistances": [],
                            "damageVulnerabilities": [],
                            "damageImmunities": [],
                            "vision": 0
                        });
                        let token = room.normalize_token(&raw);
                        let payloads = token_payloads(&room, &token, None);
                        room.tokens.push(token);
                        payloads
                    };
                    table.mark_dirty();
                    emit_token(&io, "tokenCreated", payloads).await;
                    broadcast_players(&io, &table).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "createToken",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let payloads = {
                        let mut room = table.room.lock().unwrap();
                        if room.tokens.len() as i64 >= LIMITS.max_tokens {
                            drop(room);
                            emit_error(&socket, "Слишком много фишек на столе");
                            return;
                        }
                        let map_type = requested_map_type(&room, data.get("mapType"));
                        let name = {
                            let wanted = sanitize_name(data.get("name").unwrap_or(&Value::Null));
                            if wanted.is_empty() {
                                format!("Фишка {}", room.tokens.len() + 1)
                            } else {
                                wanted
                            }
                        };
                        let armor = data.get("armorClass");
                        let armor_class = match armor {
                            Some(value) => {
                                let raw = number(value);
                                if raw.is_finite() && raw >= 1.0 {
                                    json!(sheet::clamp_int(value, 1, 60, 10))
                                } else {
                                    Value::Null
                                }
                            }
                            None => Value::Null,
                        };
                        let max_hp = data
                            .get("maxHp")
                            .map(number)
                            .filter(|value| value.is_finite() && *value > 0.0)
                            .map(|_| json!(sheet::clamp_int(data.get("maxHp").unwrap_or(&Value::Null), 1, LIMITS.max_token_hp, 1)))
                            .unwrap_or(Value::Null);
                        let color = TOKEN_PALETTE[room.tokens.len() % TOKEN_PALETTE.len()];
                        let raw = json!({
                            "id": content::new_token_id(),
                            "name": name,
                            "mapType": map_type,
                            "mapId": sheet::sanitize_text(data.get("mapId").unwrap_or(&Value::Null), 80),
                            "x": 0.5,
                            "y": 0.5,
                            "imageUrl": Value::Null,
                            "assignedPlayerId": Value::Null,
                            "side": if data.get("side").and_then(Value::as_str) == Some("ally") { "ally" } else { "enemy" },
                            "attackBonus": sheet::clamp_int(data.get("attackBonus").unwrap_or(&Value::Null), -5, 20, 0),
                            "armorClass": armor_class,
                            "hidden": true,
                            "color": color,
                            "size": sheet::clamp_int(data.get("size").unwrap_or(&Value::Null), 1, 4, 1),
                            "maxHp": max_hp,
                            "hp": max_hp,
                            "conditions": [],
                            "light": content::normalize_token_light(&Value::Null),
                            "damageResistances": [],
                            "damageVulnerabilities": [],
                            "damageImmunities": [],
                            "vision": 0
                        });
                        let token = room.normalize_token(&raw);
                        let payloads = token_payloads(&room, &token, None);
                        room.tokens.push(token);
                        payloads
                    };
                    table.mark_dirty();
                    emit_token(&io, "tokenCreated", payloads).await;
                }
            },
        );
    }

    // --- simple token fields ---------------------------------------------
    {
        let table = table.clone();
        socket.on(
            "setTokenSize",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        token["size"] = json!(sheet::clamp_int(
                            data.get("size").unwrap_or(&Value::Null),
                            1,
                            4,
                            1
                        ));
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenSide",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        let side = data.get("side").and_then(Value::as_str).unwrap_or("");
                        if side != "ally" && side != "enemy" {
                            return Err("Неизвестная сторона фишки".to_string());
                        }
                        if token.get("side").and_then(Value::as_str) == Some(side) {
                            return Ok(false);
                        }
                        token["side"] = json!(side);
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenAttackBonus",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        if truthy(token.get("assignedPlayerId")) {
                            return Err("Бонус атаки игрока берётся из его листа".to_string());
                        }
                        token["attackBonus"] = json!(sheet::clamp_int(
                            data.get("attackBonus").unwrap_or(&Value::Null),
                            -5,
                            20,
                            0
                        ));
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenArmorClass",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        if truthy(token.get("assignedPlayerId")) {
                            return Err("Класс брони игрока берётся из его листа".to_string());
                        }
                        token["armorClass"] = match data.get("armorClass") {
                            // A missing field is "unknown" only to the eye; the
                            // reference clamps undefined to the default of 10.
                            None => json!(sheet::clamp_int(&Value::Null, 1, 60, 10)),
                            Some(value) => {
                                let empty = matches!(value, Value::Null)
                                    || matches!(value, Value::String(text) if text.is_empty());
                                if empty || number(value) == 0.0 {
                                    Value::Null
                                } else {
                                    json!(sheet::clamp_int(value, 1, 60, 10))
                                }
                            }
                        };
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenConcentration",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        if truthy(token.get("assignedPlayerId")) {
                            return Err("Концентрация героя живёт в его листе".to_string());
                        }
                        let value = sheet::sanitize_text(
                            data.get("concentration").unwrap_or(&Value::Null),
                            LIMITS.max_concentration_length,
                        )
                        .trim()
                        .to_string();
                        if value.is_empty() {
                            if let Some(object) = token.as_object_mut() {
                                object.remove("concentration");
                            }
                        } else {
                            token["concentration"] = json!(value);
                        }
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenNameHidden",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        let hidden = !matches!(data.get("hidden"), Some(Value::Bool(false)));
                        if truthy(token.get("hideName")) == hidden {
                            return Ok(false);
                        }
                        token["hideName"] = json!(hidden);
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenLabelsHidden",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        let hidden = !matches!(data.get("hidden"), Some(Value::Bool(false)));
                        if truthy(token.get("hideLabels")) == hidden {
                            return Ok(false);
                        }
                        token["hideLabels"] = json!(hidden);
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "renameToken",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        let name = sanitize_name(data.get("name").unwrap_or(&Value::Null));
                        if name.is_empty() {
                            return Err("Имя фишки не может быть пустым".to_string());
                        }
                        token["name"] = json!(name);
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenLight",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        let radius = data.get("radius").map(number).unwrap_or(f64::NAN);
                        if !radius.is_finite() || radius < 0.0 {
                            return Err("Дальность света задана неверно".to_string());
                        }
                        let color = data
                            .get("color")
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| {
                                token
                                    .get("light")
                                    .and_then(|light| light.get("color"))
                                    .and_then(Value::as_str)
                                    .map(str::to_string)
                            })
                            .unwrap_or_else(|| "#ffb347".to_string());
                        token["light"] = content::normalize_token_light(
                            &json!({ "radius": radius, "color": color }),
                        );
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenDefenses",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, false, |token| {
                        token["damageResistances"] = content::normalize_damage_defenses(
                            data.get("resistances").unwrap_or(&Value::Null),
                        );
                        token["damageVulnerabilities"] = content::normalize_damage_defenses(
                            data.get("vulnerabilities").unwrap_or(&Value::Null),
                        );
                        token["damageImmunities"] = content::normalize_damage_defenses(
                            data.get("immunities").unwrap_or(&Value::Null),
                        );
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenConditions",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, true, |token| {
                        let conditions = content::normalize_conditions(
                            data.get("conditions").unwrap_or(&Value::Null),
                        );
                        let durations = content::normalize_condition_durations(
                            data.get("conditionDurations").unwrap_or(&Value::Null),
                            &conditions,
                        );
                        let units = content::normalize_condition_units(
                            data.get("conditionDurationUnits").unwrap_or(&Value::Null),
                            &conditions,
                            &durations,
                        );
                        token["conditions"] = conditions;
                        token["conditionDurations"] = durations;
                        token["conditionDurationUnits"] = units;
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenAura",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, true, |token| {
                        let feet = data.get("feet").cloned().unwrap_or(Value::Null);
                        let color = data.get("color").cloned().unwrap_or(Value::Null);
                        token["aura"] =
                            content::normalize_aura(&json!({ "feet": feet, "color": color }));
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenVision",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    update_token(&socket, &io, &table, &token_id, true, |token| {
                        token["vision"] =
                            content::normalize_vision(data.get("vision").unwrap_or(&Value::Null));
                        Ok(true)
                    })
                    .await;
                }
            },
        );
    }

    // --- emote ------------------------------------------------------------
    {
        let table = table.clone();
        socket.on(
            "setTokenEmote",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let token_id = js_string(data.get("tokenId"));
                    let is_dm = session.role == ROLE_DM;
                    let (payloads, has_emote) = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            drop(room);
                            emit_error(&socket, "Фишка не найдена");
                            return;
                        };
                        let owner = js_string(room.tokens[index].get("assignedPlayerId"));
                        if !is_dm && owner != session.player_id {
                            drop(room);
                            emit_error(&socket, "Эмоут ставит владелец фишки или мастер");
                            return;
                        }
                        let text = sheet::sanitize_text(
                            data.get("text").unwrap_or(&Value::Null),
                            LIMITS.max_token_emote_length,
                        )
                        .trim()
                        .to_string();
                        if text.is_empty() {
                            if let Some(object) = room.tokens[index].as_object_mut() {
                                object.remove("emote");
                            }
                        } else {
                            let until = (now_ms() + EMOTE_TTL_MS) as i64;
                            room.tokens[index]["emote"] = json!({ "text": text, "until": until });
                        }
                        let token = room.tokens[index].clone();
                        (token_payloads(&room, &token, None), !text.is_empty())
                    };
                    emit_token(&io, "tokenUpdated", payloads).await;
                    if !has_emote {
                        return;
                    }
                    // The bubble is not persisted: it expires on its own.
                    let timer_table = table.clone();
                    let timer_io = io.clone();
                    let timer_token = token_id.clone();
                    let _ = tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(6200)).await;
                        let payloads = {
                            let mut room = timer_table.room.lock().unwrap();
                            let Some(index) = token_index(&room, &timer_token) else {
                                return;
                            };
                            let Some(until) = room.tokens[index]
                                .get("emote")
                                .and_then(|emote| emote.get("until"))
                                .map(number)
                            else {
                                return;
                            };
                            if until > now_ms() as f64 {
                                return;
                            }
                            if let Some(object) = room.tokens[index].as_object_mut() {
                                object.remove("emote");
                            }
                            let token = room.tokens[index].clone();
                            token_payloads(&room, &token, None)
                        };
                        timer_table.mark_dirty();
                        emit_token(&timer_io, "tokenUpdated", payloads).await;
                    });
                }
            },
        );
    }

    // --- visibility -------------------------------------------------------
    {
        let table = table.clone();
        socket.on(
            "setTokenHidden",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    let hidden = !matches!(data.get("hidden"), Some(Value::Bool(false)));
                    if !set_visibility(&io, &table, &token_id, hidden).await {
                        emit_error(&socket, "Фишка не найдена");
                    }
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "tokensBatch",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let mut clean: Vec<(String, Value, String)> = Vec::new();
                    {
                        let room = table.room.lock().unwrap();
                        let actions: Vec<Value> = data
                            .get("actions")
                            .and_then(Value::as_array)
                            .map(|items| {
                                items
                                    .iter()
                                    .take(LIMITS.max_tokens as usize)
                                    .cloned()
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut seen: HashSet<String> = HashSet::new();
                        for action in actions {
                            let kind = action.get("kind").and_then(Value::as_str).unwrap_or("");
                            if kind != "hidden" && kind != "delete" && kind != "side" {
                                continue;
                            }
                            let token_id = js_string(action.get("tokenId"));
                            if token_index(&room, &token_id).is_none() || seen.contains(&token_id) {
                                continue;
                            }
                            seen.insert(token_id.clone());
                            clean.push((
                                kind.to_string(),
                                action.get("value").cloned().unwrap_or(Value::Null),
                                token_id,
                            ));
                        }
                    }
                    if clean.is_empty() {
                        emit_error(&socket, "Нет фишек для групповой операции");
                        return;
                    }
                    push_undo(&mut table.room.lock().unwrap());
                    let mut removed = false;
                    for (kind, value, token_id) in clean {
                        if kind == "hidden" {
                            let hidden = !matches!(value, Value::Bool(false));
                            set_visibility(&io, &table, &token_id, hidden).await;
                        } else if kind == "side" {
                            update_token(&socket, &io, &table, &token_id, false, |token| {
                                token["side"] = json!(if value.as_str() == Some("ally") {
                                    "ally"
                                } else {
                                    "enemy"
                                });
                                Ok(true)
                            })
                            .await;
                        } else {
                            let (payloads, initiative) = {
                                let mut room = table.room.lock().unwrap();
                                let Some(index) = token_index(&room, &token_id) else {
                                    continue;
                                };
                                let token = room.tokens.remove(index);
                                let dropped = drop_initiative_by_ref(&mut room, &token_id);
                                let initiative = if dropped > 0 {
                                    Some(room.initiative.clone())
                                } else {
                                    None
                                };
                                let payloads = token_payloads(
                                    &room,
                                    &token,
                                    Some(json!({ "tokenId": token_id })),
                                );
                                (payloads, initiative)
                            };
                            table.mark_dirty();
                            if let Some(initiative) = initiative {
                                let _ = io.emit("initiativeUpdated", &initiative).await;
                            }
                            emit_token(&io, "tokenDeleted", payloads).await;
                            removed = true;
                        }
                    }
                    table.mark_dirty();
                    if removed {
                        broadcast_players(&io, &table).await;
                    }
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setAllTokensHidden",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let hidden = !matches!(data.get("hidden"), Some(Value::Bool(false)));
                    let side = data
                        .get("side")
                        .and_then(Value::as_str)
                        .filter(|side| *side == "ally" || *side == "enemy")
                        .map(str::to_string);
                    let changed: Vec<String> = {
                        let room = table.room.lock().unwrap();
                        room.tokens
                            .iter()
                            .filter(|token| {
                                side.as_deref()
                                    .map(|side| {
                                        token.get("side").and_then(Value::as_str) == Some(side)
                                    })
                                    .unwrap_or(true)
                            })
                            .filter(|token| truthy(token.get("hidden")) != hidden)
                            .map(|token| js_string(token.get("id")))
                            .collect()
                    };
                    if changed.is_empty() {
                        return;
                    }
                    for token_id in changed {
                        set_visibility(&io, &table, &token_id, hidden).await;
                    }
                }
            },
        );
    }

    // --- assignment, images, DM note -------------------------------------
    {
        let table = table.clone();
        socket.on(
            "assignTokenToPlayer",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    let raw = data.get("playerId");
                    let payloads = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            drop(room);
                            emit_error(&socket, "Фишка не найдена");
                            return;
                        };
                        let assign = match raw {
                            None | Some(Value::Null) => None,
                            Some(Value::String(text)) if text.is_empty() => None,
                            Some(value) => {
                                let player_id = js_string(Some(value));
                                if !room.players.contains_key(&player_id) {
                                    drop(room);
                                    emit_error(&socket, "Игрок не найден");
                                    return;
                                }
                                Some(player_id)
                            }
                        };
                        match assign {
                            None => {
                                room.tokens[index]["assignedPlayerId"] = Value::Null;
                            }
                            Some(player_id) => {
                                room.tokens[index]["assignedPlayerId"] = json!(player_id);
                                room.tokens[index]["side"] = json!("ally");
                                room.tokens[index]["armorClass"] = Value::Null;
                                room.tokens[index]["attackBonus"] = json!(0);
                                sync_sheet_tokens(&mut room, &player_id);
                            }
                        }
                        let token = room.tokens[index].clone();
                        token_payloads(&room, &token, None)
                    };
                    table.mark_dirty();
                    emit_token(&io, "tokenUpdated", payloads).await;
                    broadcast_players(&io, &table).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenImage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let token_id = js_string(data.get("tokenId"));
                    let (is_dm, owner) = {
                        let room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            drop(room);
                            emit_error(&socket, "Фишка не найдена");
                            return;
                        };
                        (
                            session.role == ROLE_DM,
                            js_string(room.tokens[index].get("assignedPlayerId")),
                        )
                    };
                    if !is_dm && owner != session.player_id {
                        emit_error(&socket, "Можно менять изображение только своей фишки");
                        return;
                    }
                    let decoded = match decode_image(
                        data.get("image").and_then(Value::as_str),
                        LIMITS.max_image_bytes as usize,
                    ) {
                        Ok(decoded) => decoded,
                        Err(message) => {
                            emit_error(&socket, message);
                            return;
                        }
                    };
                    let Some(store) = table.store.clone() else {
                        emit_error(&socket, "Не удалось сохранить изображение фишки");
                        return;
                    };
                    let Some(campaign_id) = table.campaign_id() else {
                        emit_error(&socket, "Не удалось сохранить изображение фишки");
                        return;
                    };
                    let file_name =
                        match store.save_image(&campaign_id, &decoded.bytes, decoded.ext) {
                            Ok(name) => name,
                            Err(error) => {
                                emit_error(
                                    &socket,
                                    save_error(&error, "Не удалось сохранить изображение фишки"),
                                );
                                return;
                            }
                        };
                    let url = format!("/files/{file_name}");
                    let payloads = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            return;
                        };
                        room.tokens[index]["imageUrl"] = json!(url);
                        reset_image_crop(&mut room.tokens[index]);
                        room.tokens[index]["avatarFromSheet"] = json!(false);
                        let token = room.tokens[index].clone();
                        token_payloads(&room, &token, None)
                    };
                    table.mark_dirty();
                    emit_token(&io, "tokenUpdated", payloads).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setTokenGroupImage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let wanted = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        LIMITS.max_name_length,
                    );
                    if wanted.is_empty() {
                        emit_error(&socket, "Не указано название группы");
                        return;
                    }
                    let key = group_key(&wanted);
                    let decoded = match decode_image(
                        data.get("image").and_then(Value::as_str),
                        LIMITS.max_image_bytes as usize,
                    ) {
                        Ok(decoded) => decoded,
                        Err(message) => {
                            emit_error(&socket, message);
                            return;
                        }
                    };
                    if !table
                        .room
                        .lock()
                        .unwrap()
                        .tokens
                        .iter()
                        .any(|token| group_key(&js_string(token.get("name"))) == key)
                    {
                        emit_error(&socket, "Фишек с таким названием нет");
                        return;
                    }
                    let Some(store) = table.store.clone() else {
                        emit_error(&socket, "Не получилось сохранить картинку");
                        return;
                    };
                    let Some(campaign_id) = table.campaign_id() else {
                        emit_error(&socket, "Не получилось сохранить картинку");
                        return;
                    };
                    let file_name =
                        match store.save_image(&campaign_id, &decoded.bytes, decoded.ext) {
                            Ok(name) => name,
                            Err(error) => {
                                emit_error(
                                    &socket,
                                    save_error(&error, "Не получилось сохранить картинку"),
                                );
                                return;
                            }
                        };
                    let url = format!("/files/{file_name}");
                    let (group, visible) = {
                        let mut room = table.room.lock().unwrap();
                        for token in room.tokens.iter_mut() {
                            if group_key(&js_string(token.get("name"))) == key {
                                token["imageUrl"] = json!(url);
                                reset_image_crop(token);
                            }
                        }
                        let group: Vec<Value> = room
                            .tokens
                            .iter()
                            .filter(|token| group_key(&js_string(token.get("name"))) == key)
                            .cloned()
                            .collect();
                        let visible: Vec<Value> = group
                            .iter()
                            .filter(|token| room.is_token_visible_to_players(token))
                            .map(|token| room.token_for_viewer(token, false))
                            .collect();
                        (group, visible)
                    };
                    table.mark_dirty();
                    let _ = io
                        .to("dm-sockets")
                        .emit("tokensUpdated", &json!({ "tokens": group }))
                        .await;
                    if !visible.is_empty() {
                        let _ = io
                            .except("dm-sockets")
                            .emit("tokensUpdated", &json!({ "tokens": visible }))
                            .await;
                    }
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setDmNote",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let player_id = js_string(data.get("playerId"));
                    let note = sheet::sanitize_text(
                        data.get("note").unwrap_or(&Value::Null),
                        LIMITS.max_dm_note,
                    );
                    let found = {
                        let mut room = table.room.lock().unwrap();
                        match room.players.get_mut(&player_id) {
                            Some(player) => {
                                player["dmNote"] = json!(note);
                                true
                            }
                            None => false,
                        }
                    };
                    if !found {
                        emit_error(&socket, "Игрок не найден");
                        return;
                    }
                    table.mark_dirty();
                    let _ = io
                        .to("dm-sockets")
                        .emit(
                            "dmNoteUpdated",
                            &json!({ "playerId": player_id, "note": note }),
                        )
                        .await;
                }
            },
        );
    }

    // --- movement ---------------------------------------------------------
    {
        let table = table.clone();
        socket.on(
            "moveToken",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let token_id = js_string(data.get("tokenId"));
                    let is_dm = session.role == ROLE_DM;
                    let (payloads, map_payloads) = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            drop(room);
                            emit_error(&socket, "Фишка не найдена");
                            return;
                        };
                        let owner = js_string(room.tokens[index].get("assignedPlayerId"));
                        if !is_dm && owner != session.player_id {
                            drop(room);
                            emit_error(&socket, "Можно двигать только свою фишку");
                            return;
                        }
                        if !is_dm {
                            let conditions = room.tokens[index]
                                .get("conditions")
                                .cloned()
                                .unwrap_or(Value::Null);
                            let blocker = rules::condition_blocks_move(&conditions);
                            if !blocker.is_null() {
                                let label = js_string(blocker.get("label")).to_lowercase();
                                drop(room);
                                emit_error(
                                    &socket,
                                    format!("Фишка «{label}»: двигать её нельзя, пока состояние не снято"),
                                );
                                return;
                            }
                            if let Some(sheet) = room.sheets.get(&session.player_id) {
                                let multiplier = rules::exhaustion_effects(
                                    sheet.get("exhaustion").unwrap_or(&Value::Null),
                                )
                                .get("speedMultiplier")
                                .and_then(Value::as_f64)
                                .unwrap_or(1.0);
                                if multiplier == 0.0 {
                                    drop(room);
                                    emit_error(&socket, "Истощение 5: скорость 0, двигать фишку нельзя");
                                    return;
                                }
                            }
                        }

                        let old_x = room.tokens[index].get("x").map(number).unwrap_or(0.5);
                        let old_y = room.tokens[index].get("y").map(number).unwrap_or(0.5);
                        let x = sheet::clamp_number(data.get("x").unwrap_or(&Value::Null), 0.0, 1.0, old_x);
                        let y = sheet::clamp_number(data.get("y").unwrap_or(&Value::Null), 0.0, 1.0, old_y);
                        if x == old_x && y == old_y {
                            return;
                        }
                        push_undo(&mut room);
                        room.tokens[index]["x"] = json!(x);
                        room.tokens[index]["y"] = json!(y);

                        let final_move = truthy(data.get("final"));
                        let mut meta = json!({ "tokenId": token_id, "x": x, "y": y });
                        if final_move {
                            meta["final"] = json!(true);
                        }
                        if let Some(from) = data.get("from") {
                            let col = from.get("col").map(number).unwrap_or(f64::NAN);
                            let row = from.get("row").map(number).unwrap_or(f64::NAN);
                            if col.is_finite() && row.is_finite() {
                                meta["from"] = json!({
                                    "col": col.round() as i64,
                                    "row": row.round() as i64
                                });
                            }
                        }
                        if let Some(path) = data.get("path").and_then(Value::as_array) {
                            let cells: Vec<Value> = path
                                .iter()
                                .take(200)
                                .filter_map(|cell| {
                                    let col = cell.get("col").map(number).unwrap_or(f64::NAN);
                                    let row = cell.get("row").map(number).unwrap_or(f64::NAN);
                                    if col.is_finite() && row.is_finite() {
                                        Some(json!({
                                            "col": col.round() as i64,
                                            "row": row.round() as i64
                                        }))
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if cells.len() > 1 {
                                meta["path"] = json!(cells);
                            }
                        }
                        let path_present = meta.get("path").is_some();
                        if final_move && path_present {
                            let steps = meta["path"]
                                .as_array()
                                .map(|cells| cells.len().saturating_sub(1))
                                .unwrap_or(0) as f64;
                            let spent_before = room.tokens[index]
                                .get("movedCells")
                                .map(number)
                                .filter(|value| value.is_finite())
                                .unwrap_or(0.0);
                            let moved = (spent_before + steps).min(1000.0) as i64;
                            room.tokens[index]["movedCells"] = json!(moved);
                            meta["spent"] = json!(steps as i64);
                            meta["movedCells"] = json!(moved);
                        }

                        let token = room.tokens[index].clone();
                        let map_type = js_string(token.get("mapType"));
                        let token_map_id = js_string(token.get("mapId"));
                        let assigned = truthy(token.get("assignedPlayerId"));
                        let mut map_payloads = None;
                        if final_move && assigned && room.is_token_visible_to_players(&token) {
                            if open_fog_follow(&mut room, &map_type, &token_map_id, x, y) {
                                map_payloads = Some(map_update_payloads(&room, &map_type));
                            }
                        }
                        (token_payloads(&room, &token, Some(meta)), map_payloads)
                    };
                    table.mark_dirty();
                    if let Some(map_payloads) = map_payloads {
                        emit_map_update(&io, map_payloads).await;
                    }
                    emit_token(&io, "tokenMoved", payloads).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "moveTokenToActiveMap",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    let payloads = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            drop(room);
                            emit_error(&socket, "Фишка не найдена");
                            return;
                        };
                        let (Some(active_map), Some(active_map_id)) =
                            (room.active_map.clone(), room.active_map_id.clone())
                        else {
                            drop(room);
                            emit_error(&socket, "Сначала покажите карту игрокам");
                            return;
                        };
                        if !room.is_map_published(&active_map, Some(&active_map_id)) {
                            drop(room);
                            emit_error(&socket, "Эта карта ещё не показана игрокам");
                            return;
                        }
                        let same_map = room.tokens[index].get("mapType").and_then(Value::as_str)
                            == Some(active_map.as_str())
                            && (js_string(room.tokens[index].get("mapId")).is_empty()
                                || room.tokens[index].get("mapId").and_then(Value::as_str)
                                    == Some(active_map_id.as_str()));
                        if same_map {
                            drop(room);
                            emit_error(&socket, "Фишка уже на этой карте");
                            return;
                        }
                        push_undo(&mut room);
                        room.tokens[index]["mapType"] = json!(active_map);
                        room.tokens[index]["mapId"] = json!(active_map_id);
                        room.tokens[index]["x"] = json!(0.5);
                        room.tokens[index]["y"] = json!(0.5);
                        let token = room.tokens[index].clone();
                        token_payloads(&room, &token, None)
                    };
                    table.mark_dirty();
                    emit_token(&io, "tokenCreated", payloads).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "moveAllTokensToActiveMap",
            move |socket: SocketRef, io: SocketIo, _data: Data<Value>| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let tokens = {
                        let mut room = table.room.lock().unwrap();
                        let (Some(active_map), Some(active_map_id)) =
                            (room.active_map.clone(), room.active_map_id.clone())
                        else {
                            drop(room);
                            emit_error(&socket, "Сначала покажите карту игрокам");
                            return;
                        };
                        if !room.is_map_published(&active_map, Some(&active_map_id)) {
                            drop(room);
                            emit_error(&socket, "Эта карта ещё не показана игрокам");
                            return;
                        }
                        let indices: Vec<usize> = room
                            .tokens
                            .iter()
                            .enumerate()
                            .filter(|(_, token)| {
                                !(token.get("mapType").and_then(Value::as_str)
                                    == Some(active_map.as_str())
                                    && (js_string(token.get("mapId")).is_empty()
                                        || token.get("mapId").and_then(Value::as_str)
                                            == Some(active_map_id.as_str())))
                            })
                            .map(|(index, _)| index)
                            .collect();
                        if indices.is_empty() {
                            drop(room);
                            emit_error(&socket, "Все фишки уже на этой карте");
                            return;
                        }
                        push_undo(&mut room);
                        let mut payloads = Vec::new();
                        for (index, position) in indices.iter().enumerate() {
                            room.tokens[*position]["mapType"] = json!(active_map);
                            room.tokens[*position]["mapId"] = json!(active_map_id);
                            room.tokens[*position]["x"] =
                                json!(0.5 + ((index % 3) as f64 - 1.0) * 0.04);
                            room.tokens[*position]["y"] = json!(0.5 + (index / 3) as f64 * 0.05);
                            let token = room.tokens[*position].clone();
                            payloads.push(token_payloads(&room, &token, None));
                        }
                        payloads
                    };
                    table.mark_dirty();
                    for payloads in tokens {
                        emit_token(&io, "tokenCreated", payloads).await;
                    }
                    broadcast_players(&io, &table).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "deleteToken",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let token_id = js_string(data.get("tokenId"));
                    let (payloads, initiative) = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = token_index(&room, &token_id) else {
                            drop(room);
                            emit_error(&socket, "Фишка не найдена");
                            return;
                        };
                        let token = room.tokens.remove(index);
                        let dropped = drop_initiative_by_ref(&mut room, &token_id);
                        let initiative = if dropped > 0 {
                            Some(room.initiative.clone())
                        } else {
                            None
                        };
                        let payloads =
                            token_payloads(&room, &token, Some(json!({ "tokenId": token_id })));
                        (payloads, initiative)
                    };
                    table.mark_dirty();
                    if let Some(initiative) = initiative {
                        let _ = io.emit("initiativeUpdated", &initiative).await;
                    }
                    emit_token(&io, "tokenDeleted", payloads).await;
                    broadcast_players(&io, &table).await;
                }
            },
        );
    }
}
