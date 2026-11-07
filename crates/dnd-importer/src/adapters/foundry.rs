//! Адаптер Foundry VTT.
//!
//! ВАЖНО: документация форматов Foundry недоступна при разработке, поэтому
//! имена полей ниже — проверяемые предположения, а не гарантия. Всё, что не
//! распознано, попадает в отчёт как «пропущено», а не приводится к виду наугад.
//! Перед этапом 2 нужно снять дамп настоящего файла (docs/ИМПОРТ-ПРИКЛЮЧЕНИЙ.md,
//! раздел 16) и поправить это место.
//!
//! Код из модулей не выполняется: читаются только JSON и картинки.

use std::cell::RefCell;
use std::collections::HashMap;

use base64::Engine;
use serde_json::{Map, Value, json};

use crate::adapters::zip::{normalize_asset_path, read_zip_json_and_assets};
use crate::core::pipeline::{Adapter, Extracted, Input};
use crate::errors::{E_EMPTY, E_UNREADABLE, ImportError, import_error};
use crate::js;

/// Типы документов, которые мы умеем различать.
///
/// Ключи — разные написания одного и того же типа: в самом Foundry документ
/// называется `JournalEntry`, а в выгрузке Adventure ссылка лежит под ключом
/// `journal`. Без алиасов заметки молча превращались бы в предметы.
fn doc_type(name: &str) -> Option<&'static str> {
    Some(match name {
        "adventure" => "adventure",
        "scene" => "scene",
        "actor" => "actor",
        "item" => "item",
        "journal" | "journalentry" => "journal",
        "rolltable" => "rolltable",
        "macro" => "macro",
        _ => return None,
    })
}

const ZIP_MAGIC: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];

pub fn looks_like_zip(buffer: &[u8]) -> bool {
    buffer.starts_with(&ZIP_MAGIC)
}

pub fn looks_like_json(buffer: &[u8]) -> bool {
    for &byte in buffer.iter().take(512) {
        if matches!(byte, 0x20 | 0x09 | 0x0a | 0x0d | 0xef | 0xbb | 0xbf) {
            continue;
        }
        return byte == 0x7b || byte == 0x5b; // { или [
    }
    false
}

fn lower(value: Option<&Value>) -> String {
    let text = js::string_or(value, "").to_lowercase();
    text.chars().filter(|ch| ch.is_ascii_lowercase()).collect()
}

fn lower_str(text: &str) -> String {
    text.to_lowercase().chars().filter(|ch| ch.is_ascii_lowercase()).collect()
}

/// Приводит текст поля к строке с переносами строк.
///
/// В выгрузках встречаются оба вида: `{html}` и `{content}`. Второй — родной
/// для заметок Foundry: там лепят `pages[].text.content`, и раньше мы такой
/// текст теряли, принося в игру пустую заметку.
fn rich_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(text)) => strip_tags(text),
        Some(Value::Object(map)) => {
            for key in ["html", "content", "text"] {
                if let Some(Value::String(text)) = map.get(key) {
                    if !text.trim().is_empty() {
                        return strip_tags(text);
                    }
                }
            }
            String::new()
        }
        _ => String::new(),
    }
}

/// Убирает теги, оставляя читаемый текст.
fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let bytes = html.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'<' {
            if let Some((size, replacement)) = match_tag(&html[index..]) {
                out.push_str(replacement);
                index += size;
                continue;
            }
            if let Some(offset) = html[index..].find('>') {
                out.push(' ');
                index += offset + 1;
                continue;
            }
            out.push('<');
            index += 1;
            continue;
        }
        let ch = html[index..].chars().next().expect("index is a char boundary");
        out.push(ch);
        index += ch.len_utf8();
    }
    let collapsed = collapse_spaces_tabs(&out);
    collapse_newlines(&collapsed).trim().to_string()
}

/// `<br>`, `</p>`, `</div>`, `</li>`, `</h1>`..`</h6>` с их заменой.
fn match_tag(text: &str) -> Option<(usize, &'static str)> {
    let bytes = text.as_bytes();
    let is_ci = |index: usize, expected: u8| {
        bytes.get(index).is_some_and(|byte| byte.eq_ignore_ascii_case(&expected))
    };
    if is_ci(1, b'b') && is_ci(2, b'r') {
        let mut index = 3;
        while bytes.get(index).is_some_and(|byte| byte.is_ascii_whitespace()) {
            index += 1;
        }
        if bytes.get(index) == Some(&b'/') {
            index += 1;
        }
        if bytes.get(index) == Some(&b'>') {
            return Some((index + 1, "\n"));
        }
    }
    if bytes.get(1) == Some(&b'/') {
        let closing: &[(&[u8], usize)] = &[
            (b"p", 1),
            (b"div", 3),
            (b"li", 2),
            (b"h1", 2),
            (b"h2", 2),
            (b"h3", 2),
            (b"h4", 2),
            (b"h5", 2),
            (b"h6", 2),
        ];
        for (name, length) in closing {
            let matches = name
                .iter()
                .enumerate()
                .all(|(offset, byte)| is_ci(2 + offset, *byte));
            if matches && bytes.get(2 + length) == Some(&b'>') {
                return Some((3 + length, "\n"));
            }
        }
    }
    None
}

fn collapse_spaces_tabs(text: &str) -> String {
    let mut out = String::new();
    let mut previous_space = false;
    for ch in text.chars() {
        if matches!(ch, ' ' | '\t') {
            if !previous_space {
                out.push(' ');
            }
            previous_space = true;
        } else {
            out.push(ch);
            previous_space = false;
        }
    }
    out
}

fn collapse_newlines(text: &str) -> String {
    let mut out = String::new();
    let mut run = 0;
    for ch in text.chars() {
        if ch == '\n' {
            run += 1;
            if run <= 2 {
                out.push(ch);
            }
        } else {
            run = 0;
            out.push(ch);
        }
    }
    out
}

/// Текст заметки приключения.
///
/// Раньше мы читали только `text.html`, и настоящие выгрузки Foundry
/// импортировались с пустым текстом: в них лепят `pages`, а внутри страницы —
/// `text.content`. Читаем оба варианта: `pages` важнее, одиночный `text`
/// встречается в старых выгрузках и в самодельных файлах.
fn journal_text(doc: &Value) -> String {
    let pages = js::as_array(js::get(doc, "pages"));
    if !pages.is_empty() {
        let chunks: Vec<String> = pages
            .iter()
            .filter_map(|page| {
                let title = match js::get(page, "name").filter(|value| js::truthy(value)) {
                    Some(name) => js::string(name).trim().to_string(),
                    None => String::new(),
                };
                let body = rich_text(js::get(page, "text"));
                if body.is_empty() {
                    return None;
                }
                Some(if title.is_empty() { body } else { format!("{title}\n{body}") })
            })
            .collect();
        if !chunks.is_empty() {
            return chunks.join("\n\n");
        }
    }
    rich_text(js::nullish(&[
        js::get(doc, "text"),
        js::get(doc, "description"),
        js::get(doc, "content"),
    ]))
}

/// Достаёт картинку документа.
///
/// Поле с картинкой у Foundry не одно, и это главная причина, по которой
/// настоящие выгрузки приходили с пустыми сценами:
///   * `img` — обычные документы (актёры, предметы);
///   * `background` — сцена; внутри это объект, где путь лежит в `src`
///     (так делают новые версии) или в `data`/`img` (так было раньше);
///   * `texture` — токен на сцене, устроен так же, как `background`.
fn document_image(doc: &Value, resolve_asset: Option<&dyn Fn(&str) -> String>) -> String {
    for key in ["img", "background", "texture", "foreground"] {
        let Some(container) = js::get(doc, key) else {
            continue;
        };
        if !js::truthy(container) {
            continue;
        }
        if let Value::String(text) = container {
            let text = text.trim();
            if !text.is_empty() {
                return pick(text, resolve_asset);
            }
        }
        if matches!(container, Value::Object(_)) {
            for inner in ["src", "data", "img"] {
                if let Some(Value::String(text)) = js::get(container, inner) {
                    let text = text.trim();
                    if !text.is_empty() {
                        return pick(text, resolve_asset);
                    }
                }
            }
        }
    }
    String::new()
}

/// Если это путь внутри выгрузки, достаём картинку из архива.
fn pick(value: &str, resolve_asset: Option<&dyn Fn(&str) -> String>) -> String {
    if let Some(resolve) = resolve_asset {
        if !value
            .get(..5)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
        {
            let from_archive = resolve(value);
            if !from_archive.is_empty() {
                return from_archive;
            }
        }
    }
    value.to_string()
}

/// Токены сцены: позиция в пикселях, пригодится при маппинге.
fn scene_tokens(doc: &Value, resolve_asset: Option<&dyn Fn(&str) -> String>) -> Vec<Value> {
    js::as_array(js::get(doc, "tokens"))
        .iter()
        .filter(|token| matches!(token, Value::Object(_)))
        .map(|token| {
            // В новых версиях Foundry сам токен часто безымянный, а данные актёра
            // лежат в `delta`. Без этого фишка приезжала бы «без названия».
            let delta = js::get(token, "delta")
                .filter(|value| matches!(value, Value::Object(_)))
                .cloned()
                .unwrap_or_else(|| json!({}));
            let name = js::string_or(
                js::get(token, "name")
                    .filter(|value| js::truthy(value))
                    .or_else(|| js::get(token, "displayName").filter(|value| js::truthy(value)))
                    .or_else(|| js::get(&delta, "name").filter(|value| js::truthy(value))),
                "",
            );
            let source_id = {
                let explicit = js::string_or(
                    js::get(token, "_id")
                        .filter(|value| js::truthy(value))
                        .or_else(|| js::get(token, "id").filter(|value| js::truthy(value))),
                    "",
                );
                if explicit.is_empty() { name.clone() } else { explicit }
            };
            let width = js::number_to_value(js::number_or_zero(js::get(token, "width")));
            let height = js::number_to_value(js::number_or_zero(js::get(token, "height")));
            json!({
                "kind": "actor",
                "sourceId": source_id,
                "name": name,
                "image": document_image(token, resolve_asset),
                "raw": {
                    "x": js::number_to_value(js::number_or_zero(js::get(token, "x"))),
                    "y": js::number_to_value(js::number_or_zero(js::get(token, "y"))),
                    "width": width,
                    "height": height,
                    "actorId": js::string_or(js::get(token, "actorId"), ""),
                    "hidden": js::get(token, "hidden").is_some_and(js::truthy),
                }
            })
        })
        .collect()
}

fn stat_number(value: Option<&Value>) -> f64 {
    js::number_or_zero(value)
}

/// Статблок актёра из `system` Foundry. Поля dnd5e лежат по-разному в разных
/// версиях, поэтому читаем оба известных вида и молча пропускаем остальное:
/// лучше показать меньше, чем уронить импорт.
fn actor_statblock(doc: &Value) -> Option<Value> {
    if !js::get(doc, "system").is_some_and(js::truthy) && !js::get(doc, "data").is_some_and(js::truthy)
    {
        return None;
    }
    let system = js::get(doc, "system")
        .filter(|value| js::truthy(value))
        .or_else(|| js::get(doc, "data").filter(|value| js::truthy(value)))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let attributes = js::get(&system, "attributes")
        .filter(|value| js::truthy(value))
        .cloned()
        .unwrap_or_else(|| json!({}));

    let ac_raw = js::get(&attributes, "ac");
    let ac = if ac_raw.is_some_and(|value| matches!(value, Value::Object(_))) {
        let ac_raw = ac_raw.expect("checked above");
        js::nullish(&[js::get(ac_raw, "value"), js::get(ac_raw, "flat")])
    } else {
        ac_raw
    };
    let hp_raw = js::get(&attributes, "hp");
    let hp = if hp_raw.is_some_and(|value| matches!(value, Value::Object(_))) {
        let hp_raw = hp_raw.expect("checked above");
        js::nullish(&[js::get(hp_raw, "max"), js::get(hp_raw, "value")])
    } else {
        hp_raw
    };

    let movement = js::get(&attributes, "movement")
        .filter(|value| js::truthy(value))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut speed_parts = Vec::new();
    for (key, label) in [
        ("walk", ""),
        ("fly", "полёт"),
        ("swim", "плавание"),
        ("climb", "лазание"),
        ("burrow", "копание"),
    ] {
        let value = stat_number(js::get(&movement, key));
        if value > 0.0 {
            let value = js::number_to_value(value);
            speed_parts.push(if label.is_empty() {
                format!("{value} фт")
            } else {
                format!("{label} {value} фт")
            });
        }
    }

    let abilities_raw = js::get(&system, "abilities")
        .filter(|value| js::truthy(value))
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut abilities = Map::new();
    for key in ["str", "dex", "con", "int", "wis", "cha"] {
        let entry = js::get(&abilities_raw, key);
        let value = match entry {
            Some(entry) if matches!(entry, Value::Object(_)) => stat_number(js::get(entry, "value")),
            entry => stat_number(entry),
        };
        abilities.insert(key.to_string(), js::number_to_value(if value > 0.0 { value } else { 10.0 }));
    }

    let cr = js::get(&system, "details")
        .and_then(|details| js::get(details, "cr"))
        .filter(|value| js::truthy(value))
        .map(js::string)
        .unwrap_or_default();

    Some(json!({
        "ac": js::number_to_value(stat_number(ac)),
        "hp": js::number_to_value(stat_number(hp)),
        "speed": speed_parts.join(", "),
        "size": 1,
        "cr": cr,
        "abilities": Value::Object(abilities),
        "attacks": "",
        "notes": "",
    }))
}

/// Приводит один документ Foundry к плоскому описанию.
pub fn describe_document(
    doc: &Value,
    fallback_kind: Option<&str>,
    resolve_asset: Option<&dyn Fn(&str) -> String>,
) -> Value {
    let type_name = {
        let direct = js::get(doc, "type")
            .filter(|value| js::truthy(value))
            .or_else(|| js::get(doc, "documentName").filter(|value| js::truthy(value)));
        let mut name = lower(direct);
        if name.is_empty() {
            if let Some(fallback) = fallback_kind {
                name = lower_str(fallback);
            }
        }
        name
    };
    let empty = json!({});
    let raw = if matches!(doc, Value::Object(_)) { doc } else { &empty };
    // Неизвестный тип не превращаем молча в предмет: такой документ попадёт
    // в отчёт как пропущенный, и ДМ увидит, что содержимое осталось за бортом.
    let kind = doc_type(&type_name).unwrap_or("unknown");

    let source_id = js::string_or(
        js::get(raw, "_id")
            .filter(|value| js::truthy(value))
            .or_else(|| js::get(raw, "id").filter(|value| js::truthy(value)))
            .or_else(|| js::get(raw, "name").filter(|value| js::truthy(value))),
        "",
    );
    let name = js::string_or(js::get(raw, "name"), "");
    let tags = match js::get(raw, "tags") {
        Some(Value::Array(items)) => Value::Array(items.clone()),
        _ => json!([]),
    };
    let children = if kind == "scene" {
        Value::Array(scene_tokens(raw, resolve_asset))
    } else {
        json!([])
    };

    let text = if kind == "journal" {
        journal_text(raw)
    } else {
        rich_text(js::nullish(&[
            js::get(raw, "text"),
            js::get(raw, "description"),
            js::get(raw, "content"),
        ]))
    };

    let mut raw_map = Map::new();
    raw_map.insert("text".to_string(), Value::String(text));
    raw_map.insert("type".to_string(), Value::String(js::string_or(js::get(raw, "type"), "")));
    raw_map.insert("formula".to_string(), Value::String(js::string_or(js::get(raw, "formula"), "")));
    raw_map.insert(
        "sceneWidth".to_string(),
        js::number_to_value(js::number_or_zero(js::get(raw, "width"))),
    );
    raw_map.insert(
        "sceneHeight".to_string(),
        js::number_to_value(js::number_or_zero(js::get(raw, "height"))),
    );
    // Стены сцены: пиксельные отрезки и метки двери/окна. Всё остальное
    // (секретность, звуки, пороги) отбрасывается, чтобы отчёт честно
    // показывал, что сохранилось.
    if kind == "scene" {
        if let Some(Value::Array(walls)) = js::get(raw, "walls") {
            let mapped: Vec<Value> = walls
                .iter()
                .map(|wall| {
                    let coordinates = match js::get(wall, "c") {
                        Some(Value::Array(parts)) => {
                            parts.iter().map(|part| js::number_to_value(js::number(part))).collect()
                        }
                        _ => vec![Value::from(0), Value::from(0), Value::from(0), Value::from(0)],
                    };
                    json!({
                        "c": coordinates,
                        "door": js::get(wall, "d").map(js::number).unwrap_or(0.0) == 1.0,
                        "ds": js::number_to_value(js::number_or_zero(js::get(wall, "ds"))),
                        "window": js::get(wall, "window").is_some_and(js::truthy),
                    })
                })
                .collect();
            raw_map.insert("walls".to_string(), Value::Array(mapped));
        }
    }
    if kind == "actor" {
        if let Some(statblock) = actor_statblock(raw) {
            raw_map.insert("statblock".to_string(), statblock);
        }
    }

    json!({
        "kind": kind,
        "sourceId": source_id,
        "name": name,
        "tags": tags,
        "image": document_image(raw, resolve_asset),
        "children": children,
        "raw": Value::Object(raw_map),
    })
}

/// Разворачивает содержимое Adventure: внутри лежат ссылки на сцены и акты.
pub fn describe_adventure(doc: &Value, resolve_asset: Option<&dyn Fn(&str) -> String>) -> Vec<Value> {
    let mut out = Vec::new();
    let mut push = |entry: &Value, kind: &str| {
        if !js::truthy(entry) {
            return;
        }
        let item = match entry {
            Value::String(id) => json!({ "_id": id }),
            Value::Object(_) => entry.clone(),
            _ => json!({}),
        };
        let mut item = match item {
            Value::Object(map) => map,
            _ => Map::new(),
        };
        item.insert("type".to_string(), Value::String(kind.to_string()));
        out.push(describe_document(&Value::Object(item), Some(kind), resolve_asset));
    };

    for (key, kind) in [
        ("scenes", "scene"),
        ("actors", "actor"),
        ("items", "item"),
        ("journal", "journal"),
        ("rollTables", "rolltable"),
        ("macros", "macro"),
        ("macro", "macro"),
    ] {
        for entry in js::as_array(js::get(doc, key)) {
            push(entry, kind);
        }
    }

    if out.is_empty() {
        // Adventure может лежать одним файлом с вложенными документами.
        for key in ["scenes", "actors", "items", "journal", "rollTables"] {
            for entry in js::as_array(js::get(doc, key)) {
                if matches!(entry, Value::Object(_)) && js::get(entry, "type").is_some_and(js::truthy) {
                    out.push(describe_document(entry, None, resolve_asset));
                }
            }
        }
    }

    out
}

/// Собирает функцию, которая по пути вида `assets/карта.webp` находит картинку
/// внутри архива и отдаёт её как data-URL.
///
/// Foundry в выгруженном приключении пишет путь относительно папки выгрузки.
/// Прямо в zip он может оказаться с любым числом ведущих папок, поэтому ищем не
/// по точному совпадению, а по тому, чем заканчивается путь: это спасает и
/// `assets/x.webp`, и `adventure/assets/x.webp`.
fn make_asset_resolver<'a>(
    assets: &'a [(String, Vec<u8>)],
) -> Box<dyn Fn(&str) -> String + 'a> {
    if assets.is_empty() {
        return Box::new(|_| String::new());
    }
    let mut by_name: HashMap<String, &'a [u8]> = HashMap::new();
    let mut normalized: Vec<String> = Vec::new();
    for (name, data) in assets {
        let name = normalize_asset_path(name);
        by_name.insert(name.clone(), data);
        normalized.push(name);
    }

    // Кэш готовых строк по пути. Без него одна и та же карта, на которую
    // ссылаются сцена и тридцать её токенов, превращалась в тридцать одну
    // копию base64.
    let data_urls = RefCell::new(HashMap::<String, String>::new());
    // Поиск по «хвосту» пути нужен редко, но он линейный по всем картинкам.
    // Поэтому индекс строится один раз: имя файла → полный путь, первый выигрывает.
    let mut by_suffix: HashMap<String, String> = HashMap::new();
    for name in &normalized {
        let file = name.rsplit('/').next().unwrap_or(name).to_string();
        by_suffix.entry(file).or_insert_with(|| name.clone());
    }

    Box::new(move |path: &str| {
        let to_data_url = |name: &str, data_urls: &RefCell<HashMap<String, String>>| -> String {
            if let Some(cached) = data_urls.borrow().get(name) {
                return cached.clone();
            }
            let Some(data) = by_name.get(name) else {
                return String::new();
            };
            let url = format!(
                "data:image/{};base64,{}",
                extension_of(name),
                base64::engine::general_purpose::STANDARD.encode(data)
            );
            data_urls.borrow_mut().insert(name.to_string(), url.clone());
            url
        };

        let wanted = normalize_asset_path(path);
        if wanted.is_empty() {
            return String::new();
        }
        if by_name.contains_key(&wanted) {
            return to_data_url(&wanted, &data_urls);
        }
        let file = wanted.rsplit('/').next().unwrap_or(&wanted);
        match by_suffix.get(file) {
            Some(full) => to_data_url(full, &data_urls),
            None => String::new(),
        }
    })
}

fn extension_of(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "jpeg"
    } else if lower.ends_with(".webp") {
        "webp"
    } else if lower.ends_with(".gif") {
        "gif"
    } else {
        "png"
    }
}

pub fn from_json_buffer(buffer: &[u8]) -> Result<Extracted, ImportError> {
    let text = String::from_utf8_lossy(buffer);
    let doc: Value = serde_json::from_str(&text).map_err(|error| {
        import_error(E_UNREADABLE, format!("Файл не похож на корректный JSON: {error}"))
    })?;
    let list: Vec<&Value> = match &doc {
        Value::Array(items) => items.iter().collect(),
        other => vec![other],
    };
    let mut entities = Vec::new();
    for entry in list {
        if let Value::Array(items) = entry {
            for item in items {
                entities.push(describe_document(item, None, None));
            }
        } else if matches!(entry, Value::Object(_)) {
            let is_adventure = lower(js::get(entry, "type")).eq("adventure")
                || matches!(js::get(entry, "scenes"), Some(Value::Array(_)));
            if is_adventure {
                entities.extend(describe_adventure(entry, None));
            } else {
                entities.push(describe_document(entry, None, None));
            }
        }
    }
    if entities.is_empty() {
        return Err(import_error(
            E_EMPTY,
            "В файле не нашлось документов, которые можно импортировать.",
        ));
    }
    let first = match &doc {
        Value::Array(items) => items.first(),
        other => Some(other),
    };
    let source_id = js::string_or(first.and_then(|doc| js::get(doc, "_id")), "json");
    let source_title = js::string_or(first.and_then(|doc| js::get(doc, "name")), "Файл Foundry");
    Ok(Extracted { source_id, source_title, entities })
}

pub fn from_zip_buffer(buffer: &[u8]) -> Result<Extracted, ImportError> {
    let contents = read_zip_json_and_assets(buffer).map_err(|error| import_error(E_UNREADABLE, error))?;
    // Картинки лежат рядом с json, а не внутри него: без них сцены приедут
    // пустыми, потому что фон — это отдельный файл в выгрузке.
    let resolver = make_asset_resolver(&contents.assets);
    let resolve: Option<&dyn Fn(&str) -> String> = Some(&*resolver);
    let mut entities = Vec::new();
    let mut title = "Модуль Foundry".to_string();

    let manifest = contents.files.iter().find(|(name, _)| name == "module.json");
    if let Some((_, data)) = manifest {
        if let Ok(parsed) = serde_json::from_str::<Value>(&String::from_utf8_lossy(data)) {
            title = js::string_or(
                js::get(&parsed, "title").filter(|value| js::truthy(value)).or_else(|| {
                    js::get(&parsed, "name").filter(|value| js::truthy(value))
                }),
                &title,
            );
        }
    }

    for (name, data) in &contents.files {
        if name == "module.json" {
            continue;
        }
        let Ok(doc) = serde_json::from_str::<Value>(&String::from_utf8_lossy(data)) else {
            continue; // битый JSON внутри архива не должен рушить весь импорт
        };
        let list: Vec<&Value> = match &doc {
            Value::Array(items) => items.iter().collect(),
            other => vec![other],
        };
        for entry in list {
            if !matches!(entry, Value::Object(_)) {
                continue;
            }
            let is_adventure = lower(js::get(entry, "type")).eq("adventure")
                || matches!(js::get(entry, "scenes"), Some(Value::Array(_)));
            if is_adventure {
                // Если манифеста нет, показываем имя самого приключения, а не служебную заглушку.
                if manifest.is_none() {
                    if let Some(Value::String(name)) = js::get(entry, "name").filter(|value| js::truthy(value)) {
                        title = name.clone();
                    }
                }
                entities.extend(describe_adventure(entry, resolve));
            } else if js::get(entry, "type").is_some_and(js::truthy)
                || js::get(entry, "documentName").is_some_and(js::truthy)
            {
                entities.push(describe_document(entry, None, resolve));
            }
        }
    }

    if entities.is_empty() {
        return Err(import_error(
            E_EMPTY,
            "В архиве нет документов, которые можно импортировать.",
        ));
    }
    Ok(Extracted {
        source_id: format!("zip:{title}"),
        source_title: title,
        entities,
    })
}

pub struct Foundry;

impl Adapter for Foundry {
    fn id(&self) -> &'static str {
        "foundry"
    }

    fn can_handle(&self, input: &Input<'_>) -> bool {
        looks_like_zip(input.data) || looks_like_json(input.data)
    }

    fn extract(&self, input: &Input<'_>) -> Result<Extracted, ImportError> {
        if looks_like_zip(input.data) {
            from_zip_buffer(input.data)
        } else {
            from_json_buffer(input.data)
        }
    }
}
