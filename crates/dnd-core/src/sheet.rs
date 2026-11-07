//! Character sheets, statblocks and imported adventures from `server/state.js`.
//!
//! These are the pure normalizers: each one takes a `serde_json::Value` and
//! returns a `Value`, exactly like the JS versions accept and return plain
//! objects.

use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// Numeric limits from `LIMITS` in `server/state.js`.
///
/// Only the constants; the table state and its collections stay in `rules`.
pub struct Limits {
    pub max_name_length: i64,
    pub max_chat_length: i64,
    pub max_sheet_text_length: i64,
    pub max_image_bytes: i64,
    pub max_avatar_bytes: i64,
    pub max_audio_bytes: i64,
    pub max_dice_history: i64,
    pub max_initiative_entries: i64,
    pub max_chat_history: i64,
    pub max_tokens: i64,
    pub max_token_hp: i64,
    pub max_token_emote_length: i64,
    pub max_death_saves: i64,
    pub max_token_conditions: i64,
    pub max_exhaustion: i64,
    pub max_inspiration: i64,
    pub max_concentration_length: i64,
    pub max_coins: i64,
    pub max_spell_slots_per_level: i64,
    pub max_dm_note: i64,
    pub max_journal: i64,
    pub max_journal_entries: i64,
    pub max_journal_title: i64,
    pub max_journal_text: i64,
    pub max_journal_tags: i64,
    pub max_journal_tag: i64,
    pub max_calendar_events: i64,
    pub max_calendar_title: i64,
    pub max_party_notes: i64,
    pub max_inventory_items: i64,
    pub max_inventory_name: i64,
    pub max_inventory_notes: i64,
    pub max_roll_requests: i64,
    pub max_sheet_attacks: i64,
    pub max_level: i64,
    pub min_ability: i64,
    pub max_ability: i64,
    pub max_maps_per_type: i64,
    pub max_scenes: i64,
    pub max_scene_tokens: i64,
    pub max_scene_name: i64,
    pub max_adventure_notes: i64,
    pub max_adventure_items: i64,
    pub max_adventure_tables: i64,
    pub max_adventure_table_rows: i64,
    pub max_sheet_spells: i64,
    pub max_adventure_actors: i64,
    pub max_adventure_spells: i64,
    pub max_map_pins: i64,
    pub max_adventure_title: i64,
    pub max_adventure_text: i64,
    pub max_adventure_image_ref: i64,
    pub max_avatar_ref: i64,
    pub max_soundboard: i64,
    pub max_audio_ref: i64,
}

pub const LIMITS: Limits = Limits {
    max_name_length: 40,
    max_chat_length: 1000,
    max_sheet_text_length: 4000,
    max_image_bytes: 10 * 1024 * 1024,
    max_avatar_bytes: 2 * 1024 * 1024,
    max_audio_bytes: 2 * 1024 * 1024,
    max_dice_history: 200,
    max_initiative_entries: 40,
    max_chat_history: 300,
    max_tokens: 200,
    max_token_hp: 9999,
    max_token_emote_length: 80,
    max_death_saves: 3,
    max_token_conditions: 16,
    max_exhaustion: 6,
    max_inspiration: 99,
    max_concentration_length: 120,
    max_coins: 999999,
    max_spell_slots_per_level: 9,
    max_dm_note: 4000,
    max_journal: 20000,
    max_journal_entries: 300,
    max_journal_title: 120,
    max_journal_text: 8000,
    max_journal_tags: 8,
    max_journal_tag: 32,
    max_calendar_events: 200,
    max_calendar_title: 120,
    max_party_notes: 20000,
    max_inventory_items: 500,
    max_inventory_name: 120,
    max_inventory_notes: 1000,
    max_roll_requests: 20,
    max_sheet_attacks: 20,
    max_level: 20,
    min_ability: 1,
    max_ability: 30,
    max_maps_per_type: 20,
    max_scenes: 20,
    max_scene_tokens: 200,
    max_scene_name: 60,
    max_adventure_notes: 300,
    max_adventure_items: 500,
    max_adventure_tables: 200,
    max_adventure_table_rows: 100,
    max_sheet_spells: 60,
    max_adventure_actors: 200,
    max_adventure_spells: 400,
    max_map_pins: 60,
    max_adventure_title: 120,
    max_adventure_text: 20000,
    max_adventure_image_ref: 2048,
    max_avatar_ref: 2048,
    max_soundboard: 40,
    max_audio_ref: 2048,
};

pub const ABILITY_KEYS: [&str; 6] = ["str", "dex", "con", "int", "wis", "cha"];

pub const ABILITY_LABELS: [(&str, &str); 6] = [
    ("str", "Сила"),
    ("dex", "Ловкость"),
    ("con", "Телосложение"),
    ("int", "Интеллект"),
    ("wis", "Мудрость"),
    ("cha", "Харизма"),
];

pub const COIN_KEYS: [&str; 5] = ["cp", "sp", "ep", "gp", "pp"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Skill {
    pub key: &'static str,
    pub label: &'static str,
    pub ability: &'static str,
}

/// 18 навыков D&D 5e 2014 и их базовая характеристика.
pub const SKILLS: [Skill; 18] = [
    Skill {
        key: "acrobatics",
        label: "Акробатика",
        ability: "dex",
    },
    Skill {
        key: "animalHandling",
        label: "Уход за животными",
        ability: "wis",
    },
    Skill {
        key: "arcana",
        label: "Магия",
        ability: "int",
    },
    Skill {
        key: "athletics",
        label: "Атлетика",
        ability: "str",
    },
    Skill {
        key: "deception",
        label: "Обман",
        ability: "cha",
    },
    Skill {
        key: "history",
        label: "История",
        ability: "int",
    },
    Skill {
        key: "insight",
        label: "Проницательность",
        ability: "wis",
    },
    Skill {
        key: "intimidation",
        label: "Запугивание",
        ability: "cha",
    },
    Skill {
        key: "investigation",
        label: "Анализ",
        ability: "int",
    },
    Skill {
        key: "medicine",
        label: "Медицина",
        ability: "wis",
    },
    Skill {
        key: "nature",
        label: "Природа",
        ability: "int",
    },
    Skill {
        key: "perception",
        label: "Внимательность",
        ability: "wis",
    },
    Skill {
        key: "performance",
        label: "Выступление",
        ability: "cha",
    },
    Skill {
        key: "persuasion",
        label: "Убеждение",
        ability: "cha",
    },
    Skill {
        key: "religion",
        label: "Религия",
        ability: "int",
    },
    Skill {
        key: "sleightOfHand",
        label: "Ловкость рук",
        ability: "dex",
    },
    Skill {
        key: "stealth",
        label: "Скрытность",
        ability: "dex",
    },
    Skill {
        key: "survival",
        label: "Выживание",
        ability: "wis",
    },
];

/// `SPELL_SLOT_LEVELS` из `renderer/dnd5e.js`: девять ступеней.
const SPELL_SLOT_LEVELS: usize = 9;

pub type ProficiencyMap = BTreeMap<String, bool>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Abilities {
    pub str: i64,
    pub dex: i64,
    pub con: i64,
    pub int: i64,
    pub wis: i64,
    pub cha: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct Coins {
    pub cp: i64,
    pub sp: i64,
    pub ep: i64,
    pub gp: i64,
    pub pp: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetClass {
    pub name: String,
    pub level: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attack {
    pub name: String,
    pub bonus: i64,
    pub damage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetSpell {
    pub id: String,
    pub name: String,
    pub level: i64,
    pub prepared: bool,
    pub note: String,
}

/// `createEmptySheet` returns this shape; the extra fields that
/// `normalizeSheet` always adds are `None` here so they stay out of the JSON.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Sheet {
    pub player_name: String,
    pub character_name: String,
    pub char_class: String,
    pub subclass: String,
    pub classes: Vec<SheetClass>,
    pub race: String,
    pub background: String,
    pub alignment: String,
    pub level: i64,
    pub experience: i64,
    pub proficiency_bonus_manual: Option<i64>,
    pub abilities: Abilities,
    pub save_proficiencies: ProficiencyMap,
    pub skill_proficiencies: ProficiencyMap,
    pub skill_expertise: ProficiencyMap,
    pub hp: i64,
    pub max_hp: i64,
    pub temp_hp: i64,
    pub hit_dice: String,
    pub hit_dice_spent: i64,
    pub inspiration: i64,
    pub exhaustion: i64,
    pub concentration: String,
    pub coins: Coins,
    pub armor_class: i64,
    pub initiative_misc: i64,
    pub speed: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub death_saves_successes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub death_saves_failures: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spell_slots_used: Option<[i64; SPELL_SLOT_LEVELS]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spell_slots_max: Option<[i64; SPELL_SLOT_LEVELS]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack_list: Option<Vec<Attack>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spells: Option<Vec<SheetSpell>>,
    pub attacks: String,
    pub equipment: String,
    pub features: String,
    pub notes: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Statblock {
    pub ac: i64,
    pub hp: i64,
    pub speed: String,
    pub size: i64,
    pub cr: String,
    pub abilities: Abilities,
    pub attacks: String,
    pub notes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageCrop {
    pub image_zoom: f64,
    pub image_x: f64,
    pub image_y: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdventureNote {
    pub id: String,
    pub title: String,
    pub text: String,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdventureItem {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub id: String,
    pub label: String,
    pub weight: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdventureTable {
    pub id: String,
    pub name: String,
    pub formula: String,
    pub description: String,
    pub rows: Vec<TableRow>,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdventureActor {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub image: String,
    pub image_zoom: f64,
    pub image_x: f64,
    pub image_y: f64,
    pub favorite: bool,
    pub statblock: Option<Statblock>,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdventureSpell {
    pub id: String,
    pub name: String,
    pub level: i64,
    pub school: String,
    pub text: String,
    pub source_id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Adventure {
    pub id: String,
    pub name: String,
    pub source: String,
    pub imported_at: String,
    pub notes: Vec<AdventureNote>,
    pub items: Vec<AdventureItem>,
    pub tables: Vec<AdventureTable>,
    pub actors: Vec<AdventureActor>,
    pub spells: Vec<AdventureSpell>,
}

// ---------------------------------------------------------------------------
// JS helpers
// ---------------------------------------------------------------------------

fn to_value<T: Serialize>(value: T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

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
        _ => None,
    }
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number
            .as_f64()
            .is_some_and(|num| num != 0.0 && !num.is_nan()),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

fn js_truthy_opt(value: Option<&Value>) -> bool {
    value.is_some_and(js_truthy)
}

/// `String(value)` for the scalar inputs a JSON form can carry.
fn js_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        // shortcut: JS String() gives "[object Object]" and joins arrays; such values are not user text here.
        _ => String::new(),
    }
}

/// `String(value || fallback)`.
fn js_string_or(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(value) if js_truthy(value) => js_string(value),
        _ => fallback.to_string(),
    }
}

fn field<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|map| map.get(key))
}

/// `container && container[key]`: a falsy container is returned as is.
fn js_and_member<'a>(container: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    match container? {
        Value::Object(map) => map.get(key),
        falsy @ (Value::Null | Value::Bool(false)) => Some(falsy),
        Value::Number(number) if number.as_f64() == Some(0.0) => Some(container?),
        _ => None,
    }
}

/// `Math.round`: halves go toward +Infinity, unlike `f64::round`.
fn js_round(number: f64) -> f64 {
    (number + 0.5).floor()
}

pub fn clamp_int(value: &Value, min: i64, max: i64, fallback: i64) -> i64 {
    match js_number(value) {
        Some(num) if num.is_finite() => js_round(num).clamp(min as f64, max as f64) as i64,
        _ => fallback,
    }
}

fn clamp_int_or(value: Option<&Value>, min: i64, max: i64, fallback: i64) -> i64 {
    match value {
        Some(value) => clamp_int(value, min, max, fallback),
        None => fallback,
    }
}

pub fn clamp_number(value: &Value, min: f64, max: f64, fallback: f64) -> f64 {
    match js_number(value) {
        Some(num) if num.is_finite() => num.clamp(min, max),
        _ => fallback,
    }
}

fn clamp_number_or(value: Option<&Value>, min: f64, max: f64, fallback: f64) -> f64 {
    match value {
        Some(value) => clamp_number(value, min, max, fallback),
        None => fallback,
    }
}

pub fn sanitize_text(value: &Value, max_length: i64) -> String {
    if matches!(value, Value::Null) {
        return String::new();
    }
    let text = js_string(value).replace('\u{0}', "");
    truncate_utf16(&text, max_length)
}

fn sanitize_text_opt(value: Option<&Value>, max_length: i64) -> String {
    match value {
        Some(value) => sanitize_text(value, max_length),
        None => String::new(),
    }
}

/// `String.prototype.slice(0, maxLength)` counts UTF-16 units, so cut there
/// rather than by chars.
fn truncate_utf16(text: &str, max_length: i64) -> String {
    if max_length < 0 {
        return String::new();
    }
    let limit = max_length as usize;
    if text.encode_utf16().count() <= limit {
        return text.to_string();
    }
    let mut units = 0;
    let mut out = String::new();
    for ch in text.chars() {
        let width = ch.len_utf16();
        if units + width > limit {
            break;
        }
        units += width;
        out.push(ch);
    }
    out
}

fn or_default(text: String, fallback: &str) -> String {
    if text.is_empty() {
        fallback.to_string()
    } else {
        text
    }
}

fn is_objectish(value: &Value) -> bool {
    matches!(value, Value::Object(_) | Value::Array(_))
}

fn empty_proficiency_map(keys: &[&str]) -> ProficiencyMap {
    keys.iter().map(|key| ((*key).to_string(), false)).collect()
}

fn skill_keys() -> Vec<&'static str> {
    SKILLS.iter().map(|skill| skill.key).collect()
}

pub fn empty_coins() -> Coins {
    Coins::default()
}

fn coins_from(input: &Value) -> Coins {
    Coins {
        cp: clamp_int_or(field(input, "cp"), 0, LIMITS.max_coins, 0),
        sp: clamp_int_or(field(input, "sp"), 0, LIMITS.max_coins, 0),
        ep: clamp_int_or(field(input, "ep"), 0, LIMITS.max_coins, 0),
        gp: clamp_int_or(field(input, "gp"), 0, LIMITS.max_coins, 0),
        pp: clamp_int_or(field(input, "pp"), 0, LIMITS.max_coins, 0),
    }
}

pub fn normalize_coins(input: &Value) -> Value {
    let source = if is_objectish(input) {
        input
    } else {
        &Value::Null
    };
    to_value(coins_from(source))
}

fn default_abilities() -> Abilities {
    Abilities {
        str: 10,
        dex: 10,
        con: 10,
        int: 10,
        wis: 10,
        cha: 10,
    }
}

fn abilities_from(input: Option<&Value>) -> Abilities {
    Abilities {
        str: clamp_int_or(
            field_opt(input, "str"),
            LIMITS.min_ability,
            LIMITS.max_ability,
            10,
        ),
        dex: clamp_int_or(
            field_opt(input, "dex"),
            LIMITS.min_ability,
            LIMITS.max_ability,
            10,
        ),
        con: clamp_int_or(
            field_opt(input, "con"),
            LIMITS.min_ability,
            LIMITS.max_ability,
            10,
        ),
        int: clamp_int_or(
            field_opt(input, "int"),
            LIMITS.min_ability,
            LIMITS.max_ability,
            10,
        ),
        wis: clamp_int_or(
            field_opt(input, "wis"),
            LIMITS.min_ability,
            LIMITS.max_ability,
            10,
        ),
        cha: clamp_int_or(
            field_opt(input, "cha"),
            LIMITS.min_ability,
            LIMITS.max_ability,
            10,
        ),
    }
}

fn field_opt<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    value.and_then(|value| field(value, key))
}

fn empty_sheet(player_name: &str) -> Sheet {
    Sheet {
        player_name: player_name.to_string(),
        character_name: String::new(),
        char_class: String::new(),
        subclass: String::new(),
        classes: Vec::new(),
        race: String::new(),
        background: String::new(),
        alignment: String::new(),
        level: 1,
        experience: 0,
        proficiency_bonus_manual: None,
        abilities: default_abilities(),
        save_proficiencies: empty_proficiency_map(&ABILITY_KEYS),
        skill_proficiencies: empty_proficiency_map(&skill_keys()),
        skill_expertise: empty_proficiency_map(&skill_keys()),
        hp: 10,
        max_hp: 10,
        temp_hp: 0,
        hit_dice: "1d8".to_string(),
        hit_dice_spent: 0,
        inspiration: 0,
        exhaustion: 0,
        concentration: String::new(),
        coins: empty_coins(),
        armor_class: 10,
        initiative_misc: 0,
        speed: 30,
        death_saves_successes: None,
        death_saves_failures: None,
        spell_slots_used: None,
        spell_slots_max: None,
        attack_list: None,
        spells: None,
        attacks: String::new(),
        equipment: String::new(),
        features: String::new(),
        notes: String::new(),
    }
}

pub fn create_empty_sheet(player_name: &str) -> Value {
    to_value(empty_sheet(player_name))
}

// ---------------------------------------------------------------------------
// Sheet parts
// ---------------------------------------------------------------------------

fn classes_from(value: &Value) -> Vec<SheetClass> {
    let entries = value.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut result = Vec::new();
    let mut sum = 0;
    for entry in entries {
        if result.len() >= 6 {
            break;
        }
        let name = sanitize_text_opt(field(entry, "name"), 60);
        if name.is_empty() {
            continue;
        }
        let room = LIMITS.max_level - sum;
        if room <= 0 {
            break;
        }
        let level = clamp_int_or(field(entry, "level"), 1, LIMITS.max_level, 1).min(room);
        result.push(SheetClass { name, level });
        sum += level;
    }
    result
}

pub fn normalize_sheet_classes(value: &Value) -> Value {
    to_value(classes_from(value))
}

fn attacks_from(value: &Value) -> Vec<Attack> {
    let entries = value.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut result = Vec::new();
    for entry in entries.iter().take(LIMITS.max_sheet_attacks as usize) {
        if !entry.is_object() {
            continue;
        }
        let name = sanitize_text_opt(field(entry, "name"), 60);
        let damage = sanitize_text_opt(field(entry, "damage"), 80);
        let bonus = clamp_int_or(field(entry, "bonus"), -20, 30, 0);
        if name.is_empty() && damage.is_empty() && bonus == 0 {
            continue;
        }
        result.push(Attack {
            name,
            bonus,
            damage,
        });
    }
    result
}

pub fn normalize_attack_list(value: &Value) -> Value {
    to_value(attacks_from(value))
}

fn spells_from(value: &Value) -> Vec<SheetSpell> {
    let entries = value.as_array().map(Vec::as_slice).unwrap_or(&[]);
    let mut result = Vec::new();
    for entry in entries.iter().take(LIMITS.max_sheet_spells as usize) {
        if !entry.is_object() {
            continue;
        }
        let name = sanitize_text_opt(field(entry, "name"), 60);
        let note = sanitize_text_opt(field(entry, "note"), 200);
        if name.is_empty() && note.is_empty() {
            continue;
        }
        let id = or_default(
            sanitize_text_opt(field(entry, "id"), 40),
            &format!("spell-{}", result.len()),
        );
        result.push(SheetSpell {
            id,
            name,
            level: clamp_int_or(field(entry, "level"), 0, 9, 0),
            prepared: js_truthy_opt(field(entry, "prepared")),
            note,
        });
    }
    result
}

pub fn normalize_sheet_spells(value: &Value) -> Value {
    to_value(spells_from(value))
}

fn clamp_slot_array(raw: Option<&Value>) -> [i64; SPELL_SLOT_LEVELS] {
    let rows = raw.and_then(Value::as_array);
    let mut slots = [0i64; SPELL_SLOT_LEVELS];
    for (index, slot) in slots.iter_mut().enumerate() {
        *slot = clamp_int_or(
            rows.and_then(|rows| rows.get(index)),
            0,
            LIMITS.max_spell_slots_per_level,
            0,
        );
    }
    slots
}

pub fn normalize_sheet(input: &Value, fallback_player_name: &str) -> Value {
    let mut sheet = empty_sheet(fallback_player_name);
    if !is_objectish(input) {
        return to_value(sheet);
    }

    let player_name = sanitize_text_opt(field(input, "playerName"), LIMITS.max_name_length);
    sheet.player_name = if player_name.is_empty() {
        fallback_player_name.to_string()
    } else {
        player_name
    };
    sheet.character_name = sanitize_text_opt(field(input, "characterName"), LIMITS.max_name_length);
    sheet.char_class = sanitize_text_opt(field(input, "charClass"), 60);
    sheet.subclass = sanitize_text_opt(field(input, "subclass"), 60);
    sheet.classes = classes_from(field(input, "classes").unwrap_or(&Value::Null));
    sheet.race = sanitize_text_opt(field(input, "race"), 60);
    sheet.background = sanitize_text_opt(field(input, "background"), 60);
    sheet.alignment = sanitize_text_opt(field(input, "alignment"), 60);
    sheet.level = clamp_int_or(field(input, "level"), 1, LIMITS.max_level, 1);
    sheet.experience = clamp_int_or(field(input, "experience"), 0, 9_999_999, 0);
    sheet.proficiency_bonus_manual = match field(input, "proficiencyBonusManual") {
        None | Some(Value::Null) => None,
        Some(value) => Some(clamp_int(value, 0, 12, 2)),
    };
    sheet.abilities = abilities_from(field(input, "abilities"));

    let saves = field(input, "saveProficiencies");
    for key in ABILITY_KEYS {
        sheet
            .save_proficiencies
            .insert(key.to_string(), js_truthy_opt(field_opt(saves, key)));
    }
    let proficiencies = field(input, "skillProficiencies");
    let expertise = field(input, "skillExpertise");
    for skill in SKILLS.iter() {
        let proficient = js_truthy_opt(field_opt(proficiencies, skill.key));
        sheet
            .skill_proficiencies
            .insert(skill.key.to_string(), proficient);
        let expert = js_truthy_opt(field_opt(expertise, skill.key));
        // Компетентность имеет смысл только вместе с владением.
        sheet
            .skill_expertise
            .insert(skill.key.to_string(), expert && proficient);
    }

    sheet.hp = clamp_int_or(field(input, "hp"), -999, 9999, 10);
    sheet.max_hp = clamp_int_or(field(input, "maxHp"), 0, 9999, 10);
    sheet.temp_hp = clamp_int_or(field(input, "tempHp"), 0, 9999, 0);
    sheet.death_saves_successes = Some(clamp_int_or(
        field(input, "deathSavesSuccesses"),
        0,
        LIMITS.max_death_saves,
        0,
    ));
    sheet.death_saves_failures = Some(clamp_int_or(
        field(input, "deathSavesFailures"),
        0,
        LIMITS.max_death_saves,
        0,
    ));
    sheet.spell_slots_used = Some(clamp_slot_array(field(input, "spellSlotsUsed")));
    sheet.spell_slots_max = Some(clamp_slot_array(field(input, "spellSlotsMax")));
    sheet.hit_dice = sanitize_text_opt(field(input, "hitDice"), 20);
    sheet.hit_dice_spent = clamp_int_or(field(input, "hitDiceSpent"), 0, LIMITS.max_level, 0);
    sheet.inspiration = clamp_int_or(field(input, "inspiration"), 0, LIMITS.max_inspiration, 0);
    sheet.exhaustion = clamp_int_or(field(input, "exhaustion"), 0, LIMITS.max_exhaustion, 0);
    sheet.concentration = sanitize_text_opt(
        field(input, "concentration"),
        LIMITS.max_concentration_length,
    )
    .trim()
    .to_string();
    sheet.coins = coins_from(field(input, "coins").unwrap_or(&Value::Null));
    sheet.armor_class = clamp_int_or(field(input, "armorClass"), 0, 99, 10);
    sheet.initiative_misc = clamp_int_or(field(input, "initiativeMisc"), -20, 20, 0);
    sheet.speed = clamp_int_or(field(input, "speed"), 0, 999, 30);

    sheet.attack_list = Some(attacks_from(
        field(input, "attackList").unwrap_or(&Value::Null),
    ));
    sheet.spells = Some(spells_from(field(input, "spells").unwrap_or(&Value::Null)));
    sheet.attacks = sanitize_text_opt(field(input, "attacks"), LIMITS.max_sheet_text_length);
    sheet.equipment = sanitize_text_opt(field(input, "equipment"), LIMITS.max_sheet_text_length);
    sheet.features = sanitize_text_opt(field(input, "features"), LIMITS.max_sheet_text_length);
    sheet.notes = sanitize_text_opt(field(input, "notes"), LIMITS.max_sheet_text_length);

    // Exhaustion lowers the hit point ceiling, and current hp must follow it:
    // otherwise the sheet keeps health above its own maximum.
    let mut sheet = to_value(sheet);
    let max_hp = crate::rules::effective_max_hp(&sheet);
    if let Some(hp) = sheet.get("hp").and_then(Value::as_f64) {
        if hp > max_hp {
            sheet["hp"] = serde_json::json!(max_hp as i64);
        }
    }
    sheet
}

// ---------------------------------------------------------------------------
// Statblocks and images
// ---------------------------------------------------------------------------

fn statblock_from(input: Option<&Value>) -> Option<Statblock> {
    let input = input?;
    if !is_objectish(input) {
        return None;
    }
    Some(Statblock {
        ac: clamp_int_or(field(input, "ac"), 0, 99, 0),
        hp: clamp_int_or(field(input, "hp"), 0, LIMITS.max_token_hp, 0),
        speed: sanitize_text_opt(field(input, "speed"), 60),
        size: clamp_int_or(field(input, "size"), 1, 4, 1),
        cr: sanitize_text_opt(field(input, "cr"), 20),
        abilities: abilities_from(field(input, "abilities")),
        attacks: sanitize_text_opt(field(input, "attacks"), LIMITS.max_sheet_text_length),
        notes: sanitize_text_opt(field(input, "notes"), LIMITS.max_sheet_text_length),
    })
}

pub fn normalize_statblock(input: &Value) -> Value {
    match statblock_from(Some(input)) {
        Some(statblock) => to_value(statblock),
        None => Value::Null,
    }
}

fn crop_from(input: Option<&Value>) -> ImageCrop {
    let source = input
        .filter(|value| is_objectish(value))
        .unwrap_or(&Value::Null);
    let zoom = clamp_number_or(field(source, "imageZoom"), 1.0, 4.0, 1.0);
    let limit = (zoom - 1.0) * 50.0;
    ImageCrop {
        image_zoom: zoom,
        image_x: clamp_number_or(field(source, "imageX"), -limit, limit, 0.0),
        image_y: clamp_number_or(field(source, "imageY"), -limit, limit, 0.0),
    }
}

pub fn normalize_image_crop(input: &Value) -> Value {
    to_value(crop_from(Some(input)))
}

/// `/files/<имя>` с именем из `[A-Za-z0-9_.-]{1,80}`.
fn is_files_ref(text: &str) -> bool {
    let Some(rest) = text.strip_prefix("/files/") else {
        return false;
    };
    let mut length = 0;
    for ch in rest.chars() {
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')) {
            return false;
        }
        length += 1;
    }
    (1..=80).contains(&length)
}

fn strip_prefix_ci<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    text.get(..prefix.len())
        .filter(|head| head.eq_ignore_ascii_case(prefix))
        .map(|_| &text[prefix.len()..])
}

/// `^https?:\/\/[^\s]+$` с регистронезависимой схемой.
fn is_http_url(text: &str) -> bool {
    let rest = strip_prefix_ci(text, "http://").or_else(|| strip_prefix_ci(text, "https://"));
    match rest {
        Some(rest) => !rest.is_empty() && !rest.chars().any(char::is_whitespace),
        None => false,
    }
}

pub fn normalize_adventure_image(value: &Value) -> String {
    let Value::String(_) = value else {
        return String::new();
    };
    let text = sanitize_text(value, LIMITS.max_adventure_image_ref)
        .trim()
        .to_string();
    if is_files_ref(&text) || is_http_url(&text) {
        text
    } else {
        String::new()
    }
}

// ---------------------------------------------------------------------------
// Adventures
// ---------------------------------------------------------------------------

fn list(value: Option<&Value>, max: i64) -> Vec<Value> {
    match value.and_then(Value::as_array) {
        Some(items) => items.iter().take(max as usize).cloned().collect(),
        None => Vec::new(),
    }
}

/// `Number.isFinite(Number(container && container[key])) ? clampInt(...) : undefined`.
fn clamped_member(container: Option<&Value>, key: &str) -> Option<i64> {
    let value = js_and_member(container, key)?;
    let finite = js_number(value).is_some_and(|number| number.is_finite());
    if !finite {
        return None;
    }
    Some(clamp_int(value, 1, 9999, 1))
}

fn adventure_from(input: &Value) -> Option<Adventure> {
    if !is_objectish(input) {
        return None;
    }

    let notes = list(field(input, "notes"), LIMITS.max_adventure_notes)
        .iter()
        .enumerate()
        .map(|(index, item)| AdventureNote {
            id: js_string_or(field(item, "id"), &format!("note-{index}")),
            title: or_default(
                sanitize_text_opt(field(item, "title"), LIMITS.max_adventure_title),
                "Заметка",
            ),
            text: sanitize_text_opt(field(item, "text"), LIMITS.max_adventure_text),
            source_id: js_string_or(field(item, "sourceId"), ""),
        })
        .collect();

    let items = list(field(input, "items"), LIMITS.max_adventure_items)
        .iter()
        .enumerate()
        .map(|(index, item)| AdventureItem {
            id: js_string_or(field(item, "id"), &format!("item-{index}")),
            name: or_default(
                sanitize_text_opt(field(item, "name"), LIMITS.max_adventure_title),
                "Предмет",
            ),
            kind: sanitize_text_opt(field(item, "type"), 60),
            source_id: js_string_or(field(item, "sourceId"), ""),
        })
        .collect();

    let tables = list(field(input, "tables"), LIMITS.max_adventure_tables)
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let rows = list(field(item, "rows"), LIMITS.max_adventure_table_rows)
                .iter()
                .enumerate()
                .map(|(row_index, row)| TableRow {
                    id: js_string_or(field(row, "id"), &format!("row-{row_index}")),
                    label: or_default(sanitize_text_opt(field(row, "label"), 200), "Строка"),
                    weight: clamp_int_or(field(row, "weight"), 1, 99, 1),
                    min: clamped_member(Some(row), "min"),
                    max: clamped_member(Some(row), "max"),
                })
                .collect();
            AdventureTable {
                id: js_string_or(field(item, "id"), &format!("table-{index}")),
                name: or_default(
                    sanitize_text_opt(field(item, "name"), LIMITS.max_adventure_title),
                    "Таблица",
                ),
                formula: sanitize_text_opt(field(item, "formula"), 200),
                description: sanitize_text_opt(
                    field(item, "description"),
                    LIMITS.max_adventure_text,
                ),
                rows,
                source_id: js_string_or(field(item, "sourceId"), ""),
            }
        })
        .collect();

    let actors = list(field(input, "actors"), LIMITS.max_adventure_actors)
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let crop = crop_from(Some(item));
            AdventureActor {
                id: js_string_or(field(item, "id"), &format!("actor-{index}")),
                name: or_default(
                    sanitize_text_opt(field(item, "name"), LIMITS.max_adventure_title),
                    "Персонаж",
                ),
                kind: or_default(sanitize_text_opt(field(item, "type"), 20), "npc"),
                image: normalize_adventure_image(field(item, "image").unwrap_or(&Value::Null)),
                image_zoom: crop.image_zoom,
                image_x: crop.image_x,
                image_y: crop.image_y,
                favorite: js_truthy_opt(field(item, "favorite")),
                statblock: statblock_from(field(item, "statblock")),
                source_id: js_string_or(field(item, "sourceId"), ""),
            }
        })
        .collect();

    let spells = list(field(input, "spells"), LIMITS.max_adventure_spells)
        .iter()
        .enumerate()
        .map(|(index, item)| AdventureSpell {
            id: js_string_or(field(item, "id"), &format!("spell-{index}")),
            name: or_default(
                sanitize_text_opt(field(item, "name"), LIMITS.max_adventure_title),
                "Заклинание",
            ),
            level: clamp_int_or(field(item, "level"), 0, 9, 0),
            school: sanitize_text_opt(field(item, "school"), 60),
            text: sanitize_text_opt(field(item, "text"), LIMITS.max_adventure_text),
            source_id: js_string_or(field(item, "sourceId"), ""),
        })
        .collect();

    Some(Adventure {
        id: js_string_or(field(input, "id"), ""),
        name: or_default(
            sanitize_text_opt(field(input, "name"), LIMITS.max_adventure_title),
            "Приключение",
        ),
        source: or_default(sanitize_text_opt(field(input, "source"), 40), "неизвестно"),
        imported_at: match field(input, "importedAt") {
            Some(value) if js_truthy(value) => js_string(value),
            _ => now_iso8601(),
        },
        notes,
        items,
        tables,
        actors,
        spells,
    })
}

pub fn normalize_adventure(input: &Value) -> Value {
    match adventure_from(input) {
        Some(adventure) => to_value(adventure),
        None => Value::Null,
    }
}

/// `new Date().toISOString()` without pulling a date crate.
fn now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let days = (now.as_secs() / 86_400) as i64;
    let secs = (now.as_secs() % 86_400) as i64;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60,
        now.subsec_millis()
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { y + 1 } else { y }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_sheet_has_reference_shape() {
        let sheet = create_empty_sheet("");
        assert_eq!(sheet["playerName"], json!(""));
        assert_eq!(sheet["level"], json!(1));
        assert_eq!(sheet["experience"], json!(0));
        assert_eq!(sheet["proficiencyBonusManual"], Value::Null);
        assert_eq!(
            sheet["abilities"],
            json!({"str":10,"dex":10,"con":10,"int":10,"wis":10,"cha":10})
        );
        assert_eq!(sheet["coins"], json!({"cp":0,"sp":0,"ep":0,"gp":0,"pp":0}));
        assert_eq!(sheet["hitDice"], json!("1d8"));
        assert_eq!(sheet["armorClass"], json!(10));
        assert_eq!(sheet["speed"], json!(30));
        assert_eq!(sheet["saveProficiencies"].as_object().unwrap().len(), 6);
        assert_eq!(sheet["skillProficiencies"].as_object().unwrap().len(), 18);
        assert_eq!(sheet["attackList"], Value::Null);
        assert!(sheet.get("deathSavesSuccesses").is_none());
    }

    #[test]
    fn empty_maps_and_coins() {
        assert_eq!(
            empty_coins(),
            Coins {
                cp: 0,
                sp: 0,
                ep: 0,
                gp: 0,
                pp: 0
            }
        );
        let map = empty_proficiency_map(&ABILITY_KEYS);
        assert_eq!(map.len(), 6);
        assert!(map.values().all(|value| !*value));
        let skills = empty_proficiency_map(&skill_keys());
        assert_eq!(skills.len(), 18);
        assert!(skills.contains_key("animalHandling"));
    }

    #[test]
    fn sheet_from_garbage_is_empty_and_has_no_extra_fields() {
        for garbage in [json!("text"), json!(42), json!(null), json!([1, 2])] {
            let sheet = normalize_sheet(&garbage, "Игрок");
            assert_eq!(sheet["playerName"], json!("Игрок"));
            assert_eq!(sheet["level"], json!(1));
            assert_eq!(sheet["abilities"]["str"], json!(10));
        }
        let sheet = normalize_sheet(&json!("text"), "Игрок");
        assert!(sheet.get("deathSavesSuccesses").is_none());
        assert!(sheet.get("attackList").is_none());
    }

    #[test]
    fn name_truncation_and_fallback() {
        let long = "а".repeat(50);
        let sheet = normalize_sheet(
            &json!({"playerName": long, "characterName": "б".repeat(120)}),
            "Запасной",
        );
        assert_eq!(sheet["playerName"].as_str().unwrap().chars().count(), 40);
        assert_eq!(sheet["characterName"].as_str().unwrap().chars().count(), 40);

        let sheet = normalize_sheet(&json!({"playerName": "   "}), "Запасной");
        assert_eq!(sheet["playerName"], json!("   "));
        let sheet = normalize_sheet(&json!({"playerName": ""}), "Запасной");
        assert_eq!(sheet["playerName"], json!("Запасной"));
    }

    #[test]
    fn ability_clamps_handle_numbers_strings_and_null() {
        let sheet = normalize_sheet(
            &json!({"abilities": {"str": 100, "dex": -5, "con": "12", "int": "abc", "wis": null, "cha": 3.6}}),
            "",
        );
        let abilities = &sheet["abilities"];
        assert_eq!(abilities["str"], json!(30));
        assert_eq!(abilities["dex"], json!(1));
        assert_eq!(abilities["con"], json!(12));
        assert_eq!(abilities["int"], json!(10));
        assert_eq!(abilities["wis"], json!(1));
        assert_eq!(abilities["cha"], json!(4));
    }

    #[test]
    fn level_and_experience_clamps() {
        assert_eq!(
            normalize_sheet(&json!({"level": 99}), "")["level"],
            json!(20)
        );
        assert_eq!(normalize_sheet(&json!({"level": 0}), "")["level"], json!(1));
        assert_eq!(
            normalize_sheet(&json!({"level": "abc"}), "")["level"],
            json!(1)
        );
        assert_eq!(
            normalize_sheet(&json!({"level": 25.6}), "")["level"],
            json!(20)
        );
        assert_eq!(
            normalize_sheet(&json!({"experience": 1e9}), "")["experience"],
            json!(9999999)
        );
        assert_eq!(
            normalize_sheet(&json!({"experience": -4}), "")["experience"],
            json!(0)
        );
    }

    #[test]
    fn coin_clamps() {
        let coins =
            normalize_coins(&json!({"cp": -5, "sp": "7", "ep": 1e9, "gp": "abc", "pp": null}));
        assert_eq!(coins["cp"], json!(0));
        assert_eq!(coins["sp"], json!(7));
        assert_eq!(coins["ep"], json!(999999));
        assert_eq!(coins["gp"], json!(0));
        assert_eq!(coins["pp"], json!(0));
        assert_eq!(
            normalize_coins(&json!("junk")),
            json!({"cp":0,"sp":0,"ep":0,"gp":0,"pp":0})
        );
    }

    #[test]
    fn combat_and_resource_clamps() {
        let sheet = normalize_sheet(
            &json!({
                "hp": -5000,
                "maxHp": 100000,
                "tempHp": -3,
                "armorClass": 200,
                "speed": -5,
                "inspiration": 500,
                "exhaustion": 9,
                "hitDiceSpent": 50,
                "initiativeMisc": -99,
                "deathSavesSuccesses": 7,
                "deathSavesFailures": -2
            }),
            "",
        );
        assert_eq!(sheet["hp"], json!(-999));
        assert_eq!(sheet["maxHp"], json!(9999));
        assert_eq!(sheet["tempHp"], json!(0));
        assert_eq!(sheet["armorClass"], json!(99));
        assert_eq!(sheet["speed"], json!(0));
        assert_eq!(sheet["inspiration"], json!(99));
        assert_eq!(sheet["exhaustion"], json!(6));
        assert_eq!(sheet["hitDiceSpent"], json!(20));
        assert_eq!(sheet["initiativeMisc"], json!(-20));
        assert_eq!(sheet["deathSavesSuccesses"], json!(3));
        assert_eq!(sheet["deathSavesFailures"], json!(0));
    }

    #[test]
    fn proficiency_bonus_manual_is_null_or_clamped() {
        assert_eq!(
            normalize_sheet(&json!({}), "")["proficiencyBonusManual"],
            Value::Null
        );
        assert_eq!(
            normalize_sheet(&json!({"proficiencyBonusManual": null}), "")["proficiencyBonusManual"],
            Value::Null
        );
        assert_eq!(
            normalize_sheet(&json!({"proficiencyBonusManual": 99}), "")["proficiencyBonusManual"],
            json!(12)
        );
        assert_eq!(
            normalize_sheet(&json!({"proficiencyBonusManual": "abc"}), "")["proficiencyBonusManual"],
            json!(2)
        );
    }

    #[test]
    fn saving_throw_proficiencies_use_js_truthiness() {
        let sheet = normalize_sheet(
            &json!({"saveProficiencies": {"str": 1, "dex": 0, "con": "x", "int": null, "wis": false}}),
            "",
        );
        assert_eq!(sheet["saveProficiencies"]["str"], json!(true));
        assert_eq!(sheet["saveProficiencies"]["dex"], json!(false));
        assert_eq!(sheet["saveProficiencies"]["con"], json!(true));
        assert_eq!(sheet["saveProficiencies"]["int"], json!(false));
        assert_eq!(sheet["saveProficiencies"]["wis"], json!(false));
        assert_eq!(sheet["saveProficiencies"]["cha"], json!(false));
    }

    #[test]
    fn expertise_requires_proficiency() {
        let sheet = normalize_sheet(
            &json!({
                "skillProficiencies": {"stealth": true, "arcana": "yes"},
                "skillExpertise": {"stealth": true, "arcana": true, "athletics": true}
            }),
            "",
        );
        assert_eq!(sheet["skillExpertise"]["stealth"], json!(true));
        assert_eq!(sheet["skillExpertise"]["arcana"], json!(true));
        assert_eq!(sheet["skillExpertise"]["athletics"], json!(false));
    }

    #[test]
    fn attack_list_skips_empty_and_clamps() {
        let attacks = normalize_attack_list(&json!([
            {"name": "", "bonus": 0, "damage": ""},
            {"name": "Меч", "bonus": 99, "damage": "1d8+3"},
            {"name": "Лук", "bonus": -99, "damage": ""},
            "junk",
            {"name": "x".repeat(80), "damage": "y".repeat(100)},
        ]));
        let attacks = attacks.as_array().unwrap();
        assert_eq!(attacks.len(), 3);
        assert_eq!(attacks[0]["name"], json!("Меч"));
        assert_eq!(attacks[0]["bonus"], json!(30));
        assert_eq!(attacks[1]["bonus"], json!(-20));
        assert_eq!(attacks[2]["name"].as_str().unwrap().chars().count(), 60);
        assert_eq!(attacks[2]["damage"].as_str().unwrap().chars().count(), 80);

        let many: Vec<Value> = (0..25)
            .map(|index| json!({"name": format!("a{index}")}))
            .collect();
        let list = normalize_attack_list(&Value::Array(many));
        assert_eq!(list.as_array().unwrap().len(), 20);
        assert_eq!(normalize_attack_list(&json!({})), json!([]));
    }

    #[test]
    fn sheet_spells_default_ids_and_clamp_level() {
        let spells = normalize_sheet_spells(&json!([
            {"name": "", "note": ""},
            {"name": "Огненный шар", "level": 12, "prepared": 1, "note": "n".repeat(300)},
            null,
            {"name": "", "note": "только заметка", "id": "kept"}
        ]));
        let spells = spells.as_array().unwrap();
        assert_eq!(spells.len(), 2);
        assert_eq!(spells[0]["id"], json!("spell-0"));
        assert_eq!(spells[0]["level"], json!(9));
        assert_eq!(spells[0]["prepared"], json!(true));
        assert_eq!(spells[0]["note"].as_str().unwrap().chars().count(), 200);
        assert_eq!(spells[1]["id"], json!("kept"));
        assert_eq!(spells[1]["level"], json!(0));
        assert_eq!(spells[1]["prepared"], json!(false));
    }

    #[test]
    fn classes_drop_unnamed_and_cap_total_levels() {
        let classes = normalize_sheet_classes(&json!([
            {"name": "", "level": 3},
            {"name": "Воин", "level": 25},
            {"name": "Маг", "level": 15},
            {"name": "Плут", "level": 5},
            {"name": "Бард", "level": 1},
            {"name": "Жрец", "level": 1},
            {"name": "Монах", "level": 1},
            {"name": "Паладин", "level": 1}
        ]));
        assert_eq!(classes, json!([{"name": "Воин", "level": 20}]));

        let six = normalize_sheet_classes(&json!([
            {"name": "a", "level": 3},
            {"name": "b", "level": 3},
            {"name": "c", "level": 3},
            {"name": "d", "level": 3},
            {"name": "e", "level": 3},
            {"name": "f", "level": 5},
            {"name": "g", "level": 1}
        ]));
        assert_eq!(six.as_array().unwrap().len(), 6);
        assert_eq!(six[5]["name"], json!("f"));
        assert_eq!(six[5]["level"], json!(5));

        assert_eq!(normalize_sheet_classes(&json!(42)), json!([]));
    }

    #[test]
    fn statblock_rejects_garbage_and_fills_defaults() {
        assert_eq!(normalize_statblock(&json!("x")), Value::Null);
        assert_eq!(normalize_statblock(&json!(null)), Value::Null);
        assert_eq!(normalize_statblock(&json!(7)), Value::Null);

        let block = normalize_statblock(&json!({"hp": 50000, "size": 9, "abilities": {"str": 99}}));
        assert_eq!(block["ac"], json!(0));
        assert_eq!(block["hp"], json!(9999));
        assert_eq!(block["size"], json!(4));
        assert_eq!(block["speed"], json!(""));
        assert_eq!(block["cr"], json!(""));
        assert_eq!(block["abilities"]["str"], json!(30));
        assert_eq!(block["abilities"]["dex"], json!(10));
        assert_eq!(block["attacks"], json!(""));
        assert_eq!(block["notes"], json!(""));
    }

    #[test]
    fn adventure_image_accepts_files_and_http_only() {
        assert_eq!(
            normalize_adventure_image(&json!("/files/goblin.png")),
            "/files/goblin.png"
        );
        assert_eq!(
            normalize_adventure_image(&json!("  /files/a-b_c.1.png  ")),
            "/files/a-b_c.1.png"
        );
        assert_eq!(normalize_adventure_image(&json!("/files/")), "");
        assert_eq!(normalize_adventure_image(&json!("/files/a/b.png")), "");
        assert_eq!(
            normalize_adventure_image(&json!("https://example.com/x.png")),
            "https://example.com/x.png"
        );
        assert_eq!(
            normalize_adventure_image(&json!("HTTPS://example.com")),
            "HTTPS://example.com"
        );
        assert_eq!(
            normalize_adventure_image(&json!("https://exa mple.com")),
            ""
        );
        assert_eq!(normalize_adventure_image(&json!("javascript:alert(1)")), "");
        assert_eq!(normalize_adventure_image(&json!(42)), "");
        assert_eq!(
            normalize_adventure_image(&json!(format!("/files/{}", "a".repeat(81)))),
            ""
        );
    }

    #[test]
    fn image_crop_limits_follow_zoom() {
        let crop = normalize_image_crop(&json!({"imageZoom": 10, "imageX": 1000, "imageY": -1000}));
        assert_eq!(crop["imageZoom"], json!(4.0));
        assert_eq!(crop["imageX"], json!(150.0));
        assert_eq!(crop["imageY"], json!(-150.0));

        let crop = normalize_image_crop(&json!({"imageZoom": 0.5}));
        assert_eq!(crop["imageZoom"], json!(1.0));
        assert_eq!(crop["imageX"], json!(0.0));
        assert_eq!(crop["imageY"], json!(0.0));

        let crop = normalize_image_crop(&json!({"imageZoom": 2, "imageX": "abc", "imageY": 10}));
        assert_eq!(crop["imageZoom"], json!(2.0));
        assert_eq!(crop["imageX"], json!(0.0));
        assert_eq!(crop["imageY"], json!(10.0));

        assert_eq!(
            normalize_image_crop(&json!("junk")),
            json!({"imageZoom": 1.0, "imageX": 0.0, "imageY": 0.0})
        );
    }

    #[test]
    fn adventure_defaults_and_nested_normalization() {
        assert_eq!(normalize_adventure(&json!("junk")), Value::Null);
        assert_eq!(normalize_adventure(&json!(null)), Value::Null);

        let adventure = normalize_adventure(&json!({
            "name": "",
            "source": "",
            "importedAt": "2020-01-01T00:00:00.000Z",
            "notes": [{"title": ""}, 42],
            "items": [{"name": ""}],
            "tables": [{"name": "d20", "rows": [{"label": "", "weight": 0, "min": null}]}],
            "actors": [{
                "name": "Гоблин",
                "type": "",
                "image": "/files/goblin.png",
                "imageZoom": 2,
                "imageX": 999,
                "favorite": 1,
                "statblock": {"hp": -5}
            }],
            "spells": [{"name": "", "level": 99}]
        }));

        assert_eq!(adventure["id"], json!(""));
        assert_eq!(adventure["name"], json!("Приключение"));
        assert_eq!(adventure["source"], json!("неизвестно"));
        assert_eq!(adventure["importedAt"], json!("2020-01-01T00:00:00.000Z"));

        assert_eq!(adventure["notes"][0]["id"], json!("note-0"));
        assert_eq!(adventure["notes"][0]["title"], json!("Заметка"));
        assert_eq!(adventure["notes"][1]["id"], json!("note-1"));
        assert_eq!(adventure["notes"][1]["title"], json!("Заметка"));

        assert_eq!(adventure["items"][0]["id"], json!("item-0"));
        assert_eq!(adventure["items"][0]["name"], json!("Предмет"));
        assert_eq!(adventure["items"][0]["type"], json!(""));

        let row = &adventure["tables"][0]["rows"][0];
        assert_eq!(row["id"], json!("row-0"));
        assert_eq!(row["label"], json!("Строка"));
        assert_eq!(row["weight"], json!(1));
        assert_eq!(row["min"], json!(1));
        assert!(row.get("max").is_none());

        let actor = &adventure["actors"][0];
        assert_eq!(actor["id"], json!("actor-0"));
        assert_eq!(actor["type"], json!("npc"));
        assert_eq!(actor["image"], json!("/files/goblin.png"));
        assert_eq!(actor["imageZoom"], json!(2.0));
        assert_eq!(actor["imageX"], json!(50.0));
        assert_eq!(actor["favorite"], json!(true));
        assert_eq!(actor["statblock"]["hp"], json!(0));

        assert_eq!(adventure["spells"][0]["id"], json!("spell-0"));
        assert_eq!(adventure["spells"][0]["name"], json!("Заклинание"));
        assert_eq!(adventure["spells"][0]["level"], json!(9));
    }

    #[test]
    fn adventure_list_limits_are_applied() {
        let notes: Vec<Value> = (0..400)
            .map(|index| json!({"title": format!("n{index}")}))
            .collect();
        let adventure = normalize_adventure(&json!({"notes": notes, "items": "nope"}));
        assert_eq!(adventure["notes"].as_array().unwrap().len(), 300);
        assert_eq!(adventure["items"], json!([]));
        assert!(adventure["importedAt"].as_str().unwrap().ends_with('Z'));
    }

    #[test]
    fn sanitize_text_removes_nul_and_truncates() {
        assert_eq!(sanitize_text(&Value::Null, 10), "");
        assert_eq!(sanitize_text(&json!("a\u{0}b"), 10), "ab");
        assert_eq!(sanitize_text(&json!("0123456789abc"), 10), "0123456789");
        assert_eq!(sanitize_text(&json!(42), 10), "42");
        assert_eq!(sanitize_text(&json!(true), 10), "true");
        assert_eq!(sanitize_text(&json!("короче"), 10), "короче");
    }

    #[test]
    fn clamp_helpers_match_js_rounding_and_fallbacks() {
        assert_eq!(clamp_int(&json!(5.5), 0, 10, 0), 6);
        assert_eq!(clamp_int(&json!(-2.5), -20, 20, 0), -2);
        assert_eq!(clamp_int(&json!("7.4"), 0, 10, 0), 7);
        assert_eq!(clamp_int(&json!("NaN"), 0, 10, 3), 3);
        assert_eq!(clamp_int(&json!([1]), 0, 10, 3), 3);
        assert_eq!(clamp_int(&json!(true), 0, 10, 3), 1);
        assert_eq!(clamp_int(&json!(null), 1, 10, 3), 1);

        assert_eq!(clamp_number(&json!(2.25), 1.0, 2.0, 0.0), 2.0);
        assert_eq!(clamp_number(&json!(-1), 0.0, 2.0, 0.5), 0.0);
        assert_eq!(clamp_number(&json!("junk"), 0.0, 2.0, 0.5), 0.5);
        assert_eq!(clamp_number(&json!(null), 0.0, 2.0, 0.5), 0.0);
    }

    #[test]
    fn sheet_object_input_gets_extra_fields() {
        let sheet = normalize_sheet(&json!({}), "");
        assert_eq!(sheet["deathSavesSuccesses"], json!(0));
        assert_eq!(sheet["deathSavesFailures"], json!(0));
        assert_eq!(sheet["spellSlotsUsed"], json!([0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(sheet["spellSlotsMax"], json!([0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(sheet["attackList"], json!([]));
        assert_eq!(sheet["spells"], json!([]));

        let slots = normalize_sheet(
            &json!({"spellSlotsUsed": [1, 2, 3, 4, 5, 6, 7, 8, 9, 10], "spellSlotsMax": ["2", -3]}),
            "",
        );
        assert_eq!(slots["spellSlotsUsed"], json!([1, 2, 3, 4, 5, 6, 7, 8, 9]));
        assert_eq!(slots["spellSlotsMax"], json!([2, 0, 0, 0, 0, 0, 0, 0, 0]));
    }

    #[test]
    fn sheet_text_fields_are_trimmed_and_capped() {
        let sheet = normalize_sheet(
            &json!({
                "charClass": "  Воин  ",
                "concentration": "  Благословение  ",
                "notes": "n".repeat(5000),
                "hitDice": "d".repeat(30)
            }),
            "",
        );
        assert_eq!(sheet["charClass"], json!("  Воин  "));
        assert_eq!(sheet["concentration"], json!("Благословение"));
        assert_eq!(sheet["notes"].as_str().unwrap().chars().count(), 4000);
        assert_eq!(sheet["hitDice"].as_str().unwrap().chars().count(), 20);
    }

    #[test]
    fn exhaustion_pulls_hp_down_to_effective_max() {
        let sheet = normalize_sheet(&json!({ "maxHp": 100, "hp": 100, "exhaustion": 4 }), "");
        assert_eq!(sheet["maxHp"], json!(100));
        assert_eq!(sheet["hp"], json!(50));

        let light = normalize_sheet(&json!({ "maxHp": 30, "hp": 20 }), "");
        assert_eq!(light["hp"], json!(20));
    }
}
