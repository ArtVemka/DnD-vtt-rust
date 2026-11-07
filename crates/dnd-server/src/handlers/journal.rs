//! Journal, adventure, scenes.

use std::path::PathBuf;
use std::sync::Arc;

use dnd_core::campaign::CampaignError;
use dnd_core::content;
use dnd_core::media;
use dnd_core::room::{ROLE_DM, RoomState};
use dnd_core::rules::MAP_TYPES;
use dnd_core::session_log::export_session_markdown;
use dnd_core::sheet::{self, LIMITS};
use serde_json::{Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{AckSender, Data, SocketRef};

use super::{require_dm, require_joined};
use crate::table::Table;

const TOKEN_PALETTE: [&str; 8] = [
    "#c96f4a", "#4a8fc9", "#6fb45f", "#b45fa0", "#c9a34a", "#5f7fb4", "#a05f5f", "#4ab4a0",
];

fn emit_error(socket: &SocketRef, message: &str) {
    let _ = socket.emit("errorMessage", &json!({ "message": message }));
}

fn ack_error(ack: AckSender, message: &str) {
    let _ = ack.send(&json!({ "ok": false, "error": message }));
}

fn js_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number
            .as_f64()
            .is_some_and(|num| num != 0.0 && !num.is_nan()),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

fn js_number(value: Option<&Value>) -> f64 {
    match value {
        None | Some(Value::Null) => 0.0,
        Some(Value::Bool(flag)) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                0.0
            } else {
                trimmed.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        _ => f64::NAN,
    }
}

/// `groupKey`: name without case and extra spaces.
fn group_key(name: &str) -> String {
    name.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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

fn save_journal_entry(room: &mut RoomState, entry: Value) -> Value {
    push_undo(room);
    let id = entry
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Some(index) = room
        .journal_entries
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(id.as_str()))
    {
        room.journal_entries[index] = entry.clone();
    } else {
        room.journal_entries.push(entry.clone());
        if room.journal_entries.len() > LIMITS.max_journal_entries as usize {
            room.journal_entries.remove(0);
        }
    }
    entry
}

fn delete_journal_entry(room: &mut RoomState, id: &str) -> bool {
    let next: Vec<Value> = room
        .journal_entries
        .iter()
        .filter(|item| item.get("id").and_then(Value::as_str) != Some(id))
        .cloned()
        .collect();
    if next.len() == room.journal_entries.len() {
        return false;
    }
    push_undo(room);
    room.journal_entries = next;
    true
}

async fn broadcast_journal(io: &SocketIo, table: &Arc<Table>) {
    let (all, visible) = {
        let room = table.room.lock().unwrap();
        let visible = room
            .journal_entries
            .iter()
            .filter(|entry| js_truthy(entry.get("visible")))
            .cloned()
            .collect();
        (
            Value::Array(room.journal_entries.clone()),
            Value::Array(visible),
        )
    };
    let _ = io.within("dm-sockets").emit("journalUpdated", &all).await;
    let _ = io
        .within("table")
        .except("dm-sockets")
        .emit("journalUpdated", &visible)
        .await;
}

fn new_actor_id() -> String {
    format!("actor-{:08x}", rand::random::<u32>())
}

fn new_spell_id() -> String {
    format!("spell-{:08x}", rand::random::<u32>())
}

/// `data/srd.json`: working dir first, then the reference tree next to this
/// workspace (the Rust port does not ship a copy of the compendium).
fn load_srd() -> Option<Value> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let candidates = [
        PathBuf::from("data/srd.json"),
        PathBuf::from("server/data/srd.json"),
        manifest.join("data/srd.json"),
        manifest.join("../../../DnD/server/data/srd.json"),
        manifest.join("../../data/srd.json"),
    ];
    for path in candidates {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
        if let Ok(value) = serde_json::from_str::<Value>(text) {
            return Some(value);
        }
    }
    None
}

fn section_groups(adventure: &Value, section: &str, key: &str) -> usize {
    adventure
        .get(section)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|entry| {
                    group_key(entry.get("name").and_then(Value::as_str).unwrap_or("")) == key
                })
                .count()
        })
        .unwrap_or(0)
}

async fn emit_adventure(io: &SocketIo, adventure: &Value) {
    let _ = io
        .within("dm-sockets")
        .emit("adventureUpdated", adventure)
        .await;
}

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    {
        let table = table.clone();
        socket.on(
            "saveJournal",
            move |socket: SocketRef, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let text = sheet::sanitize_text(
                        data.get("text").unwrap_or(&Value::Null),
                        LIMITS.max_journal,
                    );
                    {
                        let mut room = table.room.lock().unwrap();
                        push_undo(&mut room);
                        room.journal = text.clone();
                    }
                    table.mark_dirty();
                    let _ = ack.send(&json!({ "ok": true, "text": text }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "sessionLogExport",
            move |socket: SocketRef, Data::<Value>(_data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Экспорт доступен только мастеру");
                        return;
                    }
                    let campaign_id = table.campaign_id();
                    let result = match (&table.store, campaign_id) {
                        (Some(store), Some(id)) => store
                            .dir_for(&id)
                            .map(|dir| export_session_markdown(&dir))
                            .unwrap_or_else(|_| (String::new(), "session.md".to_string())),
                        _ => (String::new(), "session.md".to_string()),
                    };
                    if result.0.is_empty() {
                        ack_error(ack, "Хронология сессии пуста");
                        return;
                    }
                    let _ = ack.send(&json!({ "ok": true, "text": result.0, "name": result.1 }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "saveJournalEntry",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let entry = content::normalize_journal_entry(&data);
                    if entry.is_null() {
                        emit_error(&socket, "Нужен заголовок записи");
                        ack_error(ack, "Нужен заголовок записи");
                        return;
                    }
                    let entry = {
                        let mut room = table.room.lock().unwrap();
                        save_journal_entry(&mut room, entry)
                    };
                    table.mark_dirty();
                    broadcast_journal(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true, "entry": entry }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "journalImage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let decoded = media::decode_image(
                        data.get("image").and_then(Value::as_str),
                        LIMITS.max_image_bytes as usize,
                    );
                    let decoded = match decoded {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            emit_error(&socket, &error);
                            ack_error(ack, &error);
                            return;
                        }
                    };
                    let saved = match (&table.store, table.campaign_id()) {
                        (Some(store), Some(id)) => {
                            store.save_image(&id, &decoded.bytes, decoded.ext).ok()
                        }
                        _ => None,
                    };
                    let Some(saved) = saved else {
                        emit_error(&socket, "Не удалось сохранить снимок");
                        ack_error(ack, "Не удалось сохранить снимок");
                        return;
                    };
                    let title = {
                        let text = sheet::sanitize_text(
                            data.get("title").unwrap_or(&Value::Null),
                            LIMITS.max_journal_title,
                        );
                        let text = text.trim();
                        if text.is_empty() {
                            "Скриншот стола".to_string()
                        } else {
                            text.to_string()
                        }
                    };
                    let entry = content::normalize_journal_entry(&json!({
                        "kind": "item",
                        "title": title,
                        "text": "",
                        "image": format!("/files/{saved}")
                    }));
                    let entry = {
                        let mut room = table.room.lock().unwrap();
                        save_journal_entry(&mut room, entry)
                    };
                    table.mark_dirty();
                    broadcast_journal(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true, "entry": entry }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "deleteJournalEntry",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let id = data.get("id").and_then(Value::as_str).unwrap_or("");
                    let deleted = {
                        let mut room = table.room.lock().unwrap();
                        delete_journal_entry(&mut room, id)
                    };
                    if !deleted {
                        emit_error(&socket, "Запись не найдена");
                        ack_error(ack, "Запись не найдена");
                        return;
                    }
                    table.mark_dirty();
                    broadcast_journal(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "journalRequest",
            move |socket: SocketRef, Data::<Value>(_data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    let entries = {
                        let room = table.room.lock().unwrap();
                        room.journal_entries.clone()
                    };
                    let payload = if session.role == ROLE_DM {
                        Value::Array(entries)
                    } else {
                        Value::Array(
                            entries
                                .into_iter()
                                .filter(|entry| js_truthy(entry.get("visible")))
                                .collect(),
                        )
                    };
                    let _ = socket.emit("journalUpdated", &payload);
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "importSrd",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(_data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Импорт доступен только мастеру");
                        return;
                    }
                    let Some(srd) = load_srd() else {
                        ack_error(ack, "В компендиуме нет ничего нового");
                        return;
                    };
                    let mut adventure = {
                        let room = table.room.lock().unwrap();
                        room.adventure.clone().unwrap_or_else(|| {
                            sheet::normalize_adventure(&json!({
                                "name": "Приключение",
                                "source": "srd"
                            }))
                        })
                    };
                    let mut actors = adventure
                        .get("actors")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut spells = adventure
                        .get("spells")
                        .and_then(Value::as_array)
                        .cloned()
                        .unwrap_or_default();
                    let mut added_actors = 0;
                    let mut added_spells = 0;

                    if let Some(entries) = srd.get("actors").and_then(Value::as_array) {
                        for entry in entries {
                            let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
                            if name.is_empty() {
                                continue;
                            }
                            if actors
                                .iter()
                                .any(|item| item.get("name").and_then(Value::as_str) == Some(name))
                            {
                                continue;
                            }
                            if actors.len() >= LIMITS.max_adventure_actors as usize {
                                break;
                            }
                            actors.push(json!({
                                "id": new_actor_id(),
                                "name": name,
                                "type": "npc",
                                "image": "",
                                "favorite": false,
                                "statblock": sheet::normalize_statblock(
                                    entry.get("statblock").unwrap_or(&Value::Null)
                                ),
                                "sourceId": "srd"
                            }));
                            added_actors += 1;
                        }
                    }
                    if let Some(entries) = srd.get("spells").and_then(Value::as_array) {
                        for entry in entries {
                            let name = entry.get("name").and_then(Value::as_str).unwrap_or("");
                            if name.is_empty() {
                                continue;
                            }
                            if spells
                                .iter()
                                .any(|item| item.get("name").and_then(Value::as_str) == Some(name))
                            {
                                continue;
                            }
                            if spells.len() >= LIMITS.max_adventure_spells as usize {
                                break;
                            }
                            spells.push(json!({
                                "id": new_spell_id(),
                                "name": name,
                                "level": sheet::clamp_int(
                                    entry.get("level").unwrap_or(&Value::Null),
                                    0,
                                    9,
                                    0
                                ),
                                "school": sheet::sanitize_text(
                                    entry.get("school").unwrap_or(&Value::Null),
                                    40
                                ),
                                "text": sheet::sanitize_text(
                                    entry.get("text").unwrap_or(&Value::Null),
                                    LIMITS.max_sheet_text_length
                                ),
                                "sourceId": "srd"
                            }));
                            added_spells += 1;
                        }
                    }
                    if added_actors == 0 && added_spells == 0 {
                        ack_error(ack, "В компендиуме нет ничего нового");
                        return;
                    }
                    if let Some(object) = adventure.as_object_mut() {
                        object.insert("actors".to_string(), Value::Array(actors));
                        object.insert("spells".to_string(), Value::Array(spells));
                    }
                    {
                        let mut room = table.room.lock().unwrap();
                        room.adventure = Some(adventure.clone());
                    }
                    table.mark_dirty();
                    emit_adventure(&io, &adventure).await;
                    let _ = ack.send(&json!({
                        "ok": true,
                        "actors": added_actors,
                        "spells": added_spells
                    }));
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "setAdventureGroupImage",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let checked = {
                        let room = table.room.lock().unwrap();
                        match room.adventure.as_ref() {
                            None => Err("В кампании нет приключения"),
                            Some(_) => {
                                let section = data
                                    .get("section")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                if section != "items" && section != "actors" {
                                    Err("Картинку массово можно назначить только предметам и персонажам")
                                } else {
                                    let wanted = sheet::sanitize_text(
                                        data.get("name").unwrap_or(&Value::Null),
                                        LIMITS.max_name_length,
                                    );
                                    if wanted.is_empty() {
                                        Err("Не указано название группы")
                                    } else {
                                        Ok((section.to_string(), wanted))
                                    }
                                }
                            }
                        }
                    };
                    let (section, wanted) = match checked {
                        Ok(value) => value,
                        Err(message) => {
                            emit_error(&socket, message);
                            return;
                        }
                    };
                    let decoded = media::decode_image(
                        data.get("image").and_then(Value::as_str),
                        LIMITS.max_image_bytes as usize,
                    );
                    let decoded = match decoded {
                        Ok(decoded) => decoded,
                        Err(error) => {
                            emit_error(&socket, &error);
                            return;
                        }
                    };
                    let key = group_key(&wanted);
                    let has_group = {
                        let room = table.room.lock().unwrap();
                        room.adventure
                            .as_ref()
                            .map(|adventure| section_groups(adventure, &section, &key) > 0)
                            .unwrap_or(false)
                    };
                    if !has_group {
                        emit_error(&socket, "В приключении нет объектов с таким названием");
                        return;
                    }
                    let saved = match (&table.store, table.campaign_id()) {
                        (Some(store), Some(id)) => {
                            store.save_image(&id, &decoded.bytes, decoded.ext)
                        }
                        _ => {
                            emit_error(&socket, "Не получилось сохранить картинку");
                            return;
                        }
                    };
                    let file_name = match saved {
                        Ok(name) => name,
                        Err(CampaignError::Quota { message, .. }) => {
                            emit_error(&socket, &message);
                            return;
                        }
                        Err(_) => {
                            emit_error(&socket, "Не получилось сохранить картинку");
                            return;
                        }
                    };
                    let url = format!("/files/{file_name}");
                    let adventure = {
                        let mut room = table.room.lock().unwrap();
                        let Some(adventure) = room.adventure.as_mut() else {
                            return;
                        };
                        if let Some(items) = adventure
                            .get_mut(section.as_str())
                            .and_then(Value::as_array_mut)
                        {
                            for entry in items.iter_mut() {
                                if group_key(
                                    entry.get("name").and_then(Value::as_str).unwrap_or(""),
                                ) == key
                                {
                                    entry["image"] = json!(url);
                                }
                            }
                        }
                        adventure.clone()
                    };
                    table.mark_dirty();
                    emit_adventure(&io, &adventure).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "deleteAdventureActor",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    if table.room.lock().unwrap().adventure.is_none() {
                        emit_error(&socket, "В кампании нет приключения");
                        return;
                    }
                    let name = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        LIMITS.max_adventure_title,
                    )
                    .trim()
                    .to_string();
                    if name.is_empty() {
                        emit_error(&socket, "Не указано название персонажа");
                        return;
                    }
                    let key = group_key(&name);
                    let (adventure, removed) = {
                        let mut room = table.room.lock().unwrap();
                        if room.adventure.is_none() {
                            emit_error(&socket, "В кампании нет приключения");
                            return;
                        }
                        let adventure = room.adventure.as_mut().unwrap();
                        let mut removed = false;
                        if let Some(items) =
                            adventure.get_mut("actors").and_then(Value::as_array_mut)
                        {
                            let before = items.len();
                            items.retain(|entry| {
                                group_key(entry.get("name").and_then(Value::as_str).unwrap_or(""))
                                    != key
                            });
                            removed = items.len() != before;
                        }
                        (adventure.clone(), removed)
                    };
                    if !removed {
                        emit_error(&socket, "В приключении нет такого персонажа");
                        return;
                    }
                    table.mark_dirty();
                    emit_adventure(&io, &adventure).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "deleteAdventureSpell",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    if table.room.lock().unwrap().adventure.is_none() {
                        emit_error(&socket, "В кампании нет приключения");
                        return;
                    }
                    let name = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        LIMITS.max_adventure_title,
                    )
                    .trim()
                    .to_string();
                    if name.is_empty() {
                        emit_error(&socket, "Не указано название заклинания");
                        return;
                    }
                    let key = group_key(&name);
                    let (adventure, removed) = {
                        let mut room = table.room.lock().unwrap();
                        if room.adventure.is_none() {
                            emit_error(&socket, "В кампании нет приключения");
                            return;
                        }
                        let adventure = room.adventure.as_mut().unwrap();
                        let mut removed = false;
                        if let Some(items) =
                            adventure.get_mut("spells").and_then(Value::as_array_mut)
                        {
                            let before = items.len();
                            items.retain(|entry| {
                                group_key(entry.get("name").and_then(Value::as_str).unwrap_or(""))
                                    != key
                            });
                            removed = items.len() != before;
                        }
                        (adventure.clone(), removed)
                    };
                    if !removed {
                        emit_error(&socket, "В приключении нет такого заклинания");
                        return;
                    }
                    table.mark_dirty();
                    emit_adventure(&io, &adventure).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "setAdventureActorFavorite",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    if table.room.lock().unwrap().adventure.is_none() {
                        emit_error(&socket, "В кампании нет приключения");
                        return;
                    }
                    let name = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        LIMITS.max_adventure_title,
                    )
                    .trim()
                    .to_string();
                    if name.is_empty() {
                        emit_error(&socket, "Не указано название персонажа");
                        return;
                    }
                    let favorite = js_truthy(data.get("favorite"));
                    let key = group_key(&name);
                    let (adventure, found) = {
                        let mut room = table.room.lock().unwrap();
                        if room.adventure.is_none() {
                            emit_error(&socket, "В кампании нет приключения");
                            return;
                        }
                        let adventure = room.adventure.as_mut().unwrap();
                        let mut found = 0;
                        if let Some(items) =
                            adventure.get_mut("actors").and_then(Value::as_array_mut)
                        {
                            for entry in items.iter_mut() {
                                if group_key(
                                    entry.get("name").and_then(Value::as_str).unwrap_or(""),
                                ) == key
                                {
                                    entry["favorite"] = json!(favorite);
                                    found += 1;
                                }
                            }
                        }
                        (adventure.clone(), found)
                    };
                    if found == 0 {
                        emit_error(&socket, "В приключении нет такого персонажа");
                        return;
                    }
                    table.mark_dirty();
                    emit_adventure(&io, &adventure).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "setAdventureGroupStatblock",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let name = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        LIMITS.max_adventure_title,
                    )
                    .trim()
                    .to_string();
                    if name.is_empty() {
                        emit_error(&socket, "Не указано название персонажа");
                        return;
                    }
                    let create = js_truthy(data.get("create"));
                    let key = group_key(&name);
                    let mut error: Option<&'static str> = None;
                    let adventure = {
                        let mut room = table.room.lock().unwrap();
                        if room.adventure.is_none() {
                            room.adventure = Some(sheet::normalize_adventure(&json!({
                                "name": "Приключение",
                                "source": "вручную",
                                "actors": []
                            })));
                        }
                        let adventure = room.adventure.as_mut().unwrap();
                        let mut group = adventure
                            .get("actors")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|entry| {
                                group_key(entry.get("name").and_then(Value::as_str).unwrap_or(""))
                                    == key
                            })
                            .count();
                        if group == 0 {
                            if !create {
                                error = Some("В приключении нет персонажа с таким названием");
                            } else if adventure
                                .get("actors")
                                .and_then(Value::as_array)
                                .map(|items| items.len() >= LIMITS.max_adventure_actors as usize)
                                .unwrap_or(false)
                            {
                                error = Some("В приключении уже слишком много персонажей");
                            } else {
                                let actor = json!({
                                    "id": new_actor_id(),
                                    "name": name,
                                    "type": "npc",
                                    "image": "",
                                    "statblock": null,
                                    "sourceId": ""
                                });
                                if let Some(items) =
                                    adventure.get_mut("actors").and_then(Value::as_array_mut)
                                {
                                    items.push(actor);
                                }
                                group = 1;
                            }
                        }
                        if error.is_none() && group > 0 {
                            let block = sheet::normalize_statblock(
                                data.get("statblock").unwrap_or(&Value::Null),
                            );
                            if let Some(items) =
                                adventure.get_mut("actors").and_then(Value::as_array_mut)
                            {
                                for entry in items.iter_mut() {
                                    if group_key(
                                        entry.get("name").and_then(Value::as_str).unwrap_or(""),
                                    ) == key
                                    {
                                        entry["statblock"] = block.clone();
                                    }
                                }
                            }
                        }
                        adventure.clone()
                    };
                    if let Some(message) = error {
                        emit_error(&socket, message);
                        return;
                    }
                    table.mark_dirty();
                    emit_adventure(&io, &adventure).await;
                }
            },
        );
    }

    {
        let table = table.clone();
        socket.on(
            "createTokenFromAdventureActor",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        ack_error(ack, "Это действие доступно только ДМ");
                        return;
                    }
                    let name = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        LIMITS.max_adventure_title,
                    )
                    .trim()
                    .to_string();
                    if name.is_empty() {
                        emit_error(&socket, "Не указано название персонажа");
                        ack_error(ack, "Не указано название персонажа");
                        return;
                    }
                    let mut error: Option<&'static str> = None;
                    let mut created = Vec::new();
                    let mut payloads = Vec::new();
                    {
                        let mut room = table.room.lock().unwrap();
                        if room.adventure.is_none() {
                            error = Some("В кампании нет приключения");
                        } else if room.tokens.len() >= LIMITS.max_tokens as usize {
                            error = Some("Слишком много фишек на столе");
                        } else {
                            let key = group_key(&name);
                            let actor = room
                                .adventure
                                .as_ref()
                                .unwrap()
                                .get("actors")
                                .and_then(Value::as_array)
                                .and_then(|items| {
                                    items.iter().find(|entry| {
                                        group_key(
                                            entry.get("name").and_then(Value::as_str).unwrap_or(""),
                                        ) == key
                                    })
                                })
                                .cloned();
                            if actor.is_none() {
                                error = Some("В приключении нет такого персонажа");
                            }
                            if let Some(actor) = actor {
                                let block = actor.get("statblock").cloned().unwrap_or(json!({}));
                                let block = if block.is_object() { block } else { json!({}) };
                                let requested_map = data
                                    .get("mapType")
                                    .and_then(Value::as_str)
                                    .unwrap_or("");
                                let map_type = if MAP_TYPES.contains(&requested_map) {
                                    requested_map.to_string()
                                } else {
                                    room.active_map.clone().unwrap_or_else(|| "battle".to_string())
                                };
                                let max_hp = {
                                    let hp = js_number(block.get("hp"));
                                    if hp.is_finite() && hp > 0.0 {
                                        json!(sheet::clamp_int(
                                            block.get("hp").unwrap_or(&Value::Null),
                                            1,
                                            LIMITS.max_token_hp,
                                            1
                                        ))
                                    } else {
                                        Value::Null
                                    }
                                };
                                let image_text = sheet::normalize_adventure_image(
                                    actor.get("image").unwrap_or(&Value::Null),
                                );
                                let image = if image_text.is_empty() {
                                    Value::Null
                                } else {
                                    json!(image_text)
                                };
                                let requested = sheet::clamp_int(
                                    data.get("count").unwrap_or(&Value::Null),
                                    1,
                                    20,
                                    1,
                                ) as usize;
                                let free = LIMITS.max_tokens as usize - room.tokens.len();
                                if free == 0 {
                                    error = Some("Лимит фишек на столе уже достигнут");
                                } else {
                                    let count = requested.min(free);
                                    let base_x = sheet::clamp_number(
                                        data.get("x").unwrap_or(&Value::Null),
                                        0.0,
                                        1.0,
                                        0.5,
                                    );
                                    let base_y = sheet::clamp_number(
                                        data.get("y").unwrap_or(&Value::Null),
                                        0.0,
                                        1.0,
                                        0.5,
                                    );
                                    let rows = count.div_ceil(3);
                                    let map_id = sheet::sanitize_text(
                                        data.get("mapId").unwrap_or(&Value::Null),
                                        80,
                                    );
                                    for index in 0..count {
                                        let x = sheet::clamp_number(
                                            &json!(
                                                base_x
                                                    + ((index % 3) as f64 - 1.0) * 0.05
                                            ),
                                            0.02,
                                            0.98,
                                            0.5,
                                        );
                                        let y = sheet::clamp_number(
                                            &json!(
                                                base_y
                                                    + ((index / 3) as f64
                                                        - (rows as f64 - 1.0) / 2.0)
                                                        * 0.05
                                            ),
                                            0.02,
                                            0.98,
                                            0.5,
                                        );
                                        let armor_class = {
                                            let ac = js_number(block.get("ac"));
                                            if ac.is_finite() && ac >= 1.0 {
                                                json!(sheet::clamp_int(
                                                    block.get("ac").unwrap_or(&Value::Null),
                                                    1,
                                                    60,
                                                    10
                                                ))
                                            } else {
                                                Value::Null
                                            }
                                        };
                                        let token = json!({
                                            "id": content::new_token_id(),
                                            "name": actor.get("name").cloned().unwrap_or(json!("Фишка")),
                                            "mapType": map_type,
                                            "mapId": map_id,
                                            "x": x,
                                            "y": y,
                                            "imageUrl": image,
                                            "imageZoom": actor.get("imageZoom").cloned().unwrap_or(json!(1)),
                                            "imageX": actor.get("imageX").cloned().unwrap_or(json!(0)),
                                            "imageY": actor.get("imageY").cloned().unwrap_or(json!(0)),
                                            "assignedPlayerId": Value::Null,
                                            "side": "enemy",
                                            "armorClass": armor_class,
                                            "attackBonus": 0,
                                            "movedCells": 0,
                                            "hidden": true,
                                            "color": TOKEN_PALETTE[room.tokens.len() % TOKEN_PALETTE.len()],
                                            "size": sheet::clamp_int(
                                                block.get("size").unwrap_or(&Value::Null),
                                                1,
                                                4,
                                                1
                                            ),
                                            "maxHp": max_hp,
                                            "hp": max_hp,
                                            "conditions": [],
                                            "damageResistances": [],
                                            "damageVulnerabilities": [],
                                            "damageImmunities": [],
                                            "vision": 0
                                        });
                                        room.tokens.push(token.clone());
                                        payloads.push(room.token_for_viewer(&token, true));
                                        created.push(token);
                                    }
                                }
                            }
                        }
                    }
                    if let Some(message) = error {
                        emit_error(&socket, message);
                        ack_error(ack, message);
                        return;
                    }
                    table.mark_dirty();
                    for payload in payloads {
                        let _ = io.within("dm-sockets").emit("tokenCreated", &payload).await;
                    }
                    let _ = ack.send(&json!({ "ok": true, "tokens": created }));
                }
            },
        );
    }
}
