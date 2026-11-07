//! Ошибки конвейера импорта.
//!
//! Каждая ошибка несёт код: интерфейс показывает текст, но по коду понимает,
//! что именно произошло, и решает, можно ли предложить повторить.

use serde_json::{Value, json};

pub const E_NO_ADAPTER: &str = "no-adapter";
pub const E_UNREADABLE: &str = "unreadable";
pub const E_EMPTY: &str = "empty";
pub const E_TOO_LARGE: &str = "too-large";

#[derive(Debug, Clone)]
pub struct ImportError {
    pub code: String,
    pub message: String,
    pub details: Value,
}

impl ImportError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            details: json!({}),
        }
    }
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ImportError {}

/// `importError(code, message, details = {})`.
pub fn import_error(code: impl Into<String>, message: impl Into<String>) -> ImportError {
    ImportError::new(code, message)
}
