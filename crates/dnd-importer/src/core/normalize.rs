//! Нормализация: содержимое источника → единый список сущностей.
//!
//! Здесь же проверяются картинки. Проверка та же, что и при загрузке карт в
//! игре (server/image.js): по сигнатуре файла, а не по расширению. Импорт не
//! должен стать вторым, более слабым путём внутрь приложения.

use serde_json::{Value, json};

use crate::js;
use dnd_core::media::decode_image;
use dnd_core::sheet::LIMITS;

/// Какие виды сущностей вообще имеет смысл переносить.
pub const KINDS: [&str; 6] = ["scene", "actor", "item", "journal", "rolltable", "macro"];

/// Человеческие названия видов — для интерфейса.
pub fn kind_label(kind: &str) -> &str {
    match kind {
        "scene" => "Сцена",
        "actor" => "Персонаж",
        "item" => "Предмет",
        "journal" => "Заметка",
        "rolltable" => "Таблица",
        "macro" => "Макрос",
        _ => kind,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub data: String,
    pub mime: String,
    pub bytes: usize,
}

impl Image {
    pub fn to_value(&self) -> Value {
        json!({ "data": self.data, "mime": self.mime, "bytes": self.bytes })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageError {
    pub reason: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entity {
    pub kind: String,
    pub source_id: String,
    pub name: String,
    pub tags: Vec<String>,
    pub image: Option<Image>,
    pub image_error: Option<ImageError>,
    pub children: Vec<Entity>,
    pub raw: Option<Value>,
}

impl Entity {
    pub fn to_value(&self) -> Value {
        json!({
            "kind": self.kind,
            "sourceId": self.source_id,
            "name": self.name,
            "tags": self.tags,
            "image": self.image.as_ref().map(Image::to_value),
            "imageError": self.image_error.as_ref().map(|error| json!({
                "reason": error.reason,
                "path": error.path
            })),
            "children": self.children.iter().map(Entity::to_value).collect::<Vec<_>>(),
            "raw": self.raw,
        })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Skipped {
    pub source_id: String,
    pub name: String,
    pub reason: String,
}

impl Skipped {
    pub fn to_value(&self) -> Value {
        json!({ "sourceId": self.source_id, "name": self.name, "reason": self.reason })
    }
}

pub struct NormalizeResult {
    pub entities: Vec<Entity>,
    pub skipped: Vec<Skipped>,
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    text.as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix.as_bytes()))
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    if starts_with_ci(text, prefix) {
        Some(&text[prefix.len()..])
    } else {
        None
    }
}

/// `^data:image/[a-z+]+;` под /i: возвращает подтип, если он вообще разобран.
fn image_subtype(text: &str) -> Option<String> {
    let rest = strip_prefix_ci(text, "data:image/")?;
    let end = rest
        .char_indices()
        .find(|(_, ch)| !(ch.is_ascii_alphabetic() || *ch == '+'))
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    if end == 0 || !rest[end..].starts_with(';') {
        return None;
    }
    Some(rest[..end].to_ascii_lowercase())
}

/// `^data:image/(png|jpeg|jpg|webp);base64,[A-Za-z0-9+/=]+$` под /i.
fn is_supported_image_data_url(text: &str) -> bool {
    let Some(rest) = strip_prefix_ci(text.trim(), "data:image/") else {
        return false;
    };
    let Some((subtype, rest)) = rest.split_once(';') else {
        return false;
    };
    if !matches!(subtype.to_ascii_lowercase().as_str(), "png" | "jpeg" | "jpg" | "webp") {
        return false;
    }
    let Some(body) = strip_prefix_ci(rest, "base64,") else {
        return false;
    };
    !body.is_empty()
        && body
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'='))
}

/// Принимает изображение в любом виде, который встречается у источников:
/// data URL, «data:img» из Foundry или голые base64.
pub fn normalize_image(value: &Value) -> Result<Image, ImageError> {
    let Value::String(text) = value else {
        return Err(ImageError { reason: "пусто".to_string(), path: String::new() });
    };
    let text = text.trim();
    if text.is_empty() {
        return Err(ImageError { reason: "пусто".to_string(), path: String::new() });
    }

    // Foundry внутри слоёв и токенов пишет «data:img» без указания типа.
    let mut candidate = text.to_string();
    if starts_with_ci(&candidate, "data:img;") {
        let rest = &candidate["data:img;".len()..];
        let rest = strip_prefix_ci(rest, "base64,").unwrap_or(rest);
        candidate = format!("data:image/png;base64,{rest}");
    }
    if let Some(subtype) = image_subtype(&candidate) {
        if !matches!(subtype.as_str(), "png" | "jpeg" | "jpg" | "webp") {
            return Err(ImageError {
                reason: "тип изображения не поддерживается".to_string(),
                path: String::new(),
            });
        }
    }
    if !starts_with_ci(&candidate, "data:") {
        // Настоящая выгрузка Foundry хранит не картинку, а путь к ней в своих
        // файлах: `modules/…/Amber_Temple.webp`. Если выгрузка пришла архивом,
        // адаптер уже достал эту картинку и подставил data-URL; значит, путь до
        // нас дошёл из json, который лежит рядом с файлами Foundry, а не в выгрузке.
        return Err(ImageError {
            reason: "картинка лежит в файлах Foundry, а в выгрузке её нет".to_string(),
            path: candidate,
        });
    }

    if !is_supported_image_data_url(&candidate) {
        return Err(ImageError {
            reason: "Поддерживаются только изображения PNG, JPG и WEBP".to_string(),
            path: String::new(),
        });
    }
    let decoded = decode_image(Some(&candidate), LIMITS.max_image_bytes as usize)
        .map_err(|reason| ImageError { reason, path: String::new() })?;
    Ok(Image {
        data: candidate,
        mime: format!("image/{}", decoded.ext),
        bytes: decoded.bytes.len(),
    })
}

/// Приводит сырое описание документа к сущности.
pub fn to_entity(raw: &Value) -> Entity {
    let kind = js::get(raw, "kind")
        .and_then(Value::as_str)
        .filter(|kind| KINDS.contains(kind))
        .unwrap_or("item")
        .to_string();
    let image_raw = js::get(raw, "image").filter(|value| js::truthy(value));
    let (image, image_error) = match image_raw {
        Some(value) => match normalize_image(value) {
            Ok(image) => (Some(image), None),
            Err(error) => (None, Some(error)),
        },
        None => (None, None),
    };
    let name = js::truncate_utf16(&js::string_or(js::get(raw, "name"), ""), 200);
    let tags = match js::get(raw, "tags") {
        Some(Value::Array(items)) => {
            items.iter().map(js::string).take(32).collect()
        }
        _ => Vec::new(),
    };
    let children = match js::get(raw, "children") {
        Some(Value::Array(items)) => items.iter().map(to_entity).collect(),
        _ => Vec::new(),
    };
    let inner = js::get(raw, "raw").filter(|value| js::truthy(value)).and_then(|value| {
        matches!(value, Value::Object(_) | Value::Array(_)).then(|| value.clone())
    });
    Entity {
        kind,
        source_id: js::string_or(js::get(raw, "sourceId"), ""),
        name,
        tags,
        image,
        image_error,
        children,
        raw: inner,
    }
}

/// Прогоняет список сущностей источника через нормализацию.
///
/// Сущности без идентификатора и без имени выбрасываются: иначе в предпросмотре
/// невозможно их различать и выбирать. Причина каждой потери попадает в отчёт.
pub fn normalize(raw_entities: &[Value]) -> NormalizeResult {
    let mut entities = Vec::new();
    let mut skipped = Vec::new();

    for raw in raw_entities {
        if !matches!(raw, Value::Object(_)) {
            continue;
        }
        let source_id = js::string_or(js::get(raw, "sourceId"), "");
        let name = js::string_or(js::get(raw, "name"), "").trim().to_string();
        if source_id.is_empty() && name.is_empty() {
            skipped.push(Skipped {
                source_id: String::new(),
                name: String::new(),
                reason: "нет ни идентификатора, ни названия".to_string(),
            });
            continue;
        }
        let known = js::get(raw, "kind")
            .and_then(Value::as_str)
            .is_some_and(|kind| KINDS.contains(&kind));
        if !known {
            let kind = match js::get(raw, "kind") {
                Some(value) => js::string(value),
                None => "undefined".to_string(),
            };
            skipped.push(Skipped {
                source_id,
                name,
                reason: format!("неизвестный вид «{kind}»"),
            });
            continue;
        }
        let mut entity_raw = raw.clone();
        if let Value::Object(map) = &mut entity_raw {
            map.insert(
                "sourceId".to_string(),
                Value::String(if source_id.is_empty() { name.clone() } else { source_id }),
            );
            map.insert("name".to_string(), Value::String(name));
        }
        entities.push(to_entity(&entity_raw));
    }

    NormalizeResult { entities, skipped }
}
