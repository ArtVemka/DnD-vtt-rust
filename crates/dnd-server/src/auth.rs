//! Connection helpers from `server/socket.js`: grid equality, loopback check,
//! game-time arithmetic, constant-time secret compare, name sanitizing.

use dnd_core::sheet::{LIMITS, sanitize_text};
use serde_json::Value;

/// `sameGrid`: strict equality of the six grid fields.
pub fn same_grid(before: &Value, after: &Value) -> bool {
    if !before.is_object() || !after.is_object() {
        return false;
    }
    [
        "enabled",
        "cell",
        "columns",
        "feetPerCell",
        "offsetX",
        "offsetY",
    ]
    .iter()
    .all(|key| before.get(*key) == after.get(*key))
}

/// `isLoopbackAddress`. Only literal loopback forms are accepted.
pub fn is_loopback_address(address: &str) -> bool {
    address == "127.0.0.1"
        || address == "::1"
        || address == "::ffff:127.0.0.1"
        || address.starts_with("127.")
        || address.starts_with("::ffff:127.")
}

/// Hours of in-game time between two `{day, hour}` points.
pub fn hours_between(before: (i64, i64), after: (i64, i64)) -> i64 {
    (after.0 - before.0) * 24 + (after.1 - before.1)
}

/// `addHours`: wraps hours inside the day, days start at 1.
pub fn add_hours(time: (i64, i64), hours: i64) -> (i64, i64) {
    let total = (time.0 - 1) * 24 + time.1 + hours;
    (total.div_euclid(24) + 1, total.rem_euclid(24))
}

/// `safeEqual`: equal length, non-empty, compare without early exit.
pub fn safe_equal(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.is_empty() || left.len() != right.len() {
        return false;
    }
    let mut diff = 0u8;
    for (a, b) in left.iter().zip(right) {
        diff |= a ^ b;
    }
    diff == 0
}

/// `sanitizeName`: sanitize, collapse whitespace runs, trim.
pub fn sanitize_name(raw: &Value) -> String {
    let text = sanitize_text(raw, LIMITS.max_name_length);
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            if !in_space {
                out.push(' ');
                in_space = true;
            }
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grid_equality_is_strict() {
        let a = json!({"enabled": true, "cell": 46, "columns": 0});
        let b = json!({"enabled": true, "cell": 46, "columns": 0});
        assert!(same_grid(&a, &b));
        assert!(!same_grid(
            &a,
            &json!({"enabled": true, "cell": 47, "columns": 0})
        ));
        assert!(!same_grid(&a, &Value::Null));
    }

    #[test]
    fn loopback_forms() {
        assert!(is_loopback_address("127.0.0.1"));
        assert!(is_loopback_address("127.0.0.2"));
        assert!(is_loopback_address("::1"));
        assert!(is_loopback_address("::ffff:127.0.0.1"));
        assert!(!is_loopback_address("192.168.1.5"));
        assert!(!is_loopback_address("128.0.0.1"));
        // JS accepts anything starting with "127."; addresses here are IPs.
        assert!(is_loopback_address("127.example.com"));
        assert!(!is_loopback_address(""));
    }

    #[test]
    fn game_time_math() {
        assert_eq!(hours_between((1, 8), (2, 10)), 26);
        assert_eq!(add_hours((1, 23), 2), (2, 1));
        assert_eq!(add_hours((2, 1), -2), (1, 23));
        assert_eq!(add_hours((1, 0), 24), (2, 0));
        assert_eq!(add_hours((1, 0), -1), (0, 23));
    }

    #[test]
    fn secrets_compare_safely() {
        assert!(safe_equal("abc", "abc"));
        assert!(!safe_equal("abc", "abd"));
        assert!(!safe_equal("abc", "abcd"));
        assert!(!safe_equal("", ""));
    }

    #[test]
    fn names_collapse_whitespace() {
        assert_eq!(sanitize_name(&json!("  Иван \n Петров  ")), "Иван Петров");
        assert_eq!(sanitize_name(&json!("")), "");
        assert_eq!(sanitize_name(&Value::Null), "");
        assert_eq!(sanitize_name(&json!("a".repeat(100))).chars().count(), 40);
    }
}
