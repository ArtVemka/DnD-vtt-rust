//! Soundboard, showcases, votes, calendar, timer, party notes, dice, chat, undo.
//!
//! Port of the matching handlers in `server/socket.js` and the mutators in
//! `server/state.js`. All state lives in `RoomState`; mutations happen under the
//! room lock, events are emitted after it is dropped.

use std::sync::Arc;

use dnd_core::content;
use dnd_core::dice;
use dnd_core::media;
use dnd_core::model::MAP_TYPES;
use dnd_core::room::{DM_PLAYER_ID, ROLE_DM, ROLE_PLAYER, RoomState};
use dnd_core::rules;
use dnd_core::sheet::{self, LIMITS};
use rand::Rng;
use serde_json::{Map, Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{AckSender, Data, SocketRef};
use uuid::Uuid;

use super::{now_ms, require_dm, require_joined, session_of, viewer_of};
use crate::table::Table;

static NULL: Value = Value::Null;

fn field<'a>(data: &'a Value, key: &str) -> &'a Value {
    data.get(key).unwrap_or(&NULL)
}

fn js_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        other => other.to_string(),
    }
}

fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Null => Some(0.0),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                Some(0.0)
            } else {
                text.parse::<f64>().ok()
            }
        }
        _ => None,
    }
}

fn js_integer(value: &Value) -> Option<i64> {
    let number = js_number(value)?;
    if number.is_finite() && number.fract() == 0.0 {
        Some(number as i64)
    } else {
        None
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().map(|n| n != 0.0).unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn emit_error(socket: &SocketRef, message: &str) {
    let _ = socket.emit("errorMessage", &json!({ "message": message }));
}

fn ack_error(ack: AckSender, message: &str) {
    let _ = ack.send(&json!({ "ok": false, "error": message }));
}

fn text(value: &Value, max: i64) -> String {
    sheet::sanitize_text(value, max)
}

fn save_error(error: &dnd_core::campaign::CampaignError, fallback: &str) -> String {
    match error {
        dnd_core::campaign::CampaignError::Quota { message, .. } => message.clone(),
        _ => fallback.to_string(),
    }
}

fn trimmed(value: &Value, max: i64) -> String {
    text(value, max).trim().to_string()
}

// ---------------------------------------------------------------------------
// Time helpers
// ---------------------------------------------------------------------------

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

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn now_iso() -> String {
    let millis = now_ms() as u64;
    let secs = millis / 1000;
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60,
        millis % 1000
    )
}

fn short_hex() -> String {
    Uuid::new_v4().simple().to_string()[..8].to_string()
}

// ---------------------------------------------------------------------------
// Chat history
// ---------------------------------------------------------------------------

fn push_chat(room: &mut RoomState, message: Value) -> Value {
    let mut message = message;
    let id = format!("msg-{}-{}", base36(now_ms() as u64), short_hex());
    if let Some(object) = message.as_object_mut() {
        object.insert("id".to_string(), json!(id));
    }
    room.chat_history.push(message.clone());
    let cap = LIMITS.max_chat_history as usize;
    if room.chat_history.len() > cap {
        let extra = room.chat_history.len() - cap;
        room.chat_history.drain(0..extra);
    }
    message
}

fn log_chat(table: &Table, message: &Value) {
    let Some(store) = &table.store else {
        return;
    };
    let Some(campaign_id) = table.campaign_id() else {
        return;
    };
    let Ok(dir) = store.dir_for(&campaign_id) else {
        return;
    };
    let event = json!({
        "event": "chat",
        "time": now_iso(),
        "from": message.get("from").cloned().unwrap_or(Value::Null),
        "text": message.get("text").cloned().unwrap_or(Value::Null),
        "roll": message.get("isRoll").map(truthy).unwrap_or(false),
        "whisper": message.get("whisper").map(truthy).unwrap_or(false)
    });
    dnd_core::session_log::SessionLog::new(dir).append(&event);
}

fn system_message(text: String) -> Value {
    json!({
        "from": "Стол",
        "playerId": "",
        "role": ROLE_DM,
        "avatar": dnd_core::room::DEFAULT_DM_AVATAR,
        "text": text,
        "time": now_iso()
    })
}

// ---------------------------------------------------------------------------
// Maps and tokens
// ---------------------------------------------------------------------------

/// DM and player payloads for `mapUpdated`, ready to emit.
fn map_payloads(room: &RoomState, kind: &str) -> (Value, Value) {
    let slot = room
        .maps
        .get(kind)
        .cloned()
        .unwrap_or_else(|| json!({ "items": [], "activeId": Value::Null }));
    let dm = json!({
        "type": kind,
        "items": slot.get("items").cloned().unwrap_or_else(|| json!([])),
        "activeId": slot.get("activeId").cloned().unwrap_or(Value::Null),
        "activeMap": room.active_map,
        "activeMapId": room.active_map_id
    });
    let visible = room.map_slot_for_viewer(kind, false);
    let player = json!({
        "type": kind,
        "items": visible.get("items").cloned().unwrap_or_else(|| json!([])),
        "activeId": visible.get("activeId").cloned().unwrap_or(Value::Null),
        "activeMap": if room.active_map_id.is_some() { json!(room.active_map) } else { Value::Null },
        "activeMapId": room.active_map_id
    });
    (dm, player)
}

fn token_views(room: &RoomState, token: &Value) -> (Value, Option<Value>) {
    let dm = room.token_for_viewer(token, true);
    let player = if room.is_token_visible_to_players(token) {
        Some(room.token_for_viewer(token, false))
    } else {
        None
    };
    (dm, player)
}

async fn emit_map_update(io: &SocketIo, payloads: &(Value, Value)) {
    let _ = io
        .within("dm-sockets")
        .emit("mapUpdated", &payloads.0)
        .await;
    let _ = io
        .within("table")
        .except("dm-sockets")
        .emit("mapUpdated", &payloads.1)
        .await;
}

fn apply_auto_light(room: &mut RoomState) -> Vec<String> {
    let darkness = rules::darkness_for_hour(&room.game_time);
    let mut changed = Vec::new();
    for kind in MAP_TYPES {
        let Some(items) = room
            .maps
            .get_mut(kind)
            .and_then(|slot| slot.get_mut("items"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        let mut touched = false;
        for item in items.iter_mut() {
            if !item
                .get("autoLight")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                continue;
            }
            let current = item.get("darkness").and_then(Value::as_f64).unwrap_or(0.0);
            if (current - darkness).abs() < 0.001 {
                continue;
            }
            item["darkness"] = json!(darkness);
            touched = true;
        }
        if touched {
            changed.push(kind.to_string());
        }
    }
    changed
}

/// `tickConditionsForHours`: only hour-long conditions count down.
fn tick_conditions_for_hours(tokens: &mut [Value], hours: i64) -> Vec<Value> {
    let elapsed = hours.max(0);
    if elapsed <= 0 {
        return Vec::new();
    }
    let mut changed = Vec::new();
    for token in tokens.iter_mut() {
        let conditions =
            content::normalize_conditions(token.get("conditions").unwrap_or(&Value::Null));
        let durations = content::normalize_condition_durations(
            token.get("conditionDurations").unwrap_or(&Value::Null),
            &conditions,
        );
        let units = content::normalize_condition_units(
            token.get("conditionDurationUnits").unwrap_or(&Value::Null),
            &conditions,
            &durations,
        );
        let mut next = durations.as_object().cloned().unwrap_or_default();
        let mut touched = false;
        for (id, value) in durations.as_object().into_iter().flatten() {
            if units.get(id).and_then(Value::as_str) != Some("hour") {
                continue;
            }
            let left = value.as_i64().unwrap_or(0) - elapsed;
            if left <= 0 {
                next.remove(id);
            } else {
                next.insert(id.clone(), json!(left));
            }
            touched = true;
        }
        if !touched {
            continue;
        }
        let kept: Vec<Value> = conditions
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|condition| {
                condition
                    .as_str()
                    .and_then(|id| next.get(id))
                    .and_then(Value::as_i64)
                    .map(|value| value > 0)
                    .unwrap_or(false)
            })
            .collect();
        token["conditions"] = Value::Array(kept.clone());
        token["conditionDurations"] = Value::Object(next);
        token["conditionDurationUnits"] = content::normalize_condition_units(
            &units,
            &Value::Array(kept),
            &token["conditionDurations"],
        );
        changed.push(token.clone());
    }
    changed
}

// ---------------------------------------------------------------------------
// Undo
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

fn undo_last(room: &mut RoomState) -> bool {
    let Some(data) = room.undo_stack.pop() else {
        return false;
    };
    if let Some(value) = data.get("maps") {
        room.maps = value.clone();
    }
    if let Some(value) = data.get("tokens").and_then(Value::as_array) {
        room.tokens = value.clone();
    }
    if let Some(value) = data.get("initiative") {
        room.initiative = value.clone();
    }
    if let Some(value) = data.get("journal") {
        room.journal = value.as_str().unwrap_or("").to_string();
    }
    if let Some(value) = data.get("journalEntries").and_then(Value::as_array) {
        room.journal_entries = value.clone();
    }
    if let Some(value) = data.get("inventory").and_then(Value::as_array) {
        room.inventory = value.clone();
    }
    if let Some(value) = data.get("inventoryCoins") {
        room.inventory_coins = sheet::normalize_coins(value);
    }
    if let Some(value) = data.get("gameTime") {
        room.game_time = content::normalize_time(value);
    }
    if let Some(value) = data.get("calendarMonths") {
        room.calendar_months = content::normalize_calendar_months(value);
    }
    if let Some(value) = data.get("calendarEvents") {
        room.calendar_events = content::normalize_calendar_events(value);
    }
    if let Some(value) = data.get("activeMap") {
        room.active_map = value.as_str().map(str::to_string);
    }
    if let Some(value) = data.get("activeMapId") {
        room.active_map_id = value.as_str().map(str::to_string);
    }
    true
}

async fn broadcast_snapshot(io: &SocketIo, table: &Arc<Table>) {
    for socket in io.sockets() {
        let Some(session) = session_of(table, &socket) else {
            continue;
        };
        if session.role.is_empty() {
            continue;
        }
        let snapshot = table.snapshot_for(&viewer_of(&session));
        let _ = socket.emit("stateReload", &snapshot);
    }
}

// ---------------------------------------------------------------------------
// Roll requests
// ---------------------------------------------------------------------------

async fn broadcast_roll_requests(io: &SocketIo, table: &Arc<Table>) {
    let (all, players) = {
        let room = table.room.lock().unwrap();
        let all = Value::Array(room.roll_requests.clone());
        let players: Vec<(String, Value)> = room
            .players
            .values()
            .filter(|player| player.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER))
            .map(|player| {
                let player_id = player
                    .get("playerId")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let visible = room
                    .roll_requests
                    .iter()
                    .filter(|request| {
                        let to = request.get("to").and_then(Value::as_str).unwrap_or("");
                        to.is_empty() || to == player_id
                    })
                    .cloned()
                    .collect();
                (player_id, Value::Array(visible))
            })
            .collect();
        (all, players)
    };
    let _ = io
        .within("dm-sockets")
        .emit("rollRequestUpdated", &all)
        .await;
    for (player_id, list) in players {
        let _ = io.to(player_id).emit("rollRequestUpdated", &list).await;
    }
}

// ---------------------------------------------------------------------------
// Votes
// ---------------------------------------------------------------------------

/// `None` means the vote is closed.
fn vote_views(room: &RoomState) -> Option<(Value, Vec<(String, Value)>)> {
    let vote = room.vote.as_ref()?;
    let options = vote
        .get("options")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let votes = vote
        .get("votes")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let counts: Vec<Value> = (0..options.len())
        .map(|index| {
            let count = votes
                .values()
                .filter(|value| value.as_i64() == Some(index as i64))
                .count();
            json!(count)
        })
        .collect();
    let players = room
        .players
        .values()
        .map(|player| {
            let player_id = player
                .get("playerId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut payload = Map::new();
            payload.insert(
                "question".to_string(),
                vote.get("question").cloned().unwrap_or(Value::Null),
            );
            payload.insert("options".to_string(), Value::Array(options.clone()));
            payload.insert("counts".to_string(), Value::Array(counts.clone()));
            if let Some(your_vote) = votes.get(&player_id) {
                payload.insert("yourVote".to_string(), your_vote.clone());
            }
            (player_id, Value::Object(payload))
        })
        .collect();
    Some((vote.clone(), players))
}

async fn emit_vote(io: &SocketIo, table: &Arc<Table>) {
    let views = {
        let room = table.room.lock().unwrap();
        vote_views(&room)
    };
    match views {
        None => {
            let _ = io.emit("voteUpdated", &Value::Null).await;
        }
        Some((dm, players)) => {
            let _ = io.within("dm-sockets").emit("voteUpdated", &dm).await;
            for (player_id, payload) in players {
                let _ = io.to(player_id).emit("voteUpdated", &payload).await;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    {
        let table = table.clone();
        socket.on(
            "setGameTime",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let (time, maps, tokens) = {
                        let mut room = table.room.lock().unwrap();
                        let next = content::normalize_time(&data);
                        if next.get("day") == room.game_time.get("day")
                            && next.get("hour") == room.game_time.get("hour")
                        {
                            return;
                        }
                        let before = room.game_time.clone();
                        push_undo(&mut room);
                        room.game_time = next;
                        let elapsed = (room.game_time["day"].as_i64().unwrap_or(1)
                            - before["day"].as_i64().unwrap_or(1))
                            * 24
                            + (room.game_time["hour"].as_i64().unwrap_or(0)
                                - before["hour"].as_i64().unwrap_or(0));
                        let changed = tick_conditions_for_hours(&mut room.tokens, elapsed);
                        let kinds = apply_auto_light(&mut room);
                        let maps = kinds
                            .iter()
                            .map(|kind| map_payloads(&room, kind))
                            .collect::<Vec<_>>();
                        let views = changed
                            .iter()
                            .map(|token| token_views(&room, token))
                            .collect::<Vec<_>>();
                        (room.game_time.clone(), maps, views)
                    };
                    table.mark_dirty();
                    for payloads in &maps {
                        emit_map_update(&io, payloads).await;
                    }
                    let _ = io.emit("gameTimeUpdated", &time).await;
                    for (dm, player) in tokens {
                        let _ = io.within("dm-sockets").emit("tokenUpdated", &dm).await;
                        if let Some(player) = player {
                            let _ = io
                                .within("table")
                                .except("dm-sockets")
                                .emit("tokenUpdated", &player)
                                .await;
                        }
                    }
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "setCalendarMonths",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let (months, changed) = {
                        let mut room = table.room.lock().unwrap();
                        let next = content::normalize_calendar_months(
                            data.get("months").unwrap_or(&Value::Null),
                        );
                        let changed = next != room.calendar_months;
                        if changed {
                            push_undo(&mut room);
                            room.calendar_months = next;
                        }
                        (room.calendar_months.clone(), changed)
                    };
                    if changed {
                        table.mark_dirty();
                        let _ = io.emit("calendarMonthsUpdated", &months).await;
                    }
                    let _ = ack.send(&json!({ "ok": true, "months": months }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "addCalendarEvent",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let (event, events) = {
                        let mut room = table.room.lock().unwrap();
                        if room
                            .calendar_events
                            .as_array()
                            .map(|list| list.len())
                            .unwrap_or(0)
                            >= LIMITS.max_calendar_events as usize
                        {
                            drop(room);
                            emit_error(&socket, "Слишком много событий в календаре");
                            ack_error(ack, "Слишком много событий в календаре");
                            return;
                        }
                        let event = content::normalize_calendar_event(&data);
                        if event.is_null() {
                            drop(room);
                            emit_error(&socket, "Нужно название и день события");
                            ack_error(ack, "Нужно название и день события");
                            return;
                        }
                        push_undo(&mut room);
                        if let Some(list) = room.calendar_events.as_array_mut() {
                            list.push(event.clone());
                        }
                        (event, room.calendar_events.clone())
                    };
                    table.mark_dirty();
                    let _ = io.emit("calendarEventsUpdated", &events).await;
                    let _ = ack.send(&json!({ "ok": true, "event": event, "events": events }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "deleteCalendarEvent",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let id = js_string(field(&data, "id"));
                    let events = {
                        let mut room = table.room.lock().unwrap();
                        let index = room.calendar_events.as_array().and_then(|list| {
                            list.iter().position(|event| {
                                event.get("id").and_then(Value::as_str) == Some(id.as_str())
                            })
                        });
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Событие не найдено");
                            ack_error(ack, "Событие не найдено");
                            return;
                        };
                        push_undo(&mut room);
                        if let Some(list) = room.calendar_events.as_array_mut() {
                            list.remove(index);
                        }
                        room.calendar_events.clone()
                    };
                    table.mark_dirty();
                    let _ = io.emit("calendarEventsUpdated", &events).await;
                    let _ = ack.send(&json!({ "ok": true, "events": events }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "setPartyNotes",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_joined(&socket, &table).is_none() {
                        return;
                    }
                    let (notes, changed) = {
                        let mut room = table.room.lock().unwrap();
                        let next = text(field(&data, "text"), LIMITS.max_party_notes);
                        let changed = next != room.party_notes;
                        if changed {
                            room.party_notes = next;
                        }
                        (room.party_notes.clone(), changed)
                    };
                    if changed {
                        table.mark_dirty();
                        let _ = io.emit("partyNotesUpdated", &json!(notes)).await;
                    }
                    let _ = ack.send(&json!({ "ok": true, "notes": notes }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "timerStart",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let seconds = sheet::clamp_int(
                        data.get("seconds").unwrap_or(&Value::Null),
                        1,
                        content::TIME_LIMITS.max_timer_seconds,
                        0,
                    );
                    if seconds == 0 {
                        emit_error(&socket, "Укажите длительность таймера");
                        return;
                    }
                    let label = trimmed(
                        data.get("label").unwrap_or(&Value::Null),
                        LIMITS.max_name_length,
                    );
                    let timer = {
                        let mut room = table.room.lock().unwrap();
                        room.timer = json!({
                            "running": true,
                            "endsAt": now_ms() as u64 + seconds as u64 * 1000,
                            "totalSec": seconds,
                            "label": label
                        });
                        room.timer.clone()
                    };
                    table.mark_dirty();
                    let _ = io.emit("timerUpdated", &timer).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on("timerStop", move |socket: SocketRef, io: SocketIo| {
            let table = table.clone();
            async move {
                if require_dm(&socket, &table).is_none() {
                    return;
                }
                let timer = {
                    let mut room = table.room.lock().unwrap();
                    if !room.timer.get("running").map(truthy).unwrap_or(false) {
                        return;
                    }
                    room.timer = content::empty_timer();
                    room.timer.clone()
                };
                table.mark_dirty();
                let _ = io.emit("timerUpdated", &timer).await;
            }
        });
    }

    {
        let table = table.clone();
        socket.on(
            "setShowcase",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let on = truthy(field(&data, "on"));
                    let showcase = {
                        let mut room = table.room.lock().unwrap();
                        if !on {
                            room.showcase = json!({
                                "on": false, "kind": "", "ref": "", "text": "", "title": "", "watchers": []
                            });
                        } else {
                            let kind = if field(&data, "kind").as_str() == Some("text") {
                                "text"
                            } else {
                                "image"
                            };
                            let raw = field(&data, "ref").as_str().unwrap_or("");
                            let reference = if raw.starts_with("/files/") {
                                raw.chars().take(120).collect::<String>()
                            } else {
                                String::new()
                            };
                            if kind == "image" && reference.is_empty() {
                                drop(room);
                                emit_error(&socket, "К кинорежиму нужна картинка из файлов стола");
                                return;
                            }
                            room.showcase = json!({
                                "on": true,
                                "kind": kind,
                                "ref": reference,
                                "text": text(field(&data, "text"), LIMITS.max_sheet_text_length),
                                "title": text(field(&data, "title"), LIMITS.max_name_length),
                                "watchers": []
                            });
                        }
                        room.showcase.clone()
                    };
                    let _ = io.emit("showcaseUpdated", &showcase).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on("showcaseSeen", move |socket: SocketRef, io: SocketIo| {
            let table = table.clone();
            async move {
                let Some(session) = require_joined(&socket, &table) else {
                    return;
                };
                let showcase = {
                    let mut room = table.room.lock().unwrap();
                    if !room.showcase.get("on").map(truthy).unwrap_or(false) {
                        return;
                    }
                    let already = room
                        .showcase
                        .get("watchers")
                        .and_then(Value::as_array)
                        .map(|list| {
                            list.iter()
                                .any(|item| item.as_str() == Some(session.player_id.as_str()))
                        })
                        .unwrap_or(false);
                    if already {
                        return;
                    }
                    if let Some(list) = room
                        .showcase
                        .get_mut("watchers")
                        .and_then(Value::as_array_mut)
                    {
                        list.push(json!(session.player_id));
                    }
                    room.showcase.clone()
                };
                let _ = io.emit("showcaseUpdated", &showcase).await;
            }
        });
    }

    {
        let table = table.clone();
        socket.on(
            "uploadSound",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    {
                        let room = table.room.lock().unwrap();
                        if room.soundboard.len() >= LIMITS.max_soundboard as usize {
                            drop(room);
                            emit_error(
                                &socket,
                                &format!(
                                    "На саундборде уже {} звуков — удалите лишние",
                                    LIMITS.max_soundboard
                                ),
                            );
                            return;
                        }
                    }
                    let decoded = match media::decode_audio(field(&data, "sound").as_str()) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            emit_error(&socket, &error);
                            return;
                        }
                    };
                    let Some(store) = table.store.clone() else {
                        emit_error(&socket, "Не удалось сохранить звук");
                        return;
                    };
                    let Some(campaign_id) = table.campaign_id() else {
                        emit_error(&socket, "Не удалось сохранить звук");
                        return;
                    };
                    let file_name =
                        match store.save_audio(&campaign_id, &decoded.bytes, decoded.ext) {
                            Ok(name) => name,
                            Err(error) => {
                                emit_error(
                                    &socket,
                                    &save_error(&error, "Не удалось сохранить звук"),
                                );
                                return;
                            }
                        };
                    let name = {
                        let clean = text(field(&data, "name"), LIMITS.max_name_length);
                        if clean.is_empty() {
                            "Звук".to_string()
                        } else {
                            clean
                        }
                    };
                    let soundboard = {
                        let mut room = table.room.lock().unwrap();
                        room.soundboard.push(json!({
                            "id": content::new_sound_id(),
                            "name": name,
                            "ref": format!("/files/{file_name}")
                        }));
                        Value::Array(room.soundboard.clone())
                    };
                    table.mark_dirty();
                    let _ = io
                        .within("dm-sockets")
                        .emit("soundboardUpdated", &soundboard)
                        .await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "deleteSound",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let id = js_string(field(&data, "id"));
                    let (removed, maps, soundboard) = {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = room.soundboard.iter().position(|item| {
                            item.get("id").and_then(Value::as_str) == Some(id.as_str())
                        }) else {
                            return;
                        };
                        let removed = room.soundboard.remove(index);
                        let reference = removed
                            .get("ref")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let mut kinds = Vec::new();
                        for kind in MAP_TYPES {
                            let Some(items) = room
                                .maps
                                .get_mut(kind)
                                .and_then(|slot| slot.get_mut("items"))
                                .and_then(Value::as_array_mut)
                            else {
                                continue;
                            };
                            let dirty = items.iter().any(|map| {
                                map.get("audioRef").and_then(Value::as_str)
                                    == Some(reference.as_str())
                            });
                            if !dirty {
                                continue;
                            }
                            for map in items.iter_mut() {
                                if map.get("audioRef").and_then(Value::as_str)
                                    == Some(reference.as_str())
                                {
                                    map["audioRef"] = Value::Null;
                                }
                            }
                            kinds.push(kind.to_string());
                        }
                        let maps = kinds
                            .iter()
                            .map(|kind| map_payloads(&room, kind))
                            .collect::<Vec<_>>();
                        (removed, maps, Value::Array(room.soundboard.clone()))
                    };
                    let _ = removed;
                    for payloads in &maps {
                        emit_map_update(&io, payloads).await;
                    }
                    table.mark_dirty();
                    let _ = io
                        .within("dm-sockets")
                        .emit("soundboardUpdated", &soundboard)
                        .await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "playSound",
            move |socket: SocketRef, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_joined(&socket, &table).is_none() {
                        return;
                    }
                    let reference = js_string(field(&data, "ref"));
                    let wanted = js_string(field(&data, "action"));
                    let action = if matches!(wanted.as_str(), "play" | "loop" | "stop") {
                        wanted
                    } else {
                        "play".to_string()
                    };
                    let known = {
                        let room = table.room.lock().unwrap();
                        if reference.is_empty() {
                            true
                        } else {
                            room.soundboard.iter().any(|item| {
                                item.get("ref").and_then(Value::as_str) == Some(reference.as_str())
                            }) || MAP_TYPES.iter().any(|kind| {
                                room.maps
                                    .get(kind)
                                    .and_then(|slot| slot.get("items"))
                                    .and_then(Value::as_array)
                                    .map(|items| {
                                        items.iter().any(|map| {
                                            map.get("audioRef").and_then(Value::as_str)
                                                == Some(reference.as_str())
                                        })
                                    })
                                    .unwrap_or(false)
                            })
                        }
                    };
                    if !known {
                        return;
                    }
                    if reference.is_empty() && action != "stop" {
                        return;
                    }
                    let cue = json!({ "ref": reference, "action": action });
                    let _ = socket.broadcast().emit("soundCue", &cue).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "rollDice",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let requested = match data.get("formula").and_then(Value::as_str) {
                        Some(formula) if !formula.trim().is_empty() => formula.to_string(),
                        _ => match data.get("sides") {
                            Some(sides) => match js_number(sides) {
                                Some(number) if number.is_finite() => {
                                    format!("d{}", number.round() as i64)
                                }
                                _ => String::new(),
                            },
                            None => String::new(),
                        },
                    };
                    let parsed = match dice::parse_formula(&requested) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            emit_error(&socket, &error.0);
                            return;
                        }
                    };
                    let is_dm = session.role == ROLE_DM;
                    let hidden = is_dm && truthy(field(&data, "hidden"));
                    let dm_only = is_dm && truthy(field(&data, "dmOnly"));
                    let label = trimmed(field(&data, "label"), 80);

                    let rolled = dice::roll_formula(&parsed, &mut |min, max| {
                        rand::rng().random_range(min..=max)
                    });
                    let mut roll = serde_json::to_value(&rolled).unwrap_or_else(|_| json!({}));
                    if let Some(object) = roll.as_object_mut() {
                        object.insert("playerId".to_string(), json!(session.player_id));
                        object.insert("playerName".to_string(), json!(session.name));
                        object.insert("role".to_string(), json!(session.role));
                        object.insert("hidden".to_string(), json!(hidden));
                        object.insert("time".to_string(), json!(now_iso()));
                    }
                    let raw_ac = data.get("targetAc").and_then(js_number);
                    let target_ac = raw_ac
                        .filter(|value| value.is_finite() && *value >= 1.0 && *value <= 60.0)
                        .map(|value| value.round() as i64);
                    if let Some(target_ac) = target_ac {
                        if !hidden && parsed.count == 1 && parsed.sides == 20 {
                            let result = roll.get("result").and_then(Value::as_i64).unwrap_or(0);
                            if let Some(object) = roll.as_object_mut() {
                                object.insert("targetAc".to_string(), json!(target_ac));
                                object.insert("hit".to_string(), json!(result >= target_ac));
                            }
                        }
                    }
                    if !label.is_empty() {
                        roll["label"] = json!(label);
                    }
                    let note = match roll.get("hit").and_then(Value::as_bool) {
                        Some(true) => Some(format!(
                            "попадание по КД {}",
                            roll.get("targetAc").and_then(Value::as_i64).unwrap_or(0)
                        )),
                        Some(false) => Some(format!(
                            "промах по КД {}",
                            roll.get("targetAc").and_then(Value::as_i64).unwrap_or(0)
                        )),
                        None => None,
                    };
                    if let Some(note) = note {
                        roll["note"] = json!(note);
                    }

                    if dm_only {
                        let mut secret = roll.clone();
                        secret["hidden"] = json!(true);
                        let _ = io.within("dm-sockets").emit("diceRollResult", &secret).await;
                        let mut dm_roll = secret.clone();
                        dm_roll["label"] = json!(label);
                        let _ = io.within("dm-sockets").emit("dmRollResult", &dm_roll).await;
                        return;
                    }

                    let mut sheet_update = None;
                    let mut placed_tokens = Vec::new();
                    let mut chat_notice = None;
                    {
                        let mut room = table.room.lock().unwrap();
                        room.dice_history.push(roll.clone());
                        let cap = LIMITS.max_dice_history as usize;
                        if room.dice_history.len() > cap {
                            let extra = room.dice_history.len() - cap;
                            room.dice_history.drain(0..extra);
                        }

                        if session.role == ROLE_PLAYER && !label.is_empty() {
                            let pending = room
                                .roll_requests
                                .iter()
                                .find(|request| {
                                    request.get("to").and_then(Value::as_str)
                                        == Some(session.player_id.as_str())
                                        && request.get("label").and_then(Value::as_str)
                                            == Some(label.as_str())
                                        && request
                                            .get("label")
                                            .and_then(Value::as_str)
                                            .map(|text| text.starts_with("Концентрация: "))
                                            .unwrap_or(false)
                                })
                                .and_then(|request| {
                                    Some((
                                        request.get("id").and_then(Value::as_str)?.to_string(),
                                        request.get("dc").and_then(Value::as_i64)?,
                                    ))
                                });
                            if let Some((request_id, dc)) = pending {
                                let result = roll.get("result").and_then(Value::as_i64);
                                if result.map(|value| value < dc).unwrap_or(false) {
                                    let spell = room
                                        .sheets
                                        .get(&session.player_id)
                                        .and_then(|sheet| sheet.get("concentration"))
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string();
                                    if !spell.is_empty() {
                                        let mut sheet = room
                                            .sheets
                                            .get(&session.player_id)
                                            .cloned()
                                            .unwrap_or_else(|| json!({}));
                                        sheet["concentration"] = json!("");
                                        room.sheets.insert(session.player_id.clone(), sheet.clone());
                                        let max_hp = rules::effective_max_hp(&sheet) as i64;
                                        let hp = sheet
                                            .get("hp")
                                            .and_then(Value::as_i64)
                                            .unwrap_or(0)
                                            .clamp(0, max_hp);
                                        for token in room.tokens.iter_mut() {
                                            if token.get("assignedPlayerId").and_then(Value::as_str)
                                                == Some(session.player_id.as_str())
                                            {
                                                token["maxHp"] = json!(max_hp);
                                                token["hp"] = json!(hp);
                                                placed_tokens.push(token.clone());
                                            }
                                        }
                                        let message = push_chat(
                                            &mut room,
                                            system_message(format!(
                                                "«{spell}» — концентрация оборвалась (провал СЛ {dc})"
                                            )),
                                        );
                                        chat_notice = Some(message);
                                        room.roll_requests.retain(|request| {
                                            request.get("id").and_then(Value::as_str)
                                                != Some(request_id.as_str())
                                        });
                                        sheet_update = Some(sheet);
                                    }
                                }
                            }
                        }
                    }
                    table.mark_dirty();

                    let for_players = dice::hide_roll_value(&roll);
                    let _ = io
                        .within("table")
                        .except("dm-sockets")
                        .emit("diceRollResult", &for_players)
                        .await;
                    let _ = io.within("dm-sockets").emit("diceRollResult", &roll).await;

                    if let Some(sheet) = sheet_update {
                        let _ = io
                            .to(session.player_id.clone())
                            .emit(
                                "characterSheetData",
                                &json!({ "playerId": session.player_id, "sheet": sheet }),
                            )
                            .await;
                        let _ = io
                            .within("dm-sockets")
                            .emit(
                                "characterSheetUpdated",
                                &json!({ "playerId": session.player_id, "sheet": sheet }),
                            )
                            .await;
                        let views = {
                            let room = table.room.lock().unwrap();
                            placed_tokens
                                .iter()
                                .map(|token| token_views(&room, token))
                                .collect::<Vec<_>>()
                        };
                        for (dm, player) in views {
                            let _ = io.within("dm-sockets").emit("tokenUpdated", &dm).await;
                            if let Some(player) = player {
                                let _ = io
                                    .within("table")
                                    .except("dm-sockets")
                                    .emit("tokenUpdated", &player)
                                    .await;
                            }
                        }
                        if let Some(message) = chat_notice {
                            let _ = io.emit("chatMessage", &message).await;
                            log_chat(&table, &message);
                        }
                        broadcast_roll_requests(&io, &table).await;
                    }
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "rollTable",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_dm(&socket, &table) else {
                        return;
                    };
                    let table_id = js_string(field(&data, "tableId"));
                    let adventure = {
                        let room = table.room.lock().unwrap();
                        room.adventure.clone()
                    };
                    let tables = adventure
                        .as_ref()
                        .and_then(|value| value.get("tables"))
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let Some(found) = tables
                        .into_iter()
                        .find(|item| item.get("id").and_then(Value::as_str) == Some(table_id.as_str()))
                    else {
                        emit_error(&socket, "Таблица с такими строками не найдена");
                        return;
                    };
                    let rows = found
                        .get("rows")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    if rows.is_empty() {
                        emit_error(&socket, "Таблица с такими строками не найдена");
                        return;
                    }
                    let formula = found
                        .get("formula")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    let ranged = !formula.is_empty()
                        && rows.len() > 1
                        && rows.iter().all(|row| {
                            js_number(row.get("min").unwrap_or(&Value::Null))
                                .map(|value| value.is_finite())
                                .unwrap_or(false)
                                && js_number(row.get("max").unwrap_or(&Value::Null))
                                    .map(|value| value.is_finite())
                                    .unwrap_or(false)
                        });
                    let mut result = Value::Null;
                    let row = if ranged {
                        let Ok(parsed) = dice::parse_formula(&formula) else {
                            emit_error(&socket, "Таблица с такими строками не найдена");
                            return;
                        };
                        let rolled = dice::roll_formula(&parsed, &mut |min, max| {
                            rand::rng().random_range(min..=max)
                        });
                        let value = rolled.result.unwrap_or(0);
                        result = json!(value);
                        rows.iter()
                            .find(|row| {
                                let min = js_number(row.get("min").unwrap_or(&Value::Null)).unwrap_or(0.0);
                                let max = js_number(row.get("max").unwrap_or(&Value::Null)).unwrap_or(0.0);
                                value as f64 >= min && value as f64 <= max
                            })
                            .or_else(|| rows.last())
                            .cloned()
                            .unwrap()
                    } else {
                        let total: i64 = rows
                            .iter()
                            .map(|row| row.get("weight").and_then(Value::as_i64).unwrap_or(1))
                            .sum();
                        let ticket = rand::rng().random_range(1..=total.max(1));
                        let mut ticket = ticket.max(1);
                        let mut chosen = rows.last().cloned().unwrap();
                        for entry in &rows {
                            ticket -= entry.get("weight").and_then(Value::as_i64).unwrap_or(1);
                            if ticket <= 0 {
                                chosen = entry.clone();
                                break;
                            }
                        }
                        chosen
                    };
                    let row_label = row
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let name = found.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                    let text = format!("Таблица «{name}» — {row_label}");
                    if truthy(field(&data, "hidden")) {
                        let payload = json!({
                            "formula": if formula.is_empty() { "таблица".to_string() } else { formula },
                            "result": result,
                            "hidden": true,
                            "label": text,
                            "time": now_iso()
                        });
                        let _ = io.within("dm-sockets").emit("dmRollResult", &payload).await;
                        return;
                    }
                    let feed = json!({
                        "playerId": session.player_id,
                        "playerName": session.name,
                        "role": session.role,
                        "formula": format!("Таблица «{name}»"),
                        "count": 1,
                        "sides": 0,
                        "values": [],
                        "keptValues": [],
                        "droppedValues": [],
                        "modifier": 0,
                        "result": result,
                        "note": row_label,
                        "hidden": false,
                        "time": now_iso()
                    });
                    let _ = io.emit("diceRollResult", &feed).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "rollRequest",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    let Some(session) = require_dm(&socket, &table) else {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    };
                    let mut to = String::new();
                    let mut to_name = String::new();
                    if let Some(target_id) = data.get("to").filter(|value| truthy(value)) {
                        let target_id = js_string(target_id);
                        let target = {
                            let room = table.room.lock().unwrap();
                            room.players.get(&target_id).cloned()
                        };
                        let Some(target) = target else {
                            emit_error(&socket, "Игрок для проверки не найден");
                            ack_error(ack, "Игрок для проверки не найден");
                            return;
                        };
                        if target.get("role").and_then(Value::as_str) != Some(ROLE_PLAYER) {
                            emit_error(&socket, "Игрок для проверки не найден");
                            ack_error(ack, "Игрок для проверки не найден");
                            return;
                        }
                        to = target_id;
                        to_name = target
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                    }
                    let request = {
                        let mut room = table.room.lock().unwrap();
                        let mut input = data.as_object().cloned().unwrap_or_default();
                        input.insert("to".to_string(), json!(to));
                        input.insert("toName".to_string(), json!(to_name));
                        input.insert("by".to_string(), json!(session.name));
                        let request = content::normalize_roll_request(&Value::Object(input));
                        if request.is_null() {
                            drop(room);
                            emit_error(&socket, "Нужно название проверки");
                            ack_error(ack, "Нужно название проверки");
                            return;
                        }
                        room.roll_requests.push(request.clone());
                        if room.roll_requests.len() > LIMITS.max_roll_requests as usize {
                            room.roll_requests.remove(0);
                        }
                        request
                    };
                    broadcast_roll_requests(&io, &table).await;
                    table.mark_dirty();
                    let _ = ack.send(&json!({ "ok": true, "request": request }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "rollRequestClear",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let id = js_string(field(&data, "id"));
                    {
                        let mut room = table.room.lock().unwrap();
                        room.roll_requests.retain(|request| {
                            request.get("id").and_then(Value::as_str) != Some(id.as_str())
                        });
                    }
                    broadcast_roll_requests(&io, &table).await;
                    table.mark_dirty();
                    let _ = ack.send(&json!({ "ok": true }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "reactionPrompt",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let player_id = js_string(field(&data, "playerId"));
                    let allowed = {
                        let room = table.room.lock().unwrap();
                        room.players
                            .get(&player_id)
                            .map(|player| {
                                player.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER)
                            })
                            .unwrap_or(false)
                    };
                    if !allowed {
                        emit_error(&socket, "Игрок не найден");
                        return;
                    }
                    let raw_ac = data.get("targetAc").and_then(js_number);
                    let target_ac = raw_ac
                        .filter(|value| value.is_finite() && *value >= 1.0 && *value <= 60.0)
                        .map(|value| value.round().clamp(1.0, 60.0) as i64);
                    let reaction = {
                        let mut room = table.room.lock().unwrap();
                        let attacker = text(field(&data, "attackerName"), LIMITS.max_name_length);
                        let attacker = if attacker.is_empty() {
                            "Противник".to_string()
                        } else {
                            attacker
                        };
                        let target = text(field(&data, "targetName"), LIMITS.max_name_length);
                        let target = if target.is_empty() {
                            "Фишка".to_string()
                        } else {
                            target
                        };
                        let bonus = sheet::clamp_int(field(&data, "bonus"), -5, 20, 0);
                        let round = room
                            .initiative
                            .get("round")
                            .and_then(Value::as_i64)
                            .unwrap_or(1);
                        let reaction = json!({
                            "id": format!("reaction-{}-{}", now_ms(), room.pending_reactions.len()),
                            "playerId": player_id,
                            "attackerName": attacker,
                            "targetName": target,
                            "bonus": bonus,
                            "targetAc": target_ac,
                            "round": round
                        });
                        room.pending_reactions.push(reaction.clone());
                        if room.pending_reactions.len() > 5 {
                            room.pending_reactions.remove(0);
                        }
                        reaction
                    };
                    let _ = io.to(player_id).emit("reactionPrompt", &reaction).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "reactionResolved",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let id = js_string(field(&data, "id"));
                    let removed = {
                        let mut room = table.room.lock().unwrap();
                        let index = room.pending_reactions.iter().position(|item| {
                            item.get("id").and_then(Value::as_str) == Some(id.as_str())
                        });
                        match index {
                            Some(index) => {
                                let reaction = room.pending_reactions[index].clone();
                                let owner = reaction
                                    .get("playerId")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                if session.role != ROLE_DM && owner != session.player_id {
                                    None
                                } else {
                                    room.pending_reactions.remove(index);
                                    Some(reaction)
                                }
                            }
                            None => None,
                        }
                    };
                    if let Some(reaction) = removed {
                        let owner = reaction
                            .get("playerId")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let payload =
                            json!({ "id": reaction.get("id").cloned().unwrap_or(Value::Null) });
                        let _ = io.to(owner).emit("reactionResolved", &payload).await;
                    }
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "chatMessage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let body = trimmed(field(&data, "text"), LIMITS.max_chat_length);
                    if body.is_empty() {
                        return;
                    }
                    let mut to = String::new();
                    if session.role == ROLE_PLAYER && truthy(field(&data, "whisper")) {
                        to = DM_PLAYER_ID.to_string();
                    } else if session.role == ROLE_DM && truthy(field(&data, "to")) {
                        let wanted = js_string(field(&data, "to"));
                        let target = {
                            let room = table.room.lock().unwrap();
                            room.players.get(&wanted).cloned()
                        };
                        if let Some(target) = target {
                            if target.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER) {
                                to = wanted;
                            }
                        }
                    }
                    let to_name = if to.is_empty() {
                        String::new()
                    } else {
                        let room = table.room.lock().unwrap();
                        room.players
                            .get(&to)
                            .and_then(|player| player.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    };
                    let mut message = Map::new();
                    message.insert("from".to_string(), json!(session.name));
                    message.insert("playerId".to_string(), json!(session.player_id));
                    message.insert("role".to_string(), json!(session.role));
                    message.insert("text".to_string(), json!(body));
                    if !to.is_empty() {
                        message.insert("whisper".to_string(), json!(true));
                        message.insert("to".to_string(), json!(to));
                        if !to_name.is_empty() {
                            message.insert("toName".to_string(), json!(to_name));
                        }
                    }
                    message.insert("time".to_string(), json!(now_iso()));
                    let message = {
                        let mut room = table.room.lock().unwrap();
                        push_chat(&mut room, Value::Object(message))
                    };
                    table.mark_dirty();
                    log_chat(&table, &message);
                    if !to.is_empty() {
                        let _ = io.within("dm-sockets").emit("chatMessage", &message).await;
                        let _ = io.to(to.clone()).emit("chatMessage", &message).await;
                        if session.role == ROLE_PLAYER {
                            let _ = io.to(session.player_id).emit("chatMessage", &message).await;
                        }
                    } else {
                        let _ = io.emit("chatMessage", &message).await;
                    }
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "deleteChatMessage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let id = js_string(field(&data, "id"));
                    {
                        let mut room = table.room.lock().unwrap();
                        let Some(index) = room.chat_history.iter().position(|message| {
                            message.get("id").and_then(Value::as_str) == Some(id.as_str())
                        }) else {
                            return;
                        };
                        room.chat_history.remove(index);
                    }
                    table.mark_dirty();
                    let _ = io.emit("chatDeleted", &json!({ "id": id })).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "editChatMessage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let id = js_string(field(&data, "id"));
                    let parsed = {
                        let room = table.room.lock().unwrap();
                        room.chat_history
                            .iter()
                            .find(|message| {
                                message.get("id").and_then(Value::as_str) == Some(id.as_str())
                            })
                            .cloned()
                    };
                    let Some(message) = parsed else {
                        emit_error(&socket, "Сообщение не найдено");
                        ack_error(ack, "Сообщение не найдено");
                        return;
                    };
                    if truthy(field(&message, "system")) {
                        emit_error(&socket, "Системные сообщения не редактируются");
                        ack_error(ack, "Системные сообщения не редактируются");
                        return;
                    }
                    let body = trimmed(field(&data, "text"), LIMITS.max_chat_length);
                    if body.is_empty() {
                        emit_error(&socket, "Текст сообщения пуст");
                        ack_error(ack, "Текст сообщения пуст");
                        return;
                    }
                    let edited_at = now_iso();
                    {
                        let mut room = table.room.lock().unwrap();
                        if let Some(target) = room.chat_history.iter_mut().find(|item| {
                            item.get("id").and_then(Value::as_str) == Some(id.as_str())
                        }) {
                            target["text"] = json!(body);
                            target["editedAt"] = json!(edited_at);
                        }
                    }
                    table.mark_dirty();
                    let update = json!({ "id": id, "text": body, "editedAt": edited_at });
                    if truthy(field(&message, "whisper")) {
                        let _ = io.within("dm-sockets").emit("chatEdited", &update).await;
                        if let Some(to) = message.get("to").and_then(Value::as_str) {
                            let _ = io.to(to.to_string()).emit("chatEdited", &update).await;
                        }
                        if message.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER) {
                            if let Some(player_id) = message.get("playerId").and_then(Value::as_str)
                            {
                                if !player_id.is_empty() {
                                    let _ = io
                                        .to(player_id.to_string())
                                        .emit("chatEdited", &update)
                                        .await;
                                }
                            }
                        }
                    } else {
                        let _ = io.emit("chatEdited", &update).await;
                    }
                    let _ = ack.send(&json!({ "ok": true, "message": update }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on("clearChat", move |socket: SocketRef, io: SocketIo| {
            let table = table.clone();
            async move {
                if require_dm(&socket, &table).is_none() {
                    return;
                }
                {
                    let mut room = table.room.lock().unwrap();
                    if room.chat_history.is_empty() {
                        return;
                    }
                    room.chat_history.clear();
                }
                table.mark_dirty();
                let _ = io.emit("chatCleared", &Value::Null).await;
            }
        });
    }

    {
        let table = table.clone();
        socket.on(
            "chatImage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_dm(&socket, &table) else {
                        return;
                    };
                    let decoded = match media::decode_image(
                        data.get("image").and_then(Value::as_str),
                        LIMITS.max_image_bytes as usize,
                    ) {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            emit_error(&socket, &error);
                            return;
                        }
                    };
                    let mut to = String::new();
                    if let Some(wanted) = data.get("to").filter(|value| truthy(value)) {
                        let wanted = js_string(wanted);
                        let target = {
                            let room = table.room.lock().unwrap();
                            room.players.get(&wanted).cloned()
                        };
                        let Some(target) = target else {
                            emit_error(&socket, "Игрок для изображения не найден");
                            return;
                        };
                        if target.get("role").and_then(Value::as_str) != Some(ROLE_PLAYER) {
                            emit_error(&socket, "Игрок для изображения не найден");
                            return;
                        }
                        to = wanted;
                    }
                    let to_name = if to.is_empty() {
                        String::new()
                    } else {
                        let room = table.room.lock().unwrap();
                        room.players
                            .get(&to)
                            .and_then(|player| player.get("name"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string()
                    };
                    let Some(store) = table.store.clone() else {
                        emit_error(&socket, "Не получилось сохранить изображение");
                        return;
                    };
                    let Some(campaign_id) = table.campaign_id() else {
                        emit_error(&socket, "Не получилось сохранить изображение");
                        return;
                    };
                    let file_name =
                        match store.save_image(&campaign_id, &decoded.bytes, decoded.ext) {
                            Ok(name) => name,
                            Err(error) => {
                                emit_error(
                                    &socket,
                                    &save_error(&error, "Не получилось сохранить изображение"),
                                );
                                return;
                            }
                        };
                    let body = trimmed(field(&data, "text"), LIMITS.max_chat_length);
                    let mut message = Map::new();
                    message.insert("from".to_string(), json!(session.name));
                    message.insert("playerId".to_string(), json!(session.player_id));
                    message.insert("role".to_string(), json!(session.role));
                    message.insert("text".to_string(), json!(body));
                    message.insert("image".to_string(), json!(format!("/files/{file_name}")));
                    if !to.is_empty() {
                        message.insert("whisper".to_string(), json!(true));
                        message.insert("to".to_string(), json!(to));
                        if !to_name.is_empty() {
                            message.insert("toName".to_string(), json!(to_name));
                        }
                    }
                    message.insert("time".to_string(), json!(now_iso()));
                    let message = {
                        let mut room = table.room.lock().unwrap();
                        push_chat(&mut room, Value::Object(message))
                    };
                    table.mark_dirty();
                    log_chat(&table, &message);
                    if !to.is_empty() {
                        let _ = io.within("dm-sockets").emit("chatMessage", &message).await;
                        let _ = io.to(to).emit("chatMessage", &message).await;
                    } else {
                        let _ = io.emit("chatMessage", &message).await;
                    }
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "startVote",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let question = trimmed(field(&data, "question"), 120);
                    let options: Vec<Value> = data
                        .get("options")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|item| trimmed(item, 40))
                        .filter(|item| !item.is_empty())
                        .take(6)
                        .map(Value::String)
                        .collect();
                    if question.is_empty() {
                        emit_error(&socket, "Нужен вопрос опроса");
                        return;
                    }
                    if options.len() < 2 {
                        emit_error(&socket, "Нужно минимум два варианта");
                        return;
                    }
                    let option_texts = options
                        .iter()
                        .filter_map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(" / ");
                    let message = {
                        let mut room = table.room.lock().unwrap();
                        room.vote = Some(json!({
                            "question": question,
                            "options": options,
                            "votes": {}
                        }));
                        push_chat(
                            &mut room,
                            system_message(format!("Опрос: {question} ({option_texts})")),
                        )
                    };
                    table.mark_dirty();
                    log_chat(&table, &message);
                    let _ = io.emit("chatMessage", &message).await;
                    emit_vote(&io, &table).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "voteCast",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let index = data.get("optionIndex").and_then(js_integer);
                    let error = {
                        let mut room = table.room.lock().unwrap();
                        if room.vote.is_none() {
                            Some("Опрос не идёт")
                        } else {
                            let length = room
                                .vote
                                .as_ref()
                                .and_then(|vote| vote.get("options"))
                                .and_then(Value::as_array)
                                .map(|list| list.len())
                                .unwrap_or(0);
                            match index {
                                Some(index) if index >= 0 && (index as usize) < length => {
                                    if let Some(vote) = room.vote.as_mut() {
                                        if let Some(votes) =
                                            vote.get_mut("votes").and_then(Value::as_object_mut)
                                        {
                                            votes.insert(session.player_id.clone(), json!(index));
                                        }
                                    }
                                    None
                                }
                                _ => Some("Нет такого варианта"),
                            }
                        }
                    };
                    if let Some(error) = error {
                        emit_error(&socket, error);
                        return;
                    }
                    emit_vote(&io, &table).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on("voteClosed", move |socket: SocketRef, io: SocketIo| {
            let table = table.clone();
            async move {
                if require_dm(&socket, &table).is_none() {
                    return;
                }
                {
                    let mut room = table.room.lock().unwrap();
                    room.vote = None;
                }
                let _ = io.emit("voteUpdated", &Value::Null).await;
            }
        });
    }

    {
        let table = table.clone();
        socket.on(
            "undoDmAction",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(_data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let restored = {
                        let mut room = table.room.lock().unwrap();
                        undo_last(&mut room)
                    };
                    if !restored {
                        ack_error(ack, "Нечего отменять");
                        return;
                    }
                    table.mark_dirty();
                    broadcast_snapshot(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true }));
                }
            },
        );
    }
}
