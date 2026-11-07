//! Discord Rich Presence over the local IPC pipe.
//!
//! Discord is optional: when the client is not running the thread stays idle
//! and every update silently does nothing, exactly like the Electron build.

use std::sync::mpsc::{self, Sender};

use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity};
use serde_json::Value;

const CLIENT_ID: &str = "1563924074315513957";
const DETAILS_LIMIT: usize = 128;
const NAME_LIMIT: usize = 64;

fn clean(value: &Value, max: usize) -> String {
    let text = value.as_str().unwrap_or("");
    let mut result = String::new();
    let mut space = false;
    for character in text.chars() {
        if character.is_control() || character.is_whitespace() {
            if !space && !result.is_empty() {
                result.push(' ');
                space = true;
            }
            continue;
        }
        space = false;
        if result.chars().count() >= max {
            break;
        }
        result.push(character);
    }
    result.trim().to_string()
}

fn players_label(count: u64) -> String {
    let total = count.min(9999);
    let mod10 = total % 10;
    let mod100 = total % 100;
    let word = if mod10 == 1 && mod100 != 11 {
        "игрок"
    } else if (2..=4).contains(&mod10) && !(10..20).contains(&mod100) {
        "игрока"
    } else {
        "игроков"
    };
    format!("{total} {word}")
}

fn compose(input: &Value, _started_at: i64) -> Option<(String, String)> {
    let mode = input.get("mode").and_then(Value::as_str)?;
    if mode != "dm" && mode != "player" {
        return None;
    }
    let campaign = clean(
        input.get("campaignName").unwrap_or(&Value::Null),
        NAME_LIMIT,
    );
    let character = clean(
        input.get("characterName").unwrap_or(&Value::Null),
        NAME_LIMIT,
    );
    let player = clean(input.get("playerName").unwrap_or(&Value::Null), NAME_LIMIT);
    let count = input
        .get("playerCount")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        .max(0.0) as u64;

    let (details, state) = if mode == "dm" {
        (
            if campaign.is_empty() {
                "Ведёт стол".to_string()
            } else {
                format!("Мастерит «{campaign}»")
            },
            players_label(count),
        )
    } else {
        let who = if !character.is_empty() {
            character
        } else if !player.is_empty() {
            player
        } else {
            "Игрок".to_string()
        };
        (format!("Играет: {who}"), "Стол мастера".to_string())
    };

    Some((
        clean(&Value::String(details), DETAILS_LIMIT),
        clean(&Value::String(state), DETAILS_LIMIT),
    ))
}

pub struct Presence {
    sender: Sender<Value>,
}

impl Presence {
    pub fn start() -> Self {
        let (sender, receiver) = mpsc::channel::<Value>();
        std::thread::spawn(move || {
            let mut client: Option<DiscordIpcClient> = None;
            let mut started_at = 0i64;
            for input in receiver {
                let enabled = input
                    .get("enabled")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
                    && matches!(
                        input.get("mode").and_then(Value::as_str),
                        Some("dm") | Some("player")
                    );
                if !enabled {
                    if let Some(active) = client.as_mut() {
                        let _ = active.clear_activity();
                    }
                    continue;
                }
                if client.is_none() {
                    let mut fresh = DiscordIpcClient::new(CLIENT_ID);
                    if fresh.connect().is_err() {
                        continue;
                    }
                    client = Some(fresh);
                }
                if started_at == 0 {
                    started_at = chrono::Utc::now().timestamp();
                }
                let Some((details, state)) = compose(&input, started_at) else {
                    continue;
                };
                let assets = activity::Assets::new()
                    .large_image("app_icon")
                    .large_text("D&D Table")
                    .small_image("d20")
                    .small_text("D&D 5e");
                let timestamps = activity::Timestamps::new().start(started_at);
                let payload = activity::Activity::new()
                    .details(&details)
                    .state(&state)
                    .timestamps(timestamps)
                    .assets(assets);
                if let Some(active) = client.as_mut() {
                    if active.set_activity(payload).is_err() {
                        client = None;
                    }
                }
            }
            if let Some(mut active) = client {
                let _ = active.clear_activity();
                let _ = active.close();
            }
        });
        Self { sender }
    }

    pub fn update(&self, input: Value) {
        let _ = self.sender.send(input);
    }
}
