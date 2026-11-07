//! Campaign storage on disk. Port of `server/campaign.js`.
//!
//! ```text
//! <dataDir>/campaigns/<id>/campaign.json
//! <dataDir>/campaigns/<id>/files/<name>
//! ```
//!
//! Synchronous on purpose: the payloads are JSON documents and images up to a
//! few megabytes, and the desktop calls this outside the socket loop.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::model::{CAMPAIGN_FORMAT_VERSION, empty_maps};
use crate::sheet::sanitize_text;

pub const MAX_FILES_BYTES: u64 = 512 * 1024 * 1024;
const SWEEP_KEEP_MS: u64 = 2 * 60 * 1000;
const KEEP_BACKUPS: usize = 5;

#[derive(Debug)]
pub enum CampaignError {
    Quota { file_name: String, message: String },
    Message(String),
    Io(io::Error),
}

impl std::fmt::Display for CampaignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CampaignError::Quota { message, .. } => f.write_str(message),
            CampaignError::Message(message) => f.write_str(message),
            CampaignError::Io(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for CampaignError {}

impl From<io::Error> for CampaignError {
    fn from(error: io::Error) -> Self {
        CampaignError::Io(error)
    }
}

impl CampaignError {
    pub fn message(text: impl Into<String>) -> Self {
        CampaignError::Message(text.into())
    }
}

pub type Result<T> = std::result::Result<T, CampaignError>;

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn now_ms() -> i64 {
    Utc::now().timestamp_millis()
}

fn stamp() -> String {
    now_iso().replace(':', "-").replace('.', "-")
}

fn base36(mut value: u64) -> String {
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
    String::from_utf8(out).expect("base36 is ascii")
}

fn random_hex(bytes: usize) -> String {
    Uuid::new_v4().simple().to_string()[..bytes * 2].to_string()
}

fn valid_campaign_id(id: &str) -> bool {
    (4..=64).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub fn valid_file_name(name: &str) -> bool {
    (1..=80).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

fn image_ext(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "png" => Some("png"),
        "jpg" | "jpeg" => Some("jpg"),
        "webp" => Some("webp"),
        _ => None,
    }
}

fn audio_ext(ext: &str) -> Option<&'static str> {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" | "mpeg" => Some("mp3"),
        "ogg" => Some("ogg"),
        "wav" | "x-wav" => Some("wav"),
        _ => None,
    }
}

fn maps_have_images(slot: &Value) -> bool {
    if !slot.is_object() {
        return false;
    }
    match slot.get("items") {
        Some(Value::Array(items)) => !items.is_empty(),
        _ => slot.get("url").is_some_and(Value::is_string),
    }
}

fn add_ref(refs: &mut std::collections::HashSet<String>, value: Option<&Value>) {
    let Some(text) = value.and_then(Value::as_str) else {
        return;
    };
    let Some(name) = text.strip_prefix("/files/") else {
        return;
    };
    if valid_file_name(name) {
        refs.insert(name.to_string());
    }
}

/// File names the state still references, so the sweeper keeps them.
pub fn collect_file_refs(state: &Value) -> std::collections::HashSet<String> {
    let mut refs = std::collections::HashSet::new();

    if let Some(maps) = state.get("maps").and_then(Value::as_object) {
        for slot in maps.values() {
            if !slot.is_object() {
                continue;
            }
            add_ref(&mut refs, slot.get("url"));
            for item in slot
                .get("items")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                add_ref(&mut refs, item.get("url"));
                add_ref(&mut refs, item.get("audioRef"));
            }
        }
    }
    for sound in state
        .get("soundboard")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        add_ref(&mut refs, sound.get("ref"));
    }
    for token in state
        .get("tokens")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        add_ref(&mut refs, token.get("imageUrl"));
    }
    for player in state
        .get("players")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        add_ref(&mut refs, player.get("avatar"));
    }
    for message in state
        .get("chatHistory")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        add_ref(&mut refs, message.get("image"));
    }
    if let Some(adventure) = state.get("adventure") {
        for actor in adventure
            .get("actors")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            add_ref(&mut refs, actor.get("image"));
        }
        for item in adventure
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            add_ref(&mut refs, item.get("image"));
        }
    }
    refs
}

fn copy_dir(source: &Path, target: &Path) -> io::Result<()> {
    fs::create_dir_all(target)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let from = entry.path();
        let to = target.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

fn remove_dir(path: &Path) {
    if let Err(error) = fs::remove_dir_all(path) {
        if error.kind() != io::ErrorKind::NotFound {
            eprintln!("[campaign] не удалось удалить {}: {error}", path.display());
        }
    }
}

pub struct CampaignStore {
    root_dir: PathBuf,
    max_files_bytes: u64,
}

impl CampaignStore {
    pub fn new(root_dir: impl Into<PathBuf>, max_files_bytes: Option<u64>) -> Result<Self> {
        let root_dir = root_dir.into();
        fs::create_dir_all(&root_dir)?;
        Ok(Self {
            root_dir,
            max_files_bytes: max_files_bytes.unwrap_or(MAX_FILES_BYTES),
        })
    }

    pub fn dir_for(&self, campaign_id: &str) -> Result<PathBuf> {
        if !valid_campaign_id(campaign_id) {
            return Err(CampaignError::message(
                "Некорректный идентификатор кампании",
            ));
        }
        Ok(self.root_dir.join(campaign_id))
    }

    pub fn files_dir(&self, campaign_id: &str) -> Result<PathBuf> {
        Ok(self.dir_for(campaign_id)?.join("files"))
    }

    pub fn file_path(&self, campaign_id: &str, file_name: &str) -> Result<PathBuf> {
        let safe = Path::new(file_name)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        if !valid_file_name(&safe) {
            return Err(CampaignError::message("Некорректное имя файла"));
        }
        Ok(self.files_dir(campaign_id)?.join(safe))
    }

    /// Saved campaigns for the load screen, freshest first.
    pub fn list(&self) -> Result<Vec<Value>> {
        let mut campaigns = Vec::new();
        for entry in fs::read_dir(&self.root_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let path = entry.path().join("campaign.json");
            let Ok(raw) = fs::read_to_string(path) else {
                continue;
            };
            let Ok(data) = serde_json::from_str::<Value>(&raw) else {
                continue;
            };
            let state = data.get("state");
            let count = |key: &str| {
                state
                    .and_then(|state| state.get(key))
                    .and_then(Value::as_array)
                    .map(Vec::len)
                    .unwrap_or(0)
            };
            let version = data
                .get("version")
                .and_then(Value::as_i64)
                .filter(|value| *value != 0)
                .unwrap_or(CAMPAIGN_FORMAT_VERSION);
            let slot = |key: &str| {
                state
                    .and_then(|state| state.get("maps"))
                    .and_then(|maps| maps.get(key))
                    .is_some_and(maps_have_images)
            };
            campaigns.push(json!({
                "id": entry.file_name().to_string_lossy(),
                "name": data.get("name").cloned().unwrap_or(Value::Null),
                "createdAt": data.get("createdAt").cloned().unwrap_or(Value::Null),
                "updatedAt": data.get("updatedAt").cloned().unwrap_or(Value::Null),
                "version": version,
                "players": count("players"),
                "tokens": count("tokens"),
                "maps": {
                    "world": slot("world"),
                    "battle": slot("battle"),
                    "sketch": slot("sketch")
                }
            }));
        }
        campaigns.sort_by(|a, b| {
            let left = a.get("updatedAt").and_then(Value::as_str).unwrap_or("");
            let right = b.get("updatedAt").and_then(Value::as_str).unwrap_or("");
            right.cmp(left)
        });
        Ok(campaigns)
    }

    pub fn create(&self, name: &str) -> Result<Value> {
        let trimmed = sanitize_text(&Value::String(name.to_string()), 60)
            .trim()
            .to_string();
        let title = if trimmed.is_empty() {
            "Новая кампания".to_string()
        } else {
            trimmed
        };
        let id = Uuid::new_v4().to_string();
        let now = now_iso();
        let mut campaign = json!({
            "id": id,
            "name": title,
            "createdAt": now,
            "updatedAt": now,
            "version": CAMPAIGN_FORMAT_VERSION,
            "state": {
                "players": [],
                "sheets": {},
                "maps": empty_maps(),
                "activeMap": Value::Null,
                "grid": { "enabled": false, "cell": 46 },
                "tokens": [],
                "diceHistory": [],
                "chatHistory": [],
                "journal": "",
                "savedAt": now
            }
        });
        fs::create_dir_all(self.files_dir(&id)?)?;
        self.save(&mut campaign)?;
        Ok(campaign)
    }

    pub fn load(&self, id: &str) -> Result<Value> {
        let file = self.dir_for(id)?.join("campaign.json");
        let raw = fs::read_to_string(file)?;
        let data: Value = serde_json::from_str(&raw)
            .map_err(|_| CampaignError::message("Файл кампании повреждён"))?;
        if !data.is_object() {
            return Err(CampaignError::message("Файл кампании повреждён"));
        }
        fs::create_dir_all(self.files_dir(id)?)?;
        let name = sanitize_text(data.get("name").unwrap_or(&Value::Null), 60);
        let name = if name.is_empty() {
            "Кампания".to_string()
        } else {
            name
        };
        Ok(json!({
            "id": id,
            "name": name,
            "createdAt": data.get("createdAt").cloned().unwrap_or_else(|| json!(now_iso())),
            "updatedAt": data.get("updatedAt").cloned().unwrap_or_else(|| json!(now_iso())),
            "version": data.get("version").and_then(Value::as_i64).filter(|v| *v != 0).unwrap_or(CAMPAIGN_FORMAT_VERSION),
            "state": data.get("state").filter(|state| state.is_object()).cloned().unwrap_or_else(|| json!({}))
        }))
    }

    /// Writes the campaign, keeping the previous file as `.bak`.
    pub fn save(&self, campaign: &mut Value) -> Result<()> {
        let id = campaign
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| CampaignError::message("Некорректный идентификатор кампании"))?;
        let dir = self.dir_for(&id)?;
        fs::create_dir_all(self.files_dir(&id)?)?;

        let object = campaign
            .as_object_mut()
            .ok_or_else(|| CampaignError::message("Кампания должна быть объектом"))?;
        object.insert("updatedAt".to_string(), json!(now_iso()));
        let version_ok = object
            .get("version")
            .and_then(Value::as_i64)
            .is_some_and(|value| value != 0);
        if !version_ok {
            object.insert("version".to_string(), json!(CAMPAIGN_FORMAT_VERSION));
        }

        let payload = serde_json::to_string_pretty(campaign)
            .map_err(|error| CampaignError::message(error.to_string()))?;
        let target = dir.join("campaign.json");
        let tmp = dir.join("campaign.json.tmp");
        fs::write(&tmp, payload)?;
        if target.exists() {
            fs::copy(&target, dir.join("campaign.json.bak"))?;
        }
        fs::rename(&tmp, &target)?;
        Ok(())
    }

    pub fn backups_dir(&self) -> PathBuf {
        self.root_dir
            .parent()
            .unwrap_or(Path::new("."))
            .join("backups")
    }

    pub fn backup_label(&self, campaign_id: &str, label: &str) -> String {
        let source = if label.is_empty() { campaign_id } else { label };
        let filtered: String = source
            .chars()
            .filter(|c| {
                !matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
                    && !c.is_control()
            })
            .collect();
        let trimmed = filtered.trim();
        let mut dashed = String::new();
        let mut in_space = false;
        for c in trimmed.chars() {
            if c.is_whitespace() {
                if !in_space {
                    dashed.push('-');
                    in_space = true;
                }
            } else {
                dashed.push(c);
                in_space = false;
            }
        }
        let label: String = dashed.chars().take(60).collect();
        if label.is_empty() {
            campaign_id.to_string()
        } else {
            label
        }
    }

    pub fn export_to(&self, campaign_id: &str, label: &str, target_dir: &Path) -> Result<PathBuf> {
        let source = self.dir_for(campaign_id)?;
        let base = self.backup_label(campaign_id, label);
        let mut target = target_dir.join(&base);
        if target.exists() {
            target = target_dir.join(format!("{base}-{}", now_ms()));
        }
        copy_dir(&source, &target)?;
        Ok(target)
    }

    pub fn import_from(&self, source_dir: &Path) -> Result<Value> {
        let raw = fs::read_to_string(source_dir.join("campaign.json"))?;
        let mut data: Value = serde_json::from_str(&raw)
            .map_err(|_| CampaignError::message("в папке нет campaign.json"))?;
        if !data.is_object() {
            return Err(CampaignError::message("в папке нет campaign.json"));
        }
        let id = Uuid::new_v4().to_string();
        let dest = self.dir_for(&id)?;
        copy_dir(source_dir, &dest)?;
        let updated = now_iso();
        data["id"] = json!(id);
        data["updatedAt"] = json!(updated);
        let payload = serde_json::to_string_pretty(&data)
            .map_err(|error| CampaignError::message(error.to_string()))?;
        fs::write(dest.join("campaign.json"), payload)?;
        let name = sanitize_text(data.get("name").unwrap_or(&Value::Null), 60);
        Ok(json!({
            "id": id,
            "name": if name.is_empty() { "Импортированная кампания".to_string() } else { name },
            "createdAt": data.get("createdAt").cloned().unwrap_or(Value::Null),
            "updatedAt": updated
        }))
    }

    pub fn list_backups(&self, campaign_id: &str, label: &str) -> Result<Vec<Value>> {
        let root = self.backups_dir();
        let prefix = format!("{}-", self.backup_label(campaign_id, label));
        let Ok(entries) = fs::read_dir(&root) else {
            return Ok(Vec::new());
        };
        let mut result = Vec::new();
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            if !file_type.is_dir() || !name.starts_with(&prefix) {
                continue;
            }
            let full = root.join(&name);
            let mtime = fs::metadata(&full)
                .and_then(|meta| meta.modified())
                .map(|time| {
                    time.duration_since(UNIX_EPOCH)
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            result.push(json!({ "name": name, "path": full.to_string_lossy(), "mtimeMs": mtime }));
        }
        result.sort_by(|a, b| {
            let left = a.get("mtimeMs").and_then(Value::as_u64).unwrap_or(0);
            let right = b.get("mtimeMs").and_then(Value::as_u64).unwrap_or(0);
            right.cmp(&left)
        });
        Ok(result)
    }

    /// Millisecond timestamp of the newest backup.
    pub fn last_backup_time(&self, campaign_id: &str, label: &str) -> Result<Option<u64>> {
        let list = self.list_backups(campaign_id, label)?;
        Ok(list
            .first()
            .and_then(|item| item.get("mtimeMs"))
            .and_then(Value::as_u64))
    }

    pub fn backup(&self, campaign_id: &str, label: &str) -> Result<PathBuf> {
        let source = self.dir_for(campaign_id)?;
        let root = self.backups_dir();
        fs::create_dir_all(&root)?;
        let safe = self.backup_label(campaign_id, label);
        let dest = root.join(format!("{safe}-{}", stamp()));
        copy_dir(&source, &dest)?;

        let mut names: Vec<String> = fs::read_dir(&root)?
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .filter(|name| name.starts_with(&format!("{safe}-")))
            .collect();
        names.sort();
        while names.len() > KEEP_BACKUPS {
            let oldest = names.remove(0);
            remove_dir(&root.join(oldest));
        }
        Ok(dest)
    }

    pub fn restore_backup(&self, campaign_id: &str, backup_name: &str) -> Result<PathBuf> {
        let name = backup_name;
        let base = Path::new(name).file_name().and_then(|part| part.to_str());
        if name.is_empty() || base != Some(name) || name.contains("..") {
            return Err(CampaignError::message("Некорректное имя копии"));
        }
        let root = self.backups_dir();
        let source = root.join(name);
        let resolved_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        let resolved_source = source.canonicalize().unwrap_or_else(|_| source.clone());
        if resolved_source.parent() != Some(resolved_root.as_path()) {
            return Err(CampaignError::message("Копия вне папки копий"));
        }
        if !source.is_dir() {
            return Err(CampaignError::message("Копия не найдена"));
        }

        let target = self.dir_for(campaign_id)?;
        let incoming = target.with_file_name(format!(
            "{}.restore-{}",
            target.file_name().unwrap_or_default().to_string_lossy(),
            stamp()
        ));
        copy_dir(&source, &incoming)?;
        remove_dir(&target);
        if fs::rename(&incoming, &target).is_err() {
            copy_dir(&incoming, &target)?;
            remove_dir(&incoming);
        }
        Ok(target)
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        remove_dir(&self.dir_for(id)?);
        Ok(())
    }

    pub fn save_image(&self, campaign_id: &str, bytes: &[u8], ext: &str) -> Result<String> {
        self.save_file(campaign_id, bytes, ext, image_ext, "png")
    }

    pub fn save_audio(&self, campaign_id: &str, bytes: &[u8], ext: &str) -> Result<String> {
        self.save_file(campaign_id, bytes, ext, audio_ext, "mp3")
    }

    fn save_file(
        &self,
        campaign_id: &str,
        bytes: &[u8],
        ext: &str,
        whitelist: fn(&str) -> Option<&'static str>,
        fallback: &str,
    ) -> Result<String> {
        let safe_ext = whitelist(ext).unwrap_or(fallback);
        let file_name = format!("{}-{}.{safe_ext}", base36(now_ms() as u64), random_hex(6));
        fs::create_dir_all(self.files_dir(campaign_id)?)?;
        let used = self.files_bytes(campaign_id)?;
        if used + bytes.len() as u64 > self.max_files_bytes {
            let megabytes = self.max_files_bytes / 1024 / 1024;
            return Err(CampaignError::Quota {
                file_name: file_name.clone(),
                message: format!(
                    "В папке кампании больше {megabytes} МБ изображений. Удалите лишние карты или портреты."
                ),
            });
        }
        fs::write(self.file_path(campaign_id, &file_name)?, bytes)?;
        Ok(file_name)
    }

    pub fn files_bytes(&self, campaign_id: &str) -> Result<u64> {
        let Ok(entries) = fs::read_dir(self.files_dir(campaign_id)?) else {
            return Ok(0);
        };
        let mut total = 0;
        for entry in entries.flatten() {
            if entry
                .file_type()
                .map(|kind| kind.is_file())
                .unwrap_or(false)
            {
                total += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            }
        }
        Ok(total)
    }

    /// Deletes files no longer referenced by the state.
    pub fn sweep_files(
        &self,
        campaign_id: &str,
        state: &Value,
        keep_ms: Option<u64>,
    ) -> Result<usize> {
        let keep_ms = keep_ms.unwrap_or(SWEEP_KEEP_MS);
        let keep = collect_file_refs(state);
        let Ok(entries) = fs::read_dir(self.files_dir(campaign_id)?) else {
            return Ok(0);
        };
        let now = now_ms();
        let mut removed = 0;
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            if !file_type.is_file() || keep.contains(&name) {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            let mtime_ms = meta
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            if keep_ms > 0 && now - mtime_ms < keep_ms as i64 {
                continue;
            }
            if fs::remove_file(entry.path()).is_ok() {
                removed += 1;
            }
        }
        Ok(removed)
    }

    pub fn file_size(&self, campaign_id: &str, file_name: &str) -> u64 {
        self.file_path(campaign_id, file_name)
            .ok()
            .and_then(|path| fs::metadata(path).ok())
            .map(|meta| meta.len())
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root() -> PathBuf {
        std::env::temp_dir().join(format!("dnd-campaign-{}", Uuid::new_v4()))
    }

    fn store() -> (CampaignStore, PathBuf) {
        let root = temp_root();
        let store = CampaignStore::new(&root, None).expect("store");
        (store, root)
    }

    #[test]
    fn creates_lists_and_loads() {
        let (store, root) = store();
        let campaign = store.create("  Шахты  ").expect("create");
        assert_eq!(campaign["name"], "Шахты");

        let list = store.list().expect("list");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["id"], campaign["id"]);
        assert_eq!(list[0]["players"], 0);
        assert_eq!(list[0]["maps"]["world"], false);

        let loaded = store.load(campaign["id"].as_str().unwrap()).expect("load");
        assert_eq!(loaded["name"], "Шахты");
        assert_eq!(loaded["state"]["maps"]["battle"]["items"], json!([]));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_bad_ids_and_names() {
        let (store, root) = store();
        assert!(store.load("../../etc").is_err());
        // `basename` semantics, same as the JS: the sheet name is what counts.
        assert!(store.file_path("abcd", "../x.png").is_ok());
        assert!(store.file_path("abcd", "ok-name_1.png").is_ok());
        assert!(store.file_path("abcd", "bad name.png").is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn saves_atomically_with_backup_copy() {
        let (store, root) = store();
        let mut campaign = store.create("Тест").expect("create");
        campaign["name"] = json!("Новое имя");
        store.save(&mut campaign).expect("save");
        assert!(
            root.join(campaign["id"].as_str().unwrap())
                .join("campaign.json.bak")
                .exists()
        );
        let loaded = store.load(campaign["id"].as_str().unwrap()).expect("load");
        assert_eq!(loaded["name"], "Новое имя");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn enforces_file_quota() {
        let root = temp_root();
        let store = CampaignStore::new(&root, Some(10)).expect("store");
        let campaign = store.create("Квота").expect("create");
        let id = campaign["id"].as_str().unwrap();
        let error = store.save_image(id, &[0u8; 20], "png").unwrap_err();
        assert!(matches!(error, CampaignError::Quota { .. }));
        assert_eq!(
            error.to_string(),
            "В папке кампании больше 0 МБ изображений. Удалите лишние карты или портреты."
        );
        assert!(store.save_image(id, &[0u8; 4], "png").is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sweeps_unreferenced_files_only() {
        let (store, root) = store();
        let campaign = store.create("Свалка").expect("create");
        let id = campaign["id"].as_str().unwrap();
        let kept = store.save_image(id, b"keep", "png").expect("save");
        let dropped = store.save_image(id, b"drop", "png").expect("save");
        let state =
            json!({ "maps": { "world": { "items": [{ "url": format!("/files/{kept}") }] } } });
        let removed = store.sweep_files(id, &state, Some(0)).expect("sweep");
        assert_eq!(removed, 1);
        let files = store.files_dir(id).unwrap();
        assert!(files.join(&kept).exists());
        assert!(!files.join(&dropped).exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn exports_and_imports_under_new_id() {
        let (store, root) = store();
        let campaign = store.create("Экспорт").expect("create");
        let id = campaign["id"].as_str().unwrap();
        store.save_image(id, b"img", "png").expect("save");

        let target = root.join("export");
        fs::create_dir_all(&target).expect("target");
        let exported = store.export_to(id, "Экспорт", &target).expect("export");
        let imported = store.import_from(&exported).expect("import");
        assert_ne!(imported["id"], campaign["id"]);
        assert_eq!(imported["name"], "Экспорт");
        let loaded = store.load(imported["id"].as_str().unwrap()).expect("load");
        assert_eq!(loaded["name"], "Экспорт");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn keeps_five_backups() {
        let (store, root) = store();
        let campaign = store.create("Копии").expect("create");
        let id = campaign["id"].as_str().unwrap();
        for _ in 0..6 {
            store.backup(id, "Копии").expect("backup");
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let backups = store.list_backups(id, "Копии").expect("list");
        assert_eq!(backups.len(), 5);
        let newest = backups[0]["name"].as_str().unwrap().to_string();
        store.restore_backup(id, &newest).expect("restore");
        assert!(store.load(id).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn label_strips_forbidden_characters() {
        let (store, root) = store();
        assert_eq!(store.backup_label("abcd", "Моя: игра?"), "Моя-игра");
        assert_eq!(store.backup_label("abcd", "   "), "abcd");
        assert_eq!(store.backup_label("abcd", "a  b"), "a-b");
        let _ = fs::remove_dir_all(root);
    }
}
