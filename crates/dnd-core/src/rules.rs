//! Rules tables and geometry from `renderer/dnd5e.js`.
//!
//! Tables are typed constants; anything that reads untrusted JSON (campaign
//! files, client messages) takes and returns `serde_json::Value`, matching the
//! plain-object boundary of the reference.

use std::cmp::Ordering;

use serde::Serialize;
use serde_json::{json, Map, Value};

pub use crate::model::MAP_TYPES;

/// JS `undefined` stand-in for field lookups: behaves like `null` for `|| ''`
/// and `clampNumber`, but numeric readers use `Option` to keep `NaN` semantics.
static NULL: Value = Value::Null;

fn field<'a>(value: &'a Value, key: &str) -> &'a Value {
    value.get(key).unwrap_or(&NULL)
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

fn js_round(number: f64) -> f64 {
    if number.is_finite() {
        (number + 0.5).floor()
    } else {
        number
    }
}

fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a > b {
        a
    } else {
        b
    }
}

fn js_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else if a < b {
        a
    } else {
        b
    }
}

fn js_parse_number(text: &str) -> f64 {
    let text = text.trim();
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let lower = text.to_ascii_lowercase();
    if matches!(lower.as_str(), "inf" | "+inf" | "-inf" | "nan" | "+nan" | "-nan") {
        return f64::NAN;
    }
    text.parse::<f64>().unwrap_or(f64::NAN)
}

/// JS `Number(value)`.
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
        Value::String(text) => js_parse_number(text),
        Value::Array(items) => match items.len() {
            0 => 0.0,
            1 => {
                let item = &items[0];
                if item.is_null() {
                    0.0
                } else {
                    js_parse_number(&js_to_string(item))
                }
            }
            _ => f64::NAN,
        },
        Value::Object(_) => f64::NAN,
    }
}

/// JS `Number(value)` where a missing field is `undefined` (not `null`).
fn number_of(value: Option<&Value>) -> f64 {
    match value {
        Some(value) => js_number(value),
        None => f64::NAN,
    }
}

/// JS `Number(value) || fallback`.
fn num_or(value: Option<&Value>, fallback: f64) -> f64 {
    let number = number_of(value);
    if number == 0.0 || number.is_nan() {
        fallback
    } else {
        number
    }
}

fn js_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => match number.as_f64() {
            Some(number) => number != 0.0 && !number.is_nan(),
            None => false,
        },
        Value::String(text) => !text.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

fn js_number_to_string(number: f64) -> String {
    if number.is_nan() {
        return "NaN".to_string();
    }
    if number == f64::INFINITY {
        return "Infinity".to_string();
    }
    if number == f64::NEG_INFINITY {
        return "-Infinity".to_string();
    }
    if number == 0.0 {
        return "0".to_string();
    }
    if number.fract() == 0.0 && number.abs() < 1e15 {
        return format!("{}", number as i64);
    }
    format!("{number}")
}

/// JS `String(value)`.
fn js_to_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => js_number_to_string(number.as_f64().unwrap_or(f64::NAN)),
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

/// JS `String(value || '')`.
fn js_str_or_empty(value: &Value) -> String {
    if js_truthy(value) {
        js_to_string(value)
    } else {
        String::new()
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// JSON number like JS: whole values stay whole (`10`, not `10.0`).
fn number_value(number: f64) -> Value {
    if !number.is_finite() {
        return Value::Null;
    }
    if number.fract() == 0.0 && number.abs() < 9e15 {
        json!(number as i64)
    } else {
        json!(number)
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

pub const ABILITY_KEYS: [&str; 6] = ["str", "dex", "con", "int", "wis", "cha"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ability {
    pub key: &'static str,
    pub label: &'static str,
    pub abbr: &'static str,
}

pub const ABILITIES: [Ability; 6] = [
    Ability { key: "str", label: "Сила", abbr: "СИЛ" },
    Ability { key: "dex", label: "Ловкость", abbr: "ЛОВ" },
    Ability { key: "con", label: "Телосложение", abbr: "ТЕЛ" },
    Ability { key: "int", label: "Интеллект", abbr: "ИНТ" },
    Ability { key: "wis", label: "Мудрость", abbr: "МДР" },
    Ability { key: "cha", label: "Харизма", abbr: "ХАР" },
];

/// `ABILITY_LABELS`.
pub fn ability_labels() -> Value {
    let mut labels = Map::new();
    for ability in ABILITIES {
        labels.insert(ability.key.to_string(), Value::String(ability.label.to_string()));
    }
    Value::Object(labels)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub key: &'static str,
    pub label: &'static str,
    pub ability: &'static str,
}

pub const SKILLS: [Skill; 18] = [
    Skill { key: "acrobatics", label: "Акробатика", ability: "dex" },
    Skill { key: "animalHandling", label: "Уход за животными", ability: "wis" },
    Skill { key: "arcana", label: "Магия", ability: "int" },
    Skill { key: "athletics", label: "Атлетика", ability: "str" },
    Skill { key: "deception", label: "Обман", ability: "cha" },
    Skill { key: "history", label: "История", ability: "int" },
    Skill { key: "insight", label: "Проницательность", ability: "wis" },
    Skill { key: "intimidation", label: "Запугивание", ability: "cha" },
    Skill { key: "investigation", label: "Анализ", ability: "int" },
    Skill { key: "medicine", label: "Медицина", ability: "wis" },
    Skill { key: "nature", label: "Природа", ability: "int" },
    Skill { key: "perception", label: "Внимательность", ability: "wis" },
    Skill { key: "performance", label: "Выступление", ability: "cha" },
    Skill { key: "persuasion", label: "Убеждение", ability: "cha" },
    Skill { key: "religion", label: "Религия", ability: "int" },
    Skill { key: "sleightOfHand", label: "Ловкость рук", ability: "dex" },
    Skill { key: "stealth", label: "Скрытность", ability: "dex" },
    Skill { key: "survival", label: "Выживание", ability: "wis" },
];

pub const HIT_DICE: [&str; 4] = ["1d6", "1d8", "1d10", "1d12"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DnDClass {
    pub label: &'static str,
    pub hit_die: &'static str,
}

pub const CLASSES: [DnDClass; 12] = [
    DnDClass { label: "Варвар", hit_die: "d12" },
    DnDClass { label: "Бард", hit_die: "d8" },
    DnDClass { label: "Воин", hit_die: "d10" },
    DnDClass { label: "Волшебник", hit_die: "d6" },
    DnDClass { label: "Друид", hit_die: "d8" },
    DnDClass { label: "Жрец", hit_die: "d8" },
    DnDClass { label: "Колдун", hit_die: "d8" },
    DnDClass { label: "Монах", hit_die: "d8" },
    DnDClass { label: "Паладин", hit_die: "d10" },
    DnDClass { label: "Плут", hit_die: "d8" },
    DnDClass { label: "Следопыт", hit_die: "d10" },
    DnDClass { label: "Чародей", hit_die: "d6" },
];

pub const MAX_CLASS_ENTRIES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Condition {
    pub id: &'static str,
    pub label: &'static str,
    pub short: &'static str,
    pub effect: &'static str,
    #[serde(skip_serializing_if = "is_false")]
    pub no_move: bool,
}

pub const CONDITIONS: [Condition; 16] = [
    Condition { id: "blinded", label: "Слепой", short: "Слепой", effect: "blindfold", no_move: false },
    Condition { id: "charmed", label: "Очарован", short: "Очар.", effect: "charmed", no_move: false },
    Condition { id: "deafened", label: "Глухой", short: "Глухой", effect: "deafened", no_move: false },
    Condition { id: "dazzled", label: "Ослеплён", short: "Ослеп", effect: "dazzled", no_move: false },
    Condition { id: "exhaustion", label: "Истощение", short: "Истощ.", effect: "exhaustion", no_move: false },
    Condition { id: "frightened", label: "Испуган", short: "Испуг", effect: "frightened", no_move: false },
    Condition { id: "grappled", label: "Схвачен", short: "Схват", effect: "grappled", no_move: false },
    Condition { id: "incapacitated", label: "Недееспособен", short: "Недеесп.", effect: "chained", no_move: true },
    Condition { id: "invisible", label: "Невидим", short: "Невид.", effect: "veil", no_move: false },
    Condition { id: "paralyzed", label: "Парализован", short: "Парал.", effect: "bolt", no_move: true },
    Condition { id: "petrified", label: "Окаменел", short: "Окамен.", effect: "stone", no_move: true },
    Condition { id: "poisoned", label: "Отравлен", short: "Отравл.", effect: "poison", no_move: false },
    Condition { id: "prone", label: "Сбит с ног", short: "Лежит", effect: "prone", no_move: false },
    Condition { id: "restrained", label: "Опутан", short: "Опут.", effect: "vines", no_move: true },
    Condition { id: "stunned", label: "Оглушён", short: "Оглуш.", effect: "dizzy", no_move: true },
    Condition { id: "unconscious", label: "Без сознания", short: "Без созн.", effect: "unconscious", no_move: true },
];

/// `MOVE_BLOCKING`.
pub fn move_blocking() -> Vec<&'static str> {
    CONDITIONS
        .iter()
        .filter(|condition| condition.no_move)
        .map(|condition| condition.id)
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SurfaceType {
    pub id: &'static str,
    pub label: &'static str,
    pub short: &'static str,
    pub color: &'static str,
}

pub const SURFACE_TYPES: [SurfaceType; 8] = [
    SurfaceType { id: "fire", label: "Огонь", short: "огонь", color: "#e0702c" },
    SurfaceType { id: "acid", label: "Кислота", short: "кислота", color: "#a86ed8" },
    SurfaceType { id: "water", label: "Вода", short: "вода", color: "#3f8fd0" },
    SurfaceType { id: "ice", label: "Лёд", short: "лёд", color: "#9fd8e8" },
    SurfaceType { id: "poison", label: "Яд", short: "яд", color: "#7fb23f" },
    SurfaceType { id: "slick", label: "Скользкость", short: "скользко", color: "#c9b46a" },
    SurfaceType { id: "smoke", label: "Дым", short: "дым", color: "#8b8b93" },
    SurfaceType { id: "rubble", label: "Руины", short: "руины", color: "#8d7a63" },
];

/// `SURFACE_IDS`.
pub fn surface_ids() -> Vec<&'static str> {
    SURFACE_TYPES.iter().map(|surface| surface.id).collect()
}

/// `DAMAGE_TYPES`: first pair has an empty id ("no type").
pub const DAMAGE_TYPES: [(&str, &str); 13] = [
    ("", "без типа"),
    ("огонь", "огонь"),
    ("холод", "холод"),
    ("электричество", "электричество"),
    ("кислота", "кислота"),
    ("яд", "яд"),
    ("некротический", "некротический"),
    ("излучение", "излучение"),
    ("силовой", "силовой"),
    ("дробящий", "дробящий"),
    ("колющий", "колющий"),
    ("рубящий", "рубящий"),
    ("психический", "психический"),
];

/// `DAMAGE_TYPE_IDS`.
pub fn damage_type_ids() -> Vec<&'static str> {
    DAMAGE_TYPES
        .iter()
        .map(|(id, _)| *id)
        .filter(|id| !id.is_empty())
        .collect()
}

/// `damageAfterDefenses`.
pub fn damage_after_defenses(raw_amount: &Value, raw_type: &Value, defenses: &Value) -> Value {
    let amount = js_max(0.0, js_round(or_value(js_number(raw_amount), 0.0))) as i64;
    let damage_type = js_str_or_empty(raw_type).trim().to_lowercase();
    if damage_type.is_empty() {
        return json!({ "amount": amount, "kind": "normal" });
    }
    let includes = |key: &str| -> bool {
        match defenses.get(key) {
            Some(Value::Array(list)) => list
                .iter()
                .any(|entry| matches!(entry, Value::String(text) if text == &damage_type)),
            _ => false,
        }
    };
    if includes("immunities") {
        return json!({ "amount": 0, "kind": "immune" });
    }
    let resistant = includes("resistances");
    let vulnerable = includes("vulnerabilities");
    let multiplier = if resistant { 0.5 } else { 1.0 } * if vulnerable { 2.0 } else { 1.0 };
    let amount = (amount as f64 * multiplier + 1e-9).floor() as i64;
    let kind = if multiplier < 1.0 {
        "resistant"
    } else if multiplier > 1.0 {
        "vulnerable"
    } else {
        "normal"
    };
    json!({ "amount": amount, "kind": kind })
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GridLimits {
    pub min_cell: i64,
    pub max_cell: i64,
    pub default_cell: i64,
    pub min_columns: i64,
    pub max_columns: i64,
    pub min_columns_step: f64,
    pub max_offset: i64,
    pub min_feet_per_cell: i64,
    pub max_feet_per_cell: i64,
    pub default_feet_per_cell: i64,
}

pub const GRID: GridLimits = GridLimits {
    min_cell: 8,
    max_cell: 512,
    default_cell: 46,
    min_columns: 2,
    max_columns: 200,
    min_columns_step: 0.5,
    max_offset: 4096,
    min_feet_per_cell: 1,
    max_feet_per_cell: 50,
    default_feet_per_cell: 5,
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LightPreset {
    pub id: &'static str,
    pub label: &'static str,
    pub radius: i64,
    pub color: &'static str,
}

pub const LIGHT_PRESETS: [LightPreset; 12] = [
    LightPreset { id: "torch", label: "Факел", radius: 20, color: "#ffab4d" },
    LightPreset { id: "lantern", label: "Фонарь", radius: 30, color: "#ffd27a" },
    LightPreset { id: "campfire", label: "Костёр", radius: 40, color: "#ff8b3d" },
    LightPreset { id: "candle", label: "Свеча", radius: 5, color: "#fff0c2" },
    LightPreset { id: "hood", label: "Фонарь в капюшоне", radius: 10, color: "#b9c8ff" },
    LightPreset { id: "moon", label: "Лунный свет", radius: 60, color: "#7d9cff" },
    LightPreset { id: "stars", label: "Свет звёзд", radius: 80, color: "#9fd0ff" },
    LightPreset { id: "torchlight", label: "Свет фонаря на клинке", radius: 25, color: "#ffc27a" },
    LightPreset { id: "will-o-wisp", label: "Болотный огонь", radius: 20, color: "#8ef0a8" },
    LightPreset { id: "arcane", label: "Волшебный свет", radius: 30, color: "#c49bff" },
    LightPreset { id: "blood", label: "Красный свет", radius: 20, color: "#ff6b6b" },
    LightPreset { id: "daylight", label: "Дневной свет", radius: 120, color: "#fff6e0" },
];

pub const LIGHT_AMBIENT: &str = "#080a18";

pub const DARKNESS_BY_HOUR: [f64; 24] = [
    0.9, 0.95, 0.95, 0.95, 0.9, 0.75, // 00..05
    0.45, 0.2, 0.0, // 06..08
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, // 09..17
    0.05, 0.2, 0.4, 0.6, 0.8, 0.9, // 18..23
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SketchLimits {
    pub max_side: i64,
}

pub const SKETCH: SketchLimits = SketchLimits { max_side: 1600 };

pub const DICE_SIDES: [u32; 7] = [4, 6, 8, 10, 12, 20, 100];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Fog {
    pub columns: usize,
    pub rows: usize,
    pub open: &'static str,
    pub closed: &'static str,
}

pub const FOG: Fog = Fog {
    columns: 64,
    rows: 40,
    open: "1",
    closed: "0",
};

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LightLimits {
    pub max_walls: usize,
    pub max_lights: usize,
    pub max_pins: usize,
    pub min_radius: f64,
    pub max_radius: f64,
    pub rays: usize,
}

pub const LIGHT_LIMITS: LightLimits = LightLimits {
    max_walls: 400,
    max_lights: 80,
    max_pins: 60,
    min_radius: 1.0,
    max_radius: 200.0,
    rays: 64,
};

pub const SPELL_SLOT_LEVELS: [u8; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 9];

/// Rows by character level 1..20, columns by spell level 1..9.
pub const FULL_CASTER_SLOTS: [[u8; 9]; 20] = [
    [2, 0, 0, 0, 0, 0, 0, 0, 0],
    [3, 0, 0, 0, 0, 0, 0, 0, 0],
    [4, 2, 0, 0, 0, 0, 0, 0, 0],
    [4, 3, 0, 0, 0, 0, 0, 0, 0],
    [4, 3, 3, 0, 0, 0, 0, 0, 0],
    [4, 3, 3, 3, 0, 0, 0, 0, 0],
    [4, 4, 3, 3, 0, 0, 0, 0, 0],
    [4, 4, 3, 3, 1, 0, 0, 0, 0],
    [4, 4, 3, 3, 2, 0, 0, 0, 0],
    [4, 4, 3, 3, 2, 1, 0, 0, 0],
    [4, 4, 3, 3, 2, 1, 0, 0, 0],
    [4, 4, 3, 3, 2, 1, 1, 0, 0],
    [4, 4, 3, 3, 2, 1, 1, 0, 0],
    [4, 4, 3, 3, 2, 1, 1, 1, 0],
    [4, 4, 3, 3, 2, 1, 1, 1, 0],
    [4, 4, 3, 3, 2, 1, 1, 1, 1],
    [4, 4, 3, 3, 2, 1, 1, 1, 1],
    [4, 4, 3, 3, 3, 1, 1, 1, 1],
    [4, 4, 3, 3, 3, 2, 1, 1, 1],
    [4, 4, 3, 3, 3, 2, 2, 1, 1],
];

pub const FULL_CASTER_CLASSES: [&str; 10] = [
    "бард", "bard", "жрец", "cleric", "друид", "druid", "чародей", "sorcerer", "волшебник", "wizard",
];

/// `MAP_LABELS`.
pub fn map_labels() -> Value {
    json!({
        "world": "Общая карта",
        "battle": "Карта локации / битвы",
        "sketch": "Рисунок",
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Calendar {
    pub days_per_month: i64,
    pub max_months: i64,
    pub month_names: [&'static str; 12],
}

pub const CALENDAR: Calendar = Calendar {
    days_per_month: 30,
    max_months: 24,
    month_names: [
        "января", "февраля", "марта", "апреля", "мая", "июня",
        "июля", "августа", "сентября", "октября", "ноября", "декабря",
    ],
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub max_name_length: i64,
    pub max_chat_length: i64,
    pub max_image_bytes: i64,
    pub max_token_hp: i64,
    pub max_token_conditions: i64,
    pub max_dm_note: i64,
    pub max_avatar_bytes: i64,
    pub max_audio_bytes: i64,
    pub min_ability: i64,
    pub max_ability: i64,
    pub min_level: i64,
    pub max_level: i64,
    pub min_hp: i64,
    pub max_hp: i64,
    pub max_death_saves: i64,
    pub max_spell_slots_per_level: i64,
    pub min_armor_class: i64,
    pub max_armor_class: i64,
    pub min_initiative_misc: i64,
    pub max_initiative_misc: i64,
    pub min_speed: i64,
    pub max_speed: i64,
    pub min_proficiency_bonus: i64,
    pub max_proficiency_bonus: i64,
    pub max_experience: i64,
    pub max_sheet_text_length: i64,
    pub max_hit_dice_length: i64,
    pub max_class_length: i64,
    pub max_sheet_attacks: i64,
    pub max_attack_name: i64,
    pub max_attack_damage: i64,
    pub max_exhaustion: i64,
    pub max_inspiration: i64,
    pub max_concentration_length: i64,
    pub max_coins: i64,
}

pub const LIMITS: Limits = Limits {
    max_name_length: 40,
    max_chat_length: 1000,
    max_image_bytes: 10 * 1024 * 1024,
    max_token_hp: 9999,
    max_token_conditions: 16,
    max_dm_note: 4000,
    max_avatar_bytes: 2 * 1024 * 1024,
    max_audio_bytes: 2 * 1024 * 1024,
    min_ability: 1,
    max_ability: 30,
    min_level: 1,
    max_level: 20,
    min_hp: -999,
    max_hp: 9999,
    max_death_saves: 3,
    max_spell_slots_per_level: 9,
    min_armor_class: 0,
    max_armor_class: 99,
    min_initiative_misc: -20,
    max_initiative_misc: 20,
    min_speed: 0,
    max_speed: 999,
    min_proficiency_bonus: 0,
    max_proficiency_bonus: 12,
    max_experience: 9999999,
    max_sheet_text_length: 4000,
    max_hit_dice_length: 20,
    max_class_length: 60,
    max_sheet_attacks: 20,
    max_attack_name: 60,
    max_attack_damage: 80,
    max_exhaustion: 6,
    max_inspiration: 99,
    max_concentration_length: 120,
    max_coins: 999999,
};

pub const AURA_PRESETS: [i64; 4] = [10, 15, 20, 30];

// ---------------------------------------------------------------------------
// Classes
// ---------------------------------------------------------------------------

/// `hitDieSides`.
pub fn hit_die_sides(value: &Value) -> Option<i64> {
    let text = js_str_or_empty(value);
    let bytes = text.as_bytes();
    for start in 0..bytes.len() {
        let mut index = start;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index >= bytes.len() || !(bytes[index] == b'd' || bytes[index] == b'D') {
            continue;
        }
        index += 1;
        let digits_start = index;
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index == digits_start {
            continue;
        }
        let Ok(sides) = text[digits_start..index].parse::<i64>() else {
            return None;
        };
        return (2..=100).contains(&sides).then_some(sides);
    }
    None
}

/// `classHitDie`.
pub fn class_hit_die(name: &Value) -> Option<&'static str> {
    let value = js_str_or_empty(name).trim().to_lowercase();
    if value.is_empty() {
        return None;
    }
    CLASSES
        .iter()
        .find(|class| class.label.to_lowercase() == value)
        .map(|class| class.hit_die)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClassLevel {
    name: String,
    level: i64,
}

fn clamp_number_opt(value: Option<&Value>, min: f64, max: f64, fallback: f64) -> f64 {
    match value {
        Some(value) => clamp_number(value, min, max, fallback),
        None => fallback,
    }
}

fn normalize_classes_vec(value: &Value) -> Vec<ClassLevel> {
    let empty = Vec::new();
    let list = value.as_array().unwrap_or(&empty);
    let mut result = Vec::new();
    let mut sum = 0;
    for entry in list {
        if result.len() >= MAX_CLASS_ENTRIES {
            break;
        }
        let name = truncate_chars(
            js_str_or_empty(field(entry, "name")).trim(),
            LIMITS.max_class_length as usize,
        );
        if name.is_empty() {
            continue;
        }
        let room = LIMITS.max_level - sum;
        if room <= 0 {
            break;
        }
        let level = clamp_number_opt(
            entry.get("level"),
            LIMITS.min_level as f64,
            LIMITS.max_level as f64,
            1.0,
        ) as i64;
        let level = level.min(room);
        result.push(ClassLevel { name, level });
        sum += level;
    }
    result
}

/// `normalizeClasses`.
pub fn normalize_classes(value: &Value) -> Value {
    Value::Array(
        normalize_classes_vec(value)
            .into_iter()
            .map(|entry| json!({ "name": entry.name, "level": entry.level }))
            .collect(),
    )
}

fn class_levels_vec(sheet: &Value) -> Vec<ClassLevel> {
    let raw = if sheet.is_object() { sheet } else { &NULL };
    let list = normalize_classes_vec(field(raw, "classes"));
    if !list.is_empty() {
        return list;
    }
    let name = js_str_or_empty(field(raw, "charClass")).trim().to_string();
    if name.is_empty() {
        return Vec::new();
    }
    vec![ClassLevel {
        name: truncate_chars(&name, LIMITS.max_class_length as usize),
        level: clamp_number_opt(raw.get("level"), LIMITS.min_level as f64, LIMITS.max_level as f64, 1.0)
            as i64,
    }]
}

/// `classLevels`.
pub fn class_levels(sheet: &Value) -> Value {
    Value::Array(
        class_levels_vec(sheet)
            .into_iter()
            .map(|entry| json!({ "name": entry.name, "level": entry.level }))
            .collect(),
    )
}

/// `totalCharacterLevel`.
pub fn total_character_level(sheet: &Value) -> i64 {
    let list = class_levels_vec(sheet);
    if list.is_empty() {
        return clamp_number_opt(
            sheet.get("level"),
            LIMITS.min_level as f64,
            LIMITS.max_level as f64,
            1.0,
        ) as i64;
    }
    let sum: i64 = list.iter().map(|entry| entry.level).sum();
    sum.clamp(LIMITS.min_level, LIMITS.max_level)
}

/// `hitDicePool`.
pub fn hit_dice_pool(sheet: &Value) -> Value {
    let mut pool: Vec<(String, i64, i64)> = Vec::new();
    for entry in class_levels_vec(sheet) {
        let Some(die) = class_hit_die(&Value::String(entry.name)) else {
            continue;
        };
        let Some(sides) = hit_die_sides(&Value::String(die.to_string())) else {
            continue;
        };
        if let Some(current) = pool.iter_mut().find(|(name, _, _)| name == die) {
            current.2 += entry.level;
        } else {
            pool.push((die.to_string(), sides, entry.level));
        }
    }
    if pool.is_empty() {
        if let Some(sides) = hit_die_sides(&Value::String(js_str_or_empty(field(sheet, "hitDice")))) {
            pool.push((format!("d{sides}"), sides, 1));
        }
    }
    Value::Array(
        pool.into_iter()
            .map(|(die, sides, count)| json!({ "die": die, "sides": sides, "count": count }))
            .collect(),
    )
}

/// `combinedCasterLevel`.
pub fn combined_caster_level(sheet: &Value) -> i64 {
    class_levels_vec(sheet)
        .iter()
        .filter(|entry| {
            FULL_CASTER_CLASSES.contains(&entry.name.trim().to_lowercase().as_str())
        })
        .map(|entry| entry.level)
        .sum()
}

/// `casterSlotRow`.
pub fn caster_slot_row(sheet: &Value) -> Value {
    let level = combined_caster_level(sheet);
    if level == 0 {
        return Value::Null;
    }
    let level = level.clamp(1, 20) as usize;
    Value::Array(
        FULL_CASTER_SLOTS[level - 1]
            .iter()
            .map(|slots| json!(slots))
            .collect(),
    )
}

/// `spellSlotRow`.
pub fn spell_slot_row(char_class: &Value, level: &Value) -> Value {
    let name = js_str_or_empty(char_class).trim().to_lowercase();
    if !FULL_CASTER_CLASSES.contains(&name.as_str()) {
        return Value::Null;
    }
    let lv = js_min(
        20.0,
        js_max(1.0, js_round(or_value(js_number(level), 1.0))),
    ) as usize;
    Value::Array(
        FULL_CASTER_SLOTS[lv - 1]
            .iter()
            .map(|slots| json!(slots))
            .collect(),
    )
}

fn or_value(number: f64, fallback: f64) -> f64 {
    if number == 0.0 || number.is_nan() {
        fallback
    } else {
        number
    }
}

/// `spellSlotMax`.
pub fn spell_slot_max(char_class: &Value, level: &Value, slot_level: &Value) -> Value {
    let row = spell_slot_row(char_class, level);
    let Some(row) = row.as_array() else {
        return Value::Null;
    };
    let index = match slot_level {
        Value::Number(number) => {
            let value = number.as_f64().unwrap_or(f64::NAN);
            if value.is_finite() && value.fract() == 0.0 && (1.0..=9.0).contains(&value) {
                value as i64
            } else {
                return Value::Null;
            }
        }
        _ => return Value::Null,
    };
    row[(index - 1) as usize].clone()
}

// ---------------------------------------------------------------------------
// Grid and surfaces
// ---------------------------------------------------------------------------

/// `roundColumns`.
pub fn round_columns(value: &Value) -> f64 {
    let number = js_number(value);
    if !number.is_finite() {
        return 0.0;
    }
    js_round(number / GRID.min_columns_step) * GRID.min_columns_step
}

/// `normalizeGrid`.
pub fn normalize_grid(raw: &Value) -> Value {
    let grid = if raw.is_object() { raw } else { &NULL };
    let offset_x = js_round(clamp_number(
        field(grid, "offsetX"),
        -(GRID.max_offset as f64),
        GRID.max_offset as f64,
        0.0,
    )) as i64;
    let offset_y = js_round(clamp_number(
        field(grid, "offsetY"),
        -(GRID.max_offset as f64),
        GRID.max_offset as f64,
        0.0,
    )) as i64;
    json!({
        "enabled": js_truthy(field(grid, "enabled")),
        "cell": clamp_number(
            field(grid, "cell"),
            GRID.min_cell as f64,
            GRID.max_cell as f64,
            GRID.default_cell as f64,
        ) as i64,
        "columns": number_value(js_min(
            GRID.max_columns as f64,
            js_max(0.0, round_columns(field(grid, "columns"))),
        )),
        "feetPerCell": clamp_number(
            field(grid, "feetPerCell"),
            GRID.min_feet_per_cell as f64,
            GRID.max_feet_per_cell as f64,
            GRID.default_feet_per_cell as f64,
        ) as i64,
        "offsetX": offset_x,
        "offsetY": offset_y,
    })
}

/// `gridCellSize`.
pub fn grid_cell_size(grid: &Value, natural_width: &Value) -> f64 {
    let columns = or_value(number_of(grid.get("columns")), 0.0);
    let width = or_value(js_number(natural_width), 0.0);
    if columns > 1.0 && width > 0.0 {
        let fitted = width / columns;
        return js_min(GRID.max_cell as f64, js_max(GRID.min_cell as f64, fitted));
    }
    let manual = or_value(number_of(grid.get("cell")), GRID.default_cell as f64);
    js_min(GRID.max_cell as f64, js_max(GRID.min_cell as f64, manual))
}

fn js_parse_int(text: &str) -> Option<i64> {
    let text = text.trim_start();
    let bytes = text.as_bytes();
    let mut index = 0;
    let mut negative = false;
    if index < bytes.len() && (bytes[index] == b'+' || bytes[index] == b'-') {
        negative = bytes[index] == b'-';
        index += 1;
    }
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    if index == start {
        return None;
    }
    let digits: i64 = text[start..index].parse().ok()?;
    Some(if negative { -digits } else { digits })
}

/// `normalizeSurface`.
pub fn normalize_surface(raw: &Value) -> Value {
    let mut cells = Map::new();
    let source = if raw.is_object() {
        raw.get("cells")
    } else {
        None
    };
    let Some(source) = source.and_then(Value::as_object) else {
        return json!({ "cells": Value::Object(cells) });
    };
    let ids = surface_ids();
    for (key, value) in source {
        if cells.len() >= 1500 {
            break;
        }
        let parts: Vec<&str> = key.split(',').collect();
        if parts.len() != 2 {
            continue;
        }
        let (Some(col), Some(row)) = (js_parse_int(parts[0]), js_parse_int(parts[1])) else {
            continue;
        };
        if col < 0 || row < 0 {
            continue;
        }
        let surface_type = js_str_or_empty(value);
        if !ids.contains(&surface_type.as_str()) {
            continue;
        }
        cells.insert(format!("{col},{row}"), Value::String(surface_type));
    }
    json!({ "cells": Value::Object(cells) })
}

// ---------------------------------------------------------------------------
// Walls and lights
// ---------------------------------------------------------------------------

/// `normalizeWall`.
pub fn normalize_wall(raw: &Value) -> Value {
    if !raw.is_object() {
        return Value::Null;
    }
    let x1 = number_of(raw.get("x1"));
    let y1 = number_of(raw.get("y1"));
    let x2 = number_of(raw.get("x2"));
    let y2 = number_of(raw.get("y2"));
    if ![x1, y1, x2, y2].iter().all(|value| value.is_finite()) {
        return Value::Null;
    }
    let ax = js_max(0.0, js_min(1.0, x1));
    let ay = js_max(0.0, js_min(1.0, y1));
    let bx = js_max(0.0, js_min(1.0, x2));
    let by = js_max(0.0, js_min(1.0, y2));
    if (bx - ax).hypot(by - ay) < 0.004 {
        return Value::Null;
    }
    let mut wall = Map::new();
    wall.insert(
        "id".to_string(),
        Value::String(truncate_chars(&js_str_or_empty(field(raw, "id")), 40)),
    );
    wall.insert("x1".to_string(), json!(ax));
    wall.insert("y1".to_string(), json!(ay));
    wall.insert("x2".to_string(), json!(bx));
    wall.insert("y2".to_string(), json!(by));
    if let Some(door) = raw.get("door").filter(|door| door.is_object()) {
        wall.insert(
            "door".to_string(),
            json!({
                "open": matches!(door.get("open"), Some(Value::Bool(true))),
                "locked": matches!(door.get("locked"), Some(Value::Bool(true))),
            }),
        );
    }
    if matches!(raw.get("window"), Some(Value::Bool(true))) {
        wall.insert("window".to_string(), Value::Bool(true));
    }
    Value::Object(wall)
}

/// `normalizeWalls`.
pub fn normalize_walls(raw: &Value) -> Value {
    let Some(list) = raw.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        list.iter()
            .take(LIGHT_LIMITS.max_walls)
            .map(normalize_wall)
            .filter(|wall| !wall.is_null())
            .collect(),
    )
}

fn is_hex_color(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 7 && bytes[0] == b'#' && bytes[1..].iter().all(u8::is_ascii_hexdigit)
}

/// `normalizeLight`.
pub fn normalize_light(raw: &Value) -> Value {
    if !raw.is_object() {
        return Value::Null;
    }
    let x = number_of(raw.get("x"));
    let y = number_of(raw.get("y"));
    if !x.is_finite() || !y.is_finite() {
        return Value::Null;
    }
    let radius = clamp_number(
        field(raw, "radius"),
        LIGHT_LIMITS.min_radius,
        LIGHT_LIMITS.max_radius,
        20.0,
    );
    let color = match field(raw, "color") {
        Value::String(text) if is_hex_color(text) => text.clone(),
        _ => "#ffb347".to_string(),
    };
    json!({
        "id": truncate_chars(&js_str_or_empty(field(raw, "id")), 40),
        "x": js_max(0.0, js_min(1.0, x)),
        "y": js_max(0.0, js_min(1.0, y)),
        "radius": js_round(radius) as i64,
        "color": color,
        "dim": js_truthy(field(raw, "dim")),
        "hidden": js_truthy(field(raw, "hidden")),
    })
}

/// `normalizeLights`.
pub fn normalize_lights(raw: &Value) -> Value {
    let Some(list) = raw.as_array() else {
        return Value::Array(Vec::new());
    };
    Value::Array(
        list.iter()
            .take(LIGHT_LIMITS.max_lights)
            .map(normalize_light)
            .filter(|light| !light.is_null())
            .collect(),
    )
}

/// `lightGrid`.
pub fn light_grid(grid: &Value, aspect: &Value) -> Value {
    let safe = if grid.is_object() { grid } else { &NULL };
    let ratio = js_number(aspect);
    let width = {
        let columns = number_of(safe.get("columns"));
        if columns > 0.0 {
            columns
        } else {
            GRID.max_columns as f64
        }
    };
    let usable = if ratio.is_finite() && ratio > 0.0 {
        ratio
    } else {
        1.0
    };
    json!({
        "columns": js_max(GRID.min_columns as f64, js_round(width)) as i64,
        "rows": js_max(1.0, js_round(width / usable)) as i64,
        "feetPerCell": clamp_number(
            field(safe, "feetPerCell"),
            GRID.min_feet_per_cell as f64,
            GRID.max_feet_per_cell as f64,
            GRID.default_feet_per_cell as f64,
        ) as i64,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CellWall {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

/// `castRay`. Distances are in grid cells.
pub fn cast_ray(ox: f64, oy: f64, dx: f64, dy: f64, limit: f64, walls: &[CellWall]) -> f64 {
    let mut best = limit;
    for wall in walls {
        let ex = wall.x2 - wall.x1;
        let ey = wall.y2 - wall.y1;
        let denominator = dx * ey - dy * ex;
        if denominator.abs() < 1e-9 {
            continue;
        }
        let t = ((wall.x1 - ox) * ey - (wall.y1 - oy) * ex) / denominator;
        let u = ((wall.x1 - ox) * dy - (wall.y1 - oy) * dx) / denominator;
        if t <= 1e-6 || u < 0.0 || u > 1.0 {
            continue;
        }
        if t < best {
            best = t;
        }
    }
    best
}

/// `distanceToSegment`.
pub fn distance_to_segment(px: f64, py: f64, x1: f64, y1: f64, x2: f64, y2: f64) -> f64 {
    let ex = x2 - x1;
    let ey = y2 - y1;
    let length_squared = ex * ex + ey * ey;
    if length_squared < 1e-12 {
        return (px - x1).hypot(py - y1);
    }
    let t = js_max(
        0.0,
        js_min(1.0, ((px - x1) * ex + (py - y1) * ey) / length_squared),
    );
    (px - (x1 + ex * t)).hypot(py - (y1 + ey * t))
}

fn wall_value_f64(wall: &Value, key: &str) -> f64 {
    number_of(wall.get(key))
}

fn walls_to_cells(walls: &Value) -> Vec<CellWall> {
    walls
        .as_array()
        .map(|list| {
            list.iter()
                .map(|wall| CellWall {
                    x1: wall_value_f64(wall, "x1"),
                    y1: wall_value_f64(wall, "y1"),
                    x2: wall_value_f64(wall, "x2"),
                    y2: wall_value_f64(wall, "y2"),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// `lightPolygon`.
pub fn light_polygon(light: &Value, walls: &Value, grid: &Value, options: &Value) -> Value {
    let columns = js_max(1.0, number_of(grid.get("columns")));
    let rows = js_max(1.0, number_of(grid.get("rows")));
    let radius = number_of(light.get("radius"));
    let reach = if js_truthy(field(light, "dim")) {
        js_max(LIGHT_LIMITS.min_radius, radius / 2.0)
    } else {
        radius
    };
    let feet_per_cell = number_of(grid.get("feetPerCell"));
    let radius_cells = js_max(LIGHT_LIMITS.min_radius, reach / js_max(0.001, feet_per_cell));
    let ox = number_of(light.get("x")) * columns;
    let oy = number_of(light.get("y")) * rows;
    let all_cells = walls_to_cells(walls);
    let segments: Vec<CellWall> = all_cells
        .into_iter()
        .filter(|wall| {
            distance_to_segment(ox, oy, wall.x1, wall.y1, wall.x2, wall.y2) < radius_cells + 1.0
        })
        .collect();

    let mut angles: Vec<f64> = Vec::new();
    let rays = js_max(
        16.0,
        js_round(num_or(options.get("rays"), LIGHT_LIMITS.rays as f64)),
    ) as usize;
    for step in 0..rays {
        angles.push((step as f64 / rays as f64) * std::f64::consts::PI * 2.0);
    }
    for wall in &segments {
        for (px, py) in [(wall.x1, wall.y1), (wall.x2, wall.y2)] {
            let base = (py - oy).atan2(px - ox);
            angles.push(base - 0.0006);
            angles.push(base);
            angles.push(base + 0.0006);
        }
    }
    angles.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));

    let mut points = Vec::with_capacity(angles.len());
    for angle in angles {
        let dx = angle.cos();
        let dy = angle.sin();
        let distance = cast_ray(ox, oy, dx, dy, radius_cells, &segments);
        points.push(json!([ox + dx * distance, oy + dy * distance]));
    }
    json!({ "points": points, "radiusCells": radius_cells })
}

/// `wallsInCells`.
pub fn walls_in_cells(walls: &Value, grid: &Value, mode: Option<&str>) -> Value {
    let columns = js_max(1.0, number_of(grid.get("columns")));
    let rows = js_max(1.0, number_of(grid.get("rows")));
    let Some(list) = walls.as_array() else {
        return Value::Array(Vec::new());
    };
    let kept: Vec<Value> = list
        .iter()
        .filter(|wall| {
            let Some(mode) = mode else {
                return true;
            };
            let door_open = wall
                .get("door")
                .and_then(|door| door.get("open"))
                .map(js_truthy)
                .unwrap_or(false);
            if door_open {
                return false;
            }
            if js_truthy(field(wall, "window")) && mode == "light" {
                return false;
            }
            true
        })
        .map(|wall| {
            json!({
                "x1": number_of(wall.get("x1")) * columns,
                "y1": number_of(wall.get("y1")) * rows,
                "x2": number_of(wall.get("x2")) * columns,
                "y2": number_of(wall.get("y2")) * rows,
            })
        })
        .collect();
    Value::Array(kept)
}

// ---------------------------------------------------------------------------
// Fog
// ---------------------------------------------------------------------------

/// `newFogMask`.
pub fn new_fog_mask() -> String {
    FOG.closed.repeat(FOG.columns * FOG.rows)
}

/// `normalizeFogMask`.
pub fn normalize_fog_mask(raw: &Value) -> String {
    let size = FOG.columns * FOG.rows;
    let mask: String = match raw {
        Value::String(text) => text.chars().filter(|ch| *ch == '0' || *ch == '1').collect(),
        _ => String::new(),
    };
    if mask.len() == size {
        return mask;
    }
    if mask.is_empty() {
        return new_fog_mask();
    }
    let mut padded = mask;
    padded.push_str(&FOG.closed.repeat(size));
    padded.truncate(size);
    padded
}

/// `setFogWhole`.
pub fn set_fog_whole(_mask: &str, open: bool) -> String {
    if open {
        FOG.open.repeat(FOG.columns * FOG.rows)
    } else {
        new_fog_mask()
    }
}

/// `fogHasOpenArea`.
pub fn fog_has_open_area(mask: &Value) -> bool {
    match mask {
        Value::String(text) => text.contains(FOG.open),
        _ => false,
    }
}

/// `openFogAt`.
pub fn open_fog_at(mask: &Value, x: &Value, y: &Value, radius: &Value) -> String {
    let size = FOG.columns * FOG.rows;
    let current = match mask {
        Value::String(text) if text.len() == size => text.clone(),
        _ => normalize_fog_mask(mask),
    };
    let r = js_max(
        0.02,
        js_min(0.5, or_value(js_number(radius), 0.12)),
    );
    let cx = js_max(0.0, js_min(1.0, or_value(js_number(x), 0.0)));
    let cy = js_max(0.0, js_min(1.0, or_value(js_number(y), 0.0)));
    let half_x = js_max(1.0, js_round(r * FOG.columns as f64)) as i64;
    let half_y = js_max(1.0, js_round(r * FOG.rows as f64)) as i64;
    let from_x = js_max(0.0, (cx * FOG.columns as f64).floor() - half_x as f64) as i64;
    let to_x = js_min(
        (FOG.columns - 1) as f64,
        (cx * FOG.columns as f64).ceil() + half_x as f64,
    ) as i64;
    let from_y = js_max(0.0, (cy * FOG.rows as f64).floor() - half_y as f64) as i64;
    let to_y = js_min(
        (FOG.rows - 1) as f64,
        (cy * FOG.rows as f64).ceil() + half_y as f64,
    ) as i64;

    let mut bytes = current.into_bytes();
    for row in from_y..=to_y {
        for column in from_x..=to_x {
            let cell_x = (column as f64 + 0.5) / FOG.columns as f64;
            let cell_y = (row as f64 + 0.5) / FOG.rows as f64;
            let dx = (cell_x - cx) / js_max(1e-6, half_x as f64 / FOG.columns as f64);
            let dy = (cell_y - cy) / js_max(1e-6, half_y as f64 / FOG.rows as f64);
            if dx * dx + dy * dy <= 1.0 {
                let index = (row * FOG.columns as i64 + column) as usize;
                bytes[index] = b'1';
            }
        }
    }
    String::from_utf8(bytes).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Conditions
// ---------------------------------------------------------------------------

/// `conditionBlocksMove`.
pub fn condition_blocks_move(list: &Value) -> Value {
    let Some(list) = list.as_array() else {
        return Value::Null;
    };
    for id in list {
        let id = js_str_or_empty(id);
        if let Some(found) = CONDITIONS.iter().find(|condition| condition.id == id) {
            if found.no_move {
                return serde_json::to_value(found).unwrap_or(Value::Null);
            }
        }
    }
    Value::Null
}

/// `conditionEffects`.
pub fn condition_effects(list: &Value) -> Value {
    let Some(list) = list.as_array() else {
        return Value::Array(Vec::new());
    };
    let mut effects: Vec<&'static str> = Vec::new();
    for id in list {
        let id = js_str_or_empty(id);
        if let Some(found) = CONDITIONS.iter().find(|condition| condition.id == id) {
            if !found.effect.is_empty() && !effects.contains(&found.effect) {
                effects.push(found.effect);
            }
        }
    }
    Value::Array(effects.into_iter().map(|effect| json!(effect)).collect())
}

/// `conditionRollDisadvantage`.
pub fn condition_roll_disadvantage(list: &Value, kind: &str, ability: Option<&str>) -> bool {
    let Some(list) = list.as_array() else {
        return false;
    };
    let active: Vec<String> = list.iter().map(js_str_or_empty).collect();
    let has = |id: &str| active.iter().any(|value| value == id);
    if has("poisoned") && (kind == "check" || kind == "attack") {
        return true;
    }
    if has("blinded") && kind == "attack" {
        return true;
    }
    if has("restrained") && (kind == "attack" || (kind == "save" && ability == Some("dex"))) {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Numbers and exhaustion
// ---------------------------------------------------------------------------

/// `clampNumber`.
pub fn clamp_number(value: &Value, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_null() {
        return fallback;
    }
    if let Value::String(text) = value {
        if text.is_empty() {
            return fallback;
        }
    }
    let number = js_round(js_number(value));
    if !number.is_finite() {
        return fallback;
    }
    js_min(max, js_max(min, number))
}

fn ability_modifier_number(score: f64) -> i64 {
    if !score.is_finite() {
        return 0;
    }
    (score - 10.0).div_euclid(2.0) as i64
}

/// `abilityModifier`.
pub fn ability_modifier(score: &Value) -> i64 {
    ability_modifier_number(js_number(score))
}

fn proficiency_bonus_number(level: f64) -> i64 {
    let level = js_min(20.0, js_max(1.0, js_round(or_value(level, 1.0))));
    2 + (level as i64 - 1) / 4
}

/// `proficiencyBonus`.
pub fn proficiency_bonus(level: &Value) -> i64 {
    proficiency_bonus_number(js_number(level))
}

/// `formatModifier`.
pub fn format_modifier(value: &Value) -> String {
    let number = or_value(js_number(value), 0.0);
    if number >= 0.0 {
        format!("+{}", js_number_to_string(number))
    } else {
        js_number_to_string(number)
    }
}

/// `exhaustionEffects`.
pub fn exhaustion_effects(raw_level: &Value) -> Value {
    let level = clamp_number(raw_level, 0.0, LIMITS.max_exhaustion as f64, 0.0) as i64;
    json!({
        "level": level,
        "checkDisadvantage": level >= 1,
        "speedMultiplier": if level >= 5 { 0.0 } else if level >= 2 { 0.5 } else { 1.0 },
        "attackDisadvantage": level >= 3,
        "saveDisadvantage": level >= 3,
        "maxHpMultiplier": if level >= 4 { 0.5 } else { 1.0 },
        "dead": level >= 6,
    })
}

/// `effectiveSpeed`.
pub fn effective_speed(sheet: &Value) -> f64 {
    let speed = number_of(sheet.get("speed"));
    if !speed.is_finite() {
        return 0.0;
    }
    let multiplier = exhaustion_effects(field(sheet, "exhaustion"))
        .get("speedMultiplier")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    js_max(0.0, (speed * multiplier).floor())
}

/// `effectiveMaxHp`.
pub fn effective_max_hp(sheet: &Value) -> f64 {
    let max_hp = number_of(sheet.get("maxHp"));
    if !max_hp.is_finite() {
        return 0.0;
    }
    let multiplier = exhaustion_effects(field(sheet, "exhaustion"))
        .get("maxHpMultiplier")
        .and_then(Value::as_f64)
        .unwrap_or(1.0);
    js_max(0.0, (max_hp * multiplier).floor())
}

/// `darknessAlpha`.
pub fn darkness_alpha(darkness: &Value, is_dm: bool) -> i64 {
    let value = clamp_number(darkness, 0.0, 1.0, 0.0);
    if value <= 0.0 {
        return 0;
    }
    js_round(value * 255.0 * if is_dm { 0.92 } else { 1.0 }) as i64
}

/// `darknessForHour`.
pub fn darkness_for_hour(hour: &Value) -> f64 {
    let index = js_min(
        (DARKNESS_BY_HOUR.len() - 1) as f64,
        js_max(0.0, js_round(or_value(js_number(hour), 0.0))),
    ) as usize;
    DARKNESS_BY_HOUR[index]
}

// ---------------------------------------------------------------------------
// Sheet computation
// ---------------------------------------------------------------------------

/// `createEmptySheet`.
pub fn create_empty_sheet(player_name: &str) -> Value {
    let mut save_proficiencies = Map::new();
    for key in ABILITY_KEYS {
        save_proficiencies.insert(key.to_string(), Value::Bool(false));
    }
    let mut skill_proficiencies = Map::new();
    let mut skill_expertise = Map::new();
    for skill in SKILLS {
        skill_proficiencies.insert(skill.key.to_string(), Value::Bool(false));
        skill_expertise.insert(skill.key.to_string(), Value::Bool(false));
    }
    let mut abilities = Map::new();
    for key in ABILITY_KEYS {
        abilities.insert(key.to_string(), json!(10));
    }
    json!({
        "playerName": player_name,
        "characterName": "",
        "charClass": "",
        "subclass": "",
        "classes": [],
        "race": "",
        "background": "",
        "alignment": "",
        "level": 1,
        "experience": 0,
        "proficiencyBonusManual": Value::Null,
        "abilities": Value::Object(abilities),
        "saveProficiencies": Value::Object(save_proficiencies),
        "skillProficiencies": Value::Object(skill_proficiencies),
        "skillExpertise": Value::Object(skill_expertise),
        "hp": 10,
        "maxHp": 10,
        "tempHp": 0,
        "deathSavesSuccesses": 0,
        "deathSavesFailures": 0,
        "spellSlotsUsed": [0, 0, 0, 0, 0, 0, 0, 0, 0],
        "spellSlotsMax": [0, 0, 0, 0, 0, 0, 0, 0, 0],
        "hitDice": "1d8",
        "hitDiceSpent": 0,
        "inspiration": 0,
        "exhaustion": 0,
        "concentration": "",
        "coins": { "cp": 0, "sp": 0, "ep": 0, "gp": 0, "pp": 0 },
        "armorClass": 10,
        "initiativeMisc": 0,
        "speed": 30,
        "attackList": [],
        "spells": [],
        "attacks": "",
        "equipment": "",
        "features": "",
        "notes": "",
    })
}

/// `computeSpellSlots`.
pub fn compute_spell_slots(sheet: &Value) -> Value {
    let raw = if sheet.is_object() { sheet } else { &NULL };
    let used = field(raw, "spellSlotsUsed");
    let max = field(raw, "spellSlotsMax");
    let used_list = used.as_array();
    let max_list = max.as_array();
    let automatic = caster_slot_row(raw);
    let automatic_list = automatic.as_array();
    let mut rows = Vec::with_capacity(9);
    let mut any = false;
    for (index, level) in SPELL_SLOT_LEVELS.iter().enumerate() {
        let used_value = js_max(
            0.0,
            js_min(
                LIMITS.max_spell_slots_per_level as f64,
                js_round(num_or(used_list.and_then(|list| list.get(index)), 0.0)),
            ),
        ) as i64;
        let written = js_round(num_or(max_list.and_then(|list| list.get(index)), 0.0)) as i64;
        let max_value = if written > 0 {
            Some(written)
        } else {
            automatic_list
                .and_then(|list| list.get(index))
                .and_then(Value::as_i64)
        };
        if max_value.unwrap_or(0) > 0 || used_value > 0 {
            any = true;
        }
        rows.push(json!({
            "level": level,
            "used": used_value,
            "max": max_value,
        }));
    }
    json!({ "levels": if any { 9 } else { 0 }, "rows": rows })
}

/// `computeVitalState`.
pub fn compute_vital_state(sheet: &Value) -> Value {
    let raw = if sheet.is_object() { sheet } else { &NULL };
    let mut hp = js_number(field(raw, "hp"));
    if !hp.is_finite() {
        hp = 0.0;
    }
    let clamp_death = |value: &Value| -> i64 {
        js_max(
            0.0,
            js_min(
                LIMITS.max_death_saves as f64,
                js_round(num_or(Some(value), 0.0)),
            ),
        ) as i64
    };
    let success = clamp_death(field(raw, "deathSavesSuccesses"));
    let failure = clamp_death(field(raw, "deathSavesFailures"));
    if hp > 0.0 {
        return json!({ "down": false, "success": 0, "failure": 0, "label": "" });
    }
    if failure >= LIMITS.max_death_saves {
        return json!({ "down": true, "success": success, "failure": failure, "label": "Мёртв" });
    }
    if success >= LIMITS.max_death_saves {
        return json!({ "down": true, "success": success, "failure": failure, "label": "Стабилизирован" });
    }
    json!({ "down": true, "success": success, "failure": failure, "label": "При смерти" })
}

fn flag_at(container: Option<&Value>, key: &str) -> bool {
    match container {
        Some(Value::Object(_)) => container
            .and_then(|value| value.get(key))
            .map(js_truthy)
            .unwrap_or(false),
        _ => false,
    }
}

/// `computeDerived`.
pub fn compute_derived(sheet: &Value) -> Value {
    let empty;
    let safe = if sheet.is_object() {
        sheet
    } else {
        empty = create_empty_sheet("");
        &empty
    };
    let level = total_character_level(safe);
    let prof_bonus = match safe.get("proficiencyBonusManual") {
        None | Some(Value::Null) => proficiency_bonus_number(level as f64),
        Some(value) => js_round(num_or(Some(value), 0.0)) as i64,
    };

    let abilities = safe.get("abilities");
    let score = |key: &str| -> f64 {
        match abilities {
            Some(value) if value.is_object() => number_of(value.get(key)),
            Some(value) if js_truthy(value) => f64::NAN,
            _ => 10.0,
        }
    };

    let mut modifiers = Map::new();
    for key in ABILITY_KEYS {
        modifiers.insert(key.to_string(), json!(ability_modifier_number(score(key))));
    }
    let modifier_of = |key: &str| -> i64 {
        modifiers.get(key).and_then(Value::as_i64).unwrap_or(0)
    };

    let mut saves = Map::new();
    for key in ABILITY_KEYS {
        let proficient = flag_at(safe.get("saveProficiencies"), key);
        let base = modifier_of(key);
        saves.insert(
            key.to_string(),
            json!({
                "ability": key,
                "proficient": proficient,
                "base": base,
                "total": base + if proficient { prof_bonus } else { 0 },
            }),
        );
    }

    let mut skills = Map::new();
    for skill in SKILLS {
        let proficient = flag_at(safe.get("skillProficiencies"), skill.key);
        let expertise = proficient && flag_at(safe.get("skillExpertise"), skill.key);
        let bonus = prof_bonus * if expertise { 2 } else if proficient { 1 } else { 0 };
        let base = modifier_of(skill.ability);
        skills.insert(
            skill.key.to_string(),
            json!({
                "key": skill.key,
                "label": skill.label,
                "ability": skill.ability,
                "proficient": proficient,
                "expertise": expertise,
                "base": base,
                "total": base + bonus,
            }),
        );
    }

    let initiative = modifier_of("dex") + js_round(num_or(safe.get("initiativeMisc"), 0.0)) as i64;
    let passive_penalty = if exhaustion_effects(field(safe, "exhaustion"))
        .get("checkDisadvantage")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        5
    } else {
        0
    };
    let perception = skills
        .get("perception")
        .and_then(|skill| skill.get("total"))
        .and_then(Value::as_i64)
        .unwrap_or(0);
    let passive_perception = 10 + perception - passive_penalty;

    json!({
        "level": level,
        "profBonus": prof_bonus,
        "modifiers": Value::Object(modifiers),
        "saves": Value::Object(saves),
        "skills": Value::Object(skills),
        "initiative": initiative,
        "passivePerception": passive_perception,
    })
}

// ---------------------------------------------------------------------------
// Calendar
// ---------------------------------------------------------------------------

fn resolve_calendar_months(months: &Value) -> Vec<String> {
    match months {
        Value::Array(list) if !list.is_empty() => list.iter().map(js_to_string).collect(),
        _ => CALENDAR
            .month_names
            .iter()
            .map(|name| name.to_string())
            .collect(),
    }
}

/// `calendarFromDay`.
pub fn calendar_from_day(raw_day: &Value, months: &Value) -> Value {
    let names = resolve_calendar_months(months);
    let number = or_value(js_number(raw_day), 1.0);
    let total = js_max(0.0, number.floor() - 1.0) as i64;
    let year_length = CALENDAR.days_per_month * names.len() as i64;
    let year = total / year_length + 1;
    let rest = total % year_length;
    let month = rest / CALENDAR.days_per_month + 1;
    let day = rest % CALENDAR.days_per_month + 1;
    let name = names
        .get((month - 1) as usize)
        .cloned()
        .unwrap_or_else(|| format!("Месяц {month}"));
    json!({ "year": year, "month": month, "day": day, "name": name })
}

/// `dayFromCalendar`.
pub fn day_from_calendar(year: &Value, month: &Value, day: &Value, months: &Value) -> i64 {
    let names = resolve_calendar_months(months);
    let clamped = |value: &Value, min: f64, max: f64| -> i64 {
        js_min(max, js_max(min, or_value(js_number(value), 1.0).floor())) as i64
    };
    let y = clamped(year, 1.0, 9999.0);
    let m = clamped(month, 1.0, names.len() as f64);
    let d = clamped(day, 1.0, CALENDAR.days_per_month as f64);
    ((y - 1) * names.len() as i64 + (m - 1)) * CALENDAR.days_per_month + d
}

/// `formatGameTime`.
pub fn format_game_time(time: &Value, months: &Value) -> String {
    let date = calendar_from_day(field(time, "day"), months);
    let hour = js_min(
        23.0,
        js_max(0.0, or_value(js_number(field(time, "hour")), 0.0).floor()),
    ) as i64;
    format!(
        "{} {} {} г., {:02}:00",
        date.get("day").and_then(Value::as_i64).unwrap_or(0),
        date.get("name").and_then(Value::as_str).unwrap_or(""),
        date.get("year").and_then(Value::as_i64).unwrap_or(0),
        hour
    )
}

fn normalize_calendar_name(value: &Value) -> String {
    let text = js_str_or_empty(value).to_lowercase().replace('ё', "е");
    let text: String = text
        .chars()
        .map(|ch| if ch == '.' || ch == ',' { ' ' } else { ch })
        .collect();
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `monthIndexFromValue`.
pub fn month_index_from_value(value: &Value, months: &Value) -> i64 {
    let names = resolve_calendar_months(months);
    let raw = match value {
        Value::Null => String::new(),
        other => js_to_string(other),
    };
    let raw = raw.trim().to_string();
    if !raw.is_empty() && raw.bytes().all(|byte| byte.is_ascii_digit()) {
        let number = raw.parse::<f64>().unwrap_or(f64::NAN);
        return if number >= 1.0 && number <= names.len() as f64 {
            number as i64
        } else {
            -1
        };
    }
    let needle = normalize_calendar_name(&Value::String(raw));
    if needle.is_empty() {
        return -1;
    }
    let mut exact = -1;
    let mut matches = Vec::new();
    for (index, name) in names.iter().enumerate() {
        let normalized = normalize_calendar_name(&Value::String(name.clone()));
        if normalized == needle {
            exact = index as i64 + 1;
        }
        if normalized.starts_with(&needle) || needle.starts_with(&normalized) {
            matches.push(index as i64 + 1);
        }
    }
    if exact > 0 {
        exact
    } else if matches.len() == 1 {
        matches[0]
    } else {
        -1
    }
}

// ---------------------------------------------------------------------------
// Encounters
// ---------------------------------------------------------------------------

pub const CR_XP: [(f64, i64); 34] = [
    (0.0, 10),
    (0.125, 25),
    (0.25, 50),
    (0.5, 100),
    (1.0, 200),
    (2.0, 450),
    (3.0, 700),
    (4.0, 1100),
    (5.0, 1800),
    (6.0, 2300),
    (7.0, 2900),
    (8.0, 3900),
    (9.0, 5000),
    (10.0, 5900),
    (11.0, 7200),
    (12.0, 8400),
    (13.0, 10000),
    (14.0, 11500),
    (15.0, 13000),
    (16.0, 15000),
    (17.0, 18000),
    (18.0, 20000),
    (19.0, 22000),
    (20.0, 25000),
    (21.0, 33000),
    (22.0, 41000),
    (23.0, 50000),
    (24.0, 62000),
    (25.0, 75000),
    (26.0, 90000),
    (27.0, 105000),
    (28.0, 120000),
    (29.0, 135000),
    (30.0, 155000),
];

/// `XP_THRESHOLDS`, index is character level; level 0 is absent.
pub const XP_THRESHOLDS: [Option<[i64; 4]>; 21] = [
    None,
    Some([25, 50, 75, 100]),
    Some([50, 100, 150, 200]),
    Some([75, 150, 225, 400]),
    Some([125, 250, 375, 500]),
    Some([250, 500, 750, 1100]),
    Some([300, 600, 900, 1400]),
    Some([350, 750, 1100, 1700]),
    Some([450, 900, 1400, 2100]),
    Some([550, 1100, 1600, 2400]),
    Some([600, 1200, 1900, 2800]),
    Some([800, 1600, 2400, 3600]),
    Some([1000, 2000, 3000, 4500]),
    Some([1100, 2200, 3400, 5100]),
    Some([1250, 2500, 3800, 5700]),
    Some([1400, 2800, 4300, 6400]),
    Some([1600, 3200, 4800, 7200]),
    Some([2000, 3900, 5900, 8800]),
    Some([2100, 4200, 6300, 9500]),
    Some([2400, 4900, 7300, 10900]),
    Some([2800, 5700, 8500, 12700]),
];

pub const XP_LEVELS: [i64; 20] = [
    0, 300, 900, 2700, 6500, 14000, 23000, 34000, 48000, 64000, 85000, 100000, 120000, 140000,
    165000, 195000, 225000, 265000, 305000, 355000,
];

fn parse_fraction(text: &str) -> Option<f64> {
    let (numerator, denominator) = text.split_once('/')?;
    let numerator = numerator.trim();
    let denominator = denominator.trim();
    if numerator.is_empty()
        || denominator.is_empty()
        || !numerator.bytes().all(|byte| byte.is_ascii_digit())
        || !denominator.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    Some(numerator / denominator)
}

/// `crValue`.
pub fn cr_value(cr: &Value) -> Option<f64> {
    match cr {
        Value::Null => return None,
        Value::Number(number) => {
            let value = number.as_f64()?;
            return (value.is_finite() && (0.0..=30.0).contains(&value)).then_some(value);
        }
        Value::String(text) if text.is_empty() => return None,
        _ => {}
    }
    let text = js_to_string(cr).trim().replacen(',', ".", 1);
    if text.is_empty() {
        return None;
    }
    let value = parse_fraction(&text)
        .or_else(|| text.parse::<f64>().ok())
        .filter(|value| value.is_finite() && (0.0..=30.0).contains(value))?;
    Some(value)
}

/// `monsterXp`.
pub fn monster_xp(cr: &Value) -> i64 {
    let Some(value) = cr_value(cr) else {
        return 0;
    };
    CR_XP
        .iter()
        .find(|(rating, _)| (rating - value).abs() < 1e-9)
        .map(|(_, xp)| *xp)
        .unwrap_or(0)
}

/// `encounterMultiplier`.
pub fn encounter_multiplier(count: &Value) -> f64 {
    let count = js_max(0.0, js_round(or_value(js_number(count), 0.0))) as i64;
    match count {
        i64::MIN..=0 => 0.0,
        1 => 1.0,
        2 => 1.5,
        3..=6 => 2.0,
        7..=10 => 2.5,
        11..=14 => 3.0,
        _ => 4.0,
    }
}

/// `encounterXp`.
pub fn encounter_xp(actors: &Value) -> Value {
    let mut count = 0i64;
    let mut base_xp = 0i64;
    let mut ignored = 0i64;
    if let Some(list) = actors.as_array() {
        for actor in list {
            let number = js_max(
                0.0,
                js_min(
                    20.0,
                    js_round(num_or(actor.get("count"), 0.0)),
                ),
            ) as i64;
            if number == 0 {
                continue;
            }
            let xp = monster_xp(field(actor, "cr"));
            if xp == 0 {
                ignored += number;
                continue;
            }
            count += number;
            base_xp += xp * number;
        }
    }
    let multiplier = encounter_multiplier(&json!(count));
    json!({
        "count": count,
        "baseXp": base_xp,
        "multiplier": number_value(multiplier),
        "adjustedXp": js_round(base_xp as f64 * multiplier) as i64,
        "ignored": ignored,
    })
}

/// `partyThresholds`.
pub fn party_thresholds(levels: &Value) -> Value {
    let mut list = Vec::new();
    if let Some(entries) = levels.as_array() {
        for value in entries {
            let level = js_round(or_value(js_number(value), 0.0)) as i64;
            if (1..=20).contains(&level) {
                list.push(level);
            }
        }
    }
    if list.is_empty() {
        return Value::Null;
    }
    let mut totals = [0i64; 4];
    for level in &list {
        let Some(row) = XP_THRESHOLDS[*level as usize] else {
            continue;
        };
        for (index, value) in row.iter().enumerate() {
            totals[index] += value;
        }
    }
    json!({
        "size": list.len() as i64,
        "easy": totals[0],
        "medium": totals[1],
        "hard": totals[2],
        "deadly": totals[3],
    })
}

/// `xpToNextLevel`. Returns `f64::INFINITY` at level 20, like JS `Infinity`.
pub fn xp_to_next_level(level: &Value) -> f64 {
    let index = js_max(
        1.0,
        js_min(20.0, js_round(or_value(js_number(level), 1.0))),
    ) as usize;
    if index >= 20 {
        return f64::INFINITY;
    }
    XP_LEVELS[index] as f64
}

/// `normalizePins`.
pub fn normalize_pins(raw: &Value) -> Value {
    let Some(list) = raw.as_array() else {
        return Value::Array(Vec::new());
    };
    let mut result = Vec::new();
    for entry in list.iter().take(LIGHT_LIMITS.max_pins) {
        if !entry.is_object() {
            continue;
        }
        let x = number_of(entry.get("x"));
        let y = number_of(entry.get("y"));
        if !x.is_finite() || !y.is_finite() {
            continue;
        }
        let id = truncate_chars(&js_str_or_empty(field(entry, "id")), 40);
        let id = if id.is_empty() {
            format!("pin-{}", result.len())
        } else {
            id
        };
        let title = truncate_chars(&js_str_or_empty(field(entry, "title")), 60);
        let title = if title.is_empty() {
            "Пин".to_string()
        } else {
            title
        };
        result.push(json!({
            "id": id,
            "x": js_max(0.0, js_min(1.0, x)),
            "y": js_max(0.0, js_min(1.0, y)),
            "title": title,
            "text": truncate_chars(&js_str_or_empty(field(entry, "text")), 2000),
        }));
    }
    Value::Array(result)
}

/// `encounterDifficulty`.
pub fn encounter_difficulty(adjusted_xp: &Value, thresholds: &Value) -> Value {
    if !js_truthy(thresholds) {
        return Value::Null;
    }
    let xp = or_value(js_number(adjusted_xp), 0.0);
    let threshold = |key: &str| number_of(thresholds.get(key));
    if xp >= threshold("deadly") {
        json!("deadly")
    } else if xp >= threshold("hard") {
        json!("hard")
    } else if xp >= threshold("medium") {
        json!("medium")
    } else if xp >= threshold("easy") {
        json!("easy")
    } else {
        json!("trivial")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    fn as_f64(number: &Value) -> f64 {
        number.as_f64().unwrap()
    }

    #[test]
    fn tables_match_reference() {
        assert_eq!(ABILITY_KEYS.len(), 6);
        assert_eq!(ability_labels()["cha"], json!("Харизма"));
        assert_eq!(SKILLS.len(), 18);
        assert_eq!(SKILLS[0].key, "acrobatics");
        assert_eq!(SKILLS[17].label, "Выживание");
        assert_eq!(CLASSES.len(), 12);
        assert_eq!(CONDITIONS.len(), 16);
        assert_eq!(SURFACE_TYPES.len(), 8);
        assert_eq!(move_blocking().len(), 6);
        assert_eq!(damage_type_ids().len(), 12);
        assert_eq!(DAMAGE_TYPES[0], ("", "без типа"));
        assert_eq!(LIGHT_PRESETS[11].label, "Дневной свет");
    }

    #[test]
    fn damage_after_defenses_cases() {
        let none = json!({});
        assert_eq!(
            damage_after_defenses(&json!(7), &json!("огонь"), &none),
            json!({ "amount": 7, "kind": "normal" })
        );
        let defenses = json!({ "resistances": ["огонь"] });
        assert_eq!(
            damage_after_defenses(&json!(7), &json!(" ОГОНЬ "), &defenses),
            json!({ "amount": 3, "kind": "resistant" })
        );
        let defenses = json!({ "vulnerabilities": ["огонь"] });
        assert_eq!(
            damage_after_defenses(&json!(7), &json!("огонь"), &defenses),
            json!({ "amount": 14, "kind": "vulnerable" })
        );
        let defenses = json!({ "immunities": ["огонь"] });
        assert_eq!(
            damage_after_defenses(&json!(7), &json!("огонь"), &defenses),
            json!({ "amount": 0, "kind": "immune" })
        );
        let defenses = json!({ "resistances": ["огонь"], "vulnerabilities": ["огонь"] });
        assert_eq!(
            damage_after_defenses(&json!(7), &json!("огонь"), &defenses),
            json!({ "amount": 7, "kind": "normal" })
        );
        assert_eq!(
            damage_after_defenses(&json!("abc"), &json!(""), &none),
            json!({ "amount": 0, "kind": "normal" })
        );
        assert_eq!(
            damage_after_defenses(&json!(-5), &json!("холод"), &none),
            json!({ "amount": 0, "kind": "normal" })
        );
        assert_eq!(
            damage_after_defenses(&json!(2.5), &json!("холод"), &none),
            json!({ "amount": 3, "kind": "normal" })
        );
    }

    #[test]
    fn clamp_number_bounds() {
        assert_eq!(clamp_number(&json!(""), 1.0, 20.0, 10.0), 10.0);
        assert_eq!(clamp_number(&Value::Null, 1.0, 20.0, 10.0), 10.0);
        assert_eq!(clamp_number(&json!("abc"), 1.0, 20.0, 10.0), 10.0);
        assert_eq!(clamp_number(&json!("15"), 1.0, 20.0, 1.0), 15.0);
        assert_eq!(clamp_number(&json!(0.5), 1.0, 20.0, 1.0), 1.0);
        assert_eq!(clamp_number(&json!(-3), 0.0, 6.0, 0.0), 0.0);
        assert_eq!(clamp_number(&json!(99), 0.0, 6.0, 0.0), 6.0);
        assert_eq!(clamp_number(&json!(2.5), 0.0, 6.0, 0.0), 3.0);
        assert_eq!(clamp_number(&json!(-1.5), 0.0, 6.0, 0.0), 0.0);
        assert_eq!(clamp_number(&json!([]), 1.0, 20.0, 10.0), 1.0);
        assert_eq!(clamp_number(&json!([5]), 1.0, 20.0, 10.0), 5.0);
    }

    #[test]
    fn hit_die_sides_edges() {
        assert_eq!(hit_die_sides(&json!("1d8")), Some(8));
        assert_eq!(hit_die_sides(&json!("d10")), Some(10));
        assert_eq!(hit_die_sides(&json!("5d8")), Some(8));
        assert_eq!(hit_die_sides(&json!("d100")), Some(100));
        assert_eq!(hit_die_sides(&json!("d101")), None);
        assert_eq!(hit_die_sides(&json!("d1")), None);
        assert_eq!(hit_die_sides(&json!("abc")), None);
        assert_eq!(hit_die_sides(&json!("")), None);
        assert_eq!(hit_die_sides(&Value::Null), None);
    }

    #[test]
    fn class_helpers() {
        assert_eq!(class_hit_die(&json!("Воин")), Some("d10"));
        assert_eq!(class_hit_die(&json!("  волшебник ")), Some("d6"));
        assert_eq!(class_hit_die(&json!("Паладин")), Some("d10"));
        assert_eq!(class_hit_die(&json!("Некромант")), None);
        assert_eq!(class_hit_die(&json!("")), None);
        assert_eq!(class_hit_die(&Value::Null), None);
    }

    #[test]
    fn normalize_classes_limits() {
        let six = value(
            r#"[
                {"name":"A","level":"abc"},
                {"name":"B","level":99},
                {"name":"C","level":1},
                {"name":"D","level":1},
                {"name":"E","level":1},
                {"name":"F","level":1},
                {"name":"G","level":1}
            ]"#,
        );
        let out = normalize_classes(&six);
        assert_eq!(out.as_array().unwrap().len(), 2);
        assert_eq!(out[0], json!({ "name": "A", "level": 1 }));
        assert_eq!(out[1], json!({ "name": "B", "level": 19 }));

        let long_name = "x".repeat(100);
        let out = normalize_classes(&json!([{ "name": long_name, "level": 1 }]));
        assert_eq!(out[0]["name"].as_str().unwrap().chars().count(), 60);

        let out = normalize_classes(&json!([
            { "name": "A", "level": 15 },
            { "name": "B", "level": 15 },
            { "name": "C", "level": 1 }
        ]));
        assert_eq!(out.as_array().unwrap().len(), 2);
        assert_eq!(out[1], json!({ "name": "B", "level": 5 }));
        assert_eq!(normalize_classes(&json!("nope")), json!([]));
    }

    #[test]
    fn class_levels_fallback() {
        let sheet = json!({ "charClass": "  Паладин ", "level": 99 });
        assert_eq!(class_levels(&sheet), json!([{ "name": "Паладин", "level": 20 }]));
        let sheet = json!({
            "charClass": "Паладин",
            "classes": [{ "name": "Бард", "level": 3 }]
        });
        assert_eq!(class_levels(&sheet), json!([{ "name": "Бард", "level": 3 }]));
        assert_eq!(class_levels(&json!({})), json!([]));
        assert_eq!(class_levels(&Value::Null), json!([]));
    }

    #[test]
    fn totals_and_hit_dice_pool() {
        let sheet = json!({
            "classes": [
                { "name": "Воин", "level": 5 },
                { "name": "Волшебник", "level": 3 }
            ]
        });
        assert_eq!(total_character_level(&sheet), 8);
        assert_eq!(
            hit_dice_pool(&sheet),
            json!([
                { "die": "d10", "sides": 10, "count": 5 },
                { "die": "d6", "sides": 6, "count": 3 }
            ])
        );
        assert_eq!(
            hit_dice_pool(&json!({ "hitDice": "1d8" })),
            json!([{ "die": "d8", "sides": 8, "count": 1 }])
        );
        assert_eq!(hit_dice_pool(&json!({})), json!([]));
        assert_eq!(total_character_level(&json!({ "level": 0 })), 1);
        assert_eq!(total_character_level(&json!({ "level": 30 })), 20);
    }

    #[test]
    fn caster_levels_and_rows() {
        let sheet = json!({
            "classes": [
                { "name": "Волшебник", "level": 5 },
                { "name": "Воин", "level": 5 },
                { "name": "жрец", "level": 2 }
            ]
        });
        assert_eq!(combined_caster_level(&sheet), 7);
        assert_eq!(
            caster_slot_row(&sheet),
            json!([4, 4, 3, 3, 0, 0, 0, 0, 0])
        );
        assert_eq!(caster_slot_row(&json!({})), Value::Null);
        assert_eq!(spell_slot_row(&json!("WIZARD"), &json!(5)), json!([4, 3, 3, 0, 0, 0, 0, 0, 0]));
        assert_eq!(spell_slot_row(&json!("Воин"), &json!(5)), Value::Null);
        assert_eq!(spell_slot_max(&json!("жрец"), &json!(5), &json!(1)), json!(4));
        assert_eq!(spell_slot_max(&json!("жрец"), &json!(5), &json!(9)), json!(0));
        assert_eq!(spell_slot_max(&json!("жрец"), &json!(5), &json!("1")), Value::Null);
        assert_eq!(spell_slot_max(&json!("жрец"), &json!(5), &json!(1.0)), json!(4));
        assert_eq!(spell_slot_max(&json!("жрец"), &json!(5), &json!(10)), Value::Null);
    }

    #[test]
    fn normalize_grid_defaults_and_clamps() {
        let empty = normalize_grid(&json!({}));
        assert_eq!(empty["enabled"], json!(false));
        assert_eq!(empty["cell"], json!(46));
        assert_eq!(empty["columns"], json!(0));
        assert_eq!(empty["feetPerCell"], json!(5));
        assert_eq!(empty["offsetX"], json!(0));
        assert_eq!(empty["offsetY"], json!(0));

        let grid = normalize_grid(&json!({
            "enabled": 1,
            "cell": 900,
            "columns": 37.3,
            "feetPerCell": 0,
            "offsetX": -99999,
            "offsetY": 12.6
        }));
        assert_eq!(grid["enabled"], json!(true));
        assert_eq!(grid["cell"], json!(512));
        assert_eq!(as_f64(&grid["columns"]), 37.5);
        assert_eq!(grid["feetPerCell"], json!(1));
        assert_eq!(grid["offsetX"], json!(-4096));
        assert_eq!(grid["offsetY"], json!(13));

        let grid = normalize_grid(&json!({ "cell": "abc", "columns": -5 }));
        assert_eq!(grid["cell"], json!(46));
        assert_eq!(grid["columns"], json!(0));
        assert_eq!(normalize_grid(&Value::Null)["cell"], json!(46));
    }

    #[test]
    fn round_columns_and_cell_size() {
        assert_eq!(round_columns(&json!(37.3)), 37.5);
        assert_eq!(round_columns(&json!(37.2)), 37.0);
        assert_eq!(round_columns(&json!(-5)), -5.0);
        assert_eq!(round_columns(&json!("abc")), 0.0);
        assert_eq!(grid_cell_size(&json!({ "columns": 4 }), &json!(1000)), 250.0);
        assert_eq!(grid_cell_size(&json!({ "columns": 4 }), &json!(100)), 25.0);
        assert_eq!(grid_cell_size(&json!({ "columns": 0, "cell": 900 }), &json!(0)), 512.0);
        assert_eq!(grid_cell_size(&json!({}), &json!(0)), 46.0);
        assert_eq!(grid_cell_size(&json!({ "cell": 1 }), &json!(0)), 8.0);
    }

    #[test]
    fn normalize_surface_rejects_and_caps() {
        let raw = json!({
            "cells": {
                "0,0": "fire",
                "1,2": "lava",
                "-1,0": "fire",
                "1.5,0": "acid",
                "a,b": "fire",
                "3,4,5": "fire"
            }
        });
        assert_eq!(
            normalize_surface(&raw),
            json!({ "cells": { "0,0": "fire", "1,0": "acid" } })
        );
        assert_eq!(normalize_surface(&json!({})), json!({ "cells": {} }));
        assert_eq!(normalize_surface(&Value::Null), json!({ "cells": {} }));

        let mut cells = Map::new();
        for index in 0..1600 {
            cells.insert(format!("{index},0"), Value::String("fire".to_string()));
        }
        let out = normalize_surface(&json!({ "cells": Value::Object(cells) }));
        assert_eq!(out["cells"].as_object().unwrap().len(), 1500);
    }

    #[test]
    fn normalize_walls_and_lights() {
        let wall = normalize_wall(&json!({
            "x1": -1, "y1": 0, "x2": 2, "y2": 1,
            "id": "0123456789012345678901234567890123456789EXTRA",
            "door": { "open": true },
            "window": true
        }));
        assert_eq!(wall["x1"], json!(0.0));
        assert_eq!(wall["x2"], json!(1.0));
        assert_eq!(wall["id"].as_str().unwrap().chars().count(), 40);
        assert_eq!(wall["door"], json!({ "open": true, "locked": false }));
        assert_eq!(wall["window"], json!(true));
        assert!(normalize_wall(&json!({ "x1": 0, "y1": 0, "x2": 0.0001, "y2": 0 })).is_null());
        assert!(normalize_wall(&json!({ "x1": "abc", "y1": 0, "x2": 1, "y2": 1 })).is_null());
        assert!(normalize_wall(&Value::Null).is_null());

        let list: Vec<Value> = (0..500)
            .map(|index| json!({ "x1": index as f64 / 1000.0, "y1": 0, "x2": 0.5, "y2": 0.5 }))
            .collect();
        assert_eq!(normalize_walls(&json!(list)).as_array().unwrap().len(), 400);
        assert_eq!(normalize_walls(&json!("nope")), json!([]));

        let light = normalize_light(&json!({
            "x": 2, "y": -1, "radius": "abc", "color": "red", "dim": 1
        }));
        assert_eq!(light["x"], json!(1.0));
        assert_eq!(light["y"], json!(0.0));
        assert_eq!(light["radius"], json!(20));
        assert_eq!(light["color"], json!("#ffb347"));
        assert_eq!(light["dim"], json!(true));
        assert_eq!(light["hidden"], json!(false));
        assert!(normalize_light(&json!({ "x": "abc", "y": 0 })).is_null());
        assert_eq!(
            normalize_light(&json!({ "x": 0, "y": 0, "radius": 500, "color": "#AABBCC" }))["radius"],
            json!(200)
        );
        let lights: Vec<Value> = (0..100)
            .map(|index| json!({ "x": index as f64 / 100.0, "y": 0.5 }))
            .collect();
        assert_eq!(normalize_lights(&json!(lights)).as_array().unwrap().len(), 80);
    }

    #[test]
    fn light_grid_and_walls_in_cells() {
        let grid = light_grid(&json!({ "columns": 40, "feetPerCell": 10 }), &json!(2));
        assert_eq!(grid, json!({ "columns": 40, "rows": 20, "feetPerCell": 10 }));
        let grid = light_grid(&json!({ "columns": 0, "feetPerCell": 500 }), &json!(0));
        assert_eq!(grid, json!({ "columns": 200, "rows": 200, "feetPerCell": 50 }));

        let walls = json!([
            { "x1": 0, "y1": 0, "x2": 1, "y2": 1 },
            { "x1": 0, "y1": 0, "x2": 0.5, "y2": 1, "door": { "open": true } },
            { "x1": 0, "y1": 0, "x2": 1, "y2": 0.5, "window": true }
        ]);
        let cells = walls_in_cells(&walls, &json!({ "columns": 10, "rows": 5 }), None);
        assert_eq!(cells.as_array().unwrap().len(), 3);
        assert_eq!(cells[0], json!({ "x1": 0.0, "y1": 0.0, "x2": 10.0, "y2": 5.0 }));

        let light = walls_in_cells(&walls, &json!({ "columns": 10, "rows": 5 }), Some("light"));
        assert_eq!(light.as_array().unwrap().len(), 1);
        let vision = walls_in_cells(&walls, &json!({ "columns": 10, "rows": 5 }), Some("vision"));
        assert_eq!(vision.as_array().unwrap().len(), 2);
    }

    #[test]
    fn cast_ray_and_distance() {
        let walls = [CellWall { x1: 5.0, y1: -1.0, x2: 5.0, y2: 1.0 }];
        assert_eq!(cast_ray(0.0, 0.0, 1.0, 0.0, 10.0, &walls), 5.0);
        assert_eq!(cast_ray(0.0, 0.0, -1.0, 0.0, 10.0, &walls), 10.0);
        let parallel = [CellWall { x1: 0.0, y1: 5.0, x2: 10.0, y2: 5.0 }];
        assert_eq!(cast_ray(0.0, 0.0, 1.0, 0.0, 10.0, &parallel), 10.0);
        assert_eq!(distance_to_segment(0.0, 0.0, 1.0, -1.0, 1.0, 1.0), 1.0);
        assert_eq!(distance_to_segment(0.0, 0.0, 1.0, 1.0, 1.0, 1.0), 2f64.sqrt());
    }

    #[test]
    fn light_polygon_circle() {
        let light = json!({ "x": 0.5, "y": 0.5, "radius": 10, "dim": false });
        let grid = json!({ "columns": 20, "rows": 20, "feetPerCell": 5 });
        let out = light_polygon(&light, &json!([]), &grid, &json!({}));
        let points = out["points"].as_array().unwrap();
        assert_eq!(points.len(), 64);
        assert_eq!(as_f64(&out["radiusCells"]), 2.0);
        for point in points {
            let x = as_f64(&point[0]) - 10.0;
            let y = as_f64(&point[1]) - 10.0;
            assert!((x.hypot(y) - 2.0).abs() < 1e-6);
        }

        let dim = json!({ "x": 0.5, "y": 0.5, "radius": 10, "dim": true });
        let out = light_polygon(&dim, &json!([]), &grid, &json!({ "rays": 16 }));
        assert_eq!(out["points"].as_array().unwrap().len(), 16);
        assert_eq!(as_f64(&out["radiusCells"]), 1.0);
    }

    #[test]
    fn darkness_curve() {
        assert_eq!(darkness_for_hour(&json!(0)), 0.9);
        assert_eq!(darkness_for_hour(&json!(6)), 0.45);
        assert_eq!(darkness_for_hour(&json!(8)), 0.0);
        assert_eq!(darkness_for_hour(&json!(18)), 0.05);
        assert_eq!(darkness_for_hour(&json!(-5)), 0.9);
        assert_eq!(darkness_for_hour(&json!(99)), 0.9);
        assert_eq!(darkness_for_hour(&json!("7.6")), 0.0);
        assert_eq!(darkness_alpha(&json!(0), false), 0);
        assert_eq!(darkness_alpha(&json!(0.4), false), 0);
        assert_eq!(darkness_alpha(&json!(0.5), false), 255);
        assert_eq!(darkness_alpha(&json!(1), true), 235);
        assert_eq!(darkness_alpha(&json!(2), true), 235);
        assert_eq!(darkness_alpha(&json!("abc"), false), 0);
    }

    #[test]
    fn conditions_helpers() {
        let blocked = condition_blocks_move(&json!(["prone", "restrained"]));
        assert_eq!(blocked["id"], json!("restrained"));
        assert_eq!(blocked["noMove"], json!(true));
        assert!(condition_blocks_move(&json!(["poisoned"])).is_null());
        assert!(condition_blocks_move(&json!("nope")).is_null());
        assert_eq!(
            condition_effects(&json!(["blinded", "invisible", "invisible", "bogus"])),
            json!(["blindfold", "veil"])
        );
        assert_eq!(condition_effects(&json!({})), json!([]));
        assert!(condition_roll_disadvantage(&json!(["poisoned"]), "check", None));
        assert!(!condition_roll_disadvantage(&json!(["poisoned"]), "save", None));
        assert!(condition_roll_disadvantage(&json!(["blinded"]), "attack", None));
        assert!(condition_roll_disadvantage(&json!(["restrained"]), "save", Some("dex")));
        assert!(!condition_roll_disadvantage(&json!(["restrained"]), "save", Some("str")));
        assert!(!condition_roll_disadvantage(&json!("nope"), "attack", None));
    }

    #[test]
    fn exhaustion_and_effective_stats() {
        let effects = exhaustion_effects(&json!(3));
        assert_eq!(effects["level"], json!(3));
        assert_eq!(effects["checkDisadvantage"], json!(true));
        assert_eq!(effects["speedMultiplier"], json!(0.5));
        assert_eq!(effects["attackDisadvantage"], json!(true));
        assert_eq!(effects["maxHpMultiplier"], json!(1.0));
        assert_eq!(exhaustion_effects(&json!(99))["dead"], json!(true));
        assert_eq!(exhaustion_effects(&json!(-4))["level"], json!(0));
        assert_eq!(exhaustion_effects(&json!("abc"))["level"], json!(0));
        assert_eq!(exhaustion_effects(&json!(5))["speedMultiplier"], json!(0.0));

        assert_eq!(effective_max_hp(&json!({ "maxHp": 50, "exhaustion": 4 })), 25.0);
        assert_eq!(effective_max_hp(&json!({ "maxHp": 50, "exhaustion": 0 })), 50.0);
        assert_eq!(effective_max_hp(&json!({ "maxHp": "abc" })), 0.0);
        assert_eq!(effective_max_hp(&json!({ "maxHp": 25, "exhaustion": 5 })), 12.0);
        assert_eq!(effective_speed(&json!({ "speed": 30, "exhaustion": 2 })), 15.0);
        assert_eq!(effective_speed(&json!({ "speed": 30, "exhaustion": 5 })), 0.0);
        assert_eq!(effective_speed(&json!({ "speed": 31 })), 31.0);
        assert_eq!(effective_speed(&json!({ "speed": "abc" })), 0.0);
        assert_eq!(ability_modifier(&json!(16)), 3);
        assert_eq!(ability_modifier(&json!(1)), -5);
        assert_eq!(ability_modifier(&json!("abc")), 0);
        assert_eq!(proficiency_bonus(&json!(1)), 2);
        assert_eq!(proficiency_bonus(&json!(5)), 3);
        assert_eq!(proficiency_bonus(&json!(17)), 6);
        assert_eq!(proficiency_bonus(&json!(99)), 6);
        assert_eq!(format_modifier(&json!(3)), "+3");
        assert_eq!(format_modifier(&json!(-2)), "-2");
        assert_eq!(format_modifier(&json!(0)), "+0");
    }

    #[test]
    fn calendar_roundtrip_and_names() {
        assert_eq!(
            calendar_from_day(&json!(37), &Value::Null),
            json!({ "year": 1, "month": 2, "day": 7, "name": "февраля" })
        );
        assert_eq!(
            calendar_from_day(&json!(0), &Value::Null),
            json!({ "year": 1, "month": 1, "day": 1, "name": "января" })
        );
        assert_eq!(day_from_calendar(&json!(1), &json!(2), &json!(7), &Value::Null), 37);
        assert_eq!(day_from_calendar(&json!(2), &json!(1), &json!(1), &Value::Null), 361);
        assert_eq!(format_game_time(&json!({ "day": 37, "hour": 9 }), &Value::Null), "7 февраля 1 г., 09:00");
        assert_eq!(format_game_time(&json!({ "day": 37, "hour": 99 }), &Value::Null), "7 февраля 1 г., 23:00");
        assert_eq!(month_index_from_value(&json!(12), &Value::Null), 12);
        assert_eq!(month_index_from_value(&json!("3"), &Value::Null), 3);
        assert_eq!(month_index_from_value(&json!(" 6 "), &Value::Null), 6);
        assert_eq!(month_index_from_value(&json!("фев"), &Value::Null), 2);
        assert_eq!(month_index_from_value(&json!("февраля"), &Value::Null), 2);
        assert_eq!(month_index_from_value(&json!("ма"), &Value::Null), -1);
        assert_eq!(month_index_from_value(&json!(13), &Value::Null), -1);
        assert_eq!(month_index_from_value(&json!(""), &Value::Null), -1);
        assert_eq!(normalize_calendar_name(&json!("Ёлка, дом")), "елка дом");
        let months = json!(["A", "B"]);
        assert_eq!(
            calendar_from_day(&json!(31), &months),
            json!({ "year": 1, "month": 2, "day": 1, "name": "B" })
        );
    }

    #[test]
    fn vital_and_spell_slots_compute() {
        assert_eq!(
            compute_vital_state(&json!({ "hp": 10 })),
            json!({ "down": false, "success": 0, "failure": 0, "label": "" })
        );
        assert_eq!(
            compute_vital_state(&json!({ "hp": 0, "deathSavesFailures": 3 }))["label"],
            json!("Мёртв")
        );
        assert_eq!(
            compute_vital_state(&json!({ "hp": 0, "deathSavesSuccesses": 99 }))["label"],
            json!("Стабилизирован")
        );
        assert_eq!(
            compute_vital_state(&json!({ "hp": -5, "deathSavesSuccesses": 1, "deathSavesFailures": 1 }))["label"],
            json!("При смерти")
        );

        let slots = compute_spell_slots(&json!({
            "classes": [{ "name": "Волшебник", "level": 5 }]
        }));
        assert_eq!(slots["levels"], json!(9));
        assert_eq!(slots["rows"][0], json!({ "level": 1, "used": 0, "max": 4 }));
        assert_eq!(slots["rows"][2], json!({ "level": 3, "used": 0, "max": 3 }));
        assert_eq!(slots["rows"][3], json!({ "level": 4, "used": 0, "max": 0 }));
        assert_eq!(slots["rows"][8], json!({ "level": 9, "used": 0, "max": 0 }));

        let slots = compute_spell_slots(&json!({
            "spellSlotsUsed": [2],
            "spellSlotsMax": [1]
        }));
        assert_eq!(slots["rows"][0], json!({ "level": 1, "used": 2, "max": 1 }));
        assert_eq!(compute_spell_slots(&json!({}))["levels"], json!(0));
    }

    #[test]
    fn derived_basic() {
        let derived = compute_derived(&json!({
            "level": 5,
            "abilities": { "str": 16, "dex": 14, "con": 10, "int": 10, "wis": 12, "cha": 10 },
            "saveProficiencies": { "str": true },
            "skillProficiencies": { "perception": true },
            "skillExpertise": { "perception": true },
            "initiativeMisc": 2,
            "exhaustion": 1
        }));
        assert_eq!(derived["level"], json!(5));
        assert_eq!(derived["profBonus"], json!(3));
        assert_eq!(derived["modifiers"]["str"], json!(3));
        assert_eq!(derived["modifiers"]["wis"], json!(1));
        assert_eq!(derived["saves"]["str"], json!({
            "ability": "str", "proficient": true, "base": 3, "total": 6
        }));
        assert_eq!(derived["skills"]["perception"]["base"], json!(1));
        assert_eq!(derived["skills"]["perception"]["total"], json!(7));
        assert_eq!(derived["initiative"], json!(4));
        assert_eq!(derived["passivePerception"], json!(12));

        let fallback = compute_derived(&json!({}));
        assert_eq!(fallback["level"], json!(1));
        assert_eq!(fallback["modifiers"]["str"], json!(0));
    }

    #[test]
    fn encounter_math() {
        assert_eq!(cr_value(&json!(0.5)), Some(0.5));
        assert_eq!(cr_value(&json!("1/2")), Some(0.5));
        assert_eq!(cr_value(&json!("1/8")), Some(0.125));
        assert_eq!(cr_value(&json!("0,5")), Some(0.5));
        assert_eq!(cr_value(&json!(" 3 ")), Some(3.0));
        assert_eq!(cr_value(&json!(31)), None);
        assert_eq!(cr_value(&json!(-1)), None);
        assert_eq!(cr_value(&json!("")), None);
        assert_eq!(cr_value(&json!("abc")), None);
        assert_eq!(cr_value(&Value::Null), None);

        assert_eq!(monster_xp(&json!("1/2")), 100);
        assert_eq!(monster_xp(&json!(0.125)), 25);
        assert_eq!(monster_xp(&json!(3)), 700);
        assert_eq!(monster_xp(&json!("bad")), 0);

        assert_eq!(encounter_multiplier(&json!(0)), 0.0);
        assert_eq!(encounter_multiplier(&json!(1)), 1.0);
        assert_eq!(encounter_multiplier(&json!(2)), 1.5);
        assert_eq!(encounter_multiplier(&json!(3)), 2.0);
        assert_eq!(encounter_multiplier(&json!(7)), 2.5);
        assert_eq!(encounter_multiplier(&json!(11)), 3.0);
        assert_eq!(encounter_multiplier(&json!(15)), 4.0);

        assert_eq!(
            encounter_xp(&json!([
                { "cr": "1/2", "count": 2 },
                { "cr": "bad", "count": 3 }
            ])),
            json!({ "count": 2, "baseXp": 200, "multiplier": 1.5, "adjustedXp": 300, "ignored": 3 })
        );
        assert_eq!(
            party_thresholds(&json!([1, 1])),
            json!({ "size": 2, "easy": 50, "medium": 100, "hard": 150, "deadly": 200 })
        );
        assert_eq!(party_thresholds(&json!([0, 21, "x"])), Value::Null);
        assert_eq!(xp_to_next_level(&json!(1)), 300.0);
        assert_eq!(xp_to_next_level(&json!(19)), 355000.0);
        assert!(xp_to_next_level(&json!(20)).is_infinite());
        assert!(xp_to_next_level(&json!(25)).is_infinite());

        let thresholds = json!({ "easy": 50, "medium": 100, "hard": 150, "deadly": 200 });
        assert_eq!(encounter_difficulty(&json!(250), &thresholds), json!("deadly"));
        assert_eq!(encounter_difficulty(&json!(150), &thresholds), json!("hard"));
        assert_eq!(encounter_difficulty(&json!(100), &thresholds), json!("medium"));
        assert_eq!(encounter_difficulty(&json!(50), &thresholds), json!("easy"));
        assert_eq!(encounter_difficulty(&json!(10), &thresholds), json!("trivial"));
        assert_eq!(encounter_difficulty(&json!(10), &Value::Null), Value::Null);
    }

    #[test]
    fn normalize_pins_defaults() {
        let pins = normalize_pins(&json!([
            { "x": 0.5, "y": 2 },
            { "x": "bad", "y": 0 },
            { "x": 0, "y": 0, "id": "", "title": "", "text": "hi" }
        ]));
        let pins = pins.as_array().unwrap();
        assert_eq!(pins.len(), 2);
        assert_eq!(pins[0], json!({
            "id": "pin-0", "x": 0.5, "y": 1.0, "title": "Пин", "text": ""
        }));
        assert_eq!(pins[1], json!({
            "id": "pin-1", "x": 0.0, "y": 0.0, "title": "Пин", "text": "hi"
        }));
        assert_eq!(normalize_pins(&json!("nope")), json!([]));
    }

    #[test]
    fn fog_helpers() {
        let mask = new_fog_mask();
        assert_eq!(mask.len(), 2560);
        assert!(mask.chars().all(|ch| ch == '0'));
        assert!(!fog_has_open_area(&json!(mask)));
        assert!(fog_has_open_area(&json!("0001")));
        assert!(!fog_has_open_area(&Value::Null));

        let normalized = normalize_fog_mask(&json!("10x1"));
        assert_eq!(normalized.len(), 2560);
        assert!(normalized.starts_with("101"));
        assert_eq!(set_fog_whole(&mask, true).len(), 2560);
        assert!(fog_has_open_area(&json!(set_fog_whole(&mask, true))));
        assert_eq!(set_fog_whole(&mask, false), mask);

        let opened = open_fog_at(&json!(mask), &json!(0.5), &json!(0.5), &json!(0.12));
        assert_eq!(opened.len(), 2560);
        assert_eq!(opened.as_bytes()[20 * 64 + 32], b'1');
        assert_eq!(&opened[..40], &"0".repeat(40));
        let twice = open_fog_at(&json!(opened), &json!(0.5), &json!(0.5), &json!(0.12));
        assert_eq!(opened, twice);
        let edge = open_fog_at(&json!(mask), &json!(0), &json!(0), &json!(0.02));
        assert_eq!(edge.as_bytes()[0], b'1');
    }

    #[test]
    fn empty_sheet_defaults() {
        let sheet = create_empty_sheet("");
        assert_eq!(sheet["playerName"], json!(""));
        assert_eq!(sheet["level"], json!(1));
        assert_eq!(sheet["abilities"]["str"], json!(10));
        assert_eq!(sheet["saveProficiencies"].as_object().unwrap().len(), 6);
        assert_eq!(sheet["skillProficiencies"].as_object().unwrap().len(), 18);
        assert_eq!(sheet["skillExpertise"].as_object().unwrap().len(), 18);
        assert_eq!(sheet["spellSlotsUsed"], json!([0, 0, 0, 0, 0, 0, 0, 0, 0]));
        assert_eq!(sheet["hitDice"], json!("1d8"));
        assert_eq!(sheet["coins"]["cp"], json!(0));
        assert_eq!(sheet["proficiencyBonusManual"], Value::Null);
        assert_eq!(create_empty_sheet("Аня")["playerName"], json!("Аня"));

        assert_eq!(map_labels()["sketch"], json!("Рисунок"));
        assert_eq!(CALENDAR.days_per_month, 30);
    }
}
