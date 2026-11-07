//! Initiative, combat mode, hp, damage, healing, attacks, rests, xp.
//!
//! Port of the combat slice of `server/socket.js`. Locks are only held while
//! state is touched; every broadcast is sent after the guard is dropped.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use dnd_core::content;
use dnd_core::dice::{self, ParsedFormula, Roll};
use dnd_core::model::MAP_TYPES;
use dnd_core::room::{ROLE_DM, ROLE_PLAYER, RoomState};
use dnd_core::rules;
use dnd_core::sheet::{self, LIMITS};
use rand::Rng;
use serde_json::{Map, Value, json};
use socketioxide::SocketIo;
use socketioxide::extract::{Data, SocketRef};

use crate::table::{Session, Table};

use super::{require_dm, require_joined};

const HP_NOTICE_DELAY_MS: u64 = 500;

const SAVE_LABELS: [(&str, &str); 6] = [
    ("str", "Сил"),
    ("dex", "Лов"),
    ("con", "Тел"),
    ("int", "Инт"),
    ("wis", "Мдр"),
    ("cha", "Хар"),
];

#[derive(Debug, Clone)]
struct Actor {
    player_id: String,
    name: String,
    role: String,
}

impl Actor {
    fn of(session: &Session) -> Self {
        Self {
            player_id: session.player_id.clone(),
            name: session.name.clone(),
            role: session.role.clone(),
        }
    }
}

#[derive(Debug, Clone)]
struct PendingHp {
    before: i64,
    actor: Actor,
    damage_type: String,
    defense: String,
    generation: u64,
}

struct HpNotices {
    next_generation: u64,
    pending: HashMap<String, PendingHp>,
}

static HP_NOTICES: LazyLock<Mutex<HpNotices>> = LazyLock::new(|| {
    Mutex::new(HpNotices {
        next_generation: 1,
        pending: HashMap::new(),
    })
});

/// Priorities for a view of one token: the full DM copy and the player copy
/// (`None` when players must not see the token at all).
type TokenView = (Value, Option<Value>);

// ---------------------------------------------------------------------------
// Small JS-compatible helpers
// ---------------------------------------------------------------------------

fn number(value: Option<&Value>) -> f64 {
    match value {
        None | Some(Value::Null) => 0.0,
        Some(Value::Bool(flag)) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(value)) => value.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        Some(Value::Array(items)) => {
            if items.len() == 1 {
                number(items.first())
            } else {
                f64::NAN
            }
        }
        Some(Value::Object(_)) => f64::NAN,
    }
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(value)) => value
            .as_f64()
            .map(|n| n != 0.0 && !n.is_nan())
            .unwrap_or(false),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

fn js_text(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(value)) => value.to_string(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| js_text(Some(item)))
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

/// `Math.round`, which rounds halves towards +Infinity.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn now_iso() -> String {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = duration.as_secs() as i64;
    let millis = duration.subsec_millis();
    let days = seconds.div_euclid(86_400);
    let rem = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days`, UTC.
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

fn hex(bytes: usize) -> String {
    let mut rng = rand::rng();
    let value: u64 = rng.random();
    let mask = if bytes >= 8 {
        u64::MAX
    } else {
        (1u64 << (bytes * 8)) - 1
    };
    format!("{:0width$x}", value & mask, width = bytes * 2)
}

fn new_initiative_id() -> String {
    format!("i-{}", hex(6))
}

fn chat_id() -> String {
    format!("msg-{}-{}", base36(now_ms()), hex(4))
}

fn base36(mut value: u128) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut buffer = Vec::new();
    while value > 0 {
        buffer.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    buffer.reverse();
    String::from_utf8(buffer).unwrap_or_default()
}

fn rand_d20() -> i64 {
    i64::from(rand::rng().random_range(1..=20u32))
}

fn roll_formula(parsed: &ParsedFormula) -> Roll {
    dice::roll_formula(parsed, &mut |min, max| rand::rng().random_range(min..=max))
}

fn emit_error(socket: &SocketRef, message: &str) {
    let _ = socket.emit("errorMessage", &json!({ "message": message }));
}

// ---------------------------------------------------------------------------
// Chat, roll requests, token broadcasts
// ---------------------------------------------------------------------------

fn make_chat(
    from: &str,
    player_id: &str,
    role: &str,
    text: String,
    avatar: Option<&Value>,
) -> Value {
    let mut message = Map::new();
    message.insert("from".to_string(), json!(from));
    message.insert("playerId".to_string(), json!(player_id));
    message.insert("role".to_string(), json!(role));
    if let Some(avatar) = avatar {
        message.insert("avatar".to_string(), avatar.clone());
    }
    message.insert("text".to_string(), json!(text));
    message.insert("system".to_string(), json!(true));
    message.insert("time".to_string(), json!(now_iso()));
    Value::Object(message)
}

fn push_chat(room: &mut RoomState, message: Value) -> Value {
    let mut message = message;
    if let Some(object) = message.as_object_mut() {
        object.insert("id".to_string(), json!(chat_id()));
    }
    room.chat_history.push(message.clone());
    let limit = LIMITS.max_chat_history as usize;
    if room.chat_history.len() > limit {
        let excess = room.chat_history.len() - limit;
        room.chat_history.drain(0..excess);
    }
    message
}

fn push_roll_request(room: &mut RoomState, request: Value) {
    if request.is_null() {
        return;
    }
    room.roll_requests.push(request);
    if room.roll_requests.len() > LIMITS.max_roll_requests as usize {
        room.roll_requests.remove(0);
    }
}

fn roll_requests_for_emit(room: &RoomState) -> Vec<(String, Value)> {
    let mut result = vec![(
        "dm-sockets".to_string(),
        Value::Array(room.roll_requests.clone()),
    )];
    for (player_id, player) in &room.players {
        if player.get("role").and_then(Value::as_str) != Some(ROLE_PLAYER) {
            continue;
        }
        let visible = room
            .roll_requests
            .iter()
            .filter(|request| match request.get("to").and_then(Value::as_str) {
                Some(to) => to == player_id,
                None => true,
            })
            .cloned()
            .collect();
        result.push((player_id.clone(), Value::Array(visible)));
    }
    result
}

async fn emit_roll_requests(io: &SocketIo, list: Vec<(String, Value)>) {
    for (room, payload) in list {
        let _ = io.to(room).emit("rollRequestUpdated", &payload).await;
    }
}

fn token_views_of(room: &RoomState, token: &Value) -> TokenView {
    let full = room.token_for_viewer(token, true);
    if !room.is_token_visible_to_players(token) {
        return (full, None);
    }
    (full, Some(room.token_for_viewer(token, false)))
}

async fn emit_token_view(io: &SocketIo, event: &str, view: &TokenView) {
    let _ = io.within("dm-sockets").emit(event, &view.0).await;
    if let Some(player_view) = &view.1 {
        let _ = io
            .within("table")
            .except("dm-sockets")
            .emit(event, player_view)
            .await;
    }
}

async fn emit_token_views(io: &SocketIo, event: &str, views: &[TokenView]) {
    for view in views {
        emit_token_view(io, event, view).await;
    }
}

fn roll_feed(
    player_id: &str,
    player_name: &str,
    role: &str,
    formula: &str,
    result: i64,
    die: i64,
    note: &str,
) -> Value {
    json!({
        "playerId": player_id,
        "playerName": if player_name.is_empty() { "Стол" } else { player_name },
        "role": if role.is_empty() { ROLE_DM } else { role },
        "formula": formula,
        "count": 1,
        "sides": 20,
        "values": [die],
        "keptValues": [die],
        "droppedValues": [],
        "modifier": 0,
        "result": result,
        "note": note,
        "hidden": false,
        "time": now_iso()
    })
}

// ---------------------------------------------------------------------------
// Tokens and damage
// ---------------------------------------------------------------------------

fn find_token(room: &RoomState, token_id: &str) -> Option<Value> {
    room.tokens
        .iter()
        .find(|token| token.get("id").and_then(Value::as_str) == Some(token_id))
        .cloned()
}

fn token_index(room: &RoomState, token_id: &str) -> Option<usize> {
    room.tokens
        .iter()
        .position(|token| token.get("id").and_then(Value::as_str) == Some(token_id))
}

fn token_name(token: &Value) -> String {
    token
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn visible_name(room: &RoomState, token: &Value) -> String {
    if room.is_token_visible_to_players(token) && !truthy(token.get("hideName")) {
        format!("«{}»", token_name(token))
    } else {
        "скрытый монстр".to_string()
    }
}

fn token_ac(room: &RoomState, token: &Value) -> i64 {
    let from_sheet = token
        .get("assignedPlayerId")
        .and_then(Value::as_str)
        .and_then(|player_id| room.sheets.get(player_id))
        .map(|sheet| sheet::clamp_int(sheet.get("armorClass").unwrap_or(&Value::Null), 1, 60, 0))
        .unwrap_or(0);
    if from_sheet != 0 {
        return from_sheet;
    }
    if number(token.get("armorClass")) > 0.0 {
        return sheet::clamp_int(token.get("armorClass").unwrap_or(&Value::Null), 1, 60, 0);
    }
    0
}

fn sync_sheet_tokens(room: &mut RoomState, player_id: &str) -> Vec<Value> {
    let Some(sheet) = room.sheets.get(player_id).cloned() else {
        return Vec::new();
    };
    let max_hp = rules::effective_max_hp(&sheet) as i64;
    let hp = sheet::clamp_int(sheet.get("hp").unwrap_or(&Value::Null), 0, max_hp, 0);
    let mut tokens = Vec::new();
    for token in room.tokens.iter_mut() {
        if token.get("assignedPlayerId").and_then(Value::as_str) != Some(player_id) {
            continue;
        }
        token["maxHp"] = json!(max_hp);
        token["hp"] = json!(hp);
        tokens.push(token.clone());
    }
    tokens
}

/// `room.setTokenHp`, split out because the handler needs the updated sheet.
fn set_token_hp_value(
    room: &mut RoomState,
    index: usize,
    value: Option<&Value>,
) -> (Value, Option<Value>) {
    let assigned = room.tokens[index]
        .get("assignedPlayerId")
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(player_id) = assigned {
        if let Some(sheet) = room.sheets.get(&player_id).cloned() {
            let max_hp = rules::effective_max_hp(&sheet) as i64;
            let was_down = number(sheet.get("hp")) <= 0.0;
            let hp = sheet::clamp_int(value.unwrap_or(&Value::Null), 0, max_hp, 0);
            let mut sheet = sheet;
            sheet["hp"] = json!(hp);
            if was_down && hp > 0 {
                sheet["deathSavesSuccesses"] = json!(0);
                sheet["deathSavesFailures"] = json!(0);
            }
            room.sheets.insert(player_id, sheet.clone());
            let token = &mut room.tokens[index];
            token["maxHp"] = json!(max_hp);
            token["hp"] = json!(hp);
            return (token.clone(), Some(sheet));
        }
    }

    let max_hp = {
        let token = &room.tokens[index];
        match token.get("maxHp") {
            None | Some(Value::Null) => None,
            Some(value) => Some(sheet::clamp_int(value, 1, LIMITS.max_token_hp, 1)),
        }
    };
    let Some(max_hp) = max_hp else {
        return (room.tokens[index].clone(), None);
    };
    let next = match value {
        Some(value) => number(Some(value)),
        None => f64::NAN,
    };
    if !next.is_finite() {
        return (room.tokens[index].clone(), None);
    }
    let next = js_round(next) as i64;
    let token = &mut room.tokens[index];
    token["hp"] = json!(next.clamp(0, max_hp));
    (token.clone(), None)
}

struct DamageOutcome {
    token_id: String,
    token: Value,
    assigned: Option<String>,
    sheet: Option<Value>,
    before_hp: i64,
    damage: i64,
    damage_type: String,
    defense: String,
    concentration: Option<(String, i64)>,
    token_concentration: Option<(String, i64)>,
}

/// `room.applyTokenDamage`. `raw_type` is sanitized here, like the reference.
fn apply_token_damage(
    room: &mut RoomState,
    token_id: &str,
    raw_amount: i64,
    raw_type: &Value,
    healing: bool,
) -> Option<DamageOutcome> {
    let index = token_index(room, token_id)?;
    let raw = raw_amount.clamp(0, 9999);
    if raw == 0 {
        return None;
    }
    let token = room.tokens[index].clone();
    let damage_type = if healing {
        String::new()
    } else {
        sheet::sanitize_text(raw_type, 40).trim().to_lowercase()
    };
    let assigned = token
        .get("assignedPlayerId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let sheet = assigned
        .as_ref()
        .and_then(|player_id| room.sheets.get(player_id))
        .cloned();
    let max_hp = match &sheet {
        Some(sheet) => Some(rules::effective_max_hp(sheet) as i64),
        None => match token.get("maxHp") {
            None | Some(Value::Null) => None,
            Some(value) => Some(sheet::clamp_int(value, 1, LIMITS.max_token_hp, 1)),
        },
    };
    let max_hp = max_hp?;
    let before_hp = match &sheet {
        Some(sheet) => number(sheet.get("hp")) as i64,
        None => sheet::clamp_int(token.get("hp").unwrap_or(&Value::Null), 0, max_hp, 0),
    };

    let defense = if healing {
        json!({ "amount": raw, "kind": "normal" })
    } else {
        let defenses = json!({
            "resistances": token.get("damageResistances").cloned().unwrap_or_else(|| json!([])),
            "vulnerabilities": token.get("damageVulnerabilities").cloned().unwrap_or_else(|| json!([])),
            "immunities": token.get("damageImmunities").cloned().unwrap_or_else(|| json!([]))
        });
        rules::damage_after_defenses(&json!(raw), &Value::String(damage_type.clone()), &defenses)
    };
    let amount = defense.get("amount").and_then(Value::as_i64).unwrap_or(0);
    let defense_kind = defense
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("normal")
        .to_string();

    let mut updated_sheet = None;
    if let Some(mut sheet) = sheet {
        let was_down = number(sheet.get("hp")) <= 0.0;
        if healing {
            let hp = sheet::clamp_int(
                &json!(number(sheet.get("hp")) as i64 + amount),
                0,
                max_hp,
                0,
            );
            sheet["hp"] = json!(hp);
            if was_down && hp > 0 {
                sheet["deathSavesSuccesses"] = json!(0);
                sheet["deathSavesFailures"] = json!(0);
            }
        } else {
            let temp = sheet::clamp_int(sheet.get("tempHp").unwrap_or(&Value::Null), 0, 9999, 0);
            let absorbed = temp.min(amount);
            sheet["tempHp"] = json!(temp - absorbed);
            let hp_damage = amount - absorbed;
            if hp_damage > 0 && was_down {
                sheet["deathSavesSuccesses"] = json!(0);
                if hp_damage >= max_hp {
                    sheet["deathSavesFailures"] = json!(LIMITS.max_death_saves);
                } else {
                    let failures = sheet::clamp_int(
                        sheet.get("deathSavesFailures").unwrap_or(&Value::Null),
                        0,
                        LIMITS.max_death_saves,
                        0,
                    );
                    sheet["deathSavesFailures"] = json!((failures + 1).min(LIMITS.max_death_saves));
                }
            }
            let hp = sheet::clamp_int(
                &json!(number(sheet.get("hp")) as i64 - hp_damage),
                0,
                max_hp,
                0,
            );
            sheet["hp"] = json!(hp);
            if !was_down && hp_damage > 0 && hp == 0 && hp_damage - before_hp >= max_hp {
                sheet["deathSavesFailures"] = json!(LIMITS.max_death_saves);
                sheet["deathSavesSuccesses"] = json!(0);
            }
        }
        let player_id = assigned.clone().unwrap_or_default();
        room.sheets.insert(player_id, sheet.clone());
        updated_sheet = Some(sheet);
    }

    let token = {
        let token = &mut room.tokens[index];
        match &updated_sheet {
            Some(sheet) => {
                token["maxHp"] = json!(max_hp);
                token["hp"] = sheet.get("hp").cloned().unwrap_or(json!(0));
            }
            None => {
                if healing {
                    token["hp"] = json!(sheet::clamp_int(
                        &json!(before_hp + amount),
                        0,
                        max_hp,
                        max_hp
                    ));
                } else {
                    token["hp"] = json!(sheet::clamp_int(&json!(before_hp - amount), 0, max_hp, 0));
                }
            }
        }
        token.clone()
    };

    let concentration = match (&updated_sheet, healing) {
        (Some(sheet), false) if amount > 0 => {
            let spell = sheet
                .get("concentration")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if spell.is_empty() {
                None
            } else {
                Some((spell, (amount / 2).max(10)))
            }
        }
        _ => None,
    };
    let token_concentration = match (&updated_sheet, healing) {
        (None, false) if amount > 0 && number(token.get("maxHp")) > 0.0 => {
            let spell = token
                .get("concentration")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if spell.is_empty() {
                None
            } else {
                Some((spell, (amount / 2).max(10)))
            }
        }
        _ => None,
    };

    Some(DamageOutcome {
        token_id: token_id.to_string(),
        token,
        assigned,
        sheet: updated_sheet,
        before_hp,
        damage: amount,
        damage_type,
        defense: defense_kind,
        concentration,
        token_concentration,
    })
}

async fn finish_damage(io: &SocketIo, table: &Arc<Table>, outcome: DamageOutcome, actor: Actor) {
    table.mark_dirty();
    let assigned = outcome.assigned.clone();
    let token_views: Vec<TokenView>;
    let mut roll_broadcast = None;
    let mut monster_feed = None;
    let mut monster_update = None;

    {
        let mut room = table.room.lock().unwrap();
        if outcome.sheet.is_some() && assigned.is_some() {
            let player_id = assigned.clone().unwrap_or_default();
            let tokens = sync_sheet_tokens(&mut room, &player_id);
            token_views = tokens
                .iter()
                .map(|token| token_views_of(&room, token))
                .collect();
        } else {
            token_views = vec![token_views_of(&room, &outcome.token)];
        }

        let mut need_requests = false;
        if let (Some(sheet), Some(player_id)) = (outcome.sheet.as_ref(), assigned.as_ref()) {
            if let Some((spell, dc)) = &outcome.concentration {
                let player_name = room
                    .players
                    .get(player_id)
                    .and_then(|player| player.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let modifier = rules::compute_derived(sheet)
                    .get("saves")
                    .and_then(|saves| saves.get("con"))
                    .and_then(|save| save.get("total"))
                    .and_then(Value::as_i64)
                    .unwrap_or(0);
                let existing = room.roll_requests.iter().position(|request| {
                    request.get("to").and_then(Value::as_str) == Some(player_id.as_str())
                        && request
                            .get("label")
                            .and_then(Value::as_str)
                            .map(|label| label.starts_with("Концентрация: "))
                            .unwrap_or(false)
                });
                match existing {
                    Some(index) => {
                        let current = room.roll_requests[index]
                            .get("dc")
                            .and_then(Value::as_i64)
                            .unwrap_or(0);
                        if *dc > current {
                            let label = format!("Концентрация: {spell} (СЛ {dc})");
                            let formula =
                                format!("1d20{}{modifier}", if modifier >= 0 { "+" } else { "" });
                            room.roll_requests[index]["dc"] = json!(dc);
                            room.roll_requests[index]["label"] = json!(label);
                            room.roll_requests[index]["formula"] = json!(formula);
                            need_requests = true;
                        }
                    }
                    None => {
                        let request = content::normalize_roll_request(&json!({
                            "label": format!("Концентрация: {spell} (СЛ {dc})"),
                            "dc": dc,
                            "formula": format!("1d20{}{modifier}", if modifier >= 0 { "+" } else { "" }),
                            "to": player_id,
                            "toName": player_name,
                            "by": actor.name
                        }));
                        push_roll_request(&mut room, request);
                        need_requests = true;
                    }
                }
            }
        }
        if need_requests {
            roll_broadcast = Some(roll_requests_for_emit(&room));
        }

        if outcome.sheet.is_none() {
            if let Some((spell, dc)) = &outcome.token_concentration {
                let roll = rand_d20();
                let success = roll >= *dc;
                let name = visible_name(&room, &outcome.token);
                monster_feed = Some(roll_feed(
                    "",
                    "Стол",
                    ROLE_DM,
                    &format!("Концентрация «{spell}»"),
                    roll,
                    roll,
                    &format!(
                        "{name}: {} (СЛ {dc})",
                        if success {
                            "успех"
                        } else {
                            "провал, заклинание сбито"
                        }
                    ),
                ));
                if !success {
                    if let Some(index) = token_index(&room, &outcome.token_id) {
                        let updated = {
                            let token = &mut room.tokens[index];
                            if let Some(object) = token.as_object_mut() {
                                object.remove("concentration");
                            }
                            token.clone()
                        };
                        monster_update = Some(token_views_of(&room, &updated));
                    }
                }
            }
        }
    }

    emit_token_views(io, "tokenUpdated", &token_views).await;

    if let (Some(sheet), Some(player_id)) = (outcome.sheet.clone(), assigned.clone()) {
        let payload = json!({ "playerId": player_id, "sheet": sheet });
        let _ = io
            .to(player_id.clone())
            .emit("characterSheetData", &payload)
            .await;
        let _ = io
            .to("dm-sockets")
            .emit("characterSheetUpdated", &payload)
            .await;
    }

    if let Some(list) = roll_broadcast {
        emit_roll_requests(io, list).await;
    }

    if let Some(feed) = monster_feed {
        let _ = io.emit("diceRollResult", &feed).await;
    }
    if let Some(view) = monster_update {
        emit_token_view(io, "tokenUpdated", &view).await;
    }

    announce_token_hp(
        table,
        io,
        &outcome.token_id,
        outcome.before_hp,
        actor,
        outcome.damage_type,
        outcome.defense,
    );
}

/// Delayed, coalesced hp announcement in chat (only for player tokens).
fn announce_token_hp(
    table: &Arc<Table>,
    io: &SocketIo,
    token_id: &str,
    before: i64,
    actor: Actor,
    damage_type: String,
    defense: String,
) {
    {
        let room = table.room.lock().unwrap();
        let Some(token) = room
            .tokens
            .iter()
            .find(|token| token.get("id").and_then(Value::as_str) == Some(token_id))
        else {
            return;
        };
        if !truthy(token.get("assignedPlayerId")) || !room.is_token_visible_to_players(token) {
            return;
        }
    }

    let generation = {
        let mut notices = HP_NOTICES.lock().unwrap();
        let generation = notices.next_generation;
        notices.next_generation += 1;
        // A burst of quick edits keeps the first "before" and one final notice,
        // exactly like the reference's debounce map.
        let before = notices
            .pending
            .get(token_id)
            .map(|pending| pending.before)
            .unwrap_or(before);
        notices.pending.insert(
            token_id.to_string(),
            PendingHp {
                before,
                actor,
                damage_type,
                defense,
                generation,
            },
        );
        generation
    };

    let table = table.clone();
    let io = io.clone();
    let token_id = token_id.to_string();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(HP_NOTICE_DELAY_MS)).await;
        let pending = {
            let mut notices = HP_NOTICES.lock().unwrap();
            match notices.pending.get(&token_id) {
                Some(pending) if pending.generation == generation => {
                    notices.pending.remove(&token_id)
                }
                _ => None,
            }
        };
        let Some(pending) = pending else {
            return;
        };
        let notice = {
            let mut room = table.room.lock().unwrap();
            let Some(token) = room
                .tokens
                .iter()
                .find(|token| token.get("id").and_then(Value::as_str) == Some(token_id.as_str()))
                .cloned()
            else {
                return;
            };
            if !room.is_token_visible_to_players(&token) {
                return;
            }
            let after = number(token.get("hp")) as i64;
            let text = if after == pending.before {
                if pending.defense == "immune" && !pending.damage_type.is_empty() {
                    let name = viewer_name(&room, &token);
                    format!("{name}: иммунитет к урону ({})", pending.damage_type)
                } else {
                    return;
                }
            } else {
                let delta = after - pending.before;
                let name = viewer_name(&room, &token);
                let kind = format!(
                    "{} {}{}",
                    if delta < 0 {
                        "урон"
                    } else {
                        "лечение"
                    },
                    delta.abs(),
                    if pending.damage_type.is_empty() {
                        String::new()
                    } else {
                        format!(", {}", pending.damage_type)
                    }
                );
                let max_hp = token
                    .get("maxHp")
                    .map(|v| js_text(Some(v)))
                    .unwrap_or_default();
                format!("{name}: {after}/{max_hp} ({kind})")
            };
            let message = make_chat(
                &pending.actor.name,
                &pending.actor.player_id,
                &pending.actor.role,
                text,
                None,
            );
            push_chat(&mut room, message)
        };
        table.mark_dirty();
        let _ = io.emit("chatMessage", &notice).await;
    });
}

fn viewer_name(room: &RoomState, token: &Value) -> String {
    let name = room
        .token_for_viewer(token, false)
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        "Фишка".to_string()
    } else {
        name
    }
}

// ---------------------------------------------------------------------------
// Initiative
// ---------------------------------------------------------------------------

fn reset_movement(room: &mut RoomState) {
    for token in room.tokens.iter_mut() {
        token["movedCells"] = json!(0);
    }
}

fn add_initiative_entry(room: &mut RoomState, raw: &Value) -> Option<Value> {
    let kind = raw
        .get("kind")
        .and_then(Value::as_str)
        .filter(|kind| content::INITIATIVE_KINDS.contains(kind))
        .unwrap_or("other")
        .to_string();
    let ref_id = if kind == "other" {
        String::new()
    } else {
        sheet::sanitize_text(raw.get("refId").unwrap_or(&Value::Null), 64)
    };
    if (kind == "player" || kind == "token") && ref_id.is_empty() {
        return None;
    }
    let entries = room
        .initiative
        .get_mut("entries")
        .and_then(Value::as_array_mut)?;
    if entries.len() >= LIMITS.max_initiative_entries as usize {
        return None;
    }
    if !ref_id.is_empty()
        && entries
            .iter()
            .any(|entry| entry.get("refId").and_then(Value::as_str) == Some(ref_id.as_str()))
    {
        return None;
    }
    let fallback = if kind == "token" {
        "Существо"
    } else {
        "Участник"
    };
    let name = sheet::sanitize_text(
        raw.get("name").unwrap_or(&Value::Null),
        LIMITS.max_name_length,
    );
    let modifier = sheet::clamp_int(
        raw.get("modifier").unwrap_or(&Value::Null),
        content::INITIATIVE.min_modifier,
        content::INITIATIVE.max_modifier,
        0,
    );
    let entry = json!({
        "id": new_initiative_id(),
        "kind": kind,
        "refId": ref_id,
        "name": if name.is_empty() { fallback.to_string() } else { name },
        "modifier": modifier,
        "total": Value::Null
    });
    entries.push(entry.clone());
    Some(entry)
}

fn remove_initiative_entry(room: &mut RoomState, entry_id: &str) -> bool {
    let turn_matches = room.initiative.get("turnId").and_then(Value::as_str) == Some(entry_id);
    let Some(entries) = room
        .initiative
        .get_mut("entries")
        .and_then(Value::as_array_mut)
    else {
        return false;
    };
    let Some(index) = entries
        .iter()
        .position(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id))
    else {
        return false;
    };
    entries.remove(index);
    if turn_matches {
        let next = entries
            .get(index)
            .or_else(|| {
                index
                    .checked_sub(1)
                    .and_then(|previous| entries.get(previous))
            })
            .and_then(|entry| entry.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        room.initiative["turnId"] = next;
        reset_movement(room);
    }
    true
}

fn roll_initiative(room: &mut RoomState, only_id: Option<&str>, disadvantage: &HashSet<String>) {
    let Some(entries) = room
        .initiative
        .get_mut("entries")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut has_total = false;
    for entry in entries.iter_mut() {
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if let Some(only) = only_id {
            if id != only {
                continue;
            }
        }
        let mut roll = rand_d20();
        if disadvantage.contains(&id) {
            roll = roll.min(rand_d20());
        }
        let modifier = entry.get("modifier").and_then(Value::as_i64).unwrap_or(0);
        entry["roll"] = json!(roll);
        entry["total"] = json!(roll + modifier);
        has_total = true;
    }
    if !has_total {
        return;
    }

    let order: HashMap<String, usize> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                entry
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                index,
            )
        })
        .collect();
    entries.sort_by(|a, b| {
        let a_total = a
            .get("total")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NEG_INFINITY);
        let b_total = b
            .get("total")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NEG_INFINITY);
        let a_modifier = a.get("modifier").and_then(Value::as_f64).unwrap_or(0.0);
        let b_modifier = b.get("modifier").and_then(Value::as_f64).unwrap_or(0.0);
        let a_id = a.get("id").and_then(Value::as_str).unwrap_or("");
        let b_id = b.get("id").and_then(Value::as_str).unwrap_or("");
        b_total
            .partial_cmp(&a_total)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b_modifier
                    .partial_cmp(&a_modifier)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                order
                    .get(a_id)
                    .unwrap_or(&usize::MAX)
                    .cmp(order.get(b_id).unwrap_or(&usize::MAX))
            })
    });

    if only_id.is_none() {
        let first = entries
            .first()
            .and_then(|entry| entry.get("id"))
            .cloned()
            .unwrap_or(Value::Null);
        room.initiative["round"] = json!(1);
        room.initiative["turnId"] = first;
        reset_movement(room);
        room.pending_reactions.clear();
    }
}

fn set_initiative_turn(room: &mut RoomState, entry_id: &str) -> bool {
    let exists = room
        .initiative
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .any(|entry| entry.get("id").and_then(Value::as_str) == Some(entry_id))
        })
        .unwrap_or(false);
    if !exists {
        return false;
    }
    if room.initiative.get("turnId").and_then(Value::as_str) != Some(entry_id) {
        reset_movement(room);
    }
    room.initiative["turnId"] = json!(entry_id);
    true
}

fn step_initiative_turn(room: &mut RoomState, dir: f64) -> bool {
    let ids: Vec<String> = room
        .initiative
        .get("entries")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| {
                    entry
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        return false;
    }
    let step = if dir >= 0.0 { 1isize } else { -1isize };
    let current_id = room
        .initiative
        .get("turnId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let round = room
        .initiative
        .get("round")
        .and_then(Value::as_i64)
        .unwrap_or(1);
    let mut new_round = round;
    let current = ids.iter().position(|id| id == &current_id);
    let next_id = match current {
        None => ids[0].clone(),
        Some(index) => {
            let next = index as isize + step;
            if next >= ids.len() as isize {
                new_round = round + 1;
                ids[0].clone()
            } else if next < 0 {
                ids[0].clone()
            } else {
                ids[next as usize].clone()
            }
        }
    };
    let changed_turn = current_id != next_id;
    room.initiative["turnId"] = json!(next_id);
    if new_round != round {
        room.initiative["round"] = json!(new_round);
        room.pending_reactions.clear();
    }
    if changed_turn {
        reset_movement(room);
    }
    true
}

fn tick_conditions_for_current_turn(room: &mut RoomState) -> Vec<Value> {
    let turn_id = room
        .initiative
        .get("turnId")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if turn_id.is_empty() {
        return Vec::new();
    }
    let entry = room
        .initiative
        .get("entries")
        .and_then(Value::as_array)
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(turn_id.as_str()))
                .cloned()
        });
    let Some(entry) = entry else {
        return Vec::new();
    };
    let kind = entry.get("kind").and_then(Value::as_str).unwrap_or("");
    let ref_id = entry.get("refId").and_then(Value::as_str).unwrap_or("");
    let mut token_ids = HashSet::new();
    if kind == "token" {
        token_ids.insert(ref_id.to_string());
    } else if kind == "player" {
        for token in &room.tokens {
            if token.get("assignedPlayerId").and_then(Value::as_str) == Some(ref_id) {
                token_ids.insert(
                    token
                        .get("id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                );
            }
        }
    }
    if token_ids.is_empty() {
        return Vec::new();
    }

    let mut changed = Vec::new();
    for token in room.tokens.iter_mut() {
        let id = token.get("id").and_then(Value::as_str).unwrap_or("");
        if !token_ids.contains(id) {
            continue;
        }
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
        let mut duration_map = durations.as_object().cloned().unwrap_or_default();
        let unit_map = units.as_object().cloned().unwrap_or_default();
        let mut touched = false;
        for id in duration_map.keys().cloned().collect::<Vec<_>>() {
            if unit_map.get(&id).and_then(Value::as_str) == Some("hour") {
                continue;
            }
            let rounds = duration_map.get(&id).and_then(Value::as_i64).unwrap_or(0);
            let next = rounds - 1;
            if next <= 0 {
                duration_map.remove(&id);
            } else {
                duration_map.insert(id, json!(next));
            }
            touched = true;
        }
        if !touched {
            continue;
        }
        let durations_value = Value::Object(duration_map);
        let conditions_value = Value::Array(
            conditions
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|condition| {
                    condition
                        .as_str()
                        .map(|id| durations_value.get(id).and_then(Value::as_i64).unwrap_or(0) > 0)
                        .unwrap_or(false)
                })
                .collect(),
        );
        token["conditions"] = conditions_value.clone();
        token["conditionDurations"] = durations_value.clone();
        token["conditionDurationUnits"] =
            content::normalize_condition_units(&units, &conditions_value, &durations_value);
        changed.push(token.clone());
    }
    changed
}

fn tick_conditions_for_hours(room: &mut RoomState, hours: f64) -> Vec<Value> {
    let elapsed = hours.floor() as i64;
    if elapsed <= 0 {
        return Vec::new();
    }
    let mut changed = Vec::new();
    for token in room.tokens.iter_mut() {
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
        let mut duration_map = durations.as_object().cloned().unwrap_or_default();
        let unit_map = units.as_object().cloned().unwrap_or_default();
        let mut touched = false;
        for id in duration_map.keys().cloned().collect::<Vec<_>>() {
            if unit_map.get(&id).and_then(Value::as_str) != Some("hour") {
                continue;
            }
            let value = duration_map.get(&id).and_then(Value::as_i64).unwrap_or(0);
            let next = value - elapsed;
            if next <= 0 {
                duration_map.remove(&id);
            } else {
                duration_map.insert(id, json!(next));
            }
            touched = true;
        }
        if !touched {
            continue;
        }
        let durations_value = Value::Object(duration_map);
        let conditions_value = Value::Array(
            conditions
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|condition| {
                    condition
                        .as_str()
                        .map(|id| durations_value.get(id).and_then(Value::as_i64).unwrap_or(0) > 0)
                        .unwrap_or(false)
                })
                .collect(),
        );
        token["conditions"] = conditions_value.clone();
        token["conditionDurations"] = durations_value.clone();
        token["conditionDurationUnits"] =
            content::normalize_condition_units(&units, &conditions_value, &durations_value);
        changed.push(token.clone());
    }
    changed
}

// ---------------------------------------------------------------------------
// Time, rest, xp
// ---------------------------------------------------------------------------

fn add_hours(time: &Value, hours: i64) -> Value {
    let day = time.get("day").and_then(Value::as_i64).unwrap_or(1);
    let hour = time.get("hour").and_then(Value::as_i64).unwrap_or(0);
    let total = (day - 1) * 24 + hour + hours;
    json!({
        "day": total.div_euclid(24) + 1,
        "hour": total.rem_euclid(24)
    })
}

fn hours_between(before: &Value, after: &Value) -> f64 {
    let before_day = before.get("day").and_then(Value::as_i64).unwrap_or(0);
    let after_day = after.get("day").and_then(Value::as_i64).unwrap_or(0);
    let before_hour = before.get("hour").and_then(Value::as_i64).unwrap_or(0);
    let after_hour = after.get("hour").and_then(Value::as_i64).unwrap_or(0);
    ((after_day - before_day) * 24 + (after_hour - before_hour)) as f64
}

/// `room.longRestParty`.
fn long_rest_party(room: &mut RoomState) -> Vec<String> {
    let mut rested = Vec::new();
    for (player_id, sheet) in room.sheets.iter_mut() {
        let pool = rules::hit_dice_pool(sheet);
        let total: i64 = pool
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| item.get("count").and_then(Value::as_i64).unwrap_or(0))
                    .sum()
            })
            .unwrap_or(0);
        let spent = sheet::clamp_int(
            sheet.get("hitDiceSpent").unwrap_or(&Value::Null),
            0,
            total,
            0,
        );
        let restore = spent.min((total / 2).max(1));
        let max_hp = rules::effective_max_hp(sheet) as i64;
        let exhaustion = number(sheet.get("exhaustion")).max(0.0) as i64;
        sheet["hp"] = json!(max_hp);
        sheet["tempHp"] = json!(0);
        sheet["deathSavesSuccesses"] = json!(0);
        sheet["deathSavesFailures"] = json!(0);
        sheet["spellSlotsUsed"] = json!([0, 0, 0, 0, 0, 0, 0, 0, 0]);
        sheet["hitDiceSpent"] = json!((spent - restore).max(0));
        sheet["concentration"] = json!("");
        sheet["exhaustion"] = json!((exhaustion - 1).max(0));
        rested.push(player_id.clone());
    }
    rested
}

/// `room.applyAutoLight`: returns the map types whose darkness changed.
fn apply_auto_light(room: &mut RoomState) -> Vec<String> {
    let darkness = rules::darkness_for_hour(room.game_time.get("hour").unwrap_or(&Value::Null));
    let mut changed: Vec<String> = Vec::new();
    if let Some(maps) = room.maps.as_object_mut() {
        for kind in MAP_TYPES {
            let Some(slot) = maps.get_mut(kind) else {
                continue;
            };
            let Some(items) = slot.get_mut("items").and_then(Value::as_array_mut) else {
                continue;
            };
            let mut touched = false;
            for item in items.iter_mut() {
                if !truthy(item.get("autoLight")) {
                    continue;
                }
                let current = number(item.get("darkness"));
                let current = if current.is_finite() { current } else { 0.0 };
                if (current - darkness).abs() < 0.001 {
                    continue;
                }
                item["darkness"] = json!(darkness);
                touched = true;
            }
            if touched && !changed.iter().any(|value| value == kind) {
                changed.push(kind.to_string());
            }
        }
    }
    changed
}

fn map_update_payloads(room: &RoomState, kind: &str) -> (Value, Value) {
    let slot = room.maps.get(kind);
    let items = slot
        .and_then(|slot| slot.get("items"))
        .cloned()
        .unwrap_or_else(|| json!([]));
    let active_id = slot
        .and_then(|slot| slot.get("activeId"))
        .cloned()
        .unwrap_or(Value::Null);
    let dm = json!({
        "type": kind,
        "items": items,
        "activeId": active_id,
        "activeMap": room.active_map,
        "activeMapId": room.active_map_id
    });
    let visible = room.map_slot_for_viewer(kind, false);
    let player = json!({
        "type": kind,
        "items": visible.get("items").cloned().unwrap_or_else(|| json!([])),
        "activeId": visible.get("activeId").cloned().unwrap_or(Value::Null),
        "activeMap": if room.active_map_id.is_some() { room.active_map.clone() } else { None },
        "activeMapId": room.active_map_id
    });
    (dm, player)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn handle_set_token_hp(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };
    let token_id = js_text(data.get("tokenId"));

    let (token, before, sheet) = {
        let mut room = table.room.lock().unwrap();
        let Some(index) = token_index(&room, &token_id) else {
            drop(room);
            emit_error(&socket, "Фишка не найдена");
            return;
        };
        let snapshot = room.tokens[index].clone();
        let assigned = snapshot
            .get("assignedPlayerId")
            .and_then(Value::as_str)
            .map(str::to_string);
        let has_sheet = assigned
            .as_ref()
            .map(|player_id| room.sheets.contains_key(player_id))
            .unwrap_or(false);
        if snapshot.get("maxHp").map(Value::is_null).unwrap_or(true) && !has_sheet {
            drop(room);
            emit_error(
                &socket,
                &format!("У фишки «{}» нет прочности", token_name(&snapshot)),
            );
            return;
        }
        let before = if has_sheet {
            assigned
                .as_ref()
                .and_then(|player_id| room.sheets.get(player_id))
                .map(|sheet| number(sheet.get("hp")) as i64)
                .unwrap_or(0)
        } else {
            number(snapshot.get("hp")) as i64
        };
        let (token, sheet) = set_token_hp_value(&mut room, index, data.get("hp"));
        (token, before, sheet)
    };

    table.mark_dirty();
    let view = {
        let room = table.room.lock().unwrap();
        token_views_of(&room, &token)
    };
    emit_token_view(&io, "tokenUpdated", &view).await;

    if let Some(sheet) = &sheet {
        let player_id = token
            .get("assignedPlayerId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let payload = json!({ "playerId": player_id, "sheet": sheet });
        let _ = io
            .to(player_id.clone())
            .emit("characterSheetData", &payload)
            .await;
        let _ = io
            .to("dm-sockets")
            .emit("characterSheetUpdated", &payload)
            .await;
    }

    if number(token.get("hp")) as i64 != before {
        announce_token_hp(
            &table,
            &io,
            &token_id,
            before,
            Actor::of(&session),
            String::new(),
            String::new(),
        );
    }
}

async fn handle_apply_token_damage(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };
    let token_id = js_text(data.get("tokenId"));
    let amount = sheet::clamp_int(data.get("amount").unwrap_or(&Value::Null), 0, 9999, 0);
    let (token_name, outcome) = {
        let mut room = table.room.lock().unwrap();
        let name = find_token(&room, &token_id).map(|token| token_name(&token));
        let outcome = if name.is_some() && amount != 0 {
            apply_token_damage(
                &mut room,
                &token_id,
                amount,
                data.get("type").unwrap_or(&Value::Null),
                truthy(data.get("healing")),
            )
        } else {
            None
        };
        (name, outcome)
    };
    let Some(token_name) = token_name else {
        emit_error(&socket, "Фишка не найдена");
        return;
    };
    if amount == 0 {
        emit_error(&socket, "Укажите количество урона или лечения");
        return;
    }
    let Some(outcome) = outcome else {
        emit_error(&socket, &format!("У фишки «{token_name}» нет прочности"));
        return;
    };
    finish_damage(&io, &table, outcome, Actor::of(&session)).await;
}

struct AttackRoll {
    d20: i64,
    hit: bool,
    total: i64,
    verdict: String,
}

fn attack_roll(bonus: i64, ac: i64, parsed: &ParsedFormula) -> AttackRoll {
    let d20 = rand_d20();
    let crit = d20 == 20;
    let hit = d20 != 1 && (crit || d20 + bonus >= ac);
    let mut total = 0;
    if hit {
        total = roll_formula(parsed).result.unwrap_or(0);
        if crit {
            total += roll_formula(parsed).result.unwrap_or(0);
        }
    }
    let verdict = if hit {
        if crit {
            "критическое попадание"
        } else {
            "попадание"
        }
    } else if d20 == 1 {
        "автопровал (1)"
    } else {
        "промах"
    };
    AttackRoll {
        d20,
        hit,
        total,
        verdict: verdict.to_string(),
    }
}

async fn run_attack(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    session: Session,
    data: Value,
    from_sheet: bool,
) {
    if from_sheet {
        let has_sheet = {
            let room = table.room.lock().unwrap();
            room.sheets.contains_key(&session.player_id)
        };
        if !has_sheet {
            emit_error(&socket, "Лист ещё не готов");
            return;
        }
    }
    let token_id = js_text(data.get("tokenId"));

    let target = {
        let room = table.room.lock().unwrap();
        find_token(&room, &token_id).map(|token| {
            let ac = token_ac(&room, &token);
            let name = visible_name(&room, &token);
            (token, ac, name)
        })
    };
    let Some((token, ac, target_name)) = target else {
        emit_error(&socket, "Фишка не найдена");
        return;
    };
    if ac < 1 {
        emit_error(&socket, "КД цели неизвестен");
        return;
    }

    let bonus = sheet::clamp_int(data.get("bonus").unwrap_or(&Value::Null), -20, 30, 0);
    let parsed = match dice::parse_formula(&js_text(data.get("damage"))) {
        Ok(parsed) => parsed,
        Err(error) => {
            emit_error(&socket, &error.to_string());
            return;
        }
    };
    let attack_name = {
        let text = sheet::sanitize_text(data.get("name").unwrap_or(&Value::Null), 60)
            .trim()
            .to_string();
        if text.is_empty() {
            "Атака".to_string()
        } else {
            text
        }
    };

    let roll = attack_roll(bonus, ac, &parsed);
    let note = format!(
        "{} по {}{}",
        roll.verdict,
        target_name,
        if roll.hit {
            format!(", урон {}", roll.total)
        } else {
            String::new()
        }
    );
    let feed = roll_feed(
        &session.player_id,
        &session.name,
        &session.role,
        &attack_name,
        roll.d20,
        roll.d20,
        &note,
    );
    let _ = io.emit("diceRollResult", &feed).await;

    if !roll.hit || roll.total == 0 {
        return;
    }

    let outcome = {
        let mut room = table.room.lock().unwrap();
        apply_token_damage(&mut room, &token_id, roll.total, &Value::Null, false)
    };
    let Some(outcome) = outcome else {
        emit_error(
            &socket,
            &format!("У фишки «{}» нет прочности", token_name(&token)),
        );
        return;
    };
    finish_damage(&io, &table, outcome, Actor::of(&session)).await;
}

async fn handle_cast_aoe(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };

    let (map_type, item) = {
        let room = table.room.lock().unwrap();
        find_map_item(&room, &data)
    };
    let Some(item) = item else {
        emit_error(&socket, "Карта не найдена");
        return;
    };
    if item.get("templates").and_then(Value::as_array).is_none() {
        emit_error(&socket, "Карта не найдена");
        return;
    }
    let template_id = js_text(data.get("templateId"));
    let template = item
        .get("templates")
        .and_then(Value::as_array)
        .and_then(|templates| {
            templates.iter().find(|template| {
                template.get("id").and_then(Value::as_str) == Some(template_id.as_str())
            })
        });
    if template.is_none() {
        emit_error(&socket, "Шаблон не найден");
        return;
    }
    let map_id = item
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let save = data
        .get("save")
        .and_then(Value::as_str)
        .filter(|save| SAVE_LABELS.iter().any(|(key, _)| key == save))
        .unwrap_or("")
        .to_string();
    if save.is_empty() {
        emit_error(&socket, "Укажите характеристику спасброска");
        return;
    }
    let dc = sheet::clamp_int(data.get("dc").unwrap_or(&Value::Null), 1, 40, 0);
    if dc == 0 {
        emit_error(&socket, "Сложность спасброска от 1 до 40");
        return;
    }
    let parsed = match dice::parse_formula(&js_text(data.get("damage"))) {
        Ok(parsed) => parsed,
        Err(error) => {
            emit_error(&socket, &error.to_string());
            return;
        }
    };
    let damage_type = sheet::sanitize_text(data.get("damageType").unwrap_or(&Value::Null), 40)
        .trim()
        .to_lowercase();
    let half_on_save = truthy(data.get("halfOnSave"));
    let spell_name = sheet::sanitize_text(data.get("spellName").unwrap_or(&Value::Null), 80)
        .trim()
        .to_string();

    let id_list: Vec<Value> = data
        .get("tokenIds")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let targets = {
        let room = table.room.lock().unwrap();
        let mut targets = Vec::new();
        let mut seen = HashSet::new();
        let mut wanted = 0usize;
        for value in &id_list {
            let id = js_text(Some(value));
            let Some(token) = find_token(&room, &id) else {
                continue;
            };
            let token_id = token
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if !seen.insert(token_id) {
                continue;
            }
            wanted += 1;
            if token.get("mapType").and_then(Value::as_str) != Some(map_type.as_str()) {
                continue;
            }
            let token_map_id = token.get("mapId").and_then(Value::as_str).unwrap_or("");
            if !token_map_id.is_empty() && token_map_id != map_id {
                continue;
            }
            if number(token.get("maxHp")) <= 0.0 {
                continue;
            }
            targets.push(token);
        }
        (targets, wanted)
    };
    let (targets, wanted) = targets;
    if wanted == 0 {
        emit_error(&socket, "В шаблоне нет фишек для урона");
        return;
    }
    if targets.is_empty() {
        emit_error(
            &socket,
            "Накрытые фишки теперь на другой карте или без прочности",
        );
        return;
    }

    let parsed_damage = js_text(data.get("damage")).trim().to_string();
    let total = roll_formula(&parsed).result.unwrap_or(0).max(0);
    let save_label = SAVE_LABELS
        .iter()
        .find(|(key, _)| *key == save)
        .map(|(_, label)| *label)
        .unwrap_or("");
    let ask_label = format!(
        "{}{} {} или {}({})",
        if spell_name.is_empty() {
            String::new()
        } else {
            format!("{spell_name}: ")
        },
        save_label,
        dc,
        parsed_damage,
        if damage_type.is_empty() {
            "урона"
        } else {
            &damage_type
        }
    );

    table.mark_dirty();

    let roll_requests = {
        let mut room = table.room.lock().unwrap();
        let mut player_ids = HashSet::new();
        for token in &targets {
            let Some(player_id) = token.get("assignedPlayerId").and_then(Value::as_str) else {
                continue;
            };
            if player_id.is_empty() || !player_ids.insert(player_id.to_string()) {
                continue;
            }
            let player_name = room
                .players
                .get(player_id)
                .and_then(|player| player.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let modifier = room
                .sheets
                .get(player_id)
                .map(|sheet| {
                    rules::compute_derived(sheet)
                        .get("saves")
                        .and_then(|saves| saves.get(&save))
                        .and_then(|save| save.get("total"))
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            let request = content::normalize_roll_request(&json!({
                "label": ask_label,
                "dc": dc,
                "formula": if modifier == 0 {
                    "1d20".to_string()
                } else {
                    format!("1d20{}{modifier}", if modifier >= 0 { "+" } else { "" })
                },
                "to": player_id,
                "toName": player_name,
                "by": session.name
            }));
            push_roll_request(&mut room, request);
        }
        if player_ids.is_empty() {
            None
        } else {
            Some(roll_requests_for_emit(&room))
        }
    };
    if let Some(list) = roll_requests {
        emit_roll_requests(&io, list).await;
    }

    for token in targets {
        if token
            .get("assignedPlayerId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .is_some()
        {
            continue;
        }
        let token_id = token
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let roll = rand_d20();
        let success = roll >= dc;
        let amount = if half_on_save && success {
            total / 2
        } else {
            total
        };
        let applied = {
            let mut room = table.room.lock().unwrap();
            apply_token_damage(
                &mut room,
                &token_id,
                amount,
                &Value::String(damage_type.clone()),
                false,
            )
            .map(|outcome| {
                let token = outcome.token.clone();
                let visible =
                    room.is_token_visible_to_players(&token) && !truthy(token.get("hideName"));
                (outcome, token_views_of(&room, &token), visible)
            })
        };
        let Some((outcome, view, visible)) = applied else {
            continue;
        };
        emit_token_view(&io, "tokenUpdated", &view).await;
        let name = if visible {
            format!("«{}»", token_name(&token))
        } else {
            "скрытый монстр".to_string()
        };
        let note = format!(
            "{name}: {} (СЛ {dc}){}",
            if success {
                "успех"
            } else {
                "провал"
            },
            if outcome.damage != 0 {
                format!(", урон {}", outcome.damage)
            } else {
                ", урон без эффекта".to_string()
            }
        );
        let formula = format!(
            "{}: {}",
            if spell_name.is_empty() {
                "Область эффекта"
            } else {
                &spell_name
            },
            save_label
        );
        let feed = roll_feed("", "Стол", ROLE_DM, &formula, roll, roll, &note);
        let _ = io.emit("diceRollResult", &feed).await;
    }
}

fn find_map_item(room: &RoomState, data: &Value) -> (String, Option<Value>) {
    let map_type = data
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let slot = room.maps.get(&map_type);
    let item = slot
        .and_then(|slot| slot.get("items"))
        .and_then(Value::as_array)
        .filter(|items| !items.is_empty())
        .and_then(|items| {
            let wanted = js_text(data.get("mapId"));
            if !wanted.is_empty() {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(wanted.as_str()))
            } else {
                let active = slot
                    .and_then(|slot| slot.get("activeId"))
                    .and_then(Value::as_str);
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == active)
                    .or_else(|| items.first())
            }
        })
        .cloned();
    (map_type, item)
}

async fn handle_request_stabilize(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };
    let token_id = js_text(data.get("tokenId"));
    let requests = {
        let mut room = table.room.lock().unwrap();
        let Some(token) = find_token(&room, &token_id) else {
            drop(room);
            emit_error(&socket, "Фишка не найдена");
            return;
        };
        let Some(player_id) = token
            .get("assignedPlayerId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
        else {
            drop(room);
            emit_error(&socket, "Стабилизируют только фишки героев");
            return;
        };
        let player_id = player_id.to_string();
        let Some(sheet) = room.sheets.get(&player_id).cloned() else {
            drop(room);
            emit_error(&socket, "Лист героя ещё не готов");
            return;
        };
        let modifier = rules::compute_derived(&sheet)
            .get("skills")
            .and_then(|skills| skills.get("medicine"))
            .and_then(|skill| skill.get("total"))
            .and_then(Value::as_i64)
            .unwrap_or(0);
        let player_name = room
            .players
            .get(&player_id)
            .and_then(|player| player.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let request = content::normalize_roll_request(&json!({
            "label": "Стабилизация: Медицина (СЛ 10)",
            "dc": 10,
            "formula": if modifier == 0 {
                "1d20".to_string()
            } else {
                format!("1d20{}{modifier}", if modifier >= 0 { "+" } else { "" })
            },
            "to": player_id,
            "toName": player_name,
            "by": session.name
        }));
        push_roll_request(&mut room, request);
        table.mark_dirty();
        Some(roll_requests_for_emit(&room))
    };
    if let Some(list) = requests {
        emit_roll_requests(&io, list).await;
    }
}

async fn handle_initiative_add(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let added = {
        let mut room = table.room.lock().unwrap();
        add_initiative_entry(&mut room, &data)
    };
    if added.is_none() {
        emit_error(&socket, "Такой участник боя уже есть в списке");
        return;
    }
    table.mark_dirty();
    emit_initiative(&io, &table).await;
}

async fn handle_initiative_remove(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let entry_id = js_text(data.get("id"));
    let removed = {
        let mut room = table.room.lock().unwrap();
        remove_initiative_entry(&mut room, &entry_id)
    };
    if !removed {
        emit_error(&socket, "Участник боя не найден");
        return;
    }
    table.mark_dirty();
    emit_initiative(&io, &table).await;
}

async fn handle_initiative_roll(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let only_id = data
        .get("id")
        .filter(|value| truthy(Some(value)))
        .map(|value| js_text(Some(value)));
    let mut combat_update = None;
    let result = {
        let mut room = table.room.lock().unwrap();
        if room
            .initiative
            .get("entries")
            .and_then(Value::as_array)
            .map(|entries| entries.is_empty())
            .unwrap_or(true)
        {
            Err("Список боя пуст")
        } else {
            let mut disadvantage = HashSet::new();
            if let Some(entries) = room.initiative.get("entries").and_then(Value::as_array) {
                for entry in entries {
                    if entry.get("kind").and_then(Value::as_str) != Some("player") {
                        continue;
                    }
                    let ref_id = entry.get("refId").and_then(Value::as_str).unwrap_or("");
                    let Some(sheet) = room.sheets.get(ref_id) else {
                        continue;
                    };
                    let disabled =
                        rules::exhaustion_effects(sheet.get("exhaustion").unwrap_or(&Value::Null))
                            .get("checkDisadvantage")
                            .and_then(Value::as_bool)
                            .unwrap_or(false);
                    if disabled {
                        if let Some(id) = entry.get("id").and_then(Value::as_str) {
                            disadvantage.insert(id.to_string());
                        }
                    }
                }
            }
            match &only_id {
                Some(entry_id) => {
                    let exists = room
                        .initiative
                        .get("entries")
                        .and_then(Value::as_array)
                        .map(|entries| {
                            entries.iter().any(|entry| {
                                entry.get("id").and_then(Value::as_str) == Some(entry_id.as_str())
                            })
                        })
                        .unwrap_or(false);
                    if !exists {
                        Err("Участник боя не найден")
                    } else {
                        if let Some(modifier) = data.get("modifier") {
                            let number = number(Some(modifier));
                            if number.is_finite() {
                                let modifier = js_round(number).clamp(
                                    content::INITIATIVE.min_modifier as f64,
                                    content::INITIATIVE.max_modifier as f64,
                                ) as i64;
                                if let Some(entries) = room
                                    .initiative
                                    .get_mut("entries")
                                    .and_then(Value::as_array_mut)
                                {
                                    if let Some(entry) = entries.iter_mut().find(|entry| {
                                        entry.get("id").and_then(Value::as_str)
                                            == Some(entry_id.as_str())
                                    }) {
                                        entry["modifier"] = json!(modifier);
                                    }
                                }
                            }
                        }
                        roll_initiative(&mut room, Some(entry_id), &disadvantage);
                        Ok(())
                    }
                }
                None => {
                    roll_initiative(&mut room, None, &disadvantage);
                    let enabled = room
                        .combat
                        .get("enabled")
                        .and_then(Value::as_bool)
                        .unwrap_or(false);
                    if !enabled {
                        room.combat["enabled"] = json!(true);
                        combat_update = Some(room.combat.clone());
                    }
                    Ok(())
                }
            }
        }
    };
    if let Err(message) = result {
        emit_error(&socket, message);
        return;
    }
    table.mark_dirty();
    if let Some(combat) = combat_update {
        let _ = io.emit("combatMode", &combat).await;
    }
    emit_initiative(&io, &table).await;
}

async fn handle_initiative_set_total(
    socket: SocketRef,
    io: SocketIo,
    table: Arc<Table>,
    data: Value,
) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let entry_id = js_text(data.get("id"));
    {
        let mut room = table.room.lock().unwrap();
        let Some(index) = room
            .initiative
            .get("entries")
            .and_then(Value::as_array)
            .and_then(|entries| {
                entries.iter().position(|entry| {
                    entry.get("id").and_then(Value::as_str) == Some(entry_id.as_str())
                })
            })
        else {
            drop(room);
            emit_error(&socket, "Участник боя не найден");
            return;
        };
        let Some(raw_total) = data.get("total") else {
            drop(room);
            emit_error(&socket, "Итог должен быть числом");
            return;
        };
        let total = number(Some(raw_total));
        if !total.is_finite() {
            drop(room);
            emit_error(&socket, "Итог должен быть числом");
            return;
        }
        room.initiative["entries"][index]["total"] = json!(js_round(total) as i64);
    }
    table.mark_dirty();
    emit_initiative(&io, &table).await;
}

async fn handle_initiative_turn(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let entry_id = js_text(data.get("id"));
    let changed = {
        let mut room = table.room.lock().unwrap();
        let before = room
            .initiative
            .get("turnId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if !set_initiative_turn(&mut room, &entry_id) {
            None
        } else {
            let after = room
                .initiative
                .get("turnId")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if before != after {
                Some(
                    tick_conditions_for_current_turn(&mut room)
                        .iter()
                        .map(|token| token_views_of(&room, token))
                        .collect::<Vec<_>>(),
                )
            } else {
                Some(Vec::new())
            }
        }
    };
    let Some(views) = changed else {
        emit_error(&socket, "Участник боя не найден");
        return;
    };
    table.mark_dirty();
    emit_token_views(&io, "tokenUpdated", &views).await;
    emit_initiative(&io, &table).await;
}

async fn handle_initiative_step(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let dir = match data.get("dir") {
        Some(value) => number(Some(value)),
        None => f64::NAN,
    };
    let ticked = {
        let mut room = table.room.lock().unwrap();
        if !step_initiative_turn(&mut room, dir) {
            None
        } else {
            Some(
                tick_conditions_for_current_turn(&mut room)
                    .iter()
                    .map(|token| token_views_of(&room, token))
                    .collect::<Vec<_>>(),
            )
        }
    };
    let Some(views) = ticked else {
        emit_error(&socket, "Список боя пуст");
        return;
    };
    table.mark_dirty();
    emit_token_views(&io, "tokenUpdated", &views).await;
    emit_initiative(&io, &table).await;
}

async fn handle_initiative_reset(socket: SocketRef, io: SocketIo, table: Arc<Table>, _data: Value) {
    let Some(_session) = require_dm(&socket, &table) else {
        return;
    };
    let combat_update = {
        let mut room = table.room.lock().unwrap();
        room.initiative = content::empty_initiative();
        reset_movement(&mut room);
        room.pending_reactions.clear();
        let enabled = room
            .combat
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if enabled {
            room.combat["enabled"] = json!(false);
            Some(room.combat.clone())
        } else {
            None
        }
    };
    table.mark_dirty();
    if let Some(combat) = combat_update {
        let _ = io.emit("combatMode", &combat).await;
    }
    emit_initiative(&io, &table).await;
}

async fn handle_set_combat_mode(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };
    let enabled = truthy(data.get("enabled"));
    let combat = {
        let mut room = table.room.lock().unwrap();
        let current = room
            .combat
            .get("enabled")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if current == enabled {
            return_combat(&socket, &room);
            return;
        }
        room.combat["enabled"] = json!(enabled);
        room.combat.clone()
    };
    table.mark_dirty();
    let _ = io.emit("combatMode", &combat).await;
    let text = if enabled {
        "Режим боя включён: нажмите на фишку, потом на клетку — фишка пройдёт по клеткам"
    } else {
        "Режим боя выключен: фишки снова можно перетаскивать"
    };
    let notice = {
        let mut room = table.room.lock().unwrap();
        let message = make_chat(
            &session.name,
            &session.player_id,
            &session.role,
            text.to_string(),
            None,
        );
        push_chat(&mut room, message)
    };
    table.mark_dirty();
    let _ = io.emit("chatMessage", &notice).await;
}

fn return_combat(socket: &SocketRef, room: &RoomState) {
    let _ = socket.emit("combatMode", &room.combat);
}

async fn handle_party_long_rest(socket: SocketRef, io: SocketIo, table: Arc<Table>, _data: Value) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };
    let (map_updates, sheet_updates, token_updates, game_time, expired) = {
        let mut room = table.room.lock().unwrap();
        let rested = long_rest_party(&mut room);
        let before = room.game_time.clone();
        let next = add_hours(&before, 8);
        room.game_time = next.clone();
        let elapsed = hours_between(&before, &next);
        let expired = tick_conditions_for_hours(&mut room, elapsed);
        let changed = apply_auto_light(&mut room);
        let map_updates: Vec<(Value, Value)> = changed
            .iter()
            .map(|kind| map_update_payloads(&room, kind))
            .collect();

        let mut sheet_updates = Vec::new();
        let mut token_views = Vec::new();
        for player_id in &rested {
            if let Some(sheet) = room.sheets.get(player_id).cloned() {
                let tokens = sync_sheet_tokens(&mut room, player_id);
                for token in &tokens {
                    token_views.push(token_views_of(&room, token));
                }
                sheet_updates.push((player_id.clone(), sheet));
            }
        }
        let expired_views = expired
            .iter()
            .map(|token| token_views_of(&room, token))
            .collect::<Vec<_>>();
        (map_updates, sheet_updates, token_views, next, expired_views)
    };
    table.mark_dirty();

    for (dm, players) in map_updates {
        let _ = io.to("dm-sockets").emit("mapUpdated", &dm).await;
        let _ = io
            .within("table")
            .except("dm-sockets")
            .emit("mapUpdated", &players)
            .await;
    }
    for (player_id, sheet) in sheet_updates {
        let payload = json!({ "playerId": player_id, "sheet": sheet });
        let _ = io
            .to(player_id.clone())
            .emit("characterSheetData", &payload)
            .await;
        let _ = io
            .to("dm-sockets")
            .emit("characterSheetUpdated", &payload)
            .await;
    }
    emit_token_views(&io, "tokenUpdated", &token_updates).await;
    let _ = io.emit("gameTimeUpdated", &game_time).await;
    emit_token_views(&io, "tokenUpdated", &expired).await;

    let notice = {
        let mut room = table.room.lock().unwrap();
        let message = make_chat(
            &session.name,
            &session.player_id,
            &session.role,
            "Партия отдыхает 8 часов: здоровье, ячейки и кости хитов восстановлены".to_string(),
            None,
        );
        push_chat(&mut room, message)
    };
    table.mark_dirty();
    let _ = io.emit("chatMessage", &notice).await;
}

struct XpUpdate {
    player_id: String,
    sheet: Value,
    hint: Option<Value>,
}

async fn handle_award_xp(socket: SocketRef, io: SocketIo, table: Arc<Table>, data: Value) {
    let Some(session) = require_dm(&socket, &table) else {
        return;
    };
    let amount = sheet::clamp_int(data.get("amount").unwrap_or(&Value::Null), 0, 999_999, 0);
    if amount == 0 {
        emit_error(&socket, "Укажите сумму опыта");
        return;
    }
    let asked: Option<Vec<String>> = data
        .get("targets")
        .and_then(Value::as_array)
        .filter(|targets| !targets.is_empty())
        .map(|targets| targets.iter().map(|value| js_text(Some(value))).collect());

    let (updates, share, targets_len, avatar) = {
        let mut room = table.room.lock().unwrap();
        let targets: Vec<String> = match asked {
            Some(asked) => asked
                .into_iter()
                .filter(|player_id| {
                    room.sheets.contains_key(player_id) && room.players.contains_key(player_id)
                })
                .collect(),
            None => room
                .players
                .values()
                .filter(|player| {
                    player.get("role").and_then(Value::as_str) == Some(ROLE_PLAYER)
                        && player
                            .get("playerId")
                            .and_then(Value::as_str)
                            .map(|player_id| room.sheets.contains_key(player_id))
                            .unwrap_or(false)
                })
                .filter_map(|player| {
                    player
                        .get("playerId")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect(),
        };
        if targets.is_empty() {
            drop(room);
            emit_error(&socket, "Некому выдавать опыт: нет листов игроков");
            return;
        }
        let share = (amount / targets.len() as i64) as i64;
        if share == 0 {
            drop(room);
            emit_error(&socket, "Сумма мельче, чем один опыт на игроков");
            return;
        }
        let mut updates = Vec::new();
        for player_id in &targets {
            let Some(sheet) = room.sheets.get(player_id).cloned() else {
                continue;
            };
            let before = number(sheet.get("experience")).max(0.0) as i64;
            let experience = sheet::clamp_int(&json!(before + share), 0, 9_999_999, 0);
            let mut sheet = sheet;
            sheet["experience"] = json!(experience);
            let level = sheet.get("level").and_then(Value::as_i64).unwrap_or(1);
            let threshold = if level < 20 {
                rules::xp_to_next_level(&json!(level))
            } else {
                f64::INFINITY
            };
            let mut hint = None;
            if threshold.is_finite()
                && experience as f64 >= threshold
                && (before as f64) < threshold
            {
                let character_name = sheet
                    .get("characterName")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let player_name = room
                    .players
                    .get(player_id)
                    .and_then(|player| player.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = if character_name.is_empty() {
                    player_name
                } else {
                    character_name
                };
                let message = make_chat(
                    "Стол",
                    "",
                    ROLE_DM,
                    format!(
                        "{name}: опыт дорос до порога {}, можно поднять уровень",
                        threshold as i64
                    ),
                    Some(&Value::Null),
                );
                hint = Some(push_chat(&mut room, message));
            }
            room.sheets.insert(player_id.clone(), sheet.clone());
            updates.push(XpUpdate {
                player_id: player_id.clone(),
                sheet,
                hint,
            });
        }
        let avatar = room
            .players
            .get(&session.player_id)
            .and_then(|player| player.get("avatar"))
            .cloned()
            .filter(|avatar| truthy(Some(avatar)))
            .unwrap_or(Value::Null);
        (updates, share, targets.len(), avatar)
    };

    table.mark_dirty();
    for update in updates {
        let payload = json!({ "playerId": update.player_id, "sheet": update.sheet });
        let _ = io
            .to(update.player_id.clone())
            .emit("characterSheetData", &payload)
            .await;
        let _ = io
            .to("dm-sockets")
            .emit("characterSheetUpdated", &payload)
            .await;
        if let Some(hint) = update.hint {
            let _ = io.emit("chatMessage", &hint).await;
        }
    }

    let announcement = {
        let mut room = table.room.lock().unwrap();
        let message = make_chat(
            &session.name,
            &session.player_id,
            &session.role,
            format!("Выдан опыт: {share} каждому из {targets_len} персонажей"),
            Some(&avatar),
        );
        push_chat(&mut room, message)
    };
    table.mark_dirty();
    let _ = io.emit("chatMessage", &announcement).await;
}

async fn emit_initiative(io: &SocketIo, table: &Arc<Table>) {
    let payload = {
        let room = table.room.lock().unwrap();
        room.initiative.clone()
    };
    let _ = io.emit("initiativeUpdated", &payload).await;
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

pub fn register(socket: &SocketRef, table: Arc<Table>) {
    macro_rules! handler {
        ($event:literal, $run:ident) => {{
            let table = table.clone();
            socket.on(
                $event,
                move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                    let table = table.clone();
                    async move { $run(socket, io, table, data).await }
                },
            );
        }};
    }

    handler!("setTokenHp", handle_set_token_hp);
    handler!("applyTokenDamage", handle_apply_token_damage);
    handler!("castAoe", handle_cast_aoe);
    handler!("requestStabilize", handle_request_stabilize);
    handler!("initiativeAdd", handle_initiative_add);
    handler!("initiativeRemove", handle_initiative_remove);
    handler!("initiativeRoll", handle_initiative_roll);
    handler!("initiativeSetTotal", handle_initiative_set_total);
    handler!("initiativeTurn", handle_initiative_turn);
    handler!("initiativeStep", handle_initiative_step);
    handler!("initiativeReset", handle_initiative_reset);
    handler!("setCombatMode", handle_set_combat_mode);
    handler!("partyLongRest", handle_party_long_rest);
    handler!("awardXp", handle_award_xp);

    {
        let table = table.clone();
        socket.on(
            "attackToken",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_joined(&socket, &table) else {
                        return;
                    };
                    if session.role != ROLE_PLAYER {
                        emit_error(&socket, "Быстрая атака идёт от листа игрока");
                        return;
                    }
                    run_attack(socket, io, table, session, data, true).await;
                }
            },
        );
    }
    {
        let table = table.clone();
        socket.on(
            "monsterAttack",
            move |socket: SocketRef, io: SocketIo, Data::<Value>(data)| {
                let table = table.clone();
                async move {
                    let Some(session) = require_dm(&socket, &table) else {
                        return;
                    };
                    run_attack(socket, io, table, session, data, false).await;
                }
            },
        );
    }
}
