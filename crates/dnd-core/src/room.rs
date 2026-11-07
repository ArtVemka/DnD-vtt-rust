//! Table state. Port of `RoomState` from `server/state.js`.
//!
//! Fields stay as JSON values because the renderer consumes the same JSON and
//! the reference works with plain objects. Methods own the invariants that the
//! server relies on: visibility, viewer snapshots, player bookkeeping.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use chrono::{SecondsFormat, Utc};
use serde_json::{Map, Value, json};

use crate::content;
use crate::dice;
use crate::model::{MAP_TYPES, empty_maps};
use crate::rules;
use crate::sheet::{self, LIMITS};

pub const ROLE_DM: &str = "dm";
pub const ROLE_PLAYER: &str = "player";
pub const DM_PLAYER_ID: &str = "dm";
pub const DEFAULT_DM_AVATAR: &str = "avatar-dm.png";

const MAP_NAME_MAX: i64 = 120;
const TOKEN_PALETTE: [&str; 8] = [
    "#c96f4a", "#4a8fc9", "#6fb45f", "#b45fa0", "#c9a34a", "#5f7fb4", "#a05f5f", "#4ab4a0",
];

#[derive(Debug, Clone)]
pub struct Viewer {
    pub player_id: String,
    pub role: String,
}

impl Viewer {
    pub fn dm() -> Self {
        Self {
            player_id: DM_PLAYER_ID.to_string(),
            role: ROLE_DM.to_string(),
        }
    }

    pub fn player(player_id: &str) -> Self {
        Self {
            player_id: player_id.to_string(),
            role: ROLE_PLAYER.to_string(),
        }
    }

    pub fn is_dm(&self) -> bool {
        self.role == ROLE_DM
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PlayerListOptions {
    pub include_tokens: bool,
    pub include_dm_notes: bool,
    pub include_hidden: bool,
}

impl Default for PlayerListOptions {
    fn default() -> Self {
        Self {
            include_tokens: true,
            include_dm_notes: false,
            include_hidden: false,
        }
    }
}

pub struct RoomState {
    pub campaign: Value,
    pub players: BTreeMap<String, Value>,
    pub sheets: BTreeMap<String, Value>,
    pub maps: Value,
    pub active_map: Option<String>,
    pub active_map_id: Option<String>,
    pub grid: Value,
    pub game_time: Value,
    pub timer: Value,
    pub calendar_months: Value,
    pub calendar_events: Value,
    pub tokens: Vec<Value>,
    pub initiative: Value,
    pub combat: Value,
    pub dice_history: Vec<Value>,
    pub chat_history: Vec<Value>,
    pub journal: String,
    pub party_notes: String,
    pub journal_entries: Vec<Value>,
    pub inventory: Vec<Value>,
    pub inventory_coins: Value,
    pub roll_requests: Vec<Value>,
    pub showcase: Value,
    pub vote: Option<Value>,
    pub scenes: Vec<Value>,
    pub soundboard: Vec<Value>,
    pub pending_reactions: Vec<Value>,
    pub undo_stack: Vec<Value>,
    pub adventure: Option<Value>,
    /// Live sockets per player. Not serialized: connections die with the process.
    pub sockets: BTreeMap<String, HashSet<String>>,
}

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// JS truthiness for loose fields (`Boolean(value)`).
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(Value::Number(number)) => number
            .as_f64()
            .map(|n| n != 0.0 && !n.is_nan())
            .unwrap_or(false),
        Some(Value::String(text)) => !text.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// `Number(value)` for the few numeric fields the reference reads loosely.
fn number_of(value: Option<&Value>) -> f64 {
    match value {
        None | Some(Value::Null) => 0.0,
        Some(Value::Bool(flag)) => {
            if *flag {
                1.0
            } else {
                0.0
            }
        }
        Some(Value::Number(number)) => number.as_f64().unwrap_or(f64::NAN),
        Some(Value::String(text)) => {
            let text = text.trim();
            if text.is_empty() {
                0.0
            } else {
                text.parse::<f64>().unwrap_or(f64::NAN)
            }
        }
        Some(Value::Array(items)) => {
            if items.len() == 1 {
                number_of(items.first())
            } else {
                f64::NAN
            }
        }
        Some(Value::Object(_)) => f64::NAN,
    }
}

fn js_string(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Bool(flag)) => flag.to_string(),
        Some(Value::Number(number)) => number.to_string(),
        Some(other) => other.to_string(),
    }
}

fn js_trim(value: &Value, max_length: i64) -> String {
    sheet::sanitize_text(value, max_length).trim().to_string()
}

fn clean_player_key(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .filter(|c| c.is_ascii_hexdigit())
        .take(128)
        .collect()
}

fn strings(value: &Value) -> Vec<Value> {
    value.as_array().cloned().unwrap_or_default()
}

impl RoomState {
    pub fn new(campaign: Value) -> Self {
        let mut state = Self {
            campaign,
            players: BTreeMap::new(),
            sheets: BTreeMap::new(),
            maps: empty_maps(),
            active_map: None,
            active_map_id: None,
            grid: json!({ "enabled": false, "cell": rules::GRID.default_cell, "columns": 0 }),
            game_time: content::empty_time(),
            timer: content::empty_timer(),
            calendar_months: content::normalize_calendar_months(&Value::Null),
            calendar_events: json!([]),
            tokens: Vec::new(),
            initiative: content::empty_initiative(),
            combat: json!({ "enabled": false }),
            dice_history: Vec::new(),
            chat_history: Vec::new(),
            journal: String::new(),
            party_notes: String::new(),
            journal_entries: Vec::new(),
            inventory: Vec::new(),
            inventory_coins: sheet::normalize_coins(&Value::Null),
            roll_requests: Vec::new(),
            showcase: json!({ "on": false, "kind": "", "ref": "", "text": "", "title": "", "watchers": [] }),
            vote: None,
            scenes: Vec::new(),
            soundboard: Vec::new(),
            pending_reactions: Vec::new(),
            undo_stack: Vec::new(),
            adventure: None,
            sockets: BTreeMap::new(),
        };
        if let Some(data) = state
            .campaign
            .get("state")
            .filter(|value| value.is_object())
            .cloned()
        {
            state.hydrate(&data);
        }
        state
    }

    pub fn hydrate(&mut self, data: &Value) {
        if !data.is_object() {
            return;
        }

        for player in data
            .get("players")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(player_id) = player.get("playerId").filter(|value| truthy(Some(value))) else {
                continue;
            };
            let player_id = js_string(Some(player_id));
            let is_dm = player_id == DM_PLAYER_ID;
            let name = {
                let text = sheet::sanitize_text(
                    player.get("name").unwrap_or(&Value::Null),
                    LIMITS.max_name_length,
                );
                if text.is_empty() {
                    if is_dm {
                        "ДМ".to_string()
                    } else {
                        "Игрок".to_string()
                    }
                } else {
                    text
                }
            };
            let avatar = player
                .get("avatar")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(|value| {
                    sheet::sanitize_text(&Value::String(value.to_string()), LIMITS.max_avatar_ref)
                })
                .map(Value::String)
                .unwrap_or(Value::Null);
            let joined_at = player
                .get("joinedAt")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or_else(now_iso);
            self.players.insert(
                player_id.clone(),
                json!({
                    "playerId": player_id,
                    "name": name,
                    "role": if is_dm { ROLE_DM } else { ROLE_PLAYER },
                    "playerKey": clean_player_key(player.get("playerKey")),
                    "avatar": avatar,
                    "dmNote": sheet::sanitize_text(player.get("dmNote").unwrap_or(&Value::Null), LIMITS.max_dm_note),
                    "online": false,
                    "joinedAt": joined_at
                }),
            );
        }

        if let Some(sheets) = data.get("sheets").and_then(Value::as_object) {
            for (player_id, sheet) in sheets {
                let name = self
                    .players
                    .get(player_id)
                    .and_then(|player| player.get("name"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                self.sheets
                    .insert(player_id.clone(), sheet::normalize_sheet(sheet, name));
            }
        }

        let stored_maps = data.get("maps");
        let mut maps = Map::new();
        for kind in MAP_TYPES {
            let slot = self.normalize_map_slot(
                stored_maps
                    .and_then(|maps| maps.get(kind))
                    .unwrap_or(&Value::Null),
            );
            maps.insert(kind.to_string(), slot);
        }
        self.maps = Value::Object(maps);

        self.active_map = data
            .get("activeMap")
            .and_then(Value::as_str)
            .filter(|value| MAP_TYPES.contains(value))
            .map(str::to_string)
            .or_else(|| {
                MAP_TYPES
                    .iter()
                    .find(|kind| {
                        self.map_items(kind)
                            .map(|items| !items.is_empty())
                            .unwrap_or(false)
                    })
                    .map(|kind| kind.to_string())
            });

        let active_slot = self
            .active_map
            .as_deref()
            .and_then(|kind| self.maps.get(kind))
            .cloned();
        let stored_active_id = data
            .get("activeMapId")
            .filter(|value| truthy(Some(value)))
            .map(|value| js_string(Some(value)))
            .unwrap_or_default();
        let stored_exists = !stored_active_id.is_empty()
            && active_slot
                .as_ref()
                .and_then(|slot| slot.get("items"))
                .and_then(Value::as_array)
                .map(|items| {
                    items.iter().any(|item| {
                        item.get("id").and_then(Value::as_str) == Some(stored_active_id.as_str())
                    })
                })
                .unwrap_or(false);
        self.active_map_id = if stored_exists {
            Some(stored_active_id)
        } else {
            active_slot
                .as_ref()
                .and_then(|slot| slot.get("activeId"))
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
        };
        if let (Some(kind), Some(active_id)) = (self.active_map.clone(), self.active_map_id.clone())
        {
            if let Some(items) = self.map_items_mut(&kind) {
                if let Some(item) = items
                    .iter_mut()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(active_id.as_str()))
                {
                    item["published"] = json!(true);
                }
            }
        }

        self.grid = rules::normalize_grid(data.get("grid").unwrap_or(&Value::Null));
        self.game_time = content::normalize_time(data.get("gameTime").unwrap_or(&Value::Null));
        self.timer = content::normalize_timer(data.get("timer").unwrap_or(&Value::Null));
        self.calendar_months =
            content::normalize_calendar_months(data.get("calendarMonths").unwrap_or(&Value::Null));
        self.calendar_events =
            content::normalize_calendar_events(data.get("calendarEvents").unwrap_or(&Value::Null));
        self.party_notes = sheet::sanitize_text(
            data.get("partyNotes").unwrap_or(&Value::Null),
            LIMITS.max_party_notes,
        );

        self.tokens = data
            .get("tokens")
            .and_then(Value::as_array)
            .map(|tokens| {
                tokens
                    .iter()
                    .take(LIMITS.max_tokens as usize)
                    .filter(|token| truthy(token.get("id")))
                    .map(|token| self.normalize_token(token))
                    .collect()
            })
            .unwrap_or_default();

        self.scenes = self.normalize_scenes(data.get("scenes").unwrap_or(&Value::Null));

        self.dice_history = last_n(
            data.get("diceHistory")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            LIMITS.max_dice_history as usize,
        );

        self.chat_history = last_n(
            data.get("chatHistory")
                .and_then(Value::as_array)
                .map(|messages| {
                    messages
                        .iter()
                        .filter(|message| !truthy(message.get("isRoll")))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default(),
            LIMITS.max_chat_history as usize,
        )
        .into_iter()
        .enumerate()
        .map(|(index, message)| {
            let mut message = match message {
                Value::Object(map) => map,
                other => {
                    let mut map = Map::new();
                    map.insert("value".to_string(), other);
                    map
                }
            };
            let has_id = message
                .get("id")
                .map(|id| truthy(Some(id)))
                .unwrap_or(false);
            if !has_id {
                let time = js_string(message.get("time"));
                message.insert("id".to_string(), json!(format!("old-{index}-{time}")));
            }
            Value::Object(message)
        })
        .collect();

        self.journal = sheet::sanitize_text(
            data.get("journal").unwrap_or(&Value::Null),
            LIMITS.max_journal,
        );
        self.initiative =
            content::normalize_initiative(data.get("initiative").unwrap_or(&Value::Null));
        self.combat = json!({
            "enabled": truthy(data.get("combat").and_then(|combat| combat.get("enabled")))
        });
        self.adventure = Some(sheet::normalize_adventure(
            data.get("adventure").unwrap_or(&Value::Null),
        ))
        .filter(|value| !value.is_null());
        self.journal_entries =
            content::normalize_journal_entries(data.get("journalEntries").unwrap_or(&Value::Null))
                .as_array()
                .cloned()
                .unwrap_or_default();
        self.inventory =
            content::normalize_inventory(data.get("inventory").unwrap_or(&Value::Null))
                .as_array()
                .cloned()
                .unwrap_or_default();
        self.inventory_coins =
            sheet::normalize_coins(data.get("inventoryCoins").unwrap_or(&Value::Null));
        self.roll_requests =
            content::normalize_roll_requests(data.get("rollRequests").unwrap_or(&Value::Null))
                .as_array()
                .cloned()
                .unwrap_or_default();
        self.soundboard =
            content::normalize_soundboard(data.get("soundboard").unwrap_or(&Value::Null))
                .as_array()
                .cloned()
                .unwrap_or_default();
    }

    pub fn serialize(&self) -> Value {
        let players: Vec<Value> = self
            .players
            .values()
            .map(|player| {
                json!({
                    "playerId": player.get("playerId").cloned().unwrap_or(Value::Null),
                    "name": player.get("name").cloned().unwrap_or(Value::Null),
                    "role": player.get("role").cloned().unwrap_or(Value::Null),
                    "playerKey": player.get("playerKey").and_then(Value::as_str).unwrap_or(""),
                    "avatar": player.get("avatar").cloned().unwrap_or(Value::Null),
                    "dmNote": player.get("dmNote").and_then(Value::as_str).unwrap_or(""),
                    "joinedAt": player.get("joinedAt").cloned().unwrap_or(Value::Null)
                })
            })
            .collect();
        let sheets: Map<String, Value> = self
            .sheets
            .iter()
            .map(|(id, sheet)| (id.clone(), sheet.clone()))
            .collect();
        json!({
            "players": players,
            "sheets": sheets,
            "maps": self.maps,
            "activeMap": self.active_map,
            "activeMapId": self.active_map_id,
            "scenes": self.scenes,
            "grid": self.grid,
            "gameTime": self.game_time,
            "timer": self.timer,
            "calendarMonths": self.calendar_months,
            "calendarEvents": self.calendar_events,
            "partyNotes": self.party_notes,
            "tokens": self.tokens,
            "initiative": self.initiative,
            "combat": self.combat,
            "diceHistory": last_n(self.dice_history.clone(), LIMITS.max_dice_history as usize),
            "chatHistory": last_n(self.chat_history.clone(), LIMITS.max_chat_history as usize),
            "journal": self.journal,
            "journalEntries": self.journal_entries,
            "inventory": self.inventory,
            "inventoryCoins": self.inventory_coins,
            "rollRequests": self.roll_requests,
            "soundboard": self.soundboard,
            "adventure": self.adventure,
            "savedAt": now_iso()
        })
    }

    pub fn snapshot_for(&self, viewer: &Viewer) -> Value {
        let is_dm = viewer.is_dm();
        let sheets: Map<String, Value> = if is_dm {
            self.sheets
                .iter()
                .map(|(id, sheet)| (id.clone(), sheet.clone()))
                .collect()
        } else {
            self.sheets
                .get(&viewer.player_id)
                .map(|sheet| {
                    let mut map = Map::new();
                    map.insert(viewer.player_id.clone(), sheet.clone());
                    map
                })
                .unwrap_or_default()
        };

        let campaign = &self.campaign;
        let campaign_view = json!({
            "id": campaign.get("id").cloned().unwrap_or(Value::Null),
            "name": campaign.get("name").and_then(Value::as_str).filter(|v| !v.is_empty()).unwrap_or("Кампания"),
            "createdAt": campaign.get("createdAt").cloned().unwrap_or(Value::Null),
            "updatedAt": campaign.get("updatedAt").cloned().unwrap_or(Value::Null),
            "version": campaign.get("version").and_then(Value::as_i64).filter(|v| *v != 0)
                .unwrap_or(crate::model::CAMPAIGN_FORMAT_VERSION)
        });

        let tokens: Vec<Value> = self
            .tokens
            .iter()
            .filter(|token| is_dm || self.is_token_visible_to_players(token))
            .map(|token| self.token_for_viewer(token, is_dm))
            .collect();

        let journal_entries: Vec<Value> = if is_dm {
            self.journal_entries.clone()
        } else {
            self.journal_entries
                .iter()
                .filter(|entry| truthy(entry.get("visible")))
                .cloned()
                .collect()
        };

        let roll_requests: Vec<Value> = if is_dm {
            self.roll_requests.clone()
        } else {
            self.roll_requests
                .iter()
                .filter(|request| {
                    !truthy(request.get("to"))
                        || request.get("to").and_then(Value::as_str)
                            == Some(viewer.player_id.as_str())
                })
                .cloned()
                .collect()
        };

        let reactions: Vec<Value> = if is_dm {
            self.pending_reactions.clone()
        } else {
            self.pending_reactions
                .iter()
                .filter(|item| {
                    item.get("playerId").and_then(Value::as_str) == Some(viewer.player_id.as_str())
                })
                .cloned()
                .collect()
        };

        json!({
            "campaign": campaign_view,
            "adventure": if is_dm { self.adventure.clone().unwrap_or(Value::Null) } else { Value::Null },
            "players": self.player_list(PlayerListOptions {
                include_tokens: true,
                include_dm_notes: is_dm,
                include_hidden: is_dm
            }),
            "sheets": sheets,
            "maps": self.maps_for_viewer(is_dm),
            "activeMap": if is_dm { json!(self.active_map) } else {
                if self.active_map_id.is_some() { json!(self.active_map) } else { Value::Null }
            },
            "activeMapId": self.active_map_id,
            "scenes": if is_dm {
                Value::Array(self.scenes.iter().map(|scene| self.scene_summary(scene)).collect())
            } else {
                json!([])
            },
            "grid": self.grid,
            "gameTime": self.game_time,
            "timer": self.timer,
            "calendarMonths": self.calendar_months,
            "calendarEvents": self.calendar_events,
            "partyNotes": self.party_notes,
            "tokens": tokens,
            "initiative": self.initiative,
            "combat": self.combat,
            "diceHistory": self.dice_history.iter()
                .rev().take(LIMITS.max_dice_history as usize).rev()
                .map(|roll| if is_dm { roll.clone() } else { dice::hide_roll_value(roll) })
                .collect::<Vec<_>>(),
            "chatHistory": self.chat_history.iter()
                .rev().take(LIMITS.max_chat_history as usize).rev()
                .filter(|message| is_dm
                    || !truthy(message.get("whisper"))
                    || message.get("playerId").and_then(Value::as_str) == Some(viewer.player_id.as_str())
                    || message.get("to").and_then(Value::as_str) == Some(viewer.player_id.as_str()))
                .cloned().collect::<Vec<_>>(),
            "journal": if is_dm { json!(self.journal) } else { json!("") },
            "journalEntries": journal_entries,
            "inventory": self.inventory,
            "inventoryCoins": self.inventory_coins,
            "rollRequests": roll_requests,
            "soundboard": if is_dm { json!(self.soundboard) } else { json!([]) },
            "reactions": reactions
        })
    }

    pub fn player_list(&self, options: PlayerListOptions) -> Vec<Value> {
        let mut token_id_by_player: BTreeMap<String, String> = BTreeMap::new();
        if options.include_tokens {
            for token in &self.tokens {
                if !options.include_hidden && !self.is_token_visible_to_players(token) {
                    continue;
                }
                if let Some(player_id) = token
                    .get("assignedPlayerId")
                    .filter(|value| truthy(Some(value)))
                {
                    let player_id = js_string(Some(player_id));
                    let token_id = token
                        .get("id")
                        .map(|id| js_string(Some(id)))
                        .unwrap_or_default();
                    token_id_by_player.entry(player_id).or_insert(token_id);
                }
            }
        }

        let mut list: Vec<Value> = self
            .players
            .values()
            .map(|player| {
                let player_id = js_string(player.get("playerId"));
                let role = player.get("role").and_then(Value::as_str).unwrap_or(ROLE_PLAYER);
                let avatar = player
                    .get("avatar")
                    .filter(|value| truthy(Some(value)))
                    .cloned()
                    .unwrap_or_else(|| {
                        if role == ROLE_DM {
                            json!(DEFAULT_DM_AVATAR)
                        } else {
                            Value::Null
                        }
                    });
                json!({
                    "playerId": player_id,
                    "name": player.get("name").cloned().unwrap_or(Value::Null),
                    "role": role,
                    "avatar": avatar,
                    "online": truthy(player.get("online")),
                    "hasSheet": self.sheets.contains_key(player.get("playerId").and_then(Value::as_str).unwrap_or("")),
                    "dmNote": if options.include_dm_notes {
                        player.get("dmNote").cloned().unwrap_or_else(|| json!(""))
                    } else {
                        json!("")
                    },
                    "tokenId": if options.include_tokens {
                        token_id_by_player.get(player.get("playerId").and_then(Value::as_str).unwrap_or(""))
                            .cloned().map(Value::String).unwrap_or(Value::Null)
                    } else {
                        Value::Null
                    }
                })
            })
            .collect();

        list.sort_by(|a, b| {
            let role_a = a.get("role").and_then(Value::as_str).unwrap_or("");
            let role_b = b.get("role").and_then(Value::as_str).unwrap_or("");
            if role_a != role_b {
                return if role_a == ROLE_DM {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            let online_a = truthy(a.get("online"));
            let online_b = truthy(b.get("online"));
            if online_a != online_b {
                return if online_a {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                };
            }
            let name_a = a.get("name").and_then(Value::as_str).unwrap_or("");
            let name_b = b.get("name").and_then(Value::as_str).unwrap_or("");
            name_a
                .to_lowercase()
                .cmp(&name_b.to_lowercase())
                .then_with(|| name_a.cmp(name_b))
        });
        list
    }

    pub fn add_player(&mut self, player_id: &str, name: &str, player_key: &str) -> Value {
        if !self.players.contains_key(player_id) {
            self.players.insert(
                player_id.to_string(),
                json!({
                    "playerId": player_id,
                    "name": name,
                    "role": ROLE_PLAYER,
                    "playerKey": player_key,
                    "avatar": Value::Null,
                    "dmNote": "",
                    "online": false,
                    "joinedAt": now_iso()
                }),
            );
            self.sheets
                .insert(player_id.to_string(), sheet::create_empty_sheet(name));
        } else if let Some(player) = self.players.get_mut(player_id) {
            player["name"] = json!(name);
            let has_key = player
                .get("playerKey")
                .and_then(Value::as_str)
                .map(|key| !key.is_empty())
                .unwrap_or(false);
            if !player_key.is_empty() && !has_key {
                player["playerKey"] = json!(player_key);
            }
        }
        self.players.get(player_id).cloned().unwrap_or(Value::Null)
    }

    pub fn add_dm(&mut self, name: &str) -> Value {
        if !self.players.contains_key(DM_PLAYER_ID) {
            self.players.insert(
                DM_PLAYER_ID.to_string(),
                json!({
                    "playerId": DM_PLAYER_ID,
                    "name": name,
                    "role": ROLE_DM,
                    "playerKey": "",
                    "avatar": Value::Null,
                    "dmNote": "",
                    "online": false,
                    "joinedAt": now_iso()
                }),
            );
        } else if let Some(dm) = self.players.get_mut(DM_PLAYER_ID) {
            dm["name"] = json!(name);
        }
        self.players
            .get(DM_PLAYER_ID)
            .cloned()
            .unwrap_or(Value::Null)
    }

    pub fn rename_player(&mut self, player_id: &str, name: &str) -> Option<Value> {
        let previous = self
            .players
            .get(player_id)
            .and_then(|player| player.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        {
            let player = self.players.get_mut(player_id)?;
            player["name"] = json!(name);
        }
        if let Some(sheet) = self.sheets.get_mut(player_id) {
            if sheet.get("playerName").and_then(Value::as_str) == Some(previous.as_str()) {
                sheet["playerName"] = json!(name);
            }
        }
        self.players.get(player_id).cloned()
    }

    pub fn attach_socket(&mut self, player_id: &str, socket_id: &str) {
        self.sockets
            .entry(player_id.to_string())
            .or_default()
            .insert(socket_id.to_string());
        if let Some(player) = self.players.get_mut(player_id) {
            player["online"] = json!(true);
        }
    }

    pub fn detach_socket(&mut self, player_id: &str, socket_id: &str) {
        let empty = if let Some(sockets) = self.sockets.get_mut(player_id) {
            sockets.remove(socket_id);
            sockets.is_empty()
        } else {
            false
        };
        if empty {
            if let Some(player) = self.players.get_mut(player_id) {
                player["online"] = json!(false);
            }
        }
    }

    pub fn is_online(&self, player_id: &str) -> bool {
        self.sockets
            .get(player_id)
            .map(|sockets| !sockets.is_empty())
            .unwrap_or(false)
    }

    pub fn has_sheet(&self, player_id: &str) -> bool {
        self.sheets.contains_key(player_id)
    }

    /// `normalizeMapSlot`: one tab of maps, including the pre-scene single-map format.
    pub fn normalize_map_slot(&self, raw: &Value) -> Value {
        let mut items: Vec<Value> = Vec::new();
        if !raw.is_object() {
            return json!({ "items": items, "activeId": Value::Null });
        }

        if let Some(url) = raw.get("url").and_then(Value::as_str) {
            items.push(self.map_item(raw, url, true));
        } else if let Some(source_items) = raw.get("items").and_then(Value::as_array) {
            for item in source_items.iter().take(LIMITS.max_maps_per_type as usize) {
                let Some(url) = item.get("url").and_then(Value::as_str) else {
                    continue;
                };
                if !item.is_object() || url.is_empty() {
                    continue;
                }
                let published = !matches!(item.get("published"), Some(Value::Bool(false)));
                items.push(self.map_item(item, url, published));
            }
        }

        let active_id = raw
            .get("activeId")
            .filter(|value| truthy(Some(value)))
            .map(|value| js_string(Some(value)))
            .filter(|id| {
                items
                    .iter()
                    .any(|item| item.get("id").and_then(Value::as_str) == Some(id.as_str()))
            })
            .map(Value::String)
            .or_else(|| items.first().and_then(|item| item.get("id")).cloned())
            .unwrap_or(Value::Null);

        json!({ "items": items, "activeId": active_id })
    }

    fn map_item(&self, source: &Value, url: &str, published: bool) -> Value {
        let name = {
            let text =
                sheet::sanitize_text(source.get("name").unwrap_or(&Value::Null), MAP_NAME_MAX);
            if text.is_empty() {
                "Карта".to_string()
            } else {
                text
            }
        };
        let id = source
            .get("id")
            .filter(|value| truthy(Some(value)))
            .map(|value| js_string(Some(value)))
            .unwrap_or_else(content::new_map_id);
        let uploaded_at = source
            .get("uploadedAt")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(now_iso);
        json!({
            "id": id,
            "url": url,
            "name": name,
            "published": published,
            "fogMask": content::normalize_fog(source.get("fogMask").unwrap_or(&Value::Null)),
            "surface": rules::normalize_surface(source.get("surface").unwrap_or(&Value::Null)),
            "walls": rules::normalize_walls(source.get("walls").unwrap_or(&Value::Null)),
            "lights": rules::normalize_lights(source.get("lights").unwrap_or(&Value::Null)),
            "templates": content::normalize_templates(source.get("templates").unwrap_or(&Value::Null)),
            "pins": rules::normalize_pins(source.get("pins").unwrap_or(&Value::Null)),
            "darkness": content::normalize_darkness(source.get("darkness").unwrap_or(&Value::Null)),
            "autoLight": source.get("autoLight").and_then(Value::as_bool).unwrap_or(false),
            "fogFollow": source.get("fogFollow").and_then(Value::as_bool).unwrap_or(false),
            "visionPerPlayer": source.get("visionPerPlayer").and_then(Value::as_bool).unwrap_or(false),
            "audioRef": content::normalize_audio_ref(source.get("audioRef").unwrap_or(&Value::Null)),
            "uploadedAt": uploaded_at
        })
    }

    pub fn current_map(&self, kind: &str) -> Option<Value> {
        let slot = self.maps.get(kind)?;
        let items = slot.get("items")?.as_array()?;
        if items.is_empty() {
            return None;
        }
        let active = slot.get("activeId").and_then(Value::as_str);
        items
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == active)
            .or_else(|| items.first())
            .cloned()
    }

    pub fn is_map_published(&self, kind: &str, map_id: Option<&str>) -> bool {
        let Some(slot) = self.maps.get(kind) else {
            return false;
        };
        let Some(items) = slot.get("items").and_then(Value::as_array) else {
            return false;
        };
        match map_id.filter(|value| !value.is_empty()) {
            None => items
                .iter()
                .any(|item| !matches!(item.get("published"), Some(Value::Bool(false)))),
            Some(map_id) => items
                .iter()
                .find(|item| item.get("id").and_then(Value::as_str) == Some(map_id))
                .map(|item| !matches!(item.get("published"), Some(Value::Bool(false))))
                .unwrap_or(false),
        }
    }

    pub fn is_token_visible_to_players(&self, token: &Value) -> bool {
        if truthy(token.get("hidden")) {
            return false;
        }
        let map_type = token.get("mapType").and_then(Value::as_str).unwrap_or("");
        let map_id = token
            .get("mapId")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty());
        self.is_map_published(map_type, map_id)
    }

    pub fn map_slot_for_viewer(&self, kind: &str, is_dm: bool) -> Value {
        let slot = self
            .maps
            .get(kind)
            .cloned()
            .unwrap_or_else(|| json!({ "items": [], "activeId": Value::Null }));
        if is_dm {
            return slot;
        }
        let items: Vec<Value> = slot
            .get("items")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| !matches!(item.get("published"), Some(Value::Bool(false))))
                    .map(|item| {
                        let mut item = item.clone();
                        if let Some(object) = item.as_object_mut() {
                            if object
                                .get("pins")
                                .and_then(Value::as_array)
                                .map(|pins| !pins.is_empty())
                                .unwrap_or(false)
                            {
                                object.remove("pins");
                            }
                        }
                        item
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut active_id = if self.active_map.as_deref() == Some(kind) {
            self.active_map_id.clone()
        } else {
            None
        };
        let active_valid = active_id
            .as_deref()
            .map(|id| {
                items
                    .iter()
                    .any(|item| item.get("id").and_then(Value::as_str) == Some(id))
            })
            .unwrap_or(false);
        if !active_valid {
            active_id = items
                .first()
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        json!({ "items": items, "activeId": active_id })
    }

    pub fn maps_for_viewer(&self, is_dm: bool) -> Value {
        if is_dm {
            return self.maps.clone();
        }
        let mut maps = Map::new();
        for kind in MAP_TYPES {
            maps.insert(kind.to_string(), self.map_slot_for_viewer(kind, false));
        }
        Value::Object(maps)
    }

    /// `normalizeToken`: repairs a stored or incoming token.
    pub fn normalize_token(&self, token: &Value) -> Value {
        let conditions =
            content::normalize_conditions(token.get("conditions").unwrap_or(&Value::Null));
        let durations = content::normalize_condition_durations(
            token.get("conditionDurations").unwrap_or(&Value::Null),
            &conditions,
        );
        let units = content::normalize_condition_units(
            token.get("conditionDurationUnits").unwrap_or(&Value::Null),
            &conditions,
            &durations,
        );

        let assigned = token
            .get("assignedPlayerId")
            .filter(|value| truthy(Some(value)))
            .map(|value| js_string(Some(value)));
        let side = match token.get("side").and_then(Value::as_str) {
            Some("ally") => "ally",
            Some("enemy") => "enemy",
            _ => {
                if assigned.is_some() {
                    "ally"
                } else {
                    "enemy"
                }
            }
        };
        let armor_number = number_of(token.get("armorClass"));
        let armor_class = if armor_number.is_finite() && armor_number >= 1.0 {
            json!(sheet::clamp_int(
                token.get("armorClass").unwrap_or(&Value::Null),
                1,
                60,
                10
            ))
        } else {
            Value::Null
        };
        let max_hp_number = number_of(token.get("maxHp"));
        let (max_hp, hp) = if max_hp_number.is_finite() && max_hp_number > 0.0 {
            let max_hp = sheet::clamp_int(
                token.get("maxHp").unwrap_or(&Value::Null),
                1,
                LIMITS.max_token_hp,
                1,
            );
            let current = number_of(token.get("hp"));
            let hp = if current.is_finite() {
                sheet::clamp_int(token.get("hp").unwrap_or(&Value::Null), 0, max_hp, 0)
            } else {
                max_hp
            };
            (json!(max_hp), json!(hp))
        } else {
            (Value::Null, Value::Null)
        };
        let color = token
            .get("color")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| TOKEN_PALETTE[0].to_string());
        let name = {
            let text = sheet::sanitize_text(
                token.get("name").unwrap_or(&Value::Null),
                LIMITS.max_name_length,
            );
            if text.is_empty() {
                "Фишка".to_string()
            } else {
                text
            }
        };
        let map_type = token
            .get("mapType")
            .and_then(Value::as_str)
            .filter(|value| MAP_TYPES.contains(value))
            .unwrap_or("battle");

        let mut result = json!({
            "id": js_string(token.get("id")),
            "name": name,
            "mapType": map_type,
            "mapId": sheet::sanitize_text(token.get("mapId").unwrap_or(&Value::Null), 80),
            "x": sheet::clamp_number(token.get("x").unwrap_or(&Value::Null), 0.0, 1.0, 0.5),
            "y": sheet::clamp_number(token.get("y").unwrap_or(&Value::Null), 0.0, 1.0, 0.5),
            "imageUrl": token.get("imageUrl").and_then(Value::as_str).map(|value| json!(value)).unwrap_or(Value::Null),
            "assignedPlayerId": assigned,
            "side": side,
            "attackBonus": sheet::clamp_int(token.get("attackBonus").unwrap_or(&Value::Null), -5, 20, 0),
            "armorClass": armor_class,
            "movedCells": sheet::clamp_int(token.get("movedCells").unwrap_or(&Value::Null), 0, 1000, 0),
            "avatarFromSheet": token.get("avatarFromSheet").and_then(Value::as_bool).unwrap_or(false),
            "hidden": truthy(token.get("hidden")),
            "light": content::normalize_token_light(token.get("light").unwrap_or(&Value::Null)),
            "hideName": truthy(token.get("hideName")),
            "hideLabels": truthy(token.get("hideLabels")),
            "color": color,
            "size": sheet::clamp_int(token.get("size").unwrap_or(&Value::Null), 1, 4, 1),
            "maxHp": max_hp,
            "hp": hp,
            "conditions": conditions,
            "conditionDurations": durations,
            "conditionDurationUnits": units,
            "aura": content::normalize_aura(token.get("aura").unwrap_or(&Value::Null)),
            "vision": content::normalize_vision(token.get("vision").unwrap_or(&Value::Null)),
            "damageResistances": content::normalize_damage_defenses(token.get("damageResistances").unwrap_or(&Value::Null)),
            "damageVulnerabilities": content::normalize_damage_defenses(token.get("damageVulnerabilities").unwrap_or(&Value::Null)),
            "damageImmunities": content::normalize_damage_defenses(token.get("damageImmunities").unwrap_or(&Value::Null))
        });

        if let Some(emote) =
            content::normalize_token_emote(token.get("emote").unwrap_or(&Value::Null))
        {
            result["emote"] = emote;
        }
        let concentration = js_trim(
            token.get("concentration").unwrap_or(&Value::Null),
            LIMITS.max_concentration_length,
        );
        if !concentration.is_empty() {
            result["concentration"] = json!(concentration);
        }
        if let Some(crop) = sheet::normalize_image_crop(token).as_object() {
            for (key, value) in crop {
                result[key] = value.clone();
            }
        }
        result
    }

    /// `tokenForViewer`: sheet health wins, enemy numbers are hidden from players.
    pub fn token_for_viewer(&self, token: &Value, is_dm: bool) -> Value {
        let mut result = token.clone();
        if let Some(player_id) = token.get("assignedPlayerId").and_then(Value::as_str) {
            if let Some(sheet) = self.sheets.get(player_id) {
                let max_hp = rules::effective_max_hp(sheet);
                result["maxHp"] = json!(max_hp as i64);
                result["hp"] = json!(sheet::clamp_int(
                    sheet.get("hp").unwrap_or(&Value::Null),
                    0,
                    max_hp as i64,
                    0
                ));
            }
        }
        if is_dm {
            return result;
        }
        let enemy = token.get("side").and_then(Value::as_str) == Some("enemy")
            || (!truthy(token.get("side")) && !truthy(token.get("assignedPlayerId")));
        let has_hp = truthy(result.get("maxHp"));
        if has_hp && enemy {
            result["maxHp"] = Value::Null;
            result["hp"] = Value::Null;
            result["armorClass"] = Value::Null;
        }
        if truthy(token.get("hideName")) {
            result["name"] = json!("");
            result["nameHidden"] = json!(true);
        }
        result
    }

    pub fn scene_summary(&self, scene: &Value) -> Value {
        let map_type = scene.get("mapType").and_then(Value::as_str).unwrap_or("");
        let map_id = scene.get("mapId").and_then(Value::as_str).unwrap_or("");
        let map = self
            .maps
            .get(map_type)
            .and_then(|slot| slot.get("items"))
            .and_then(Value::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("id").and_then(Value::as_str) == Some(map_id))
            });
        json!({
            "id": scene.get("id").cloned().unwrap_or(Value::Null),
            "name": scene.get("name").cloned().unwrap_or(Value::Null),
            "mapType": scene.get("mapType").cloned().unwrap_or(Value::Null),
            "mapId": scene.get("mapId").cloned().unwrap_or(Value::Null),
            "mapName": map.and_then(|item| item.get("name")).cloned().unwrap_or_else(|| json!("Карта")),
            "mapUrl": map.and_then(|item| item.get("url")).cloned().unwrap_or_else(|| json!("")),
            "tokenCount": scene.get("tokens").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
            "published": map.map(|item| !matches!(item.get("published"), Some(Value::Bool(false)))).unwrap_or(false),
            "createdAt": scene.get("createdAt").cloned().unwrap_or(Value::Null),
            "updatedAt": scene.get("updatedAt").cloned().unwrap_or(Value::Null)
        })
    }

    pub fn normalize_scenes(&self, raw: &Value) -> Vec<Value> {
        let Some(list) = raw.as_array() else {
            return Vec::new();
        };
        let mut scenes = Vec::new();
        let mut ids = BTreeSet::new();
        for item in list.iter().take(LIMITS.max_scenes as usize) {
            if !item.is_object() {
                continue;
            }
            let Some(map_type) = item
                .get("mapType")
                .and_then(Value::as_str)
                .filter(|value| MAP_TYPES.contains(value))
            else {
                continue;
            };
            let map_id = sheet::sanitize_text(item.get("mapId").unwrap_or(&Value::Null), 80);
            let map_exists = self
                .maps
                .get(map_type)
                .and_then(|slot| slot.get("items"))
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .any(|map| map.get("id").and_then(Value::as_str) == Some(map_id.as_str()))
                })
                .unwrap_or(false);
            if !map_exists {
                continue;
            }
            let mut id = js_trim(item.get("id").unwrap_or(&Value::Null), 40);
            if id.is_empty() {
                id = content::new_scene_id();
            }
            if ids.contains(&id) {
                id = content::new_scene_id();
            }
            ids.insert(id.clone());

            let tokens: Vec<Value> = item
                .get("tokens")
                .and_then(Value::as_array)
                .map(|tokens| {
                    tokens
                        .iter()
                        .take(LIMITS.max_scene_tokens as usize)
                        .filter(|token| truthy(token.get("id")))
                        .map(|token| self.normalize_token(token))
                        .filter(|token| {
                            token.get("mapType").and_then(Value::as_str) == Some(map_type)
                                && token.get("mapId").and_then(Value::as_str)
                                    == Some(map_id.as_str())
                        })
                        .collect()
                })
                .unwrap_or_default();
            let map = item
                .get("map")
                .filter(|value| value.is_object())
                .unwrap_or(&Value::Null);
            let name = js_trim(
                item.get("name").unwrap_or(&Value::Null),
                LIMITS.max_scene_name,
            );
            scenes.push(json!({
                "id": id,
                "name": if name.is_empty() { "Сцена".to_string() } else { name },
                "mapType": map_type,
                "mapId": map_id,
                "map": {
                    "fogMask": content::normalize_fog(map.get("fogMask").unwrap_or(&Value::Null)),
                    "surface": rules::normalize_surface(map.get("surface").unwrap_or(&Value::Null)),
                    "walls": rules::normalize_walls(map.get("walls").unwrap_or(&Value::Null)),
                    "lights": rules::normalize_lights(map.get("lights").unwrap_or(&Value::Null)),
                    "templates": content::normalize_templates(map.get("templates").unwrap_or(&Value::Null)),
                    "pins": rules::normalize_pins(map.get("pins").unwrap_or(&Value::Null)),
                    "darkness": content::normalize_darkness(map.get("darkness").unwrap_or(&Value::Null)),
                    "autoLight": map.get("autoLight").and_then(Value::as_bool).unwrap_or(false),
                    "fogFollow": map.get("fogFollow").and_then(Value::as_bool).unwrap_or(false),
                    "visionPerPlayer": map.get("visionPerPlayer").and_then(Value::as_bool).unwrap_or(false),
                    "audioRef": content::normalize_audio_ref(map.get("audioRef").unwrap_or(&Value::Null))
                },
                "grid": rules::normalize_grid(item.get("grid").unwrap_or(&Value::Null)),
                "tokens": tokens,
                "createdAt": text_or_now(item.get("createdAt")),
                "updatedAt": text_or_now(item.get("updatedAt"))
            }));
        }
        scenes
    }

    fn map_items(&self, kind: &str) -> Option<Vec<Value>> {
        self.maps
            .get(kind)
            .and_then(|slot| slot.get("items"))
            .and_then(Value::as_array)
            .cloned()
    }

    fn map_items_mut(&mut self, kind: &str) -> Option<&mut Vec<Value>> {
        self.maps
            .get_mut(kind)
            .and_then(|slot| slot.get_mut("items"))
            .and_then(Value::as_array_mut)
    }
}

fn last_n(mut items: Vec<Value>, limit: usize) -> Vec<Value> {
    if items.len() > limit {
        items.drain(..items.len() - limit);
    }
    items
}

fn text_or_now(value: Option<&Value>) -> String {
    let text: String = value
        .and_then(Value::as_str)
        .unwrap_or("")
        .chars()
        .take(40)
        .collect();
    if text.is_empty() { now_iso() } else { text }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn campaign(state: Value) -> Value {
        json!({
            "id": "abcd",
            "name": "Тест",
            "createdAt": "2026-01-01T00:00:00.000Z",
            "updatedAt": "2026-01-01T00:00:00.000Z",
            "version": 2,
            "state": state
        })
    }

    #[test]
    fn empty_room_has_defaults() {
        let room = RoomState::new(campaign(json!({})));
        assert_eq!(room.game_time["day"], 1);
        assert_eq!(room.game_time["hour"], 8);
        assert_eq!(room.combat["enabled"], false);
        assert!(room.tokens.is_empty());
        let snapshot = room.snapshot_for(&Viewer::dm());
        assert_eq!(snapshot["campaign"]["name"], "Тест");
        assert_eq!(snapshot["journal"], "");
    }

    #[test]
    fn hydrates_players_sheets_and_legacy_map() {
        let room = RoomState::new(campaign(json!({
            "players": [{ "playerId": "p-hero", "name": "Герой", "playerKey": "aa11" }],
            "sheets": { "p-hero": { "maxHp": 20, "hp": 12 } },
            "maps": { "battle": { "url": "/files/a.png", "name": "Поле" } },
            "tokens": [{ "id": "t1", "mapType": "battle", "x": 0.2, "y": 0.8, "maxHp": 10, "hp": 7 }]
        })));
        assert_eq!(room.players.len(), 1);
        assert_eq!(room.sheets["p-hero"]["hp"], 12);
        assert_eq!(room.maps["battle"]["items"][0]["name"], "Поле");
        assert_eq!(room.maps["battle"]["items"][0]["published"], true);
        assert_eq!(room.active_map.as_deref(), Some("battle"));
        assert_eq!(
            room.active_map_id.as_deref(),
            Some(room.maps["battle"]["items"][0]["id"].as_str().unwrap())
        );
        assert_eq!(room.tokens.len(), 1);
        assert_eq!(room.tokens[0]["hp"], 7);
    }

    #[test]
    fn player_snapshot_hides_dm_secrets() {
        let mut room = RoomState::new(campaign(json!({
            "players": [
                { "playerId": "dm", "name": "Мастер" },
                { "playerId": "p-hero", "name": "Герой" }
            ],
            "sheets": { "p-hero": { "maxHp": 20, "hp": 15 } },
            "maps": { "battle": { "items": [{ "id": "m1", "url": "/files/a.png", "published": true }] } },
            "tokens": [
                { "id": "hero", "mapType": "battle", "assignedPlayerId": "p-hero", "maxHp": 99, "hp": 99 },
                { "id": "mob", "mapType": "battle", "side": "enemy", "maxHp": 30, "hp": 30 },
                { "id": "trap", "mapType": "battle", "hidden": true }
            ],
            "journal": "секрет",
            "soundboard": [{ "id": "s1", "name": "Гром", "ref": "/files/a.mp3" }],
            "chatHistory": [
                { "id": "c1", "text": "всем", "time": 1 },
                { "id": "c2", "text": "шёпот", "whisper": true, "playerId": "p-other", "to": "p-hero", "time": 2 },
                { "id": "c3", "text": "чужой шёпот", "whisper": true, "playerId": "p-other", "to": "p-third", "time": 3 }
            ]
        })));
        room.add_player("p-hero", "Герой", "key");

        let player = room.snapshot_for(&Viewer::player("p-hero"));
        assert_eq!(player["journal"], "");
        assert_eq!(player["sheets"].as_object().unwrap().len(), 1);
        assert_eq!(player["soundboard"], json!([]));
        let tokens = player["tokens"].as_array().unwrap();
        assert_eq!(tokens.len(), 2);
        let hero = tokens.iter().find(|token| token["id"] == "hero").unwrap();
        assert_eq!(hero["maxHp"], 20);
        assert_eq!(hero["hp"], 15);
        let mob = tokens.iter().find(|token| token["id"] == "mob").unwrap();
        assert_eq!(mob["maxHp"], Value::Null);
        assert_eq!(mob["armorClass"], Value::Null);
        let chat = player["chatHistory"].as_array().unwrap();
        assert_eq!(chat.len(), 2);
        assert!(chat.iter().any(|message| message["text"] == "шёпот"));

        let dm = room.snapshot_for(&Viewer::dm());
        assert_eq!(dm["journal"], "секрет");
        assert_eq!(dm["tokens"].as_array().unwrap().len(), 3);
        assert_eq!(dm["soundboard"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn players_sort_and_carry_tokens() {
        let mut room = RoomState::new(campaign(json!({
            "maps": { "battle": { "items": [{ "id": "m1", "url": "/files/a.png", "published": true }] } }
        })));
        room.add_dm("Мастер");
        room.add_player("p-b", "Борис", "b");
        room.add_player("p-a", "анна", "a");
        room.attach_socket("p-b", "s1");
        let token = room.normalize_token(&json!({
            "id": "t1", "mapType": "battle", "mapId": room.maps["battle"]["items"][0]["id"],
            "assignedPlayerId": "p-a"
        }));
        room.tokens.push(token);

        let list = room.player_list(PlayerListOptions::default());
        assert_eq!(list[0]["role"], ROLE_DM);
        assert_eq!(list[0]["avatar"], DEFAULT_DM_AVATAR);
        assert_eq!(list[1]["playerId"], "p-b");
        assert_eq!(list[1]["online"], true);
        assert_eq!(list[2]["tokenId"], "t1");
    }

    #[test]
    fn rejoining_keeps_the_sheet_and_rename_follows() {
        let mut room = RoomState::new(campaign(json!({})));
        room.add_player("p-hero", "Старое", "k");
        room.sheets.get_mut("p-hero").unwrap()["playerName"] = json!("Старое");
        room.sheets.get_mut("p-hero").unwrap()["hp"] = json!(5);
        room.add_player("p-hero", "Новое", "");
        assert_eq!(room.players["p-hero"]["name"], "Новое");
        assert_eq!(room.sheets["p-hero"]["hp"], 5);
        room.rename_player("p-hero", "Ещё новее");
        // The sheet keeps the name it was created with until it matches.
        assert_eq!(room.sheets["p-hero"]["playerName"], "Старое");
        room.sheets.get_mut("p-hero").unwrap()["playerName"] = json!("Ещё новее");
        room.rename_player("p-hero", "Третье");
        assert_eq!(room.sheets["p-hero"]["playerName"], "Третье");
    }

    #[test]
    fn hidden_and_unpublished_maps_hide_tokens() {
        let room = RoomState::new(campaign(json!({
            "maps": { "battle": {
                "items": [
                    { "id": "shown", "url": "/files/a.png", "published": true },
                    { "id": "draft", "url": "/files/b.png", "published": false }
                ],
                "activeId": "shown"
            } }
        })));
        let shown = json!({ "id": "t1", "mapType": "battle", "mapId": "shown" });
        let draft = json!({ "id": "t2", "mapType": "battle", "mapId": "draft" });
        let old = json!({ "id": "t3", "mapType": "battle", "mapId": "" });
        assert!(room.is_token_visible_to_players(&shown));
        assert!(!room.is_token_visible_to_players(&draft));
        assert!(room.is_token_visible_to_players(&old));
    }

    #[test]
    fn scenes_without_a_live_map_are_dropped() {
        let room = RoomState::new(campaign(json!({
            "maps": { "battle": { "items": [{ "id": "m1", "url": "/files/a.png" }] } }
        })));
        let scenes = room.normalize_scenes(&json!([
            { "id": "sc-1", "mapType": "battle", "mapId": "m1", "name": "Засада" },
            { "id": "sc-2", "mapType": "battle", "mapId": "missing", "name": "Призрак" }
        ]));
        assert_eq!(scenes.len(), 1);
        assert_eq!(scenes[0]["name"], "Засада");
        assert_eq!(room.scene_summary(&scenes[0])["mapName"], "Карта");
    }

    #[test]
    fn detached_last_socket_goes_offline() {
        let mut room = RoomState::new(campaign(json!({})));
        room.add_player("p", "Игрок", "");
        room.attach_socket("p", "s1");
        room.attach_socket("p", "s2");
        room.detach_socket("p", "s1");
        assert!(room.is_online("p"));
        room.detach_socket("p", "s2");
        assert!(!room.is_online("p"));
        assert_eq!(room.players["p"]["online"], false);
    }

    #[test]
    fn serialize_keeps_secrets_for_disk() {
        let mut room = RoomState::new(campaign(json!({})));
        room.add_player("p", "Игрок", "secret-key");
        room.attach_socket("p", "s1");
        let saved = room.serialize();
        assert_eq!(saved["players"][0]["playerKey"], "secret-key");
        assert!(saved["players"][0].get("online").is_none());
        assert!(saved.get("savedAt").is_some());
    }
}
