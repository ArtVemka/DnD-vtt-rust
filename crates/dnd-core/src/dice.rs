//! Dice formula parsing and rolling. Port of `renderer/dice.js`.
//!
//! The random source is injected because the desktop uses `crypto.randomInt`
//! and the training table uses a plain RNG. Parsing is pure and deterministic.

use serde::Serialize;
use serde_json::Value;

/// Real D&D dice. Mirrors `DICE_SIDES` in `server/state.js`.
pub const DICE_SIDES: [u32; 7] = [4, 6, 8, 10, 12, 20, 100];

pub const MAX_COUNT: u32 = 50;
pub const MAX_MODIFIER: i32 = 999;
pub const MAX_RESULT: i64 = 9999;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum KeepMode {
    Kh,
    Kl,
    Dh,
    Dl,
}

impl KeepMode {
    pub fn as_str(self) -> &'static str {
        match self {
            KeepMode::Kh => "kh",
            KeepMode::Kl => "kl",
            KeepMode::Dh => "dh",
            KeepMode::Dl => "dl",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "kh" => Some(KeepMode::Kh),
            "kl" => Some(KeepMode::Kl),
            "dh" => Some(KeepMode::Dh),
            "dl" => Some(KeepMode::Dl),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Keep {
    pub mode: KeepMode,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedFormula {
    pub source: String,
    pub normalized: String,
    pub count: u32,
    pub sides: u32,
    pub keep: Option<Keep>,
    pub modifier: i32,
}

/// Parse error. `to_string` is shown to the player verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ParseError {}

fn error(message: impl Into<String>) -> ParseError {
    ParseError(message.into())
}

fn take_digits<'a>(bytes: &'a [u8], i: &mut usize) -> &'a [u8] {
    let start = *i;
    while *i < bytes.len() && bytes[*i].is_ascii_digit() {
        *i += 1;
    }
    &bytes[start..*i]
}

fn number_or_max(text: &[u8]) -> u64 {
    std::str::from_utf8(text)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(u64::MAX)
}

/// Parses `2d6+3`, `4d6dl1`, `2d20kh1+7`, `d100`. Whitespace is ignored.
pub fn parse_formula(text: &str) -> Result<ParsedFormula, ParseError> {
    let source = text.trim().to_string();
    let compact: String = source
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if compact.is_empty() {
        return Err(error("Введите формулу броска, например 2d6+3"));
    }

    let invalid = || {
        error(format!(
            "Не понял формулу «{source}». Примеры: 2d6+3, 4d6dl1, d20, 2d20kh1"
        ))
    };

    if !compact.is_ascii() {
        return Err(invalid());
    }
    let bytes = compact.as_bytes();
    let mut i = 0;

    let count_bytes = take_digits(bytes, &mut i);
    if bytes.get(i) != Some(&b'd') {
        return Err(invalid());
    }
    i += 1;

    let sides_bytes = take_digits(bytes, &mut i);
    if sides_bytes.is_empty() {
        return Err(invalid());
    }

    let mut keep = None;
    if i + 1 < bytes.len() {
        let mode_text = &compact[i..i + 2];
        if let Some(mode) = KeepMode::from_str(mode_text) {
            i += 2;
            let value_bytes = take_digits(bytes, &mut i);
            if value_bytes.is_empty() {
                return Err(invalid());
            }
            keep = Some(Keep {
                mode,
                value: number_or_max(value_bytes) as u32,
            });
        }
    }

    let mut modifier: i32 = 0;
    if i < bytes.len() {
        let sign = match bytes[i] {
            b'+' => 1,
            b'-' => -1,
            _ => return Err(invalid()),
        };
        i += 1;
        let modifier_bytes = take_digits(bytes, &mut i);
        if modifier_bytes.is_empty() {
            return Err(invalid());
        }
        let magnitude = number_or_max(modifier_bytes);
        modifier = sign * (magnitude.min(i32::MAX as u64) as i32);
    }

    if i != bytes.len() {
        return Err(invalid());
    }

    let count = if count_bytes.is_empty() {
        1
    } else {
        number_or_max(count_bytes)
    };
    let count = u32::try_from(count).unwrap_or(u32::MAX);
    let sides = number_or_max(sides_bytes);

    if !(1..=MAX_COUNT).contains(&count) {
        return Err(error(format!(
            "Костей в броске должно быть от 1 до {MAX_COUNT}"
        )));
    }
    if !DICE_SIDES.iter().any(|side| u64::from(*side) == sides) {
        let list = DICE_SIDES
            .iter()
            .map(|side| format!("d{side}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(error(format!(
            "Костей d{sides} не бывает. Доступны {list}"
        )));
    }
    if let Some(keep) = keep {
        if keep.value < 1 || keep.value > count {
            return Err(error(format!(
                "«{}{}» не имеет смысла при {count} костях в броске",
                keep.mode.as_str(),
                keep.value
            )));
        }
    }
    if modifier.abs() > MAX_MODIFIER {
        return Err(error(format!(
            "Модуль должен быть от {} до {MAX_MODIFIER}",
            -MAX_MODIFIER
        )));
    }

    Ok(ParsedFormula {
        source,
        normalized: compact,
        count,
        sides: sides as u32,
        keep,
        modifier,
    })
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// Result of a roll. Field names must match the JS object the renderer reads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Roll {
    pub formula: String,
    pub count: u32,
    pub sides: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kept_values: Option<Vec<u32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dropped_values: Option<Vec<u32>>,
    pub modifier: i32,
    #[serde(default, skip_serializing_if = "is_false")]
    pub hidden: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<i64>,
}

/// Rolls a parsed formula. `random_int(min, max)` must be inclusive.
pub fn roll_formula(
    parsed: &ParsedFormula,
    random_int: &mut dyn FnMut(u32, u32) -> u32,
) -> Roll {
    let values: Vec<u32> = (0..parsed.count)
        .map(|_| random_int(1, parsed.sides).clamp(1, parsed.sides))
        .collect();

    // Keep the rolled order: history shows the real sequence, not a sorted copy.
    let mut kept = values.clone();
    let mut dropped = Vec::new();
    if let Some(keep) = parsed.keep {
        let mut by_value = values.clone();
        by_value.sort_unstable_by(|a, b| b.cmp(a));
        let size = keep.value as usize;
        match keep.mode {
            KeepMode::Kh => kept = by_value[..size].to_vec(),
            KeepMode::Kl => kept = by_value[by_value.len() - size..].to_vec(),
            KeepMode::Dh => {
                kept = by_value[size..].to_vec();
                dropped = by_value[..size].to_vec();
            }
            KeepMode::Dl => {
                dropped = by_value[by_value.len() - size..].to_vec();
                kept = by_value[..by_value.len() - size].to_vec();
            }
        }
        dropped.sort_unstable();
    }

    let sum: i64 = kept.iter().map(|value| i64::from(*value)).sum();
    let result = (sum + i64::from(parsed.modifier)).clamp(-MAX_RESULT, MAX_RESULT);

    Roll {
        formula: parsed.normalized.clone(),
        count: parsed.count,
        sides: parsed.sides,
        values: Some(values),
        kept_values: Some(kept),
        dropped_values: Some(dropped),
        modifier: parsed.modifier,
        hidden: false,
        result: Some(result),
    }
}

/// Chat line for a roll. The author name is not included on purpose.
pub fn describe_roll(roll: &Roll) -> String {
    let head = format!("бросил {}", roll.formula);
    if roll.hidden {
        return format!("{head} тайно");
    }
    let mut text = format!(
        "{head}: {}",
        roll.result.map(|value| value.to_string()).unwrap_or_default()
    );
    if let Some(dropped) = &roll.dropped_values {
        if !dropped.is_empty() {
            let list = dropped
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" + ");
            text.push_str(&format!(" (отброшено {list})"));
        }
    }
    text
}

/// Roll as players see it when the DM rolled secretly: numbers are removed
/// server-side so they cannot leak through a state snapshot.
pub fn hide_roll_result(roll: &Roll) -> Roll {
    if !roll.hidden {
        return roll.clone();
    }
    Roll {
        values: None,
        kept_values: None,
        dropped_values: None,
        result: None,
        ..roll.clone()
    }
}

/// Keeps a hidden roll's shape but removes the numbers, so they cannot leak
/// through the state snapshot a player receives. Same as `hideRollResult`.
pub fn hide_roll_value(roll: &Value) -> Value {
    if !roll.get("hidden").and_then(Value::as_bool).unwrap_or(false) {
        return roll.clone();
    }
    let mut out = roll.as_object().cloned().unwrap_or_default();
    out.remove("result");
    out.remove("values");
    out.remove("keptValues");
    out.remove("droppedValues");
    out.insert("result".to_string(), Value::Null);
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(formula: &str) -> ParsedFormula {
        parse_formula(formula).expect("formula should parse")
    }

    /// Deterministic RNG: returns the next value from the list.
    fn sequence(values: &[u32]) -> impl FnMut(u32, u32) -> u32 + '_ {
        let mut index = 0;
        move |min, max| {
            let value = values[index].clamp(min, max);
            index += 1;
            value
        }
    }

    #[test]
    fn parses_plain_formula() {
        let formula = parsed("2d6+3");
        assert_eq!(formula.count, 2);
        assert_eq!(formula.sides, 6);
        assert_eq!(formula.modifier, 3);
        assert_eq!(formula.keep, None);
        assert_eq!(formula.normalized, "2d6+3");
    }

    #[test]
    fn trims_spaces_and_defaults_count() {
        let formula = parsed("  D 20  ");
        assert_eq!(formula.count, 1);
        assert_eq!(formula.sides, 20);
        assert_eq!(formula.source, "D 20");
        assert_eq!(formula.normalized, "d20");
    }

    #[test]
    fn parses_keep_and_drop() {
        assert_eq!(parsed("4d6dl1").keep, Some(Keep { mode: KeepMode::Dl, value: 1 }));
        assert_eq!(parsed("2d20kh1+7").keep, Some(Keep { mode: KeepMode::Kh, value: 1 }));
        assert_eq!(parsed("2d20kl1").keep, Some(Keep { mode: KeepMode::Kl, value: 1 }));
        assert_eq!(parsed("3d8dh2").keep, Some(Keep { mode: KeepMode::Dh, value: 2 }));
    }

    #[test]
    fn rejects_bad_input() {
        assert_eq!(
            parse_formula("  ").unwrap_err().0,
            "Введите формулу броска, например 2d6+3"
        );
        assert_eq!(
            parse_formula("d7").unwrap_err().0,
            "Костей d7 не бывает. Доступны d4, d6, d8, d10, d12, d20, d100"
        );
        assert_eq!(
            parse_formula("0d6").unwrap_err().0,
            "Костей в броске должно быть от 1 до 50"
        );
        assert_eq!(
            parse_formula("51d6").unwrap_err().0,
            "Костей в броске должно быть от 1 до 50"
        );
        assert_eq!(
            parse_formula("2d6+1000").unwrap_err().0,
            "Модуль должен быть от -999 до 999"
        );
        assert_eq!(
            parse_formula("4d6dl5").unwrap_err().0,
            "«dl5» не имеет смысла при 4 костях в броске"
        );
        assert!(parse_formula("2d6+3x").unwrap_err().0.starts_with("Не понял формулу «2d6+3x»"));
    }

    #[test]
    fn drops_lowest_die() {
        let roll = roll_formula(&parsed("4d6dl1"), &mut sequence(&[3, 6, 2, 5]));
        assert_eq!(roll.values, Some(vec![3, 6, 2, 5]));
        assert_eq!(roll.dropped_values, Some(vec![2]));
        assert_eq!(roll.kept_values, Some(vec![6, 5, 3]));
        assert_eq!(roll.result, Some(14));
    }

    #[test]
    fn keeps_highest_die_with_modifier() {
        let roll = roll_formula(&parsed("2d20kh1+7"), &mut sequence(&[3, 6]));
        assert_eq!(roll.kept_values, Some(vec![6]));
        assert_eq!(roll.result, Some(13));
    }

    #[test]
    fn keeps_lowest_dice() {
        let roll = roll_formula(&parsed("4d6kl2"), &mut sequence(&[3, 6, 2, 5]));
        assert_eq!(roll.kept_values, Some(vec![3, 2]));
        assert_eq!(roll.result, Some(5));
    }

    #[test]
    fn drops_highest_dice() {
        let roll = roll_formula(&parsed("4d6dh2"), &mut sequence(&[3, 6, 2, 5]));
        assert_eq!(roll.dropped_values, Some(vec![5, 6]));
        assert_eq!(roll.kept_values, Some(vec![3, 2]));
        assert_eq!(roll.result, Some(5));
    }

    #[test]
    fn describes_roll() {
        let roll = roll_formula(&parsed("4d6dl1"), &mut sequence(&[3, 6, 2, 5]));
        assert_eq!(describe_roll(&roll), "бросил 4d6dl1: 14 (отброшено 2)");

        let secret = Roll {
            hidden: true,
            ..roll
        };
        assert_eq!(describe_roll(&secret), "бросил 4d6dl1 тайно");
    }

    #[test]
    fn hides_secret_numbers() {
        let secret = Roll {
            hidden: true,
            ..roll_formula(&parsed("2d6"), &mut sequence(&[2, 5]))
        };
        let hidden = hide_roll_result(&secret);
        assert_eq!(hidden.result, None);
        assert_eq!(hidden.values, None);
        assert_eq!(hidden.kept_values, None);
        assert_eq!(hidden.dropped_values, None);
        assert_eq!(hidden.formula, "2d6");

        let public = Roll {
            hidden: false,
            ..secret
        };
        assert_eq!(hide_roll_result(&public).result, Some(7));
    }

    #[test]
    fn serializes_camel_case_like_js() {
        let roll = roll_formula(&parsed("2d6+1"), &mut sequence(&[2, 5]));
        let json = serde_json::to_value(&roll).unwrap();
        assert_eq!(json["formula"], "2d6+1");
        assert_eq!(json["keptValues"], serde_json::json!([2, 5]));
        assert_eq!(json["droppedValues"], serde_json::json!([]));
        assert_eq!(json["result"], 8);
        assert!(json.get("hidden").is_none());
    }
}
