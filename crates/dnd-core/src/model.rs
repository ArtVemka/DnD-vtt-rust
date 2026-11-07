//! Constants and pure helpers from `server/state.js`.
//!
//! Only what the storage layer needs right now; the rest lives in `sheet.rs`
//! and `rules.rs` and will move here once both ports settle.

use serde_json::{json, Value};

/// `CAMPAIGN_FORMAT_VERSION` in `server/state.js`.
pub const CAMPAIGN_FORMAT_VERSION: i64 = 2;

/// Map slots. Order matches `MAP_TYPES` in the reference.
pub const MAP_TYPES: [&str; 3] = ["world", "battle", "sketch"];

/// `sanitizeText(value, maxLength)`: stringify, drop NULs, cut to length.
pub fn sanitize_text(value: &Value, max_length: usize) -> String {
    let text = match value {
        Value::Null => return String::new(),
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        other => other.to_string(),
    };
    let text = text.replace('\0', "");
    text.chars().take(max_length).collect()
}

/// `emptyMaps()`: one empty slot per map type.
pub fn empty_maps() -> Value {
    let mut maps = serde_json::Map::new();
    for kind in MAP_TYPES {
        maps.insert(kind.to_string(), json!({ "items": [], "activeId": null }));
    }
    Value::Object(maps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_and_cuts_text() {
        assert_eq!(sanitize_text(&Value::Null, 10), "");
        assert_eq!(sanitize_text(&json!("  имя  "), 10), "  имя  ");
        assert_eq!(sanitize_text(&json!("abcdef"), 3), "abc");
        assert_eq!(sanitize_text(&json!(12), 10), "12");
        assert_eq!(sanitize_text(&json!(true), 10), "true");
        assert_eq!(sanitize_text(&json!("a\0b"), 10), "ab");
    }

    #[test]
    fn maps_have_three_slots() {
        let maps = empty_maps();
        for kind in MAP_TYPES {
            assert_eq!(maps[kind]["items"], json!([]));
            assert_eq!(maps[kind]["activeId"], Value::Null);
        }
    }
}
