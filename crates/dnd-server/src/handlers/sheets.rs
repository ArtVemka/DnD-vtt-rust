//! Character sheets, inventory, coins, spells, roll requests.
//!
//! Port of the sheet/avatar/inventory handlers from `server/socket.js` and the
//! matching `RoomState` methods in `server/state.js`. Sheet token sync lives
//! here because the reference has it on the room; this module owns the callers.

use std::sync::Arc;

use dnd_core::campaign::CampaignError;
use dnd_core::content;
use dnd_core::media::decode_image;
use dnd_core::room::{ROLE_DM, ROLE_PLAYER, RoomState};
use dnd_core::rules;
use dnd_core::sheet::{self, COIN_KEYS, LIMITS};
use rand::Rng;
use serde_json::{Map, Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{AckSender, Data, SocketRef};
use uuid::Uuid;

use super::{broadcast_players, require_dm, require_joined};
use crate::auth::sanitize_name;
use crate::table::Table;

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    {
        let table = table.clone();
        socket.on(
            "saveCharacterSheet",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move { handle_save_character_sheet(socket, io, table, data).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "castSpell",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move { handle_cast_spell(socket, io, table, data).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on("rollDeathSave", move |socket: SocketRef, io: SocketIo| {
            let table = table.clone();
            async move { handle_roll_death_save(socket, io, table).await }
        });
    }
    {
        let table = table.clone();
        socket.on("resetDeathSaves", move |socket: SocketRef, io: SocketIo| {
            let table = table.clone();
            async move { handle_reset_death_saves(socket, io, table).await }
        });
    }
    {
        let table = table.clone();
        socket.on(
            "requestCharacterSheet",
            move |socket: SocketRef, Data::<Value>(data)| {
                let table = table.clone();
                async move { handle_request_character_sheet(socket, table, data).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setPlayerName",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move { handle_set_player_name(socket, io, table, data).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setAvatar",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move { handle_set_avatar(socket, io, table, data).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "saveInventoryItem",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move { handle_save_inventory_item(socket, io, table, data, ack).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "deleteInventoryItem",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move { handle_delete_inventory_item(socket, io, table, data, ack).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "setInventoryCoins",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move { handle_set_inventory_coins(socket, io, table, data, ack).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "splitInventoryCoins",
            move |socket: SocketRef, io: SocketIo, ack: AckSender| {
                let table = table.clone();
                async move { handle_split_inventory_coins(socket, io, table, ack).await }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "giveInventoryCoins",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move { handle_give_inventory_coins(socket, io, table, data, ack).await }
            },
        );
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn emit_error(socket: &SocketRef, message: &str) {
    let _ = socket.emit("errorMessage", &json!({ "message": message }));
}

fn ack_error(ack: AckSender, message: &str) {
    let _ = ack.send(&json!({ "ok": false, "error": message }));
}

/// `String(value || '')` for the loose ids the reference reads.
fn loose_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(_) => String::new(),
    }
}

/// `clampInt(value, ...)` keeps `undefined` distinct from `null`: a missing
/// field falls back, an explicit `null` is `Number(null) = 0`.
fn clamp_int_opt(value: Option<&Value>, min: i64, max: i64, fallback: i64) -> i64 {
    match value {
        Some(value) => sheet::clamp_int(value, min, max, fallback),
        None => fallback,
    }
}

fn clamp_int(value: &Value, min: i64, max: i64, fallback: i64) -> i64 {
    sheet::clamp_int(value, min, max, fallback)
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 is ascii")
}

/// `new Date().toISOString()`. dnd-server has no date crate, so the civil date
/// comes from the days-since-epoch algorithm.
fn now_iso() -> String {
    let millis = now_millis() as i64;
    let seconds = millis.div_euclid(1000);
    let millis_part = millis.rem_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (
        day_seconds / 3600,
        (day_seconds % 3600) / 60,
        day_seconds % 60,
    );

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let year0 = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year0 + 1 } else { year0 };

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis_part:03}Z")
}

/// `msg-<base36 date>-<4 random bytes>`.
fn new_message_id() -> String {
    let hex = Uuid::new_v4().simple().to_string();
    format!("msg-{}-{}", base36(now_millis()), &hex[..8])
}

/// One token's two broadcast payloads, captured while the room is locked.
struct TokenPayload {
    visible: bool,
    dm: Value,
    players: Value,
}

impl TokenPayload {
    fn new(room: &RoomState, token: &Value) -> Self {
        Self {
            visible: room.is_token_visible_to_players(token),
            dm: room.token_for_viewer(token, true),
            players: room.token_for_viewer(token, false),
        }
    }
}

/// `emitToken`: DM always, players only when the token is visible to them.
async fn emit_token(io: &SocketIo, event: &str, payload: TokenPayload) {
    let _ = io.within("dm-sockets").emit(event, &payload.dm).await;
    if payload.visible {
        let _ = io
            .within("table")
            .except("dm-sockets")
            .emit(event, &payload.players)
            .await;
    }
}

/// `syncSheetTokens`: sheet is the only source of a character's health.
fn sync_sheet_tokens(room: &mut RoomState, player_id: &str) -> Vec<Value> {
    let Some(sheet) = room.sheets.get(player_id).cloned() else {
        return Vec::new();
    };
    let max_hp = rules::effective_max_hp(&sheet);
    let hp = clamp_int(sheet.get("hp").unwrap_or(&Value::Null), 0, max_hp as i64, 0);
    let mut updated = Vec::new();
    for token in room.tokens.iter_mut() {
        if token.get("assignedPlayerId").and_then(Value::as_str) != Some(player_id) {
            continue;
        }
        token["maxHp"] = json!(max_hp as i64);
        token["hp"] = json!(hp);
        updated.push(token.clone());
    }
    updated
}

fn payloads_of(room: &RoomState, tokens: &[Value]) -> Vec<TokenPayload> {
    tokens
        .iter()
        .map(|token| TokenPayload::new(room, token))
        .collect()
}

/// `applyAvatar`: the sheet-derived token picture follows the player's avatar.
fn apply_avatar(room: &mut RoomState, player_id: &str, url: Option<&str>) -> Vec<TokenPayload> {
    if !room.players.contains_key(player_id) {
        return Vec::new();
    }
    let avatar = url.map(|text| json!(text)).unwrap_or(Value::Null);
    if let Some(player) = room.players.get_mut(player_id) {
        player["avatar"] = avatar.clone();
    }
    let mut updated = Vec::new();
    for token in room.tokens.iter_mut() {
        let from_sheet = token
            .get("avatarFromSheet")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !from_sheet || token.get("assignedPlayerId").and_then(Value::as_str) != Some(player_id) {
            continue;
        }
        token["imageUrl"] = avatar.clone();
        token["imageZoom"] = json!(1);
        token["imageX"] = json!(0);
        token["imageY"] = json!(0);
        updated.push(token.clone());
    }
    payloads_of(room, &updated)
}

fn sheet_payload(player_id: &str, sheet: &Value) -> Value {
    json!({ "playerId": player_id, "sheet": sheet })
}

// ---------------------------------------------------------------------------
// Character sheets
// ---------------------------------------------------------------------------

async fn handle_save_character_sheet(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    if session.role != ROLE_PLAYER {
        emit_error(&socket, "ДМ не редактирует листы персонажей");
        return;
    }
    let player_id = session.player_id.clone();

    let (payload, first_sheet, tokens) = {
        let mut room = table.room.lock().unwrap();
        let fallback = room
            .players
            .get(&player_id)
            .and_then(|player| player.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let first_sheet = !room.sheets.contains_key(&player_id);
        let sheet = sheet::normalize_sheet(data.get("sheet").unwrap_or(&Value::Null), &fallback);
        room.sheets.insert(player_id.clone(), sheet);
        let tokens = sync_sheet_tokens(&mut room, &player_id);
        let payload = sheet_payload(&player_id, &room.sheets[&player_id]);
        let payloads = payloads_of(&room, &tokens);
        (payload, first_sheet, payloads)
    };

    table.mark_dirty();
    let _ = io
        .within(player_id.clone())
        .emit("characterSheetData", &payload)
        .await;
    let _ = io
        .within("dm-sockets")
        .emit("characterSheetUpdated", &payload)
        .await;
    for token in tokens {
        emit_token(&io, "tokenUpdated", token).await;
    }
    if first_sheet {
        broadcast_players(&io, &table).await;
    }
}

async fn handle_cast_spell(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    if session.role != ROLE_PLAYER {
        emit_error(&socket, "Каст заклинания идёт от лица игрока");
        return;
    }
    let player_id = session.player_id.clone();

    let prepared = {
        let mut room = table.room.lock().unwrap();
        cast_spell_locked(&mut room, &player_id, &data)
    };
    let (message, sheet) = match prepared {
        Ok(value) => value,
        Err(error) => {
            emit_error(&socket, &error);
            return;
        }
    };

    table.mark_dirty();
    let _ = io.emit("chatMessage", &message).await;
    let payload = sheet_payload(&player_id, &sheet);
    let _ = io
        .within(player_id)
        .emit("characterSheetData", &payload)
        .await;
    let _ = io
        .within("dm-sockets")
        .emit("characterSheetUpdated", &payload)
        .await;
}

fn cast_spell_locked(
    room: &mut RoomState,
    player_id: &str,
    data: &Value,
) -> Result<(Value, Value), String> {
    let Some(sheet) = room.sheets.get(player_id).cloned() else {
        return Err("Лист персонажа ещё не готов".to_string());
    };
    let spell_id = loose_string(data.get("spellId"));
    let spell = sheet
        .get("spells")
        .and_then(Value::as_array)
        .and_then(|list| {
            list.iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(spell_id.as_str()))
        })
        .cloned();
    let Some(spell) = spell else {
        return Err("Заклинание не найдено в листе".to_string());
    };

    let spell_level = clamp_int(spell.get("level").unwrap_or(&Value::Null), 0, 9, 0);
    let level = clamp_int_opt(data.get("slotLevel"), 0, 9, spell_level);
    let mut sheet = sheet;
    if level > 0 {
        let index = (level - 1) as usize;
        let used = clamp_int(
            sheet
                .get("spellSlotsUsed")
                .and_then(Value::as_array)
                .and_then(|slots| slots.get(index))
                .unwrap_or(&Value::Null),
            0,
            LIMITS.max_spell_slots_per_level,
            0,
        );
        let total = clamp_int(
            sheet
                .get("spellSlotsMax")
                .and_then(Value::as_array)
                .and_then(|slots| slots.get(index))
                .unwrap_or(&Value::Null),
            0,
            LIMITS.max_spell_slots_per_level,
            0,
        );
        if total == 0 || used >= total {
            return Err(format!("Нет свободной ячейки {level} уровня"));
        }
        if let Some(slot) = sheet
            .get_mut("spellSlotsUsed")
            .and_then(Value::as_array_mut)
            .and_then(|slots| slots.get_mut(index))
        {
            *slot = json!(used + 1);
        }
        room.sheets.insert(player_id.to_string(), sheet.clone());
    }

    let player = room.players.get(player_id);
    let player_name = player
        .and_then(|value| value.get("name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let avatar = player
        .and_then(|value| value.get("avatar"))
        .cloned()
        .unwrap_or(Value::Null);
    let character_name = sheet
        .get("characterName")
        .and_then(Value::as_str)
        .unwrap_or("");
    let spell_name = spell.get("name").and_then(Value::as_str).unwrap_or("");
    let suffix = if level > 0 {
        format!(", ячейка {level} уровня")
    } else {
        " — заговор".to_string()
    };
    let speaker = if character_name.is_empty() {
        player_name.as_str()
    } else {
        character_name
    };
    let message = json!({
        "id": new_message_id(),
        "from": player_name,
        "playerId": player_id,
        "role": ROLE_PLAYER,
        "avatar": avatar,
        "text": format!("{speaker}: «{spell_name}»{suffix}"),
        "time": now_iso()
    });
    room.chat_history.push(message.clone());
    if room.chat_history.len() > LIMITS.max_chat_history as usize {
        let excess = room.chat_history.len() - LIMITS.max_chat_history as usize;
        room.chat_history.drain(..excess);
    }
    Ok((message, sheet))
}

struct DeathSaveResult {
    sheet: Value,
    tokens: Vec<TokenPayload>,
    roll: Value,
}

async fn handle_roll_death_save(socket: SocketRef, io: SocketIo, table: Arc<Table>) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    if session.role != ROLE_PLAYER {
        emit_error(&socket, "Спасбросок смерти бросает сам игрок");
        return;
    }
    let player_id = session.player_id.clone();

    let result = {
        let mut room = table.room.lock().unwrap();
        roll_death_save_locked(&mut room, &player_id)
    };
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            emit_error(&socket, &error);
            return;
        }
    };

    table.mark_dirty();
    let payload = sheet_payload(&player_id, &result.sheet);
    let _ = io
        .within(player_id)
        .emit("characterSheetData", &payload)
        .await;
    let _ = io
        .within("dm-sockets")
        .emit("characterSheetUpdated", &payload)
        .await;
    for token in result.tokens {
        emit_token(&io, "tokenUpdated", token).await;
    }
    let _ = io.emit("diceRollResult", &result.roll).await;
}

fn roll_death_save_locked(
    room: &mut RoomState,
    player_id: &str,
) -> Result<DeathSaveResult, String> {
    let Some(sheet) = room.sheets.get(player_id).cloned() else {
        return Err("Лист персонажа ещё не готов".to_string());
    };
    let hp = sheet.get("hp").and_then(Value::as_f64).unwrap_or(10.0);
    if hp > 0.0 {
        return Err("Спасброски смерти идут при нулевом здоровье".to_string());
    }

    let disadvantage = rules::exhaustion_effects(sheet.get("exhaustion").unwrap_or(&Value::Null))
        .get("saveDisadvantage")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let mut rng = rand::rng();
    let die = if disadvantage {
        let first: i64 = rng.random_range(1..=20);
        let second: i64 = rng.random_range(1..=20);
        first.min(second)
    } else {
        rng.random_range(1..=20)
    };

    let successes = clamp_int(
        sheet.get("deathSavesSuccesses").unwrap_or(&Value::Null),
        0,
        LIMITS.max_death_saves,
        0,
    );
    let failures = clamp_int(
        sheet.get("deathSavesFailures").unwrap_or(&Value::Null),
        0,
        LIMITS.max_death_saves,
        0,
    );
    let max_saves = LIMITS.max_death_saves;

    let mut sheet = sheet;
    let outcome = if die == 20 {
        sheet["hp"] = json!(1);
        sheet["deathSavesSuccesses"] = json!(0);
        sheet["deathSavesFailures"] = json!(0);
        "персонаж приходит в себя"
    } else if die == 1 {
        sheet["deathSavesFailures"] = json!((failures + 2).min(max_saves));
        "две неудачи разом"
    } else if die >= 10 {
        sheet["deathSavesSuccesses"] = json!((successes + 1).min(max_saves));
        "успех"
    } else {
        sheet["deathSavesFailures"] = json!((failures + 1).min(max_saves));
        "неудача"
    };

    let player_name = {
        let character_name = sheet
            .get("characterName")
            .and_then(Value::as_str)
            .unwrap_or("");
        if character_name.is_empty() {
            room.players
                .get(player_id)
                .and_then(|player| player.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("Игрок")
                .to_string()
        } else {
            character_name.to_string()
        }
    };

    room.sheets.insert(player_id.to_string(), sheet.clone());
    let tokens = sync_sheet_tokens(room, player_id);

    let roll = json!({
        "playerId": player_id,
        "playerName": player_name,
        "role": ROLE_PLAYER,
        "formula": "d20",
        "count": 1,
        "sides": 20,
        "values": [die],
        "keptValues": [die],
        "droppedValues": [],
        "modifier": 0,
        "result": die,
        "isDeathSave": true,
        "note": outcome,
        "hidden": false,
        "time": now_iso()
    });
    room.dice_history.push(roll.clone());
    if room.dice_history.len() > LIMITS.max_dice_history as usize {
        let excess = room.dice_history.len() - LIMITS.max_dice_history as usize;
        room.dice_history.drain(..excess);
    }

    Ok(DeathSaveResult {
        sheet,
        tokens: payloads_of(room, &tokens),
        roll,
    })
}

async fn handle_reset_death_saves(socket: SocketRef, io: SocketIo, table: Arc<Table>) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    if session.role != ROLE_PLAYER {
        emit_error(&socket, "Счётчики спасбросков сбрасывает игрок");
        return;
    }
    let player_id = session.player_id.clone();

    let sheet = {
        let mut room = table.room.lock().unwrap();
        room.sheets.get_mut(&player_id).map(|sheet| {
            sheet["deathSavesSuccesses"] = json!(0);
            sheet["deathSavesFailures"] = json!(0);
            sheet.clone()
        })
    };
    let Some(sheet) = sheet else {
        emit_error(&socket, "Лист персонажа ещё не готов");
        return;
    };

    table.mark_dirty();
    let payload = sheet_payload(&player_id, &sheet);
    let _ = io
        .within(player_id)
        .emit("characterSheetData", &payload)
        .await;
    let _ = io
        .within("dm-sockets")
        .emit("characterSheetUpdated", &payload)
        .await;
}

async fn handle_request_character_sheet(socket: SocketRef, table: Arc<Table>, data: Value) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    let target_id = loose_string(data.get("playerId"));
    let is_dm = session.role == ROLE_DM;
    if !is_dm && target_id != session.player_id {
        emit_error(&socket, "Можно смотреть только свой лист персонажа");
        return;
    }
    let sheet = {
        let room = table.room.lock().unwrap();
        room.sheets.get(&target_id).cloned()
    };
    let Some(sheet) = sheet else {
        emit_error(&socket, "Лист персонажа не найден");
        return;
    };
    let _ = socket.emit("characterSheetData", &sheet_payload(&target_id, &sheet));
}

// ---------------------------------------------------------------------------
// Players and avatars
// ---------------------------------------------------------------------------

async fn handle_set_player_name(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    let name = sanitize_name(data.get("name").unwrap_or(&Value::Null));
    if name.is_empty() {
        emit_error(&socket, "Имя не может быть пустым");
        return;
    }
    if session.name == name {
        return;
    }

    let renamed = {
        let mut room = table.room.lock().unwrap();
        room.rename_player(&session.player_id, &name).is_some()
    };
    if !renamed {
        emit_error(&socket, "Участник не найден");
        return;
    }
    if let Some(entry) = table
        .sessions
        .lock()
        .unwrap()
        .get_mut(&socket.id.to_string())
    {
        entry.name = name;
    }

    table.mark_dirty();
    broadcast_players(&io, &table).await;
}

async fn handle_set_avatar(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_joined(&socket, &table) else {
        return;
    };
    let player_id = session.player_id.clone();
    if !table.room.lock().unwrap().players.contains_key(&player_id) {
        return;
    }

    let raw = data.get("image");
    let url = match raw {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.is_empty() => None,
        Some(Value::String(text)) => {
            let decoded = match decode_image(Some(text), LIMITS.max_avatar_bytes as usize) {
                Ok(decoded) => decoded,
                Err(error) => {
                    emit_error(&socket, &error);
                    return;
                }
            };
            let Some(store) = &table.store else {
                emit_error(&socket, "Не получилось сохранить аватар");
                return;
            };
            let Some(campaign_id) = table.campaign_id() else {
                emit_error(&socket, "Не получилось сохранить аватар");
                return;
            };
            match store.save_image(&campaign_id, &decoded.bytes, decoded.ext) {
                Ok(file_name) => Some(format!("/files/{file_name}")),
                Err(CampaignError::Quota { message, .. }) => {
                    emit_error(&socket, &message);
                    return;
                }
                Err(_) => {
                    emit_error(&socket, "Не получилось сохранить аватар");
                    return;
                }
            }
        }
        Some(_) => {
            emit_error(&socket, "Изображение не передано");
            return;
        }
    };

    let applied = {
        let mut room = table.room.lock().unwrap();
        if !room.players.contains_key(&player_id) {
            None
        } else {
            Some(apply_avatar(&mut room, &player_id, url.as_deref()))
        }
    };
    let Some(tokens) = applied else {
        return;
    };

    table.mark_dirty();
    for token in tokens {
        emit_token(&io, "tokenUpdated", token).await;
    }
    broadcast_players(&io, &table).await;
}

// ---------------------------------------------------------------------------
// Inventory
// ---------------------------------------------------------------------------

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

async fn handle_save_inventory_item(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
    ack: AckSender,
) {
    if require_dm(&socket, &table).is_none() {
        ack_error(ack, "Это действие доступно только ДМ");
        return;
    }
    let item = content::normalize_inventory_item(&data);
    if item.is_null() {
        emit_error(&socket, "Нужно название предмета");
        ack_error(ack, "Нужно название предмета");
        return;
    }

    let inventory = {
        let mut room = table.room.lock().unwrap();
        push_undo(&mut room);
        let existing = room
            .inventory
            .iter()
            .position(|entry| entry.get("id") == item.get("id"));
        match existing {
            Some(index) => room.inventory[index] = item.clone(),
            None => {
                room.inventory.push(item.clone());
                if room.inventory.len() > LIMITS.max_inventory_items as usize {
                    room.inventory.remove(0);
                }
            }
        }
        room.inventory.clone()
    };

    table.mark_dirty();
    let _ = io
        .within("table")
        .emit("inventoryUpdated", &Value::Array(inventory))
        .await;
    let _ = ack.send(&json!({ "ok": true, "item": item }));
}

async fn handle_delete_inventory_item(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
    ack: AckSender,
) {
    if require_dm(&socket, &table).is_none() {
        ack_error(ack, "Это действие доступно только ДМ");
        return;
    }
    let id = data.get("id").cloned().unwrap_or(Value::Null);
    let (removed, inventory) = {
        let mut room = table.room.lock().unwrap();
        let before = room.inventory.len();
        push_undo(&mut room);
        room.inventory.retain(|entry| entry.get("id") != Some(&id));
        (room.inventory.len() != before, room.inventory.clone())
    };
    if !removed {
        emit_error(&socket, "Предмет не найден");
        ack_error(ack, "Предмет не найден");
        return;
    }

    table.mark_dirty();
    let _ = io
        .within("table")
        .emit("inventoryUpdated", &Value::Array(inventory))
        .await;
    let _ = ack.send(&json!({ "ok": true }));
}

async fn handle_set_inventory_coins(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
    ack: AckSender,
) {
    if require_dm(&socket, &table).is_none() {
        ack_error(ack, "Это действие доступно только ДМ");
        return;
    }
    let coins = {
        let mut room = table.room.lock().unwrap();
        push_undo(&mut room);
        let coins = sheet::normalize_coins(data.get("coins").unwrap_or(&Value::Null));
        room.inventory_coins = coins.clone();
        coins
    };

    table.mark_dirty();
    let _ = io
        .within("table")
        .emit("inventoryCoinsUpdated", &coins)
        .await;
    let _ = ack.send(&json!({ "ok": true, "coins": coins }));
}

struct SplitResult {
    players: Vec<String>,
    shares: Value,
    coins: Value,
}

async fn handle_split_inventory_coins(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    ack: AckSender,
) {
    if require_dm(&socket, &table).is_none() {
        ack_error(ack, "Это действие доступно только ДМ");
        return;
    }
    let result = {
        let mut room = table.room.lock().unwrap();
        split_inventory_coins_locked(&mut room)
    };
    let Some(result) = result else {
        emit_error(&socket, "Нечего делить: нет игроков с листами");
        ack_error(ack, "Нечего делить: нет игроков с листами");
        return;
    };

    table.mark_dirty();
    let _ = io.emit("inventoryCoinsUpdated", &result.coins).await;
    for player_id in &result.players {
        let sheet = {
            let room = table.room.lock().unwrap();
            room.sheets.get(player_id).cloned()
        };
        let Some(sheet) = sheet else {
            continue;
        };
        let payload = sheet_payload(player_id, &sheet);
        let _ = io
            .within(player_id.clone())
            .emit("characterSheetData", &payload)
            .await;
        let _ = io
            .within("dm-sockets")
            .emit("characterSheetUpdated", &payload)
            .await;
    }
    let _ = ack.send(&json!({
        "ok": true,
        "shares": result.shares,
        "coins": result.coins
    }));
}

fn split_inventory_coins_locked(room: &mut RoomState) -> Option<SplitResult> {
    let players: Vec<String> = room
        .players
        .values()
        .filter(|player| player.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER))
        .map(|player| {
            player
                .get("playerId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        })
        .filter(|player_id| room.sheets.contains_key(player_id))
        .collect();
    if players.is_empty() {
        return None;
    }
    let count = players.len() as i64;

    let mut shares = Map::new();
    let mut share_values: Vec<(&str, i64)> = Vec::new();
    let mut remainder = Map::new();
    for key in COIN_KEYS {
        let total = clamp_int(
            room.inventory_coins.get(key).unwrap_or(&Value::Null),
            0,
            LIMITS.max_coins,
            0,
        );
        let share = total / count;
        shares.insert(key.to_string(), json!(share));
        share_values.push((key, share));
        remainder.insert(key.to_string(), json!(total - share * count));
    }

    for player_id in &players {
        if let Some(sheet) = room.sheets.get_mut(player_id) {
            let mut coins = Map::new();
            for (key, share) in &share_values {
                let current = clamp_int(
                    sheet
                        .get("coins")
                        .and_then(|coins| coins.get(*key))
                        .unwrap_or(&Value::Null),
                    0,
                    LIMITS.max_coins,
                    0,
                );
                coins.insert(
                    (*key).to_string(),
                    json!(clamp_int(&json!(current + share), 0, LIMITS.max_coins, 0)),
                );
            }
            sheet["coins"] = Value::Object(coins);
        }
    }

    room.inventory_coins = Value::Object(remainder.clone());
    Some(SplitResult {
        players,
        shares: Value::Object(shares),
        coins: Value::Object(remainder),
    })
}

struct GiveResult {
    player_id: String,
    coin: String,
    moved: i64,
    coins: Value,
    player_coins: Value,
}

async fn handle_give_inventory_coins(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
    ack: AckSender,
) {
    if require_dm(&socket, &table).is_none() {
        ack_error(ack, "Это действие доступно только ДМ");
        return;
    }

    let result = {
        let mut room = table.room.lock().unwrap();
        give_inventory_coins_locked(
            &mut room,
            &loose_string(data.get("playerId")),
            &loose_string(data.get("coin")),
            data.get("amount"),
        )
    };
    let result = match result {
        Ok(value) => value,
        Err(error) => {
            emit_error(&socket, &error);
            ack_error(ack, &error);
            return;
        }
    };

    table.mark_dirty();
    let _ = io.emit("inventoryCoinsUpdated", &result.coins).await;
    let sheet = {
        let room = table.room.lock().unwrap();
        room.sheets.get(&result.player_id).cloned()
    };
    if let Some(sheet) = sheet {
        let payload = sheet_payload(&result.player_id, &sheet);
        let _ = io
            .within(result.player_id.clone())
            .emit("characterSheetData", &payload)
            .await;
        let _ = io
            .within("dm-sockets")
            .emit("characterSheetUpdated", &payload)
            .await;
    }

    let _ = ack.send(&json!({
        "ok": true,
        "playerId": result.player_id,
        "coin": result.coin,
        "moved": result.moved,
        "coins": result.coins,
        "playerCoins": result.player_coins
    }));
}

fn give_inventory_coins_locked(
    room: &mut RoomState,
    player_id: &str,
    coin: &str,
    raw_amount: Option<&Value>,
) -> Result<GiveResult, String> {
    if !COIN_KEYS.contains(&coin) {
        return Err("Неизвестная монета".to_string());
    }
    let player_exists = room
        .players
        .get(player_id)
        .map(|player| player.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER))
        .unwrap_or(false);
    if !player_exists {
        return Err("Игрок не найден".to_string());
    }
    if !room.sheets.contains_key(player_id) {
        return Err("У игрока нет листа".to_string());
    }
    let amount = clamp_int_opt(raw_amount, 1, LIMITS.max_coins, 0);
    if amount == 0 {
        return Err("Укажите количество".to_string());
    }
    let available = clamp_int(
        room.inventory_coins.get(coin).unwrap_or(&Value::Null),
        0,
        LIMITS.max_coins,
        0,
    );
    if available == 0 {
        return Err("На складе нет этих монет".to_string());
    }

    let sheet_coins = {
        let sheet = room.sheets.get(player_id).expect("checked above");
        clamp_int(
            sheet
                .get("coins")
                .and_then(|coins| coins.get(coin))
                .unwrap_or(&Value::Null),
            0,
            LIMITS.max_coins,
            0,
        )
    };
    let room_in_sheet = LIMITS.max_coins - sheet_coins;
    let moved = amount.min(available).min(room_in_sheet);
    if moved == 0 {
        return Err("В кошельке листа нет места".to_string());
    }

    room.inventory_coins[coin] = json!(available - moved);
    let player_coins = {
        let sheet = room.sheets.get_mut(player_id).expect("checked above");
        let value = clamp_int(&json!(sheet_coins + moved), 0, LIMITS.max_coins, 0);
        sheet["coins"][coin] = json!(value);
        sheet.get("coins").cloned().unwrap_or(Value::Null)
    };

    Ok(GiveResult {
        player_id: player_id.to_string(),
        coin: coin.to_string(),
        moved,
        coins: room.inventory_coins.clone(),
        player_coins,
    })
}
