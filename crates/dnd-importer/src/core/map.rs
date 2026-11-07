//! Маппинг выбранных сущностей в модель D&D Стола.
//!
//! Здесь принимаются решения, которые невозможно вывести из формата источника:
//! в модели кампании ровно три вкладки карт, а сцен может быть сколько угодно,
//! и NPC в игре не имеют листа. Решения зафиксированы в docs/ИМПОРТ-ПРИКЛЮЧЕНИЙ.md.
//!
//! Результат — патч: что нужно положить в кампанию. Ничего не пишется на диск —
//! это делает вызывающий код.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value, json};

use crate::core::normalize::Entity;
use crate::js;

pub const MAP_TYPES: [&str; 3] = ["world", "battle", "sketch"];

pub fn map_label(map_type: &str) -> &str {
    match map_type {
        "world" => "Общая",
        "battle" => "Локация",
        "sketch" => "Рисунок",
        _ => map_type,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub kind: String,
    pub source_id: String,
    pub name: String,
    pub selectable: bool,
    pub selected: bool,
    pub target: Option<String>,
    pub target_label: String,
    pub note: String,
}

impl Plan {
    pub fn to_value(&self) -> Value {
        json!({
            "kind": self.kind,
            "sourceId": self.source_id,
            "name": self.name,
            "selectable": self.selectable,
            "selected": self.selected,
            "target": self.target,
            "targetLabel": self.target_label,
            "note": self.note,
        })
    }
}

pub struct Built {
    pub plans: Vec<Plan>,
    pub free_slots: Vec<String>,
}

fn mentions_foundry(text: &str) -> bool {
    text.to_lowercase().contains("файлах foundry")
}

/// План для одной сущности: что с ней будет сделано при импорте.
pub fn plan_for(entity: &Entity, free_slots: &[String]) -> Plan {
    if entity.kind == "scene" {
        let slot = free_slots.first().cloned();
        // Сцена без годной картинки не даст карты: импортировать её — значит
        // показать пользователю пустую вкладку. Лучше честно сказать заранее.
        let has_image = entity.image.is_some();
        let selectable = slot.is_some() && has_image;
        let note = if slot.is_none() {
            "В игре заняты все три вкладки карт — освободите одну, чтобы импортировать сцену"
                .to_string()
        } else if !has_image {
            if entity
                .image_error
                .as_ref()
                .is_some_and(|error| mentions_foundry(&error.reason))
            {
                "Картинка лежит в файлах Foundry, а в выгрузке её нет — выгрузите приключение из Foundry".to_string()
            } else {
                "У сцены нет подходящей картинки, а без неё карта будет пустой".to_string()
            }
        } else {
            "Картинка станет картой, токены — фишками".to_string()
        };
        let target_label = match &slot {
            Some(slot) => format!("вкладка «{}»", map_label(slot)),
            None => "свободных вкладок нет".to_string(),
        };
        return Plan {
            kind: entity.kind.clone(),
            source_id: entity.source_id.clone(),
            name: entity.name.clone(),
            selectable,
            selected: selectable,
            target: slot,
            target_label,
            note,
        };
    }

    if entity.kind == "macro" {
        return Plan {
            kind: entity.kind.clone(),
            source_id: entity.source_id.clone(),
            name: entity.name.clone(),
            selectable: false,
            selected: false,
            target: None,
            target_label: String::new(),
            note: "Макросы не переносятся и никогда не выполняются".to_string(),
        };
    }

    let (target, extra) = match entity.kind.as_str() {
        "actor" => ("персонаж в справочнике приключения", "лист не заполняется"),
        "journal" => ("заметка в журнале приключения", ""),
        "item" => ("предмет в справочнике приключения", ""),
        "rolltable" => ("таблица в справочнике приключения", ""),
        _ => ("неизвестно", ""),
    };

    Plan {
        kind: entity.kind.clone(),
        source_id: entity.source_id.clone(),
        name: entity.name.clone(),
        selectable: true,
        selected: true,
        target: None,
        target_label: target.to_string(),
        note: extra.to_string(),
    }
}

pub fn build_plans(entities: &[Entity], campaign_maps: &Value) -> Built {
    let free_slots: Vec<String> = MAP_TYPES
        .iter()
        .filter(|map_type| {
            let Some(slot) = js::get(campaign_maps, map_type) else {
                return false;
            };
            js::truthy(slot)
                && js::get(slot, "items")
                    .is_some_and(|items| items.as_array().is_some_and(|items| items.is_empty()))
        })
        .map(|map_type| (*map_type).to_string())
        .collect();
    let plans: Vec<Plan> = entities
        .iter()
        .map(|entity| plan_for(entity, &free_slots))
        .collect();
    Built { plans, free_slots }
}

/// Переводит координату из пикселей Foundry в долю от 0 до 1.
///
/// В Foundry x и y — левый верхний угол токена в пикселях, а центр лежит на
/// половине его ширины и высоты. В D&D Стол точка — центр фишки.
pub fn to_fraction(value: Option<f64>, size: Option<f64>, extent: f64) -> f64 {
    if !extent.is_finite() || extent <= 0.0 {
        return 0.5;
    }
    let center = value.unwrap_or(0.0) + size.unwrap_or(0.0) / 2.0;
    (center / extent).clamp(0.0, 1.0)
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

/// Идентификаторы для импортируемых сущностей.
/// Счётчик нужен, чтобы в пределах одной импортации не совпали два «note-1».
pub fn next_id(prefix: &str) -> String {
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst) + 1;
    format!(
        "{prefix}-{}-{}",
        js::base36(counter),
        js::base36(now_millis())
    )
}

/// `new Date().toISOString()`.
fn now_iso() -> String {
    let seconds = now_millis() / 1000;
    let days = (seconds / 86_400) as i64;
    let rest = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    let hour = rest / 3_600;
    let minute = (rest % 3_600) / 60;
    let second = rest % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.000Z")
}

/// Howard Hinnant's `civil_from_days`, days since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

fn raw_get<'a>(entity: &'a Entity, key: &str) -> Option<&'a Value> {
    js::get(entity.raw.as_ref()?, key)
}

fn raw_string(entity: &Entity, key: &str) -> String {
    js::string_or(raw_get(entity, key), "")
}

fn slot_items_mut<'a>(maps: &'a mut Map<String, Value>, slot: &str) -> Option<&'a mut Vec<Value>> {
    let entry = maps
        .entry(slot.to_string())
        .or_insert_with(|| json!({ "items": [], "activeId": null }));
    entry
        .as_object_mut()?
        .entry("items")
        .or_insert_with(|| json!([]))
        .as_array_mut()
}

pub struct MapContext<'a> {
    pub campaign_maps: &'a Value,
    pub entities: &'a [Entity],
    pub source_title: &'a str,
    pub source_id: &'a str,
}

pub struct Mapped {
    pub maps: Value,
    pub tokens: Vec<Value>,
    pub adventure: Value,
    pub applied: Vec<Value>,
    pub skipped: Vec<Value>,
}

/// Собирает патч кампании из выбранных планов.
pub fn map_to_campaign(plans: &[Plan], context: MapContext<'_>) -> Mapped {
    let by_id: std::collections::HashMap<&str, &Entity> = context
        .entities
        .iter()
        .map(|entity| (entity.source_id.as_str(), entity))
        .collect();

    let mut maps: Map<String, Value> = Map::new();
    if let Value::Object(source) = context.campaign_maps {
        for (map_type, slot) in source {
            let items = js::get(slot, "items")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut copy = Map::new();
            copy.insert("items".to_string(), Value::Array(items));
            if let Some(active) = js::get(slot, "activeId") {
                copy.insert("activeId".to_string(), active.clone());
            }
            maps.insert(map_type.clone(), Value::Object(copy));
        }
    }

    let mut tokens = Vec::new();
    let mut applied = Vec::new();
    let mut skipped = Vec::new();

    let adventure_id = next_id("adv");
    let mut adventure = json!({
        "id": adventure_id,
        "name": context.source_title,
        "source": context.source_id,
        "importedAt": now_iso(),
        "notes": [],
        "items": [],
        "tables": [],
        "actors": [],
    });
    let adventure_object = adventure.as_object_mut().expect("adventure is an object");

    // Сцены занимают вкладки по порядку; первая свободная уходит первой.
    let mut used_slots: Vec<String> = Vec::new();
    for plan in plans {
        if !plan.selected {
            if !plan.selectable {
                skipped.push(json!({
                    "kind": &plan.kind,
                    "name": &plan.name,
                    "reason": &plan.note,
                }));
            }
            continue;
        }
        let Some(entity) = by_id.get(plan.source_id.as_str()) else {
            continue;
        };

        if plan.kind == "scene" {
            let slot = used_slots.last().cloned().or_else(|| plan.target.clone());
            let Some(slot) = slot else {
                skipped.push(json!({
                    "kind": "scene",
                    "name": &plan.name,
                    "reason": "свободных вкладок нет",
                }));
                continue;
            };
            let scene_width = js::number_or_zero(raw_get(entity, "sceneWidth"));
            let scene_height = js::number_or_zero(raw_get(entity, "sceneHeight"));

            let Some(image) = &entity.image else {
                // Страховка на случай, если импорт вызвали в обход предпросмотра:
                // лучше пропустить сцену, чем отчитаться об успехе, которого не было.
                skipped.push(json!({
                    "kind": "scene",
                    "name": &plan.name,
                    "reason": "у сцены нет подходящей картинки",
                }));
                continue;
            };

            let mut map_item = Map::new();
            map_item.insert("id".to_string(), Value::String(next_id("map")));
            map_item.insert("url".to_string(), Value::String(image.data.clone()));
            map_item.insert(
                "name".to_string(),
                Value::String(if entity.name.is_empty() {
                    "Карта".to_string()
                } else {
                    entity.name.clone()
                }),
            );
            map_item.insert("importedAt".to_string(), Value::String(now_iso()));
            let map_id = map_item["id"].clone();

            if !entity.children.is_empty() {
                for child in &entity.children {
                    let raw = child.raw.as_ref();
                    let raw_number = |key: &str| {
                        raw.and_then(|value| js::get(value, key))
                            .map(|value| js::number_or_zero(Some(value)))
                    };
                    let width = raw_number("width").unwrap_or(0.0);
                    let height = raw_number("height").unwrap_or(0.0);
                    let width = if width == 0.0 { 1.0 } else { width };
                    let height = if height == 0.0 { 1.0 } else { height };
                    // Foundry хранит размер токена в клетках: 2 у большого, 3 у
                    // огромного. Берём большую сторону, потому что фишка круглая.
                    let size = js::round(width.max(height)).clamp(1.0, 4.0) as i64;
                    tokens.push(json!({
                        "id": next_id("token"),
                        "name": if child.name.is_empty() { "Фишка".to_string() } else { child.name.clone() },
                        "mapType": &slot,
                        "mapId": &map_id,
                        "x": to_fraction(raw_number("x"), raw_number("width"), scene_width),
                        "y": to_fraction(raw_number("y"), raw_number("height"), scene_height),
                        "imageUrl": child.image.as_ref().map(|image| image.data.clone()),
                        "size": size,
                        "assignedPlayerId": null,
                        "color": "#c87550",
                    }));
                }
            }
            // Стены сцены: пиксели становятся долями карты, дверь и окно — их
            // метками. Кривые отрезки normalizeWall отбросит при первом открытии
            // кампании, поэтому здесь он и не вызывается.
            let scene_walls = js::as_array(raw_get(entity, "walls"));
            if !scene_walls.is_empty() {
                let mut walls = Vec::new();
                for wall in scene_walls {
                    let parts = js::as_array(js::get(wall, "c"));
                    if parts.len() < 4 {
                        continue;
                    }
                    let mut entry = Map::new();
                    entry.insert("id".to_string(), Value::String(next_id("wall")));
                    entry.insert(
                        "x1".to_string(),
                        Value::from(to_fraction(
                            Some(js::number_or_zero(Some(&parts[0]))),
                            None,
                            scene_width,
                        )),
                    );
                    entry.insert(
                        "y1".to_string(),
                        Value::from(to_fraction(
                            Some(js::number_or_zero(Some(&parts[1]))),
                            None,
                            scene_height,
                        )),
                    );
                    entry.insert(
                        "x2".to_string(),
                        Value::from(to_fraction(
                            Some(js::number_or_zero(Some(&parts[2]))),
                            None,
                            scene_width,
                        )),
                    );
                    entry.insert(
                        "y2".to_string(),
                        Value::from(to_fraction(
                            Some(js::number_or_zero(Some(&parts[3]))),
                            None,
                            scene_height,
                        )),
                    );
                    if js::get(wall, "door").is_some_and(js::truthy) {
                        // ds: 0 закрыта, 1 открыта, 2 заперта (и секретная дверь тоже).
                        let ds = js::get(wall, "ds").map(js::number).unwrap_or(0.0);
                        entry.insert(
                            "door".to_string(),
                            json!({ "open": ds == 1.0, "locked": ds == 2.0 }),
                        );
                    }
                    if js::get(wall, "window").is_some_and(js::truthy) {
                        entry.insert("window".to_string(), Value::Bool(true));
                    }
                    walls.push(Value::Object(entry));
                }
                if !walls.is_empty() {
                    map_item.insert("walls".to_string(), Value::Array(walls));
                }
            }
            if let Some(items) = slot_items_mut(&mut maps, &slot) {
                items.push(Value::Object(map_item));
            }
            used_slots.push(slot.clone());
            applied.push(json!({
                "kind": "scene",
                "name": &plan.name,
                "where": format!("вкладка «{}»", map_label(&slot)),
            }));
            continue;
        }

        if plan.kind == "macro" {
            skipped.push(json!({
                "kind": "macro",
                "name": &plan.name,
                "reason": "макросы не переносятся",
            }));
            continue;
        }

        if plan.kind == "journal" {
            adventure_object["notes"].as_array_mut().unwrap().push(json!({
                "id": next_id("note"),
                "title": if entity.name.is_empty() { "Заметка".to_string() } else { entity.name.clone() },
                "text": raw_string(entity, "text"),
                "sourceId": &entity.source_id,
            }));
            applied.push(json!({
                "kind": "journal",
                "name": &plan.name,
                "where": "журнал приключения",
            }));
            continue;
        }

        if plan.kind == "item" {
            adventure_object["items"].as_array_mut().unwrap().push(json!({
                "id": next_id("item"),
                "name": if entity.name.is_empty() { "Предмет".to_string() } else { entity.name.clone() },
                "type": raw_string(entity, "type"),
                "sourceId": &entity.source_id,
            }));
            applied.push(json!({
                "kind": "item",
                "name": &plan.name,
                "where": "справочник предметов",
            }));
            continue;
        }

        if plan.kind == "rolltable" {
            let doc = entity.raw.as_ref();
            let results = js::as_array(doc.and_then(|doc| js::get(doc, "results")));
            let rows: Vec<Value> = results
                .iter()
                .map(|entry| {
                    let range = js::as_array(js::get(entry, "range"));
                    let label = js::get_truthy(Some(entry), "text")
                        .or_else(|| js::get_truthy(Some(entry), "label"));
                    let weight = match js::get_truthy(Some(entry), "weight") {
                        Some(weight) => js::number_to_value(js::number(weight)),
                        None => Value::from(1),
                    };
                    let mut row = Map::new();
                    row.insert("id".to_string(), Value::String(next_id("row")));
                    row.insert(
                        "label".to_string(),
                        Value::String(
                            label
                                .map(js::string)
                                .unwrap_or_else(|| "Строка".to_string()),
                        ),
                    );
                    row.insert("weight".to_string(), weight);
                    if !range.is_empty() {
                        row.insert(
                            "min".to_string(),
                            js::number_to_value(js::number(&range[0])),
                        );
                    }
                    if range.len() > 1 {
                        row.insert(
                            "max".to_string(),
                            js::number_to_value(js::number(&range[1])),
                        );
                    }
                    Value::Object(row)
                })
                .collect();
            adventure_object["tables"].as_array_mut().unwrap().push(json!({
                "id": next_id("table"),
                "name": if entity.name.is_empty() { "Таблица".to_string() } else { entity.name.clone() },
                "formula": raw_string(entity, "formula"),
                "description": raw_string(entity, "description"),
                // Строки Foundry: range — диапазон результата формулы, weight — вес.
                // Таблица без range тянет строки по весам, как мешок фишек.
                "rows": rows,
                "sourceId": &entity.source_id,
            }));
            applied.push(json!({
                "kind": "rolltable",
                "name": &plan.name,
                "where": "справочник таблиц",
            }));
            continue;
        }

        if plan.kind == "actor" {
            let entity_raw = entity.raw.as_ref();
            let statblock = entity_raw
                .and_then(|raw| js::get(raw, "statblock"))
                .filter(|value| js::truthy(value))
                .cloned()
                .unwrap_or(Value::Null);
            let actor_type = raw_string(entity, "type");
            let actor_type = if actor_type.is_empty() {
                "npc".to_string()
            } else {
                actor_type
            };
            adventure_object["actors"].as_array_mut().unwrap().push(json!({
                "id": next_id("actor"),
                "name": if entity.name.is_empty() { "Персонаж".to_string() } else { entity.name.clone() },
                "type": actor_type,
                "image": entity.image.as_ref().map(|image| image.data.clone()).unwrap_or_default(),
                "statblock": statblock,
                "sourceId": &entity.source_id,
            }));
            applied.push(json!({
                "kind": "actor",
                "name": &plan.name,
                "where": "справочник персонажей",
            }));
        }
    }

    Mapped {
        maps: Value::Object(maps),
        tokens,
        adventure,
        applied,
        skipped,
    }
}
