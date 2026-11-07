//! Session log: JSONL next to `campaign.json`. Port of `server/session-log.js`.
//!
//! One file per day, rotated at 5 MB. Disk errors are swallowed on purpose:
//! the log is useful but must never break the table.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde_json::Value;

pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

pub struct SessionLog {
    dir: PathBuf,
    date: String,
    file: PathBuf,
}

impl SessionLog {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            date: String::new(),
            file: PathBuf::new(),
        }
    }

    pub fn current_file(&mut self) -> PathBuf {
        let date = Utc::now().format("%Y-%m-%d").to_string();
        if date != self.date {
            self.date = date.clone();
            self.file = self.dir.join(format!("session-{date}.jsonl"));
            let _ = fs::create_dir_all(&self.dir);
        }
        self.file.clone()
    }

    pub fn append(&mut self, event: &Value) {
        let file = self.current_file();
        let rotated = PathBuf::from(format!("{}.1", file.display()));
        if fs::metadata(&file)
            .map(|meta| meta.len() > MAX_LOG_BYTES)
            .unwrap_or(false)
        {
            let _ = fs::remove_file(&rotated);
            let _ = fs::rename(&file, &rotated);
        }
        if let Ok(line) = serde_json::to_string(event) {
            use std::io::Write;
            if let Ok(mut handle) = fs::OpenOptions::new().create(true).append(true).open(&file) {
                let _ = writeln!(handle, "{line}");
            }
        }
    }
}

fn is_session_file(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("session-") else {
        return false;
    };
    let rest = rest.strip_suffix(".1").unwrap_or(rest);
    let Some(date) = rest.strip_suffix(".jsonl") else {
        return false;
    };
    date.len() == 10
        && date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && date
            .chars()
            .enumerate()
            .all(|(index, c)| index == 4 || index == 7 || c.is_ascii_digit())
}

/// Markdown export of every diary file in the campaign dir.
///
/// Whispers are skipped: the export does not peek into private messages.
/// The reference joined lines without a separator, which glued the whole
/// chronology into one line; here lines are joined with `\n` on purpose.
pub fn export_session_markdown(dir: &Path) -> (String, String) {
    let mut files: Vec<String> = match fs::read_dir(dir) {
        Ok(entries) => entries
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| is_session_file(name))
            .collect(),
        Err(_) => return (String::new(), "session.md".to_string()),
    };
    files.sort();
    if files.is_empty() {
        return (String::new(), "session.md".to_string());
    }

    let mut lines = Vec::new();
    for name in &files {
        let Ok(raw) = fs::read_to_string(dir.join(name)) else {
            continue;
        };
        for line in raw.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(entry) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let event = entry.get("event").and_then(Value::as_str).unwrap_or("");
            if event == "chat"
                && entry
                    .get("whisper")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            {
                continue;
            }
            let time = entry
                .get("time")
                .and_then(Value::as_str)
                .and_then(|value| value.get(11..19))
                .unwrap_or("");
            match event {
                "chat" => {
                    let from = entry.get("from").and_then(Value::as_str).unwrap_or("?");
                    let text = entry.get("text").and_then(Value::as_str).unwrap_or("");
                    lines.push(format!("{time} {from}: {text}"));
                }
                "combat" => {
                    let on = entry.get("mode").and_then(Value::as_bool).unwrap_or(false);
                    lines.push(format!(
                        "{time} Бой: {}",
                        if on {
                            "включён"
                        } else {
                            "выключен"
                        }
                    ));
                }
                _ => lines.push(format!("{time} {entry}")),
            }
        }
    }

    let today = Utc::now().format("%Y-%m-%d").to_string();
    let header = format!("# Хронология сессии\n\nЭкспорт от {today}\n\n");
    (
        format!("{header}{}\n", lines.join("\n")),
        format!("session-{}.md", files[0]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn temp_dir() -> PathBuf {
        std::env::temp_dir().join(format!("dnd-session-{}", Uuid::new_v4()))
    }

    #[test]
    fn appends_json_lines() {
        let dir = temp_dir();
        let mut log = SessionLog::new(&dir);
        log.append(&json!({ "event": "chat", "from": "ДМ", "text": "привет" }));
        log.append(&json!({ "event": "combat", "mode": true }));
        let file = log.current_file();
        let raw = fs::read_to_string(file).expect("log");
        assert_eq!(raw.lines().count(), 2);
        let first: Value = serde_json::from_str(raw.lines().next().unwrap()).unwrap();
        assert_eq!(first["text"], "привет");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn exports_markdown_and_skips_whispers() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        let day = Utc::now().format("%Y-%m-%d").to_string();
        let file = dir.join(format!("session-{day}.jsonl"));
        let content = [
            json!({ "event": "chat", "time": "2026-11-07T10:00:00.000Z", "from": "ДМ", "text": "начали" }),
            json!({ "event": "chat", "time": "2026-11-07T10:01:00.000Z", "from": "ДМ", "text": "секрет", "whisper": true }),
            json!({ "event": "combat", "time": "2026-11-07T10:02:00.000Z", "mode": true }),
        ]
        .iter()
        .map(|value| serde_json::to_string(value).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
        fs::write(&file, content).unwrap();

        let (text, name) = export_session_markdown(&dir);
        assert!(text.contains("10:00:00 ДМ: начали"));
        assert!(!text.contains("секрет"));
        assert!(text.contains("10:02:00 Бой: включён"));
        assert_eq!(name, format!("session-session-{day}.jsonl.md"));
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn recognizes_session_file_names() {
        assert!(is_session_file("session-2026-11-07.jsonl"));
        assert!(is_session_file("session-2026-11-07.jsonl.1"));
        assert!(!is_session_file("session-2026-11-07.txt"));
        assert!(!is_session_file("campaign.json"));
        assert!(!is_session_file("session-2026-1-07.jsonl"));
    }
}
