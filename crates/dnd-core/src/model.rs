//! Constants shared by the storage and server layers.
//!
//! Numeric limits and text sanitization live in `sheet.rs`; this module keeps
//! only what has no better home.

use serde_json::{Value, json};

/// `CAMPAIGN_FORMAT_VERSION` in `server/state.js`.
pub const CAMPAIGN_FORMAT_VERSION: i64 = 2;

/// Map slots. Order matches `MAP_TYPES` in the reference.
pub const MAP_TYPES: [&str; 3] = ["world", "battle", "sketch"];

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
    fn maps_have_three_slots() {
        let maps = empty_maps();
        for kind in MAP_TYPES {
            assert_eq!(maps[kind]["items"], json!([]));
            assert_eq!(maps[kind]["activeId"], Value::Null);
        }
    }
}
