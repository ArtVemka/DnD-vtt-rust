//! Time, initiative, tokens, templates, journal and sound content from
//! `server/state.js`.
//!
//! Like `sheet.rs`, every normalizer takes untrusted JSON and returns a plain
//! `Value`, matching the object-in/object-out boundary of the reference.

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::rules::{CALENDAR, CONDITIONS, LIGHT_LIMITS, damage_type_ids, normalize_fog_mask};
use crate::sheet::{LIMITS, clamp_int, clamp_number, normalize_adventure_image, sanitize_text};

/// JS `undefined` stand-in for field lookups. `null` and a missing field behave
/// the same for `sanitizeText`, but numeric readers below use `Option` to keep
/// `Number(undefined) === NaN` apart from `Number(null) === 0`.
static NULL: Value = Value::Null;

fn member<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

// ---------------------------------------------------------------------------
// JS helpers
// ---------------------------------------------------------------------------

/// `Number(value)`; `None` is `NaN`.
fn js_number(value: &Value) -> Option<f64> {
    match value {
        Value::Null => Some(0.0),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Some(0.0);
            }
            // shortcut: hex/octal/binary numeric strings are NaN here, JS Number() reads them.
            trimmed.parse::<f64>().ok()
        }
        Value::Array(items) => match items.len() {
            0 => Some(0.0),
            1 => match &items[0] {
                Value::Null => Some(0.0),
                item => js_number(&Value::String(js_to_string(item))),
            },
            _ => None,
        },
        Value::Object(_) => None,
    }
}

fn js_round(number: f64) -> f64 {
    if number.is_finite() {
        (number + 0.5).floor()
    } else {
        number
    }
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|number| number != 0.0 && !number.is_nan()),
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn js_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| {
                if item.is_null() {
                    String::new()
                } else {
                    js_to_string(item)
                }
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `clampInt` where a missing field is `undefined` (fallback), not `null` (0).
fn clamp_member_int(object: &Value, key: &str, min: i64, max: i64, fallback: i64) -> i64 {
    match object.get(key) {
        Some(value) => clamp_int(value, min, max, fallback),
        None => fallback,
    }
}

/// `clampNumber` with the same missing-field rule.
fn clamp_member_number(object: &Value, key: &str, min: f64, max: f64, fallback: f64) -> f64 {
    match object.get(key) {
        Some(value) => clamp_number(value, min, max, fallback),
        None => fallback,
    }
}

fn is_hex_color(text: &str) -> bool {
    text.len() == 7
        && text.starts_with('#')
        && text[1..].bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `String.prototype.slice(0, maxLength)` cut at UTF-16 units.
fn truncate_utf16(text: &str, max_length: usize) -> String {
    if text.encode_utf16().count() <= max_length {
        return text.to_string();
    }
    let mut units = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.len_utf16();
        if units + width > max_length {
            break;
        }
        units += width;
        out.push(ch);
    }
    out
}

/// `text.replace(/\s+/g, ' ')`.
fn collapse_whitespace(text: &str) -> String {
    let mut out = String::new();
    let mut in_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(ch);
            in_space = false;
        }
    }
    out
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// JSON number like JS: whole values stay whole (`1`, not `1.0`).
fn number_value(number: f64) -> Value {
    if !number.is_finite() {
        return Value::Null;
    }
    if number.fract() == 0.0 && number.abs() < 9e15 {
        Value::from(number as i64)
    } else {
        Value::from(number)
    }
}

/// `new Date().toISOString()` uses millisecond precision and a `Z` suffix.
fn new_id() -> String {
    Uuid::new_v4().simple().to_string()
}

fn new_prefixed_id(prefix: &str, hex_chars: usize) -> String {
    let raw = new_id();
    format!("{prefix}-{}", &raw[..hex_chars])
}

fn valid_prefixed_id(value: &Value, prefix: &str) -> Option<String> {
    let Value::String(text) = value else {
        return None;
    };
    let rest = text.strip_prefix(prefix)?;
    let valid = rest.len() == 12
        && rest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    valid.then(|| text.clone())
}

// ---------------------------------------------------------------------------
// Time and calendar
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeLimits {
    pub min_day: i64,
    pub max_day: i64,
    pub min_hour: i64,
    pub max_hour: i64,
    pub max_timer_seconds: i64,
}

pub const TIME_LIMITS: TimeLimits = TimeLimits {
    min_day: 1,
    max_day: CALENDAR.days_per_month * CALENDAR.max_months * 9999,
    min_hour: 0,
    max_hour: 23,
    // Upper bound of a turn timer: four hours.
    max_timer_seconds: 4 * 3600,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameTime {
    pub day: i64,
    pub hour: i64,
}

pub const DEFAULT_GAME_TIME: GameTime = GameTime { day: 1, hour: 8 };

/// `emptyTime`.
pub fn empty_time() -> Value {
    json!({ "day": DEFAULT_GAME_TIME.day, "hour": DEFAULT_GAME_TIME.hour })
}

/// `normalizeTime`.
pub fn normalize_time(raw: &Value) -> Value {
    let source = if raw.is_object() { raw } else { &NULL };
    json!({
        "day": clamp_member_int(source, "day", TIME_LIMITS.min_day, TIME_LIMITS.max_day, DEFAULT_GAME_TIME.day),
        "hour": clamp_member_int(source, "hour", TIME_LIMITS.min_hour, TIME_LIMITS.max_hour, DEFAULT_GAME_TIME.hour),
    })
}

/// `emptyTimer`.
pub fn empty_timer() -> Value {
    json!({ "running": false, "endsAt": 0, "totalSec": 0, "label": "" })
}

/// `normalizeTimer`.
pub fn normalize_timer(raw: &Value) -> Value {
    let source = if raw.is_object() { raw } else { &NULL };
    let ends_at = match js_number(member(source, "endsAt")) {
        // shortcut: a non-finite endsAt becomes 0 here; JS keeps Infinity until JSON writes null.
        Some(value) if value.is_finite() && value != 0.0 => js_round(value).max(0.0) as i64,
        _ => 0,
    };
    json!({
        "running": js_truthy(member(source, "running")),
        "endsAt": ends_at,
        "totalSec": clamp_member_int(source, "totalSec", 0, TIME_LIMITS.max_timer_seconds, 0),
        "label": sanitize_text(member(source, "label"), LIMITS.max_name_length).trim(),
    })
}

/// `normalizeCalendarMonths`: non-empty names capped at `CALENDAR.maxMonths`,
/// or the default month names when nothing usable is left.
pub fn normalize_calendar_months(raw: &Value) -> Value {
    let mut months = Vec::new();
    if let Some(items) = raw.as_array() {
        for item in items {
            let name = collapse_whitespace(&sanitize_text(item, 40)).trim().to_string();
            if name.is_empty() {
                continue;
            }
            months.push(Value::String(name));
            if months.len() >= CALENDAR.max_months as usize {
                break;
            }
        }
    }
    if months.is_empty() {
        months = CALENDAR
            .month_names
            .iter()
            .map(|name| Value::String((*name).to_string()))
            .collect();
    }
    Value::Array(months)
}

/// `normalizeCalendarEvent`: `null` when there is no title or a usable day.
pub fn normalize_calendar_event(raw: &Value) -> Value {
    if !raw.is_object() {
        return Value::Null;
    }
    let title = sanitize_text(member(raw, "title"), LIMITS.max_calendar_title)
        .trim()
        .to_string();
    if title.is_empty() {
        return Value::Null;
    }
    let day = clamp_member_int(
        raw,
        "day",
        TIME_LIMITS.min_day,
        TIME_LIMITS.max_day,
        0,
    );
    if day == 0 {
        return Value::Null;
    }
    let id = sanitize_text(member(raw, "id"), 40).trim().to_string();
    let id = if id.is_empty() {
        new_calendar_event_id()
    } else {
        id
    };
    json!({ "id": id, "day": day, "title": title })
}

/// `normalizeCalendarEvents`: drops duplicates by id and caps the list.
pub fn normalize_calendar_events(raw: &Value) -> Value {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Some(items) = raw.as_array() else {
        return Value::Array(result);
    };
    for item in items {
        let event = normalize_calendar_event(item);
        if event.is_null() {
            continue;
        }
        let id = js_to_string(member(&event, "id"));
        if !seen.insert(id) {
            continue;
        }
        result.push(event);
        if result.len() >= LIMITS.max_calendar_events as usize {
            break;
        }
    }
    Value::Array(result)
}

// ---------------------------------------------------------------------------
// Initiative
// ---------------------------------------------------------------------------

pub const INITIATIVE_KINDS: [&str; 3] = ["player", "token", "other"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitiativeLimits {
    pub min_modifier: i64,
    pub max_modifier: i64,
}

pub const INITIATIVE: InitiativeLimits = InitiativeLimits {
    min_modifier: -20,
    max_modifier: 20,
};

/// `emptyInitiative`.
pub fn empty_initiative() -> Value {
    json!({ "entries": [], "turnId": Value::Null, "round": 1 })
}

/// `normalizeInitiative`.
pub fn normalize_initiative(raw: &Value) -> Value {
    let mut entries = Vec::new();
    if let Some(source) = raw.as_object() {
        if let Some(items) = source.get("entries").and_then(Value::as_array) {
            for entry in items.iter().take(LIMITS.max_initiative_entries as usize) {
                if !entry.is_object() || !js_truthy(member(entry, "id")) {
                    continue;
                }
                let total = entry.get("total").and_then(js_number);
                let total = match total {
                    Some(number) if number.is_finite() => {
                        Value::from(clamp_int(member(entry, "total"), -99, 199, 0))
                    }
                    _ => Value::Null,
                };
                let kind = match member(entry, "kind") {
                    Value::String(text) if INITIATIVE_KINDS.contains(&text.as_str()) => {
                        text.clone()
                    }
                    _ => "other".to_string(),
                };
                let name = sanitize_text(member(entry, "name"), LIMITS.max_name_length);
                entries.push(json!({
                    "id": js_to_string(member(entry, "id")),
                    "kind": kind,
                    "refId": sanitize_text(member(entry, "refId"), 64),
                    "name": if name.is_empty() { "Участник".to_string() } else { name },
                    "modifier": clamp_member_int(entry, "modifier", INITIATIVE.min_modifier, INITIATIVE.max_modifier, 0),
                    "total": total,
                }));
            }
        }
    }
    // `turnId === null` means the fight has not started; keep it that way.
    let turn = member(raw, "turnId");
    let turn_id = match turn {
        Value::String(text) if entries.iter().any(|entry| member(entry, "id") == turn) => {
            Value::String(text.clone())
        }
        _ => Value::Null,
    };
    let round = clamp_member_int(raw, "round", 1, 999, 1);
    json!({ "entries": entries, "turnId": turn_id, "round": round })
}

// ---------------------------------------------------------------------------
// Token extras
// ---------------------------------------------------------------------------

/// `normalizeConditions`: known ids only, in input order, no duplicates.
pub fn normalize_conditions(raw: &Value) -> Value {
    let Some(items) = raw.as_array() else {
        return Value::Array(Vec::new());
    };
    let mut seen: Vec<&str> = Vec::new();
    for item in items {
        let Value::String(id) = item else {
            continue;
        };
        if CONDITIONS.iter().any(|condition| condition.id == id)
            && !seen.contains(&id.as_str())
        {
            seen.push(id);
        }
        if seen.len() >= LIMITS.max_token_conditions as usize {
            break;
        }
    }
    Value::Array(seen.into_iter().map(|id| json!(id)).collect())
}

/// `normalizeConditionDurations`: positive rounds only, clamped to 0..99.
pub fn normalize_condition_durations(raw: &Value, conditions: &Value) -> Value {
    let mut durations = Map::new();
    if !raw.is_object() {
        return Value::Object(durations);
    }
    let Some(items) = conditions.as_array() else {
        return Value::Object(durations);
    };
    for condition in items {
        let Value::String(id) = condition else {
            continue;
        };
        let rounds = clamp_member_int(raw, id, 0, 99, 0);
        if rounds > 0 {
            durations.insert(id.clone(), Value::from(rounds));
        }
    }
    Value::Object(durations)
}

/// `normalizeConditionUnits`: only for conditions with a positive duration.
pub fn normalize_condition_units(raw: &Value, conditions: &Value, durations: &Value) -> Value {
    let mut units = Map::new();
    let source = if raw.is_object() { raw } else { &NULL };
    let Some(items) = conditions.as_array() else {
        return Value::Object(units);
    };
    for condition in items {
        let Value::String(id) = condition else {
            continue;
        };
        if !js_truthy(member(durations, id)) {
            continue;
        }
        let unit = match member(source, id) {
            Value::String(text) if text == "hour" => "hour",
            _ => "round",
        };
        units.insert(id.clone(), json!(unit));
    }
    Value::Object(units)
}

/// `normalizeAura`: `null` when there is no positive radius.
pub fn normalize_aura(raw: &Value) -> Value {
    if !raw.is_object() {
        return Value::Null;
    }
    let requested = raw.get("feet").and_then(js_number);
    let Some(requested) = requested.filter(|feet| feet.is_finite() && *feet > 0.0) else {
        return Value::Null;
    };
    let feet = clamp_int(&Value::from(requested), 5, 120, 0);
    if feet == 0 {
        return Value::Null;
    }
    let color = match member(raw, "color") {
        Value::String(text) if is_hex_color(text) => text.clone(),
        _ => "#6aa9ff".to_string(),
    };
    json!({ "feet": feet, "color": color })
}

/// `normalizeVision`: feet of sight, 0..240.
pub fn normalize_vision(raw: &Value) -> Value {
    json!(clamp_int(raw, 0, 240, 0))
}

/// `normalizeDamageDefenses`: known damage types, lowercased and deduplicated.
pub fn normalize_damage_defenses(raw: &Value) -> Value {
    let known = damage_type_ids();
    let mut result: Vec<String> = Vec::new();
    let source: Vec<&Value>;
    let comma;
    match raw {
        Value::Array(items) => source = items.iter().collect(),
        other => {
            let text = if js_truthy(other) {
                js_to_string(other)
            } else {
                String::new()
            };
            comma = text
                .split(',')
                .map(|part| Value::String(part.to_string()))
                .collect::<Vec<_>>();
            source = comma.iter().collect();
        }
    }
    for item in source {
        let value = sanitize_text(item, 40).trim().to_lowercase();
        if known.contains(&value.as_str()) && !result.contains(&value) {
            result.push(value);
        }
    }
    result.truncate(16);
    Value::Array(result.into_iter().map(Value::String).collect())
}

/// `normalizeFog`: delegates to the fog model.
pub fn normalize_fog(raw: &Value) -> Value {
    Value::String(normalize_fog_mask(raw))
}

/// `normalizeTokenEmote`: `None` is JS `undefined` (expired or empty emote).
pub fn normalize_token_emote(raw: &Value) -> Option<Value> {
    if !raw.is_object() {
        return None;
    }
    let text = sanitize_text(member(raw, "text"), LIMITS.max_token_emote_length)
        .trim()
        .to_string();
    let until = raw.get("until").and_then(js_number);
    let Some(until) = until.filter(|until| until.is_finite()) else {
        return None;
    };
    let now_ms = Utc::now().timestamp_millis() as f64;
    if text.is_empty() || until <= now_ms {
        return None;
    }
    Some(json!({ "text": text, "until": js_round(until) as i64 }))
}

/// `normalizeDarkness`: share of darkness from 0 to 1.
pub fn normalize_darkness(raw: &Value) -> Value {
    let value = js_number(raw).unwrap_or(f64::NAN);
    if !value.is_finite() {
        return json!(0);
    }
    number_value(value.clamp(0.0, 1.0))
}

const DEFAULT_LIGHT_COLOR: &str = "#ffb347";

/// `normalizeTokenLight`: radius in feet and color, zero when unlit.
pub fn normalize_token_light(raw: &Value) -> Value {
    if !raw.is_object() {
        return json!({ "radius": 0, "color": DEFAULT_LIGHT_COLOR });
    }
    let color = match member(raw, "color") {
        Value::String(text) if is_hex_color(text) => text.clone(),
        _ => DEFAULT_LIGHT_COLOR.to_string(),
    };
    let radius = raw.get("radius").and_then(js_number);
    let Some(radius) = radius.filter(|radius| radius.is_finite() && *radius > 0.0) else {
        return json!({ "radius": 0, "color": color });
    };
    let radius = js_round(radius).min(LIGHT_LIMITS.max_radius) as i64;
    json!({ "radius": radius, "color": color })
}

// ---------------------------------------------------------------------------
// Templates
// ---------------------------------------------------------------------------

pub const TEMPLATE_KINDS: [&str; 4] = ["sphere", "cone", "line", "square"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateLimits {
    pub max_per_map: i64,
    pub min_size: i64,
    pub max_size: i64,
}

pub const TEMPLATE_LIMITS: TemplateLimits = TemplateLimits {
    max_per_map: 20,
    min_size: 5,
    max_size: 120,
};

/// `normalizeTemplate`: `null` for anything that is not an object.
pub fn normalize_template(raw: &Value) -> Value {
    if !raw.is_object() {
        return Value::Null;
    }
    let kind = match member(raw, "kind") {
        Value::String(text) if TEMPLATE_KINDS.contains(&text.as_str()) => text.clone(),
        _ => "sphere".to_string(),
    };
    let id = match raw.get("id") {
        Some(value) if js_truthy(value) => js_to_string(value),
        _ => new_token_id(),
    };
    let color = match member(raw, "color") {
        Value::String(text) if is_hex_color(text) => text.clone(),
        _ => String::new(),
    };
    json!({
        "id": id,
        "kind": kind,
        "x": number_value(clamp_member_number(raw, "x", 0.0, 1.0, 0.5)),
        "y": number_value(clamp_member_number(raw, "y", 0.0, 1.0, 0.5)),
        "size": clamp_member_int(raw, "size", TEMPLATE_LIMITS.min_size, TEMPLATE_LIMITS.max_size, 20),
        "rotation": clamp_member_int(raw, "rotation", 0, 359, 0),
        "color": color,
    })
}

/// `normalizeTemplates`.
pub fn normalize_templates(raw: &Value) -> Value {
    let Some(items) = raw.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        items
            .iter()
            .take(TEMPLATE_LIMITS.max_per_map as usize)
            .map(normalize_template)
            .filter(|template| !template.is_null())
            .collect(),
    )
}

// ---------------------------------------------------------------------------
// Journal
// ---------------------------------------------------------------------------

pub const JOURNAL_KINDS: [&str; 5] = ["npc", "location", "quest", "faction", "item"];

/// `normalizeJournalTags`: case-insensitive duplicates dropped.
fn normalize_journal_tags(value: &Value) -> Value {
    let mut tags = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Some(items) = value.as_array() else {
        return Value::Array(tags);
    };
    for raw in items {
        let tag = sanitize_text(raw, LIMITS.max_journal_tag).trim().to_string();
        if tag.is_empty() || !seen.insert(tag.to_lowercase()) {
            continue;
        }
        tags.push(Value::String(tag));
        if tags.len() >= LIMITS.max_journal_tags as usize {
            break;
        }
    }
    Value::Array(tags)
}

/// `normalizeJournalEntry`: `null` without a title.
pub fn normalize_journal_entry(entry: &Value) -> Value {
    if !entry.is_object() {
        return Value::Null;
    }
    let title = sanitize_text(member(entry, "title"), LIMITS.max_journal_title)
        .trim()
        .to_string();
    if title.is_empty() {
        return Value::Null;
    }
    let id = valid_prefixed_id(member(entry, "id"), "j-").unwrap_or_else(new_journal_id);
    let kind = match member(entry, "kind") {
        Value::String(text) if JOURNAL_KINDS.contains(&text.as_str()) => text.clone(),
        _ => "npc".to_string(),
    };
    json!({
        "id": id,
        "kind": kind,
        "title": title,
        "text": sanitize_text(member(entry, "text"), LIMITS.max_journal_text),
        "image": normalize_adventure_image(member(entry, "image")),
        "tags": normalize_journal_tags(member(entry, "tags")),
        "visible": js_truthy(member(entry, "visible")),
        "updatedAt": now_iso(),
    })
}

/// `normalizeJournalEntries`.
pub fn normalize_journal_entries(list: &Value) -> Value {
    let mut entries = Vec::new();
    let Some(items) = list.as_array() else {
        return Value::Array(entries);
    };
    for item in items {
        let entry = normalize_journal_entry(item);
        if !entry.is_null() {
            entries.push(entry);
        }
        if entries.len() >= LIMITS.max_journal_entries as usize {
            break;
        }
    }
    Value::Array(entries)
}

// ---------------------------------------------------------------------------
// Inventory
// ---------------------------------------------------------------------------

/// `normalizeInventoryItem`: `null` without a name.
pub fn normalize_inventory_item(item: &Value) -> Value {
    if !item.is_object() {
        return Value::Null;
    }
    let name = sanitize_text(member(item, "name"), LIMITS.max_inventory_name)
        .trim()
        .to_string();
    if name.is_empty() {
        return Value::Null;
    }
    let id = valid_prefixed_id(member(item, "id"), "i-").unwrap_or_else(new_inventory_id);
    let qty = match item.get("qty").and_then(js_number) {
        Some(number) if number.is_finite() => js_round(number) as i64,
        _ => 1,
    };
    let weight = match item.get("weight").and_then(js_number) {
        Some(number) if number.is_finite() => number,
        _ => 0.0,
    };
    let weight = js_round(weight.clamp(0.0, 99999.0) * 100.0) / 100.0;
    let owner_id = match member(item, "ownerId") {
        Value::String(text) => truncate_utf16(text, 64),
        _ => String::new(),
    };
    json!({
        "id": id,
        "name": name,
        "qty": qty.clamp(0, 9999),
        "weight": number_value(weight),
        "ownerId": owner_id,
        "attunement": js_truthy(member(item, "attunement")),
        "notes": sanitize_text(member(item, "notes"), LIMITS.max_inventory_notes),
    })
}

/// `normalizeInventory`.
pub fn normalize_inventory(list: &Value) -> Value {
    let mut items = Vec::new();
    let Some(source) = list.as_array() else {
        return Value::Array(items);
    };
    for item in source {
        let entry = normalize_inventory_item(item);
        if !entry.is_null() {
            items.push(entry);
        }
        if items.len() >= LIMITS.max_inventory_items as usize {
            break;
        }
    }
    Value::Array(items)
}

// ---------------------------------------------------------------------------
// Roll requests
// ---------------------------------------------------------------------------

/// `normalizeRollRequest`: `null` without a label.
pub fn normalize_roll_request(item: &Value) -> Value {
    if !item.is_object() {
        return Value::Null;
    }
    let label = sanitize_text(member(item, "label"), 120).trim().to_string();
    if label.is_empty() {
        return Value::Null;
    }
    let raw_dc = match item.get("dc") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) if text.is_empty() => None,
        Some(value) => js_number(value),
    };
    let dc = raw_dc
        .filter(|number| number.is_finite())
        .map(|number| (js_round(number) as i64).clamp(1, 40));
    let id = valid_prefixed_id(member(item, "id"), "r-").unwrap_or_else(new_roll_request_id);
    let to = sanitize_text(member(item, "to"), 64).trim().to_string();
    let to_name = sanitize_text(member(item, "toName"), 40).trim().to_string();
    let mut request = Map::new();
    request.insert("id".to_string(), json!(id));
    request.insert("label".to_string(), json!(label));
    request.insert("dc".to_string(), dc.map_or(Value::Null, Value::from));
    request.insert(
        "formula".to_string(),
        json!(sanitize_text(member(item, "formula"), 120).trim()),
    );
    // `undefined` fields are dropped by JSON.stringify in the reference.
    if !to.is_empty() {
        request.insert("to".to_string(), json!(to));
    }
    if !to_name.is_empty() {
        request.insert("toName".to_string(), json!(to_name));
    }
    request.insert("createdAt".to_string(), json!(now_iso()));
    request.insert(
        "by".to_string(),
        json!(sanitize_text(member(item, "by"), 40).trim()),
    );
    Value::Object(request)
}

/// `normalizeRollRequests`.
pub fn normalize_roll_requests(list: &Value) -> Value {
    let mut requests = Vec::new();
    let Some(items) = list.as_array() else {
        return Value::Array(requests);
    };
    for item in items {
        let request = normalize_roll_request(item);
        if !request.is_null() {
            requests.push(request);
        }
        if requests.len() >= LIMITS.max_roll_requests as usize {
            break;
        }
    }
    Value::Array(requests)
}

// ---------------------------------------------------------------------------
// Soundboard
// ---------------------------------------------------------------------------

/// `normalizeAudioRef`: only the table's own `/files/<name>` references.
pub fn normalize_audio_ref(raw: &Value) -> Option<String> {
    let Value::String(_) = raw else {
        return None;
    };
    let reference = sanitize_text(raw, LIMITS.max_audio_ref).trim().to_string();
    is_audio_ref(&reference).then_some(reference)
}

fn is_audio_ref(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let Some(rest) = lower.strip_prefix("/files/") else {
        return false;
    };
    let Some((stem, ext)) = rest.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && matches!(ext, "mp3" | "ogg" | "wav")
}

/// `normalizeSound`: `null` when the reference is not a table file.
pub fn normalize_sound(raw: &Value) -> Value {
    if !raw.is_object() {
        return Value::Null;
    }
    let Some(reference) = normalize_audio_ref(member(raw, "ref")) else {
        return Value::Null;
    };
    let name = sanitize_text(member(raw, "name"), LIMITS.max_name_length)
        .trim()
        .to_string();
    let name = if name.is_empty() {
        "Звук".to_string()
    } else {
        name
    };
    let id = sanitize_text(member(raw, "id"), 40).trim().to_string();
    let id = if id.is_empty() { new_sound_id() } else { id };
    json!({ "id": id, "name": name, "ref": reference })
}

/// `normalizeSoundboard`: duplicates by id dropped, list capped.
pub fn normalize_soundboard(raw: &Value) -> Value {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let Some(items) = raw.as_array() else {
        return Value::Array(result);
    };
    for item in items {
        let sound = normalize_sound(item);
        if sound.is_null() {
            continue;
        }
        let id = js_to_string(member(&sound, "id"));
        if !seen.insert(id) {
            continue;
        }
        result.push(sound);
        if result.len() >= LIMITS.max_soundboard as usize {
            break;
        }
    }
    Value::Array(result)
}

// ---------------------------------------------------------------------------
// Id generators
// ---------------------------------------------------------------------------

/// `newTokenId`: 8 random bytes as hex.
pub fn new_token_id() -> String {
    new_id()[..16].to_string()
}

/// `newMapId`.
pub fn new_map_id() -> String {
    new_prefixed_id("m", 12)
}

/// `newSceneId`.
pub fn new_scene_id() -> String {
    new_prefixed_id("sc", 12)
}

/// `newJournalId`.
pub fn new_journal_id() -> String {
    new_prefixed_id("j", 12)
}

/// `newInventoryId`.
pub fn new_inventory_id() -> String {
    new_prefixed_id("i", 12)
}

/// `newCalendarEventId`.
pub fn new_calendar_event_id() -> String {
    new_prefixed_id("c", 12)
}

/// `newRollRequestId`.
pub fn new_roll_request_id() -> String {
    new_prefixed_id("r", 12)
}

/// `newSoundId`.
pub fn new_sound_id() -> String {
    new_prefixed_id("snd", 12)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn is_lower_hex(text: &str) -> bool {
        text.bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    #[test]
    fn limits_match_reference() {
        assert_eq!(TIME_LIMITS.min_day, 1);
        assert_eq!(TIME_LIMITS.max_day, 7_199_280);
        assert_eq!(TIME_LIMITS.min_hour, 0);
        assert_eq!(TIME_LIMITS.max_hour, 23);
        assert_eq!(TIME_LIMITS.max_timer_seconds, 14_400);
        assert_eq!(INITIATIVE.min_modifier, -20);
        assert_eq!(INITIATIVE.max_modifier, 20);
        assert_eq!(TEMPLATE_LIMITS.max_per_map, 20);
        assert_eq!(TEMPLATE_LIMITS.min_size, 5);
        assert_eq!(TEMPLATE_LIMITS.max_size, 120);
    }

    #[test]
    fn empty_time_and_timer() {
        assert_eq!(empty_time(), json!({ "day": 1, "hour": 8 }));
        assert_eq!(
            empty_timer(),
            json!({ "running": false, "endsAt": 0, "totalSec": 0, "label": "" })
        );
    }

    #[test]
    fn time_clamps_and_falls_back() {
        assert_eq!(
            normalize_time(&json!({ "day": 999_999_999, "hour": 99 })),
            json!({ "day": TIME_LIMITS.max_day, "hour": 23 })
        );
        assert_eq!(
            normalize_time(&json!({})),
            json!({ "day": 1, "hour": 8 }),
            "missing fields use DEFAULT_GAME_TIME"
        );
        assert_eq!(
            normalize_time(&json!("garbage")),
            json!({ "day": 1, "hour": 8 })
        );
        assert_eq!(
            normalize_time(&json!({ "day": "abc" })),
            json!({ "day": 1, "hour": 8 })
        );
        assert_eq!(
            normalize_time(&json!({ "day": -5, "hour": null })),
            json!({ "day": 1, "hour": 0 }),
            "null is Number(null) === 0, not a missing field"
        );
    }

    #[test]
    fn timer_clamps_fields() {
        let timer = normalize_timer(&json!({
            "running": 1,
            "endsAt": -50,
            "totalSec": 999_999,
            "label": "  ход  "
        }));
        assert_eq!(timer["running"], json!(true));
        assert_eq!(timer["endsAt"], json!(0));
        assert_eq!(timer["totalSec"], json!(14_400));
        assert_eq!(timer["label"], json!("ход"));

        let timer = normalize_timer(&json!({ "endsAt": "12.4", "totalSec": "7" }));
        assert_eq!(timer["endsAt"], json!(12));
        assert_eq!(timer["totalSec"], json!(7));
        assert_eq!(normalize_timer(&json!(5))["totalSec"], json!(0));
    }

    #[test]
    fn calendar_months_fall_back_and_collapse() {
        let months = normalize_calendar_months(&json!(["  Янв   арь ", "", 5]));
        assert_eq!(months, json!(["Янв арь", "5"]));

        let defaults = normalize_calendar_months(&json!([]));
        assert_eq!(defaults.as_array().unwrap().len(), 12);
        assert_eq!(defaults[0], json!(CALENDAR.month_names[0]));

        let capped = normalize_calendar_months(&Value::Array(
            (0..30).map(|index| json!(format!("м{index}"))).collect(),
        ));
        assert_eq!(capped.as_array().unwrap().len(), 24);
    }

    #[test]
    fn calendar_event_needs_title_and_day() {
        assert_eq!(normalize_calendar_event(&json!({})), Value::Null);
        assert_eq!(
            normalize_calendar_event(&json!({ "title": "  " })),
            Value::Null
        );
        assert_eq!(
            normalize_calendar_event(&json!({ "title": "Праздник" })),
            Value::Null,
            "missing day is a fallback of 0 and drops the event"
        );

        let event = normalize_calendar_event(&json!({
            "id": "  c-1  ",
            "day": 0,
            "title": " Праздник "
        }));
        assert_eq!(event["id"], json!("c-1"));
        assert_eq!(event["day"], json!(1), "day 0 clamps up to minDay");
        assert_eq!(event["title"], json!("Праздник"));

        let generated = normalize_calendar_event(&json!({ "title": "x", "day": 3 }));
        let id = generated["id"].as_str().unwrap();
        assert!(is_lower_hex(&id[2..]) && id.starts_with("c-") && id.len() == 14);
    }

    #[test]
    fn calendar_events_dedupe_and_cap() {
        let events = normalize_calendar_events(&json!([
            { "id": "c-1", "day": 2, "title": "a" },
            { "id": "c-1", "day": 3, "title": "b" },
            { "id": "c-2", "day": 4, "title": "c" }
        ]));
        assert_eq!(events.as_array().unwrap().len(), 2);
        assert_eq!(events[0]["title"], json!("a"));

        let many = Value::Array(
            (0..300)
                .map(|index| json!({ "id": format!("c-{index}"), "day": 2, "title": "t" }))
                .collect(),
        );
        assert_eq!(
            normalize_calendar_events(&many).as_array().unwrap().len(),
            LIMITS.max_calendar_events as usize
        );
        assert_eq!(
            normalize_calendar_events(&json!("x")),
            Value::Array(vec![])
        );
    }

    #[test]
    fn empty_initiative_defaults() {
        assert_eq!(
            empty_initiative(),
            json!({ "entries": [], "turnId": Value::Null, "round": 1 })
        );
    }

    #[test]
    fn initiative_garbage_returns_empty_state() {
        let state = normalize_initiative(&json!(5));
        assert_eq!(state["entries"], json!([]));
        assert_eq!(state["turnId"], Value::Null);
        assert_eq!(state["round"], json!(1));
    }

    #[test]
    fn initiative_clamps_and_filters_entries() {
        let state = normalize_initiative(&json!({
            "entries": [
                { "id": "a", "kind": "weird", "modifier": 99, "total": "x", "name": "" },
                { "id": "" },
                { "kind": "player" },
                { "id": "b", "kind": "player", "modifier": -99, "total": 250, "name": " Игрок " }
            ],
            "round": 9999
        }));
        let entries = state["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["kind"], json!("other"));
        assert_eq!(entries[0]["modifier"], json!(20));
        assert_eq!(entries[0]["total"], Value::Null);
        assert_eq!(entries[0]["name"], json!("Участник"));
        assert_eq!(entries[1]["total"], json!(199));
        assert_eq!(entries[1]["modifier"], json!(-20));
        assert_eq!(entries[1]["name"], json!(" Игрок "));
        assert_eq!(state["round"], json!(999));
    }

    #[test]
    fn initiative_turn_id_must_reference_an_entry() {
        let valid = normalize_initiative(&json!({
            "entries": [{ "id": "a" }],
            "turnId": "a"
        }));
        assert_eq!(valid["turnId"], json!("a"));

        let dangling = normalize_initiative(&json!({
            "entries": [{ "id": "a" }],
            "turnId": "b"
        }));
        assert_eq!(dangling["turnId"], Value::Null);
    }

    #[test]
    fn conditions_keep_only_known_unique_ids() {
        let conditions = normalize_conditions(&json!(["prone", "bogus", "prone", "blinded", 5]));
        assert_eq!(conditions, json!(["prone", "blinded"]));
        assert_eq!(normalize_conditions(&json!("x")), json!([]));

        let all: Vec<Value> = CONDITIONS
            .iter()
            .flat_map(|condition| [json!(condition.id), json!(condition.id)])
            .collect();
        assert_eq!(
            normalize_conditions(&Value::Array(all))
                .as_array()
                .unwrap()
                .len(),
            LIMITS.max_token_conditions as usize
        );
    }

    #[test]
    fn condition_durations_clamp_and_drop_zero() {
        let conditions = json!(["prone", "blinded", "stunned", "restrained"]);
        let durations = normalize_condition_durations(
            &json!({ "prone": 3, "blinded": 0, "stunned": 150, "restrained": "x", "bonus": 5 }),
            &conditions,
        );
        assert_eq!(durations, json!({ "prone": 3, "stunned": 99 }));
        assert_eq!(
            normalize_condition_durations(&json!([1, 2]), &conditions),
            json!({})
        );
    }

    #[test]
    fn condition_units_only_for_active_durations() {
        let conditions = json!(["prone", "blinded", "stunned"]);
        let durations = json!({ "prone": 3, "blinded": 1 });
        let units = normalize_condition_units(
            &json!({ "prone": "hour", "blinded": "day", "stunned": "hour" }),
            &conditions,
            &durations,
        );
        assert_eq!(units, json!({ "prone": "hour", "blinded": "round" }));

        assert_eq!(
            normalize_condition_units(&json!("x"), &conditions, &durations),
            json!({ "prone": "round", "blinded": "round" })
        );
    }

    #[test]
    fn aura_requires_positive_feet() {
        assert_eq!(normalize_aura(&json!({ "feet": 0 })), Value::Null);
        assert_eq!(normalize_aura(&json!({ "feet": -10 })), Value::Null);
        assert_eq!(normalize_aura(&json!({ "feet": "abc" })), Value::Null);
        assert_eq!(normalize_aura(&json!(5)), Value::Null);
        assert_eq!(
            normalize_aura(&json!({ "feet": 2, "color": "red" })),
            json!({ "feet": 5, "color": "#6aa9ff" })
        );
        assert_eq!(
            normalize_aura(&json!({ "feet": 500, "color": "#ABCDEF" })),
            json!({ "feet": 120, "color": "#ABCDEF" })
        );
    }

    #[test]
    fn vision_clamps_to_240() {
        assert_eq!(normalize_vision(&json!(300)), json!(240));
        assert_eq!(normalize_vision(&json!(-5)), json!(0));
        assert_eq!(normalize_vision(&json!("abc")), json!(0));
    }

    #[test]
    fn damage_defenses_split_and_dedupe() {
        assert_eq!(
            normalize_damage_defenses(&json!("огонь, огонь, холод, свет")),
            json!(["огонь", "холод"])
        );
        assert_eq!(
            normalize_damage_defenses(&json!(["ОГОНЬ", "кислота", "кислота"])),
            json!(["огонь", "кислота"])
        );
        assert_eq!(
            normalize_damage_defenses(&json!("огонь,холод,электричество,кислота,яд,некротический,излучение,силовой,дробящий,колющий,рубящий,психический"))
                .as_array()
                .unwrap()
                .len(),
            12
        );
    }

    #[test]
    fn fog_mask_is_padded_by_rules() {
        let mask = normalize_fog(&json!("101"));
        let text = mask.as_str().unwrap();
        assert_eq!(text.chars().count(), 2560);
        assert!(text.starts_with("101"));
        assert!(text.chars().all(|ch| ch == '0' || ch == '1'));
        assert_eq!(normalize_fog(&json!(null)).as_str().unwrap().len(), 2560);
    }

    #[test]
    fn emote_drops_expired_and_empty_text() {
        let until = Utc::now().timestamp_millis() + 60_000;
        let emote = normalize_token_emote(&json!({ "text": "  привет  ", "until": until }))
            .expect("future emote");
        assert_eq!(emote["text"], json!("привет"));
        assert_eq!(emote["until"].as_i64().unwrap(), js_round(until as f64) as i64);

        assert_eq!(
            normalize_token_emote(&json!({ "text": "x", "until": 1 })),
            None
        );
        assert_eq!(
            normalize_token_emote(&json!({ "text": "  ", "until": until })),
            None
        );
        assert_eq!(normalize_token_emote(&json!(5)), None);
    }

    #[test]
    fn darkness_clamps_to_unit_range() {
        assert_eq!(normalize_darkness(&json!(2)), json!(1));
        assert_eq!(normalize_darkness(&json!(-1)), json!(0));
        assert_eq!(normalize_darkness(&json!("abc")), json!(0));
        assert_eq!(normalize_darkness(&json!(0.25)), json!(0.25));
    }

    #[test]
    fn token_light_clamps_radius_and_color() {
        assert_eq!(
            normalize_token_light(&json!({ "radius": 999, "color": "#00ff00" })),
            json!({ "radius": 200, "color": "#00ff00" })
        );
        assert_eq!(
            normalize_token_light(&json!({ "radius": -5 })),
            json!({ "radius": 0, "color": "#ffb347" })
        );
        assert_eq!(
            normalize_token_light(&json!({ "radius": 10, "color": "blue" })),
            json!({ "radius": 10, "color": "#ffb347" })
        );
        assert_eq!(
            normalize_token_light(&json!(5)),
            json!({ "radius": 0, "color": "#ffb347" })
        );
    }

    #[test]
    fn template_defaults_and_clamps() {
        assert_eq!(normalize_template(&json!(5)), Value::Null);

        let template = normalize_template(&json!({
            "id": "t-1",
            "kind": "cone",
            "x": 2,
            "y": -1,
            "size": 1,
            "rotation": 400,
            "color": "#FFFFFF"
        }));
        assert_eq!(template["id"], json!("t-1"));
        assert_eq!(template["kind"], json!("cone"));
        assert_eq!(template["x"], json!(1));
        assert_eq!(template["y"], json!(0));
        assert_eq!(template["size"], json!(5));
        assert_eq!(template["rotation"], json!(359));
        assert_eq!(template["color"], json!("#FFFFFF"));

        let defaults = normalize_template(&json!({}));
        assert_eq!(defaults["kind"], json!("sphere"));
        assert_eq!(defaults["x"], json!(0.5));
        assert_eq!(defaults["y"], json!(0.5));
        assert_eq!(defaults["size"], json!(20));
        assert_eq!(defaults["rotation"], json!(0));
        assert_eq!(defaults["color"], json!(""));
        assert!(is_lower_hex(defaults["id"].as_str().unwrap()));
        assert_eq!(defaults["id"].as_str().unwrap().len(), 16);

        let bad_kind = normalize_template(&json!({ "kind": "cube", "color": "red" }));
        assert_eq!(bad_kind["kind"], json!("sphere"));
        assert_eq!(bad_kind["color"], json!(""));
    }

    #[test]
    fn templates_cap_and_skip_garbage() {
        let mut items: Vec<Value> = (0..25).map(|_| json!({})).collect();
        items.insert(0, json!(null));
        let templates = normalize_templates(&Value::Array(items));
        assert_eq!(templates.as_array().unwrap().len(), 19);
        assert_eq!(normalize_templates(&json!("x")), json!([]));
    }

    #[test]
    fn journal_entry_requires_title_and_normalizes_fields() {
        assert_eq!(normalize_journal_entry(&json!({})), Value::Null);
        assert_eq!(
            normalize_journal_entry(&json!({ "title": "  " })),
            Value::Null
        );

        let entry = normalize_journal_entry(&json!({
            "id": "j-0123456789ab",
            "kind": "quest",
            "title": "  Замок  ",
            "text": "текст",
            "image": "javascript:alert(1)",
            "tags": ["Лор", "лор", "  ", "  Бой "],
            "visible": 1
        }));
        assert_eq!(entry["id"], json!("j-0123456789ab"));
        assert_eq!(entry["kind"], json!("quest"));
        assert_eq!(entry["title"], json!("Замок"));
        assert_eq!(entry["text"], json!("текст"));
        assert_eq!(entry["image"], json!(""));
        assert_eq!(entry["tags"], json!(["Лор", "Бой"]));
        assert_eq!(entry["visible"], json!(true));
        assert!(
            entry["updatedAt"]
                .as_str()
                .unwrap()
                .ends_with('Z')
        );

        let generated = normalize_journal_entry(&json!({
            "id": "j-0123456789AB",
            "kind": "bogus",
            "title": "x"
        }));
        assert_eq!(generated["kind"], json!("npc"));
        let id = generated["id"].as_str().unwrap();
        assert!(id.starts_with("j-") && id.len() == 14 && is_lower_hex(&id[2..]));
    }

    #[test]
    fn journal_entries_cap() {
        let entries = Value::Array(
            (0..400)
                .map(|index| json!({ "title": format!("e{index}") }))
                .collect(),
        );
        assert_eq!(
            normalize_journal_entries(&entries)
                .as_array()
                .unwrap()
                .len(),
            LIMITS.max_journal_entries as usize
        );
    }

    #[test]
    fn inventory_item_defaults_and_clamps() {
        assert_eq!(normalize_inventory_item(&json!({})), Value::Null);

        let item = normalize_inventory_item(&json!({
            "name": "  Меч  ",
            "qty": -5,
            "weight": 1.2349,
            "ownerId": "x".repeat(70),
            "attunement": 1,
            "notes": "n"
        }));
        assert_eq!(item["name"], json!("Меч"));
        assert_eq!(item["qty"], json!(0));
        assert_eq!(item["weight"], json!(1.23));
        assert_eq!(item["ownerId"].as_str().unwrap().len(), 64);
        assert_eq!(item["attunement"], json!(true));

        let defaults = normalize_inventory_item(&json!({ "name": "Лук" }));
        assert_eq!(defaults["qty"], json!(1));
        assert_eq!(defaults["weight"], json!(0));
        assert_eq!(defaults["ownerId"], json!(""));
        assert_eq!(defaults["attunement"], json!(false));
        assert!(defaults["id"].as_str().unwrap().starts_with("i-"));

        let null_qty = normalize_inventory_item(&json!({ "name": "x", "qty": null }));
        assert_eq!(null_qty["qty"], json!(0));
        let huge = normalize_inventory_item(&json!({ "name": "x", "qty": 20_000, "weight": 999_999 }));
        assert_eq!(huge["qty"], json!(9999));
        assert_eq!(huge["weight"], json!(99_999));
    }

    #[test]
    fn inventory_cap() {
        let items = Value::Array(
            (0..600)
                .map(|index| json!({ "name": format!("i{index}") }))
                .collect(),
        );
        assert_eq!(
            normalize_inventory(&items).as_array().unwrap().len(),
            LIMITS.max_inventory_items as usize
        );
    }

    #[test]
    fn roll_request_dc_and_optional_fields() {
        assert_eq!(normalize_roll_request(&json!({})), Value::Null);
        assert_eq!(
            normalize_roll_request(&json!({ "label": "  " })),
            Value::Null
        );

        let request = normalize_roll_request(&json!({
            "label": " Спас ",
            "dc": 0,
            "formula": " 1d20 ",
            "to": "  ",
            "toName": "",
            "by": " ДМ "
        }));
        assert_eq!(request["label"], json!("Спас"));
        assert_eq!(request["dc"], json!(1));
        assert_eq!(request["formula"], json!("1d20"));
        assert!(request.get("to").is_none());
        assert!(request.get("toName").is_none());
        assert_eq!(request["by"], json!("ДМ"));
        assert!(request["createdAt"].as_str().unwrap().ends_with('Z'));
        assert!(request["id"].as_str().unwrap().starts_with("r-"));

        assert_eq!(
            normalize_roll_request(&json!({ "label": "x", "dc": 99 }))["dc"],
            json!(40)
        );
        assert_eq!(
            normalize_roll_request(&json!({ "label": "x", "dc": "" }))["dc"],
            Value::Null
        );
        assert_eq!(
            normalize_roll_request(&json!({ "label": "x", "dc": "abc" }))["dc"],
            Value::Null
        );
        assert_eq!(
            normalize_roll_request(&json!({ "label": "x", "dc": null }))["dc"],
            Value::Null
        );
    }

    #[test]
    fn roll_requests_cap() {
        let requests = Value::Array(
            (0..30)
                .map(|index| json!({ "label": format!("r{index}") }))
                .collect(),
        );
        assert_eq!(
            normalize_roll_requests(&requests)
                .as_array()
                .unwrap()
                .len(),
            LIMITS.max_roll_requests as usize
        );
    }

    #[test]
    fn audio_ref_only_accepts_table_files() {
        assert_eq!(
            normalize_audio_ref(&json!("/files/track-01.mp3")),
            Some("/files/track-01.mp3".to_string())
        );
        assert_eq!(
            normalize_audio_ref(&json!("/files/TRACK.MP3")),
            Some("/files/TRACK.MP3".to_string())
        );
        assert_eq!(normalize_audio_ref(&json!("/files/a_b.mp3")), None);
        assert_eq!(normalize_audio_ref(&json!("/files/a.mp3.exe")), None);
        assert_eq!(normalize_audio_ref(&json!("http://evil/x.mp3")), None);
        assert_eq!(normalize_audio_ref(&json!("/files/.mp3")), None);
        assert_eq!(normalize_audio_ref(&json!(5)), None);
    }

    #[test]
    fn sound_requires_own_ref() {
        assert_eq!(
            normalize_sound(&json!({ "ref": "http://evil/x.mp3" })),
            Value::Null
        );
        assert_eq!(normalize_sound(&json!(5)), Value::Null);

        let sound = normalize_sound(&json!({
            "ref": "/files/bell.ogg",
            "id": "  ",
            "name": "  "
        }));
        assert_eq!(sound["ref"], json!("/files/bell.ogg"));
        assert_eq!(sound["name"], json!("Звук"));
        let id = sound["id"].as_str().unwrap();
        assert!(id.starts_with("snd-") && id.len() == 16 && is_lower_hex(&id[4..]));

        let named = normalize_sound(&json!({
            "ref": "/files/bell.wav",
            "id": "s-1",
            "name": "  Бум "
        }));
        assert_eq!(named["id"], json!("s-1"));
        assert_eq!(named["name"], json!("Бум"));
    }

    #[test]
    fn soundboard_dedupes_and_caps() {
        let duplicates = normalize_soundboard(&json!([
            { "id": "s-1", "ref": "/files/a.mp3" },
            { "id": "s-1", "ref": "/files/b.mp3" },
            { "id": "s-2", "ref": "/files/c.mp3" }
        ]));
        assert_eq!(duplicates.as_array().unwrap().len(), 2);
        assert_eq!(duplicates[0]["ref"], json!("/files/a.mp3"));

        let many = Value::Array(
            (0..50)
                .map(|index| json!({ "id": format!("s{index}"), "ref": "/files/a.mp3" }))
                .collect(),
        );
        assert_eq!(
            normalize_soundboard(&many).as_array().unwrap().len(),
            LIMITS.max_soundboard as usize
        );
        assert_eq!(normalize_soundboard(&json!("x")), json!([]));
    }

    #[test]
    fn id_generators_match_js_formats() {
        let token = new_token_id();
        assert_eq!(token.len(), 16);
        assert!(is_lower_hex(&token));

        let cases = [
            (new_map_id(), "m-"),
            (new_scene_id(), "sc-"),
            (new_journal_id(), "j-"),
            (new_inventory_id(), "i-"),
            (new_calendar_event_id(), "c-"),
            (new_roll_request_id(), "r-"),
            (new_sound_id(), "snd-"),
        ];
        for (id, prefix) in cases {
            assert!(id.starts_with(prefix), "{id} starts with {prefix}");
            let hex = &id[prefix.len()..];
            assert_eq!(hex.len(), 12);
            assert!(is_lower_hex(hex));
        }
    }
}