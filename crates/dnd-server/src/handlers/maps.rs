//! Maps, fog, walls, lights, templates, map pings, scenes.
//!
//! Port of the map handlers in `server/socket.js` and the scene helpers in
//! `server/state.js`. Map items stay JSON because the renderer consumes the same
//! shape the reference produced.

use std::collections::{BTreeMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use dnd_core::campaign::CampaignError;
use dnd_core::content;
use dnd_core::media;
use dnd_core::model::MAP_TYPES;
use dnd_core::room::{RoomState, Viewer};
use dnd_core::rules;
use dnd_core::sheet::{self, LIMITS};
use serde_json::{Map, Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{AckSender, Data, SocketRef};

use crate::handlers::{now_ms, require_dm, require_joined};
use crate::table::Table;

/// `MAP_NAME_MAX` in `server/state.js`.
const MAP_NAME_MAX: i64 = 120;

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    {
        let handler_table = table.clone();
        socket.on(
            "uploadMap",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind = data
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !MAP_TYPES.contains(&kind.as_str()) {
                        emit_error(&socket, "Неизвестный тип карты");
                        return;
                    }
                    let count = {
                        let room = table.room.lock().unwrap();
                        map_items(&room, &kind).len()
                    };
                    if count >= LIMITS.max_maps_per_type as usize {
                        emit_error(
                            &socket,
                            &format!(
                                "В этой вкладке уже {} карт — удалите лишние",
                                LIMITS.max_maps_per_type
                            ),
                        );
                        return;
                    }
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
                    let Some(store) = table.store.clone() else {
                        emit_error(&socket, "Не удалось сохранить изображение карты");
                        return;
                    };
                    let Some(campaign_id) = table.campaign_id() else {
                        emit_error(&socket, "Не удалось сохранить изображение карты");
                        return;
                    };
                    let file_name =
                        match store.save_image(&campaign_id, &decoded.bytes, decoded.ext) {
                            Ok(name) => name,
                            Err(error) => {
                                let message = match &error {
                                    CampaignError::Quota { message, .. } => message.clone(),
                                    _ => "Не удалось сохранить изображение карты".to_string(),
                                };
                                emit_error(&socket, &message);
                                return;
                            }
                        };
                    let name = sheet::sanitize_text(
                        data.get("name").unwrap_or(&Value::Null),
                        MAP_NAME_MAX,
                    )
                    .trim()
                    .to_string();
                    let map_id = content::new_map_id();
                    let map = json!({
                        "id": map_id,
                        "url": format!("/files/{file_name}"),
                        "name": if name.is_empty() { "Карта".to_string() } else { name },
                        "published": false,
                        "fogMask": rules::new_fog_mask(),
                        "audioRef": Value::Null,
                        "surface": { "cells": {} },
                        "walls": [],
                        "lights": [],
                        "darkness": 0,
                        "uploadedAt": now_iso(),
                    });
                    {
                        let mut room = table.room.lock().unwrap();
                        push_undo(&mut room);
                        if let Some(items) = map_items_mut(&mut room, &kind) {
                            items.push(map);
                        }
                        if let Some(slot) = room.maps.get_mut(&kind) {
                            slot["activeId"] = json!(map_id);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "selectMap",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind = data
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !MAP_TYPES.contains(&kind.as_str()) {
                        emit_error(&socket, "Неизвестный тип карты");
                        return;
                    }
                    let map_id = js_string_opt(data.get("mapId")).unwrap_or_default();
                    {
                        let mut room = table.room.lock().unwrap();
                        let exists = map_items(&room, &kind).iter().any(|item| {
                            item.get("id").and_then(Value::as_str) == Some(map_id.as_str())
                        });
                        if !exists {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        }
                        if let Some(slot) = room.maps.get_mut(&kind) {
                            slot["activeId"] = json!(map_id);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "deleteMap",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind = data
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !MAP_TYPES.contains(&kind.as_str()) {
                        emit_error(&socket, "Неизвестный тип карты");
                        return;
                    }

                    let scenes_changed;
                    let mut other_type: Option<String> = None;
                    {
                        let mut room = table.room.lock().unwrap();
                        let requested = match data.get("mapId") {
                            Some(value) if js_truthy(value) => js_string(value),
                            _ => room
                                .maps
                                .get(&kind)
                                .and_then(|slot| slot.get("activeId"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        };
                        let items = map_items(&room, &kind);
                        let Some(index) = items.iter().position(|item| {
                            item.get("id").and_then(Value::as_str) == Some(requested.as_str())
                        }) else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let was_presented = room.active_map.as_deref() == Some(kind.as_str())
                            && room.active_map_id.as_deref() == Some(requested.as_str());
                        let active_was = room
                            .maps
                            .get(&kind)
                            .and_then(|slot| slot.get("activeId"))
                            .and_then(Value::as_str)
                            == Some(requested.as_str());

                        push_undo(&mut room);
                        if let Some(items) = map_items_mut(&mut room, &kind) {
                            items.remove(index);
                        }

                        // A scene without its map is dead weight in the list.
                        let before = room.scenes.len();
                        room.scenes.retain(|scene| {
                            !(scene.get("mapType").and_then(Value::as_str) == Some(kind.as_str())
                                && scene.get("mapId").and_then(Value::as_str)
                                    == Some(requested.as_str()))
                        });
                        scenes_changed = room.scenes.len() != before;

                        if active_was {
                            let remaining = map_items(&room, &kind);
                            let neighbour = remaining.get(index).or_else(|| {
                                index
                                    .checked_sub(1)
                                    .and_then(|previous| remaining.get(previous))
                            });
                            let next_id = neighbour
                                .and_then(|item| item.get("id"))
                                .cloned()
                                .unwrap_or(Value::Null);
                            if let Some(slot) = room.maps.get_mut(&kind) {
                                slot["activeId"] = next_id;
                            }
                        }

                        if was_presented {
                            let next_id = map_items(&room, &kind)
                                .iter()
                                .find(|item| {
                                    !matches!(item.get("published"), Some(Value::Bool(false)))
                                })
                                .and_then(|item| item.get("id"))
                                .and_then(Value::as_str)
                                .map(str::to_string);
                            match next_id {
                                Some(next_id) => {
                                    if let Some(slot) = room.maps.get_mut(&kind) {
                                        slot["activeId"] = json!(next_id);
                                    }
                                    room.active_map = Some(kind.clone());
                                    room.active_map_id = Some(next_id);
                                }
                                None => {
                                    let fallback = fallback_published(&room, &kind);
                                    match fallback {
                                        Some((fallback_type, fallback_id)) => {
                                            if let Some(slot) = room.maps.get_mut(&fallback_type) {
                                                slot["activeId"] = json!(fallback_id);
                                            }
                                            room.active_map = Some(fallback_type.clone());
                                            room.active_map_id = Some(fallback_id);
                                            other_type = Some(fallback_type);
                                        }
                                        None => {
                                            room.active_map = None;
                                            room.active_map_id = None;
                                        }
                                    }
                                }
                            }
                        }
                        table.mark_dirty();
                    }

                    if scenes_changed {
                        emit_scenes(&io, &table).await;
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                    if let Some(other) = other_type {
                        broadcast_map_update(&io, &table, &other).await;
                    }
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setActiveMap",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind = data
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    if !MAP_TYPES.contains(&kind.as_str()) {
                        emit_error(&socket, "Неизвестный тип карты");
                        return;
                    }

                    let showed;
                    let mut tokens = Vec::new();
                    {
                        let mut room = table.room.lock().unwrap();
                        let map_id = match data.get("mapId") {
                            Some(value) if js_truthy(value) => js_string(value),
                            _ => room
                                .maps
                                .get(&kind)
                                .and_then(|slot| slot.get("activeId"))
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .to_string(),
                        };
                        let items = map_items(&room, &kind);
                        let Some(index) = items.iter().position(|item| {
                            item.get("id").and_then(Value::as_str) == Some(map_id.as_str())
                        }) else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let was_published =
                            !matches!(items[index].get("published"), Some(Value::Bool(false)));

                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["published"] = json!(true);
                        }
                        if let Some(slot) = room.maps.get_mut(&kind) {
                            slot["activeId"] = json!(map_id);
                        }
                        room.active_map = Some(kind.clone());
                        room.active_map_id = Some(map_id.clone());
                        if !was_published {
                            // Tokens prepared on the map were DM-only until now.
                            tokens = room
                                .tokens
                                .iter()
                                .filter(|token| {
                                    token.get("mapType").and_then(Value::as_str)
                                        == Some(kind.as_str())
                                        && token.get("mapId").and_then(Value::as_str)
                                            == Some(map_id.as_str())
                                })
                                .cloned()
                                .collect();
                        }
                        showed = !was_published;
                        table.mark_dirty();
                    }

                    broadcast_map_update(&io, &table, &kind).await;
                    if showed {
                        for token in tokens {
                            emit_token(&io, &table, "tokenCreated", &token).await;
                        }
                    }
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "openMapFog",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            let mask =
                                content::normalize_fog(item.get("fogMask").unwrap_or(&Value::Null));
                            let next = rules::open_fog_at(
                                &mask,
                                data.get("x").unwrap_or(&Value::Null),
                                data.get("y").unwrap_or(&Value::Null),
                                data.get("radius").unwrap_or(&Value::Null),
                            );
                            item["fogMask"] = Value::String(next);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapFog",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            let mask =
                                content::normalize_fog(item.get("fogMask").unwrap_or(&Value::Null));
                            let open = js_truthy(data.get("open").unwrap_or(&Value::Null));
                            item["fogMask"] = Value::String(rules::set_fog_whole(
                                mask.as_str().unwrap_or(""),
                                open,
                            ));
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapFogFollow",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let follow = js_truthy(data.get("follow").unwrap_or(&Value::Null));
                        if js_truthy(
                            map_item_ref(&room, &kind, index)
                                .and_then(|item| item.get("fogFollow"))
                                .unwrap_or(&Value::Null),
                        ) == follow
                        {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["fogFollow"] = json!(follow);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapVision",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let wanted = js_truthy(data.get("visionPerPlayer").unwrap_or(&Value::Null));
                        if js_truthy(
                            map_item_ref(&room, &kind, index)
                                .and_then(|item| item.get("visionPerPlayer"))
                                .unwrap_or(&Value::Null),
                        ) == wanted
                        {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["visionPerPlayer"] = json!(wanted);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "paintMapSurface",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(entries) = data.get("cells").and_then(Value::as_array) else {
                            return;
                        };
                        if entries.is_empty() {
                            return;
                        }
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let mut cells: Map<String, Value> = item
                            .get("surface")
                            .and_then(|surface| surface.get("cells"))
                            .and_then(Value::as_object)
                            .cloned()
                            .unwrap_or_default();
                        let existing = cells.len();
                        let surface_ids = rules::surface_ids();
                        let mut changed = 0;
                        for entry in entries.iter().take(400) {
                            let Some(parts) = entry.as_array() else {
                                continue;
                            };
                            if parts.len() != 3 {
                                continue;
                            }
                            let col = js_number(&parts[0]);
                            let row = js_number(&parts[1]);
                            if !col.is_finite()
                                || !row.is_finite()
                                || col.fract() != 0.0
                                || row.fract() != 0.0
                                || col < 0.0
                                || row < 0.0
                            {
                                continue;
                            }
                            let key = format!("{},{}", col as i64, row as i64);
                            let kind_name = match &parts[2] {
                                Value::Null => String::new(),
                                other => js_string(other),
                            };
                            if !kind_name.is_empty() && !surface_ids.contains(&kind_name.as_str()) {
                                continue;
                            }
                            let current = cells.get(&key).map(js_string).unwrap_or_default();
                            if current == kind_name {
                                continue;
                            }
                            changed += 1;
                            if kind_name.is_empty() {
                                cells.remove(&key);
                            } else {
                                cells.insert(key, Value::String(kind_name));
                            }
                        }
                        if changed == 0 {
                            return;
                        }
                        let limited =
                            rules::normalize_surface(&json!({ "cells": Value::Object(cells) }));
                        let remaining = limited
                            .get("cells")
                            .and_then(Value::as_object)
                            .map(|cells| cells.len())
                            .unwrap_or(0);
                        if remaining == 0 && existing == 0 {
                            return;
                        }
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["surface"] = limited;
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "clearMapSurface",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let empty = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("surface"))
                            .and_then(|surface| surface.get("cells"))
                            .and_then(Value::as_object)
                            .map(|cells| cells.is_empty())
                            .unwrap_or(true);
                        if empty {
                            return;
                        }
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["surface"] = json!({ "cells": {} });
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapDarkness",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let darkness = js_number(data.get("darkness").unwrap_or(&Value::Null));
                    if !darkness.is_finite() {
                        return;
                    }
                    let value = darkness.clamp(0.0, 1.0);
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            // The manual slider takes lighting away from the clock.
                            if js_truthy(item.get("autoLight").unwrap_or(&Value::Null)) {
                                item["autoLight"] = json!(false);
                            }
                            let current = js_number(item.get("darkness").unwrap_or(&Value::Null));
                            if current == value {
                                return;
                            }
                            item["darkness"] = content::normalize_darkness(&json!(value));
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapAutoLight",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let on = js_truthy(data.get("on").unwrap_or(&Value::Null));
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let current = js_truthy(
                            map_item_ref(&room, &kind, index)
                                .and_then(|item| item.get("autoLight"))
                                .unwrap_or(&Value::Null),
                        );
                        if current == on {
                            return;
                        }
                        let hour = room.game_time.get("hour").cloned().unwrap_or(Value::Null);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["autoLight"] = json!(on);
                            if on {
                                item["darkness"] = content::normalize_darkness(&json!(
                                    rules::darkness_for_hour(&hour)
                                ));
                            }
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapPin",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let pin_data = data.get("pin").cloned().unwrap_or(Value::Null);
                        let pin = pin_data.as_object();
                        let mut pin_id = pin
                            .and_then(|pin| pin.get("id"))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                            .or_else(|| js_string_opt(data.get("pinId")))
                            .unwrap_or_default();
                        pin_id = pin_id.chars().take(40).collect();
                        if pin_id.is_empty() {
                            pin_id = format!("pin-{:08x}", rand::random::<u32>());
                        }
                        let pin_x = pin
                            .and_then(|pin| pin.get("x"))
                            .map(|value| {
                                let number = js_number(value);
                                if number.is_nan() { 0.0 } else { number }
                            })
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0);
                        let pin_y = pin
                            .and_then(|pin| pin.get("y"))
                            .map(|value| {
                                let number = js_number(value);
                                if number.is_nan() { 0.0 } else { number }
                            })
                            .unwrap_or(0.0)
                            .clamp(0.0, 1.0);
                        let title = sheet::sanitize_text(
                            pin.and_then(|pin| pin.get("title")).unwrap_or(&Value::Null),
                            LIMITS.max_name_length,
                        )
                        .trim()
                        .to_string();
                        let text = sheet::sanitize_text(
                            pin.and_then(|pin| pin.get("text")).unwrap_or(&Value::Null),
                            LIMITS.max_sheet_text_length,
                        );
                        let pin = json!({
                            "id": pin_id,
                            "x": pin_x,
                            "y": pin_y,
                            "title": if title.is_empty() { "Пин".to_string() } else { title },
                            "text": text,
                        });
                        let mut pins: Vec<Value> = item
                            .get("pins")
                            .and_then(Value::as_array)
                            .map(|pins| pins.iter().filter(|pin| js_truthy(pin)).cloned().collect())
                            .unwrap_or_default();
                        match pins.iter().position(|entry| {
                            entry.get("id").and_then(Value::as_str) == Some(pin_id.as_str())
                        }) {
                            // The reference replaced the pin and then pushed it
                            // again; an edited pin must not be duplicated.
                            Some(position) => pins[position] = pin,
                            None => {
                                if pins.len() >= LIMITS.max_map_pins as usize {
                                    pins.remove(0);
                                }
                                pins.push(pin);
                            }
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["pins"] = Value::Array(pins);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "deleteMapPin",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let pin_id = js_string_opt(data.get("pinId")).unwrap_or_default();
                        let pins: Vec<Value> = item
                            .get("pins")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if !pins.iter().any(|pin| {
                            pin.get("id").and_then(Value::as_str) == Some(pin_id.as_str())
                        }) {
                            drop(room);
                            emit_error(&socket, "Булавка не найдена");
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["pins"] = Value::Array(
                                pins.into_iter()
                                    .filter(|pin| {
                                        pin.get("id").and_then(Value::as_str)
                                            != Some(pin_id.as_str())
                                    })
                                    .collect(),
                            );
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapWallDoor",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(walls) = item.get("walls").and_then(Value::as_array) else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let wall_id = js_string_opt(data.get("wallId")).unwrap_or_default();
                        let mut walls = walls.clone();
                        let Some(position) = walls.iter().position(|wall| {
                            wall.get("id").and_then(Value::as_str) == Some(wall_id.as_str())
                        }) else {
                            drop(room);
                            emit_error(&socket, "Стена не найдена");
                            return;
                        };
                        let mut changed = false;
                        if let Some(wall) = walls[position].as_object_mut() {
                            if data.get("door").is_some() {
                                match data.get("door") {
                                    Some(Value::Object(door)) => {
                                        wall.insert(
                                            "door".to_string(),
                                            json!({
                                                "open": matches!(door.get("open"), Some(Value::Bool(true))),
                                                "locked": matches!(door.get("locked"), Some(Value::Bool(true))),
                                            }),
                                        );
                                    }
                                    _ => {
                                        wall.remove("door");
                                    }
                                }
                                changed = true;
                            }
                            if data.get("window").is_some() {
                                if matches!(data.get("window"), Some(Value::Bool(true))) {
                                    wall.insert("window".to_string(), json!(true));
                                } else {
                                    wall.remove("window");
                                }
                                changed = true;
                            }
                        }
                        if !changed {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["walls"] = rules::normalize_walls(&Value::Array(walls));
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "addMapWall",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let wall_id = js_string_opt(data.get("wallId"))
                            .filter(|id| !id.is_empty())
                            .unwrap_or_else(content::new_token_id);
                        let wall = rules::normalize_wall(&json!({
                            "id": wall_id,
                            "x1": data.get("x1").cloned().unwrap_or(Value::Null),
                            "y1": data.get("y1").cloned().unwrap_or(Value::Null),
                            "x2": data.get("x2").cloned().unwrap_or(Value::Null),
                            "y2": data.get("y2").cloned().unwrap_or(Value::Null),
                        }));
                        if wall.is_null()
                            || wall
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .is_empty()
                        {
                            return;
                        }
                        let walls = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("walls"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if walls.len() >= rules::LIGHT_LIMITS.max_walls {
                            drop(room);
                            emit_error(
                                &socket,
                                &format!(
                                    "Стен больше {} быть не может",
                                    rules::LIGHT_LIMITS.max_walls
                                ),
                            );
                            return;
                        }
                        // The reference forgot the undo snapshot here, which made
                        // Ctrl+Z revert an older action.
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            if !item.get("walls").map(Value::is_array).unwrap_or(false) {
                                item["walls"] = json!([]);
                            }
                            if let Some(list) = item.get_mut("walls").and_then(Value::as_array_mut)
                            {
                                list.push(wall);
                            }
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "deleteMapWall",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let wall_id = js_string_opt(data.get("wallId")).unwrap_or_default();
                        let walls: Vec<Value> = item
                            .get("walls")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if !walls.iter().any(|wall| {
                            wall.get("id").and_then(Value::as_str) == Some(wall_id.as_str())
                        }) {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["walls"] = Value::Array(
                                walls
                                    .into_iter()
                                    .filter(|wall| {
                                        wall.get("id").and_then(Value::as_str)
                                            != Some(wall_id.as_str())
                                    })
                                    .collect(),
                            );
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "clearMapWalls",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let empty = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("walls"))
                            .and_then(Value::as_array)
                            .map(|walls| walls.is_empty())
                            .unwrap_or(true);
                        if empty {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["walls"] = json!([]);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "addMapTemplate",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let templates = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("templates"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if templates.len() >= content::TEMPLATE_LIMITS.max_per_map as usize {
                            drop(room);
                            emit_error(
                                &socket,
                                &format!(
                                    "Шаблонов больше {} быть не может",
                                    content::TEMPLATE_LIMITS.max_per_map
                                ),
                            );
                            return;
                        }
                        let template_id = js_string_opt(data.get("templateId"))
                            .filter(|id| !id.is_empty())
                            .unwrap_or_else(content::new_token_id);
                        let template = content::normalize_template(&json!({
                            "id": template_id,
                            "kind": data.get("kind").cloned().unwrap_or(Value::Null),
                            "x": data.get("x").cloned().unwrap_or(Value::Null),
                            "y": data.get("y").cloned().unwrap_or(Value::Null),
                            "size": data.get("size").cloned().unwrap_or(Value::Null),
                            "rotation": data.get("rotation").cloned().unwrap_or(Value::Null),
                            "color": data.get("color").cloned().unwrap_or(Value::Null),
                        }));
                        if template.is_null() {
                            return;
                        }
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            if !item.get("templates").map(Value::is_array).unwrap_or(false) {
                                item["templates"] = json!([]);
                            }
                            if let Some(list) =
                                item.get_mut("templates").and_then(Value::as_array_mut)
                            {
                                list.push(template);
                            }
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "deleteMapTemplate",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let template_id = js_string_opt(data.get("templateId")).unwrap_or_default();
                        let templates: Vec<Value> = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("templates"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if !templates.iter().any(|template| {
                            template.get("id").and_then(Value::as_str) == Some(template_id.as_str())
                        }) {
                            return;
                        }
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["templates"] = Value::Array(
                                templates
                                    .into_iter()
                                    .filter(|template| {
                                        template.get("id").and_then(Value::as_str)
                                            != Some(template_id.as_str())
                                    })
                                    .collect(),
                            );
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "clearMapTemplates",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let empty = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("templates"))
                            .and_then(Value::as_array)
                            .map(|templates| templates.is_empty())
                            .unwrap_or(true);
                        if empty {
                            return;
                        }
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["templates"] = json!([]);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "addMapLight",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let light_id = js_string_opt(data.get("lightId"))
                            .filter(|id| !id.is_empty())
                            .unwrap_or_else(content::new_token_id);
                        let light = rules::normalize_light(&json!({
                            "id": light_id,
                            "x": data.get("x").cloned().unwrap_or(Value::Null),
                            "y": data.get("y").cloned().unwrap_or(Value::Null),
                            "radius": data.get("radius").cloned().unwrap_or(Value::Null),
                            "color": data.get("color").cloned().unwrap_or(Value::Null),
                            "dim": data.get("dim").cloned().unwrap_or(Value::Null),
                        }));
                        if light.is_null()
                            || light
                                .get("id")
                                .and_then(Value::as_str)
                                .unwrap_or("")
                                .is_empty()
                        {
                            return;
                        }
                        let lights = map_item_ref(&room, &kind, index)
                            .and_then(|item| item.get("lights"))
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if lights.len() >= rules::LIGHT_LIMITS.max_lights {
                            drop(room);
                            emit_error(
                                &socket,
                                &format!(
                                    "Источников света больше {} быть не может",
                                    rules::LIGHT_LIMITS.max_lights
                                ),
                            );
                            return;
                        }
                        // Same undo gap as `addMapWall`.
                        push_undo(&mut room);
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            if !item.get("lights").map(Value::is_array).unwrap_or(false) {
                                item["lights"] = json!([]);
                            }
                            if let Some(list) = item.get_mut("lights").and_then(Value::as_array_mut)
                            {
                                list.push(light);
                            }
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapLight",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let light_id = js_string_opt(data.get("lightId")).unwrap_or_default();
                        let lights: Vec<Value> = item
                            .get("lights")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        let Some(position) = lights.iter().position(|light| {
                            light.get("id").and_then(Value::as_str) == Some(light_id.as_str())
                        }) else {
                            drop(room);
                            emit_error(&socket, "Источник света не найден");
                            return;
                        };
                        let light = &lights[position];
                        let value_or = |key: &str| {
                            data.get(key)
                                .cloned()
                                .unwrap_or_else(|| light.get(key).cloned().unwrap_or(Value::Null))
                        };
                        let next = rules::normalize_light(&json!({
                            "id": light_id,
                            "x": value_or("x"),
                            "y": value_or("y"),
                            "radius": value_or("radius"),
                            "color": value_or("color"),
                            "dim": value_or("dim"),
                            "hidden": value_or("hidden"),
                        }));
                        if next.is_null() {
                            drop(room);
                            emit_error(&socket, "Источник света задан неверно");
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            if let Some(list) = item.get_mut("lights").and_then(Value::as_array_mut)
                            {
                                if position < list.len() {
                                    list[position] = next;
                                }
                            }
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "deleteMapLight",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let light_id = js_string_opt(data.get("lightId")).unwrap_or_default();
                        let lights: Vec<Value> = item
                            .get("lights")
                            .and_then(Value::as_array)
                            .cloned()
                            .unwrap_or_default();
                        if !lights.iter().any(|light| {
                            light.get("id").and_then(Value::as_str) == Some(light_id.as_str())
                        }) {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["lights"] = Value::Array(
                                lights
                                    .into_iter()
                                    .filter(|light| {
                                        light.get("id").and_then(Value::as_str)
                                            != Some(light_id.as_str())
                                    })
                                    .collect(),
                            );
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        let last_ping = Arc::new(Mutex::new(0u128));
        socket.on(
            "mapPing",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                let last_ping = last_ping.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    if !data.is_object() {
                        return;
                    }
                    let kind = js_string_opt(data.get("mapType")).unwrap_or_default();
                    let map_id = js_string_opt(data.get("mapId")).unwrap_or_default();
                    let x = js_number(data.get("x").unwrap_or(&Value::Null));
                    let y = js_number(data.get("y").unwrap_or(&Value::Null));
                    if !MAP_TYPES.contains(&kind.as_str())
                        || map_id.is_empty()
                        || !x.is_finite()
                        || !y.is_finite()
                    {
                        return;
                    }
                    {
                        let room = table.room.lock().unwrap();
                        if room.active_map.as_deref() != Some(kind.as_str())
                            || room.active_map_id.as_deref() != Some(map_id.as_str())
                        {
                            return;
                        }
                    }
                    let now = now_ms();
                    {
                        let mut last = last_ping.lock().unwrap();
                        if now.saturating_sub(*last) < 250 {
                            return;
                        }
                        *last = now;
                    }
                    let payload = json!({
                        "mapType": kind,
                        "mapId": map_id,
                        "x": x.clamp(0.0, 1.0),
                        "y": y.clamp(0.0, 1.0),
                        "playerId": session.player_id,
                        "name": session.name,
                        "role": session.role,
                    });
                    let _ = io.within("table").emit("mapPing", &payload).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setGrid",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let next = {
                        let mut room = table.room.lock().unwrap();
                        let mut merged = room.grid.as_object().cloned().unwrap_or_default();
                        if let Some(data) = data.as_object() {
                            for (key, value) in data {
                                merged.insert(key.clone(), value.clone());
                            }
                        }
                        merged.insert(
                            "enabled".to_string(),
                            json!(js_truthy(data.get("enabled").unwrap_or(&Value::Null))),
                        );
                        let next = rules::normalize_grid(&Value::Object(merged));
                        let same = [
                            "enabled",
                            "cell",
                            "columns",
                            "feetPerCell",
                            "offsetX",
                            "offsetY",
                        ]
                        .iter()
                        .all(|key| room.grid.get(*key) == next.get(*key));
                        if same {
                            return;
                        }
                        room.grid = next.clone();
                        table.mark_dirty();
                        next
                    };
                    let _ = io.emit("gridUpdated", &next).await;
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "setMapAudio",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        return;
                    }
                    let kind;
                    {
                        let mut room = table.room.lock().unwrap();
                        let (found_kind, index) = find_map_index(&room, &data);
                        kind = found_kind;
                        let Some(index) = index else {
                            drop(room);
                            emit_error(&socket, "Карта не найдена");
                            return;
                        };
                        let reference = js_string_opt(data.get("ref")).unwrap_or_default();
                        if !reference.is_empty() && !known_sound(&room, &reference) {
                            drop(room);
                            emit_error(&socket, "Звук не найден на саундборде");
                            return;
                        }
                        let desired = if reference.is_empty() {
                            None
                        } else {
                            Some(reference)
                        };
                        let Some(item) = map_item_ref(&room, &kind, index) else {
                            return;
                        };
                        let current = content::normalize_audio_ref(
                            item.get("audioRef").unwrap_or(&Value::Null),
                        );
                        if current == desired {
                            return;
                        }
                        if let Some(item) = map_item_set(&mut room, &kind, index) {
                            item["audioRef"] = json!(desired);
                        }
                        table.mark_dirty();
                    }
                    broadcast_map_update(&io, &table, &kind).await;
                }
            },
        );
    }

    // ------------------------------------------------------------------
    // Scenes
    // ------------------------------------------------------------------

    {
        let handler_table = table.clone();
        socket.on(
            "saveScene",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        let _ = ack.send(&json!({ "ok": false, "error": "Только ДМ" }));
                        return;
                    }
                    let scene_id = js_string_opt(data.get("sceneId")).unwrap_or_default();
                    let summary;
                    let scenes_changed;
                    {
                        let mut room = table.room.lock().unwrap();
                        let captured = capture_scene(
                            &room,
                            data.get("name").unwrap_or(&Value::Null),
                            data.get("mapType").unwrap_or(&Value::Null),
                            data.get("mapId").unwrap_or(&Value::Null),
                        );
                        let Some(mut captured) = captured else {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": "Карта не найдена" }));
                            return;
                        };
                        if !scene_id.is_empty() {
                            let Some(index) =
                                room.scenes.iter().position(|scene| {
                                    scene.get("id").and_then(Value::as_str) == Some(scene_id.as_str())
                                })
                            else {
                                drop(room);
                                let _ =
                                    ack.send(&json!({ "ok": false, "error": "Сцена не найдена" }));
                                return;
                            };
                            captured["id"] = json!(scene_id);
                            captured["createdAt"] = room.scenes[index]
                                .get("createdAt")
                                .cloned()
                                .unwrap_or(Value::Null);
                            room.scenes[index] = captured.clone();
                        } else {
                            if room.scenes.len() >= LIMITS.max_scenes as usize {
                                drop(room);
                                let _ = ack.send(&json!({
                                    "ok": false,
                                    "error": format!("Больше {} сцен не сохраняется", LIMITS.max_scenes)
                                }));
                                return;
                            }
                            room.scenes.insert(0, captured.clone());
                        }
                        summary = room.scene_summary(&captured);
                        table.mark_dirty();
                        scenes_changed = true;
                    }
                    if scenes_changed {
                        emit_scenes(&io, &table).await;
                    }
                    let _ = ack.send(&json!({ "ok": true, "scene": summary }));
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "loadScene",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        let _ = ack.send(&json!({ "ok": false, "error": "Только ДМ" }));
                        return;
                    }
                    let show = js_truthy(data.get("show").unwrap_or(&Value::Null));
                    let frame;
                    let summary;
                    {
                        let mut room = table.room.lock().unwrap();
                        let scene_id = js_string_opt(data.get("sceneId")).unwrap_or_default();
                        let Some(scene) = room
                            .scenes
                            .iter()
                            .find(|scene| {
                                scene.get("id").and_then(Value::as_str) == Some(scene_id.as_str())
                            })
                            .cloned()
                        else {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": "Сцена не найдена" }));
                            return;
                        };
                        if let Err(error) = apply_scene(&mut room, &scene, show) {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": error }));
                            return;
                        }
                        summary = room.scene_summary(&scene);
                        frame = json!({
                            "sceneId": scene.get("id").cloned().unwrap_or(Value::Null),
                            "mapType": scene.get("mapType").cloned().unwrap_or(Value::Null),
                            "mapId": scene.get("mapId").cloned().unwrap_or(Value::Null),
                            "show": show,
                        });
                        table.mark_dirty();
                    }
                    broadcast_snapshot(&io, &table);
                    let _ = io.within("dm-sockets").emit("sceneLoaded", &frame).await;
                    let _ = ack.send(&json!({ "ok": true, "scene": summary }));
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "renameScene",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        let _ = ack.send(&json!({ "ok": false, "error": "Только ДМ" }));
                        return;
                    }
                    let summary;
                    {
                        let mut room = table.room.lock().unwrap();
                        let scene_id = js_string_opt(data.get("sceneId")).unwrap_or_default();
                        let Some(index) = room.scenes.iter().position(|scene| {
                            scene.get("id").and_then(Value::as_str) == Some(scene_id.as_str())
                        }) else {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": "Сцена не найдена" }));
                            return;
                        };
                        let name = sheet::sanitize_text(
                            data.get("name").unwrap_or(&Value::Null),
                            LIMITS.max_scene_name,
                        )
                        .trim()
                        .to_string();
                        if name.is_empty() {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": "Пустое имя сцены" }));
                            return;
                        }
                        room.scenes[index]["name"] = json!(name);
                        room.scenes[index]["updatedAt"] = json!(now_iso());
                        summary = room.scene_summary(&room.scenes[index]);
                        table.mark_dirty();
                    }
                    emit_scenes(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true, "scene": summary }));
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "duplicateScene",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        let _ = ack.send(&json!({ "ok": false, "error": "Только ДМ" }));
                        return;
                    }
                    let summary;
                    {
                        let mut room = table.room.lock().unwrap();
                        let scene_id = js_string_opt(data.get("sceneId")).unwrap_or_default();
                        let Some(source) = room
                            .scenes
                            .iter()
                            .find(|scene| {
                                scene.get("id").and_then(Value::as_str) == Some(scene_id.as_str())
                            })
                            .cloned()
                        else {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": "Сцена не найдена" }));
                            return;
                        };
                        if room.scenes.len() >= LIMITS.max_scenes as usize {
                            drop(room);
                            let _ = ack.send(&json!({
                                "ok": false,
                                "error": format!("Больше {} сцен не сохраняется", LIMITS.max_scenes)
                            }));
                            return;
                        }
                        let now = now_iso();
                        let mut copy = source;
                        copy["id"] = json!(content::new_scene_id());
                        let name = sheet::sanitize_text(
                            &json!(format!(
                                "{} (копия)",
                                copy.get("name").and_then(Value::as_str).unwrap_or("")
                            )),
                            LIMITS.max_scene_name,
                        )
                        .trim()
                        .to_string();
                        copy["name"] = json!(name);
                        copy["createdAt"] = json!(now);
                        copy["updatedAt"] = json!(now);
                        summary = room.scene_summary(&copy);
                        room.scenes.insert(0, copy);
                        table.mark_dirty();
                    }
                    emit_scenes(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true, "scene": summary }));
                }
            },
        );
    }

    {
        let handler_table = table.clone();
        socket.on(
            "deleteScene",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data), ack: AckSender| {
                let table = handler_table.clone();
                async move {
                    if require_dm(&socket, &table).is_none() {
                        let _ = ack.send(&json!({ "ok": false, "error": "Только ДМ" }));
                        return;
                    }
                    {
                        let mut room = table.room.lock().unwrap();
                        let scene_id = js_string_opt(data.get("sceneId")).unwrap_or_default();
                        let before = room.scenes.len();
                        room.scenes.retain(|scene| {
                            scene.get("id").and_then(Value::as_str) != Some(scene_id.as_str())
                        });
                        if room.scenes.len() == before {
                            drop(room);
                            let _ = ack.send(&json!({ "ok": false, "error": "Сцена не найдена" }));
                            return;
                        }
                        table.mark_dirty();
                    }
                    emit_scenes(&io, &table).await;
                    let _ = ack.send(&json!({ "ok": true }));
                }
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

/// `pushUndo` from `state.js`: everything a map action can move.
fn push_undo(room: &mut RoomState) {
    let snapshot = json!({
        "maps": room.maps.clone(),
        "tokens": room.tokens.clone(),
        "initiative": room.initiative.clone(),
        "activeMap": room.active_map.clone(),
        "activeMapId": room.active_map_id.clone(),
        "journal": room.journal.clone(),
        "journalEntries": room.journal_entries.clone(),
        "inventory": room.inventory.clone(),
        "inventoryCoins": room.inventory_coins.clone(),
        "gameTime": room.game_time.clone(),
        "calendarMonths": room.calendar_months.clone(),
        "calendarEvents": room.calendar_events.clone(),
    });
    room.undo_stack.push(snapshot);
    if room.undo_stack.len() > 20 {
        room.undo_stack.remove(0);
    }
}

fn map_items(room: &RoomState, kind: &str) -> Vec<Value> {
    room.maps
        .get(kind)
        .and_then(|slot| slot.get("items"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn map_items_mut<'a>(room: &'a mut RoomState, kind: &str) -> Option<&'a mut Vec<Value>> {
    room.maps.get_mut(kind)?.get_mut("items")?.as_array_mut()
}

fn map_item_ref<'a>(room: &'a RoomState, kind: &str, index: usize) -> Option<&'a Value> {
    room.maps.get(kind)?.get("items")?.as_array()?.get(index)
}

fn map_item_set<'a>(room: &'a mut RoomState, kind: &str, index: usize) -> Option<&'a mut Value> {
    room.maps
        .get_mut(kind)?
        .get_mut("items")?
        .as_array_mut()?
        .get_mut(index)
}

/// `findMapItem`: a tab plus the map inside it. An empty tab answers
/// `("world", None)` like the reference.
fn find_map_index(room: &RoomState, payload: &Value) -> (String, Option<usize>) {
    let kind = js_string_opt(payload.get("type")).unwrap_or_default();
    let Some(items) = room
        .maps
        .get(&kind)
        .and_then(|slot| slot.get("items"))
        .and_then(Value::as_array)
    else {
        return ("world".to_string(), None);
    };
    if items.is_empty() {
        return (kind, None);
    }
    let wanted = match payload.get("mapId") {
        Some(value) if js_truthy(value) => js_string(value),
        _ => String::new(),
    };
    let index = if !wanted.is_empty() {
        items
            .iter()
            .position(|item| item.get("id").and_then(Value::as_str) == Some(wanted.as_str()))
    } else {
        let active = room
            .maps
            .get(&kind)
            .and_then(|slot| slot.get("activeId"))
            .and_then(Value::as_str);
        items
            .iter()
            .position(|item| item.get("id").and_then(Value::as_str) == active)
            .or(Some(0))
    };
    (kind, index)
}

fn fallback_published(room: &RoomState, kind: &str) -> Option<(String, String)> {
    for candidate in MAP_TYPES {
        if candidate == kind {
            continue;
        }
        let id = map_items(room, candidate)
            .iter()
            .find(|item| !matches!(item.get("published"), Some(Value::Bool(false))))
            .and_then(|item| item.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(id) = id {
            return Some((candidate.to_string(), id));
        }
    }
    None
}

async fn broadcast_map_update(io: &SocketIo, table: &Table, kind: &str) {
    let (dm_payload, player_payload) = {
        let room = table.room.lock().unwrap();
        let slot = room
            .maps
            .get(kind)
            .cloned()
            .unwrap_or_else(|| json!({ "items": [], "activeId": Value::Null }));
        let items = slot.get("items").cloned().unwrap_or_else(|| json!([]));
        let active_id = slot.get("activeId").cloned().unwrap_or(Value::Null);
        let dm_payload = json!({
            "type": kind,
            "items": items,
            "activeId": active_id,
            "activeMap": room.active_map.clone(),
            "activeMapId": room.active_map_id.clone(),
        });
        let visible = room.map_slot_for_viewer(kind, false);
        let active_map = if room
            .active_map_id
            .as_deref()
            .map(|id| !id.is_empty())
            .unwrap_or(false)
        {
            json!(room.active_map.clone())
        } else {
            Value::Null
        };
        let player_payload = json!({
            "type": kind,
            "items": visible.get("items").cloned().unwrap_or_else(|| json!([])),
            "activeId": visible.get("activeId").cloned().unwrap_or(Value::Null),
            "activeMap": active_map,
            "activeMapId": room.active_map_id.clone(),
        });
        (dm_payload, player_payload)
    };
    let _ = io
        .within("dm-sockets")
        .emit("mapUpdated", &dm_payload)
        .await;
    let _ = io
        .within("table")
        .except("dm-sockets")
        .emit("mapUpdated", &player_payload)
        .await;
}

async fn emit_scenes(io: &SocketIo, table: &Table) {
    let scenes = {
        let room = table.room.lock().unwrap();
        Value::Array(
            room.scenes
                .iter()
                .map(|scene| room.scene_summary(scene))
                .collect(),
        )
    };
    let _ = io.within("dm-sockets").emit("scenesUpdated", &scenes).await;
}

/// `emitToken`: hidden tokens and tokens on unpublished maps stay DM-only;
/// players never see monster HP.
async fn emit_token(io: &SocketIo, table: &Table, event: &str, token: &Value) {
    let (dm_view, player_view, visible) = {
        let room = table.room.lock().unwrap();
        (
            room.token_for_viewer(token, true),
            room.token_for_viewer(token, false),
            room.is_token_visible_to_players(token),
        )
    };
    let _ = io.within("dm-sockets").emit(event, &dm_view).await;
    if visible {
        let _ = io
            .within("table")
            .except("dm-sockets")
            .emit(event, &player_view)
            .await;
    }
}

/// `broadcastSnapshot`: every socket that already joined gets a fresh state.
fn broadcast_snapshot(io: &SocketIo, table: &Table) {
    for socket in io.sockets() {
        let session = table
            .sessions
            .lock()
            .unwrap()
            .get(&socket.id.to_string())
            .cloned();
        let Some(session) = session else {
            continue;
        };
        if session.role.is_empty() {
            continue;
        }
        let viewer = Viewer {
            player_id: session.player_id,
            role: session.role,
        };
        let state = table.snapshot_for(&viewer);
        let _ = socket.emit("stateReload", &state);
    }
}

fn known_sound(room: &RoomState, reference: &str) -> bool {
    if reference.is_empty() {
        return false;
    }
    if room
        .soundboard
        .iter()
        .any(|item| item.get("ref").and_then(Value::as_str) == Some(reference))
    {
        return true;
    }
    MAP_TYPES.iter().any(|kind| {
        map_items(room, kind)
            .iter()
            .any(|item| item.get("audioRef").and_then(Value::as_str) == Some(reference))
    })
}

/// `captureScene`: the map's atmosphere, the grid and its tokens.
fn capture_scene(
    room: &RoomState,
    name: &Value,
    map_type: &Value,
    map_id: &Value,
) -> Option<Value> {
    let kind = map_type.as_str().filter(|kind| MAP_TYPES.contains(kind))?;
    let wanted = map_id.as_str().unwrap_or("");
    let map = map_items(room, kind)
        .into_iter()
        .find(|item| item.get("id").and_then(Value::as_str) == Some(wanted))?;
    let map_id = map
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let tokens: Vec<Value> = room
        .tokens
        .iter()
        .filter(|token| {
            token.get("mapType").and_then(Value::as_str) == Some(kind)
                && token.get("mapId").and_then(Value::as_str) == Some(map_id.as_str())
        })
        .take(LIMITS.max_scene_tokens as usize)
        .cloned()
        .collect();
    let name = sheet::sanitize_text(name, LIMITS.max_scene_name)
        .trim()
        .to_string();
    let now = now_iso();
    Some(json!({
        "id": content::new_scene_id(),
        "name": if name.is_empty() { "Сцена".to_string() } else { name },
        "mapType": kind,
        "mapId": map_id,
        "map": {
            "fogMask": content::normalize_fog(map.get("fogMask").unwrap_or(&Value::Null)),
            "surface": rules::normalize_surface(map.get("surface").unwrap_or(&Value::Null)),
            "walls": rules::normalize_walls(map.get("walls").unwrap_or(&Value::Null)),
            "lights": rules::normalize_lights(map.get("lights").unwrap_or(&Value::Null)),
            "templates": content::normalize_templates(map.get("templates").unwrap_or(&Value::Null)),
            "pins": rules::normalize_pins(map.get("pins").unwrap_or(&Value::Null)),
            "darkness": content::normalize_darkness(map.get("darkness").unwrap_or(&Value::Null)),
            "autoLight": map.get("autoLight").and_then(Value::as_bool).unwrap_or(false),
            "fogFollow": map.get("fogFollow").and_then(Value::as_bool).unwrap_or(false),
            "visionPerPlayer": map.get("visionPerPlayer").and_then(Value::as_bool).unwrap_or(false),
            "audioRef": content::normalize_audio_ref(map.get("audioRef").unwrap_or(&Value::Null)),
        },
        "grid": room.grid.clone(),
        "tokens": tokens,
        "createdAt": now,
        "updatedAt": now,
    }))
}

/// `applyScene` from `state.js`.
fn apply_scene(room: &mut RoomState, scene: &Value, show: bool) -> Result<(), String> {
    let kind = scene
        .get("mapType")
        .and_then(Value::as_str)
        .filter(|kind| MAP_TYPES.contains(kind))
        .ok_or_else(|| "Карта сцены не найдена".to_string())?;
    let map_id = scene
        .get("mapId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let index = map_items(room, kind)
        .iter()
        .position(|item| item.get("id").and_then(Value::as_str) == Some(map_id.as_str()))
        .ok_or_else(|| "Карта сцены не найдена".to_string())?;

    push_undo(room);

    let atmosphere = scene
        .get("map")
        .filter(|map| map.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(map) = map_item_set(room, kind, index) {
        map["fogMask"] = content::normalize_fog(atmosphere.get("fogMask").unwrap_or(&Value::Null));
        map["surface"] =
            rules::normalize_surface(atmosphere.get("surface").unwrap_or(&Value::Null));
        map["walls"] = rules::normalize_walls(atmosphere.get("walls").unwrap_or(&Value::Null));
        map["lights"] = rules::normalize_lights(atmosphere.get("lights").unwrap_or(&Value::Null));
        map["templates"] =
            content::normalize_templates(atmosphere.get("templates").unwrap_or(&Value::Null));
        map["pins"] = rules::normalize_pins(atmosphere.get("pins").unwrap_or(&Value::Null));
        map["darkness"] =
            content::normalize_darkness(atmosphere.get("darkness").unwrap_or(&Value::Null));
        map["autoLight"] = json!(matches!(
            atmosphere.get("autoLight"),
            Some(Value::Bool(true))
        ));
        map["fogFollow"] = json!(matches!(
            atmosphere.get("fogFollow"),
            Some(Value::Bool(true))
        ));
        map["visionPerPlayer"] = json!(matches!(
            atmosphere.get("visionPerPlayer"),
            Some(Value::Bool(true))
        ));
        map["audioRef"] = json!(content::normalize_audio_ref(
            atmosphere.get("audioRef").unwrap_or(&Value::Null)
        ));
    }
    room.grid = rules::normalize_grid(scene.get("grid").unwrap_or(&Value::Null));

    let mut saved: BTreeMap<String, Value> = BTreeMap::new();
    for token in scene
        .get("tokens")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        if let Some(id) = token.get("id").and_then(Value::as_str) {
            saved.insert(id.to_string(), token.clone());
        }
    }

    let mut removed: HashSet<String> = HashSet::new();
    room.tokens.retain(|token| {
        let on_map = token.get("mapType").and_then(Value::as_str) == Some(kind)
            && token.get("mapId").and_then(Value::as_str) == Some(map_id.as_str());
        if !on_map {
            return true;
        }
        if js_truthy(token.get("assignedPlayerId").unwrap_or(&Value::Null)) {
            return true;
        }
        let id = token.get("id").and_then(Value::as_str).unwrap_or("");
        if saved.contains_key(id) {
            return true;
        }
        removed.insert(id.to_string());
        false
    });

    for token in saved.values() {
        // Player tokens move only when the scene is shown.
        if js_truthy(token.get("assignedPlayerId").unwrap_or(&Value::Null)) {
            continue;
        }
        let id = token
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let normalized = room.normalize_token(token);
        if let Some(existing) = room
            .tokens
            .iter_mut()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(id.as_str()))
        {
            if let (Some(target), Some(source)) = (existing.as_object_mut(), normalized.as_object())
            {
                for (key, value) in source {
                    target.insert(key.clone(), value.clone());
                }
            }
        } else {
            room.tokens.push(normalized);
        }
    }

    if show {
        let mut placed = 0usize;
        for token in room.tokens.iter_mut() {
            if !js_truthy(token.get("assignedPlayerId").unwrap_or(&Value::Null)) {
                continue;
            }
            let id = token
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if let Some(saved_token) = saved.get(&id) {
                if saved_token.get("mapType").and_then(Value::as_str) == Some(kind)
                    && saved_token.get("mapId").and_then(Value::as_str) == Some(map_id.as_str())
                {
                    token["mapType"] = json!(kind);
                    token["mapId"] = json!(map_id);
                    token["x"] = saved_token.get("x").cloned().unwrap_or_else(|| json!(0.5));
                    token["y"] = saved_token.get("y").cloned().unwrap_or_else(|| json!(0.5));
                    continue;
                }
            }
            if token.get("mapType").and_then(Value::as_str) == Some(kind)
                && token.get("mapId").and_then(Value::as_str) == Some(map_id.as_str())
            {
                continue;
            }
            token["mapType"] = json!(kind);
            token["mapId"] = json!(map_id);
            // Stagger newcomers so several heroes do not land on one point.
            let angle = placed as f64 * 1.7;
            let radius = 0.2f64.min(0.05 + placed as f64 * 0.01);
            token["x"] = json!((0.5 + angle.cos() * radius).clamp(0.05, 0.95));
            token["y"] = json!((0.5 + angle.sin() * radius).clamp(0.05, 0.95));
            placed += 1;
        }
        if let Some(map) = map_item_set(room, kind, index) {
            map["published"] = json!(true);
        }
        room.active_map = Some(kind.to_string());
        room.active_map_id = Some(map_id.clone());
    }

    // The DM must land on the map they opened.
    if let Some(slot) = room.maps.get_mut(kind) {
        slot["activeId"] = json!(map_id);
    }
    if !removed.is_empty() {
        if let Some(entries) = room
            .initiative
            .get_mut("entries")
            .and_then(Value::as_array_mut)
        {
            entries.retain(|entry| {
                !removed.contains(entry.get("refId").and_then(Value::as_str).unwrap_or(""))
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// JS value semantics
// ---------------------------------------------------------------------------

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number
            .as_f64()
            .map(|n| n != 0.0 && !n.is_nan())
            .unwrap_or(false),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn js_number(value: &Value) -> f64 {
    match value {
        Value::Null => 0.0,
        Value::Bool(flag) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Value::Number(number) => number.as_f64().unwrap_or(f64::NAN),
        Value::String(text) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        Value::Array(items) => {
            if items.len() == 1 {
                js_number(&items[0])
            } else {
                f64::NAN
            }
        }
        Value::Object(_) => f64::NAN,
    }
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

fn js_string_opt(value: Option<&Value>) -> Option<String> {
    value.map(js_string)
}

/// RFC 3339 with millis. `dnd-server` has no `chrono` dependency, and the value
/// only has to match the shape the renderer reads.
fn now_iso() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let seconds = (millis / 1000) as i64;
    let thousandths = millis % 1000;
    let days = seconds.div_euclid(86_400);
    let day_seconds = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{thousandths:03}Z",
        day_seconds / 3600,
        (day_seconds % 3600) / 60,
        day_seconds % 60,
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (year + i64::from(month <= 2), month, day)
}
