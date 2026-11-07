//! JS value semantics the importer relies on.
//!
//! The reference treats documents as plain objects and leans on `String()`,
//! `Number()`, truthiness and `Math.round`. These helpers keep the port honest
//! without pulling in a JS engine.

use serde_json::{Number, Value};

/// `String(value)` for the JSON shapes Foundry documents carry.
pub fn string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => text.clone(),
        Value::Array(items) => items.iter().map(string).collect::<Vec<_>>().join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// `String(value || fallback)`.
pub fn string_or(value: Option<&Value>, fallback: &str) -> String {
    match value {
        Some(value) if truthy(value) => string(value),
        _ => fallback.to_string(),
    }
}

/// JS truthiness for JSON values.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|num| num != 0.0 && !num.is_nan()),
        Value::String(text) => !text.is_empty(),
        _ => true,
    }
}

/// `Number(value)`; `NaN` when JS would produce `NaN`.
pub fn number(value: &Value) -> f64 {
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
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return 0.0;
            }
            // shortcut: hex/octal/binary strings are NaN here, JS Number() reads them.
            trimmed.parse::<f64>().unwrap_or(f64::NAN)
        }
        Value::Array(items) => match items.as_slice() {
            [] => 0.0,
            [one] => number(one),
            _ => f64::NAN,
        },
        Value::Object(_) => f64::NAN,
    }
}

/// `Number(value) || 0`.
pub fn number_or_zero(value: Option<&Value>) -> f64 {
    match value {
        Some(value) => {
            let number = number(value);
            if number.is_finite() && number != 0.0 {
                number
            } else {
                0.0
            }
        }
        None => 0.0,
    }
}

/// `Math.round`.
pub fn round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// JSON has no NaN or infinity: `JSON.stringify` writes `null`.
pub fn number_to_value(value: f64) -> Value {
    if !value.is_finite() {
        return Value::Null;
    }
    if value.fract() == 0.0 && value >= i64::MIN as f64 && value <= i64::MAX as f64 {
        return json_i64(value as i64);
    }
    Number::from_f64(value).map(Value::Number).unwrap_or(Value::Null)
}

fn json_i64(value: i64) -> Value {
    Value::Number(Number::from(value))
}

/// `value[key]` on an object; `None` for anything else.
pub fn get<'a>(value: &'a Value, key: &str) -> Option<&'a Value> {
    value.as_object().and_then(|map| map.get(key))
}

/// `value && value[key]`.
pub fn get_truthy<'a>(value: Option<&'a Value>, key: &str) -> Option<&'a Value> {
    get(value?, key).filter(|item| truthy(item))
}

pub fn as_array(value: Option<&Value>) -> &[Value] {
    match value {
        Some(Value::Array(items)) => items,
        _ => &[],
    }
}

/// `a ?? b ?? c`: first value that is neither null nor missing.
pub fn nullish<'a>(values: &[Option<&'a Value>]) -> Option<&'a Value> {
    values.iter().find_map(|value| match value {
        Some(Value::Null) | None => None,
        Some(value) => Some(*value),
    })
}

/// `str.slice(0, max)`, counting UTF-16 units like JS.
pub fn truncate_utf16(text: &str, max_length: usize) -> String {
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

/// `n.toString(36)`.
pub fn base36(mut value: u64) -> String {
    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(DIGITS[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).expect("base36 is ASCII")
}
