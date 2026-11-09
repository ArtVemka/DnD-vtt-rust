//! Desktop shell: hosts the table server in-process and shows the renderer.
//!
//! The webview loads the embedded renderer over Tauri's asset protocol. Players
//! on the network still get the same files from `<url>/` because the server
//! serves the renderer folder copied next to the executable.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dnd_core::campaign::CampaignStore;
use dnd_core::room::RoomState;
use dnd_core::sheet::normalize_adventure;
use dnd_server::create_app;
use dnd_server::net;
use dnd_server::table::{Table, create_io};
use serde_json::{Value, json};
use socketioxide::SocketIo;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::watch;

mod p2p;
mod presence;

/// Data lives where the Electron version kept it, so existing campaigns open.
fn portable_data_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("PORTABLE_EXECUTABLE_DIR") {
        return Some(PathBuf::from(dir).join("data"));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    dir.join("portable.txt").exists().then(|| dir.join("data"))
}

fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("DND_DATA") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = portable_data_dir() {
        return dir;
    }
    match std::env::var_os("APPDATA") {
        Some(dir) => PathBuf::from(dir).join("dnd-table"),
        None => PathBuf::from("data"),
    }
}

struct Host {
    port: u16,
    local_url: String,
    host_key: String,
    shutdown: watch::Sender<bool>,
    handle: tokio::task::JoinHandle<()>,
    table: Arc<Table>,
    io: SocketIo,
}

struct AppState {
    data_dir: PathBuf,
    campaigns_dir: PathBuf,
    portable: bool,
    current: Mutex<Option<Value>>,
    host: Mutex<Option<Host>>,
    contexts: Mutex<HashMap<String, Value>>,
    adventure_path: Mutex<Option<PathBuf>>,
    presence: presence::Presence,
    p2p_host: Mutex<Option<p2p::Host>>,
    p2p_tunnel: Mutex<Option<p2p::Tunnel>>,
    autosave_seconds: Arc<AtomicU64>,
}

impl AppState {
    fn new() -> Self {
        let data_dir = default_data_dir();
        let portable = std::env::var("DND_DATA").is_err() && portable_data_dir().is_some();
        Self {
            campaigns_dir: data_dir.join("campaigns"),
            data_dir,
            portable,
            current: Mutex::new(None),
            host: Mutex::new(None),
            contexts: Mutex::new(HashMap::new()),
            adventure_path: Mutex::new(None),
            presence: presence::Presence::start(),
            p2p_host: Mutex::new(None),
            p2p_tunnel: Mutex::new(None),
            autosave_seconds: Arc::new(AtomicU64::new(60)),
        }
    }

    fn store(&self) -> Result<CampaignStore, String> {
        CampaignStore::new(&self.campaigns_dir, None).map_err(|error| error.to_string())
    }
}

fn fail(message: impl Into<String>) -> Value {
    json!({ "ok": false, "error": message.into() })
}

fn meta(campaign: &Value) -> Value {
    json!({
        "id": campaign.get("id").cloned().unwrap_or(Value::Null),
        "name": campaign.get("name").cloned().unwrap_or(Value::Null),
        "createdAt": campaign.get("createdAt").cloned().unwrap_or(Value::Null),
        "updatedAt": campaign.get("updatedAt").cloned().unwrap_or(Value::Null),
        "version": campaign.get("version").cloned().unwrap_or(Value::Null)
    })
}

fn current_campaign(state: &AppState) -> Option<Value> {
    state.current.lock().unwrap().clone()
}

fn random_key() -> String {
    let first = uuid::Uuid::new_v4().simple().to_string();
    let second = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", &first[..32], &second[..16])
}

fn renderer_dir(app: &tauri::AppHandle) -> PathBuf {
    if let Ok(resources) = app.path().resource_dir() {
        let bundled = resources.join("renderer");
        if bundled.join("index.html").exists() {
            return bundled;
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../renderer");
    if dev.join("index.html").exists() {
        return dev;
    }
    PathBuf::from("renderer")
}

fn spawn_autosave(
    seconds: Arc<AtomicU64>,
    table: Arc<Table>,
    app: tauri::AppHandle,
    mut shutdown: watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.tick().await;
        let mut elapsed = 0u64;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    elapsed += 1;
                    if elapsed >= seconds.load(Ordering::Relaxed).clamp(5, 3600) {
                        elapsed = 0;
                        match table.save_if_dirty() {
                            Ok(true) => {
                                let campaign = table.room.lock().unwrap().campaign.clone();
                                let _ = app.emit(
                                    "campaign-saved",
                                    json!({ "meta": meta(&campaign), "reason": "auto" }),
                                );
                            }
                            Ok(false) => {}
                            Err(error) => {
                                eprintln!("[autosave] {error}");
                                let _ = app.emit(
                                    "campaign-error",
                                    json!({ "message": error.to_string() }),
                                );
                            }
                        }
                    }
                }
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        break;
                    }
                }
            }
        }
    });
}

async fn stop_host(state: &AppState) {
    let p2p_host = state.p2p_host.lock().unwrap().take();
    if let Some(host) = p2p_host {
        p2p::close_host(host).await;
    }
    let p2p_tunnel = state.p2p_tunnel.lock().unwrap().take();
    if let Some(tunnel) = p2p_tunnel {
        p2p::close_tunnel(tunnel).await;
    }
    let host = state.host.lock().unwrap().take();
    let Some(host) = host else {
        return;
    };
    let _ = host.shutdown.send(true);
    let _ = tokio::time::timeout(Duration::from_secs(3), host.handle).await;
    let campaign = host.table.room.lock().unwrap().campaign.clone();
    *state.current.lock().unwrap() = Some(campaign);
}

fn flush_on_exit(state: &AppState) {
    let host = state.host.lock().unwrap().take();
    if let Some(host) = host {
        let _ = host.table.save_if_dirty();
        let _ = host.shutdown.send(true);
    }
}

#[tauri::command]
fn app_config(state: State<'_, AppState>) -> Value {
    json!({
        "version": env!("CARGO_PKG_VERSION"),
        "versionLabel": env!("CARGO_PKG_VERSION"),
        "dataDir": state.data_dir.to_string_lossy(),
        "campaignsDir": state.campaigns_dir.to_string_lossy(),
        "portable": state.portable,
        "platform": "win32",
        "canUpdate": false,
        "isElectron": true
    })
}

#[tauri::command]
fn presence_update(state: State<'_, AppState>, input: Value) -> Value {
    state.presence.update(input);
    json!({ "ok": true })
}

#[tauri::command]
fn app_quit(app: tauri::AppHandle, state: State<'_, AppState>) -> Value {
    flush_on_exit(&state);
    app.exit(0);
    json!({ "ok": true })
}

#[tauri::command]
fn app_autosave_interval(state: State<'_, AppState>, seconds: f64) -> Value {
    let millis = (seconds * 1000.0).round();
    if !millis.is_finite() || !(5_000.0..=3_600_000.0).contains(&millis) {
        return fail("Недопустимый интервал автосохранения");
    }
    state
        .autosave_seconds
        .store((millis / 1000.0) as u64, Ordering::Relaxed);
    json!({ "ok": true, "intervalMs": millis })
}

#[tauri::command]
fn campaign_list(state: State<'_, AppState>) -> Value {
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return json!({ "campaigns": [], "error": error }),
    };
    match store.list() {
        Ok(campaigns) => json!({ "campaigns": campaigns }),
        Err(error) => json!({ "campaigns": [], "error": error.to_string() }),
    }
}

#[tauri::command]
async fn campaign_create(state: State<'_, AppState>, name: String) -> Result<Value, String> {
    stop_host(&state).await;
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    match store.create(&name) {
        Ok(campaign) => {
            let summary = meta(&campaign);
            *state.current.lock().unwrap() = Some(campaign);
            Ok(json!({ "ok": true, "campaign": summary }))
        }
        Err(error) => Ok(fail(error.to_string())),
    }
}

#[tauri::command]
async fn campaign_load(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    stop_host(&state).await;
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    match store.load(&id) {
        Ok(campaign) => {
            let summary = meta(&campaign);
            *state.current.lock().unwrap() = Some(campaign);
            Ok(json!({ "ok": true, "campaign": summary }))
        }
        Err(error) => Ok(fail(format!("Не удалось загрузить кампанию: {error}"))),
    }
}

#[tauri::command]
fn campaign_current(state: State<'_, AppState>) -> Value {
    current_campaign(&state)
        .as_ref()
        .map(meta)
        .unwrap_or(Value::Null)
}

#[tauri::command]
fn campaign_save(state: State<'_, AppState>) -> Value {
    let Some(campaign) = current_campaign(&state) else {
        return fail("Кампания не выбрана");
    };
    let id = campaign
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let hosting = state.host.lock().unwrap().is_some();
    if hosting {
        let saved = {
            let guard = state.host.lock().unwrap();
            guard.as_ref().map(|host| host.table.save_if_dirty())
        };
        if let Some(Err(error)) = saved {
            return fail(format!("Не удалось сохранить кампанию: {error}"));
        }
        let store = match state.store() {
            Ok(store) => store,
            Err(error) => return fail(error),
        };
        match store.load(&id) {
            Ok(updated) => {
                let summary = meta(&updated);
                *state.current.lock().unwrap() = Some(updated);
                return json!({ "ok": true, "campaign": summary });
            }
            Err(error) => return fail(format!("Не удалось сохранить кампанию: {error}")),
        }
    }

    json!({ "ok": true, "campaign": meta(&campaign) })
}

fn persist_before_copy(state: &AppState) -> Result<(), String> {
    let guard = state.host.lock().unwrap();
    if let Some(host) = guard.as_ref() {
        host.table
            .save_if_dirty()
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[tauri::command]
fn campaign_backup(state: State<'_, AppState>) -> Value {
    let Some(campaign) = current_campaign(&state) else {
        return fail("Сначала откройте кампанию");
    };
    let id = campaign.get("id").and_then(Value::as_str).unwrap_or("");
    let name = campaign.get("name").and_then(Value::as_str).unwrap_or("");
    if let Err(error) = persist_before_copy(&state) {
        return fail(format!("Не удалось сделать копию: {error}"));
    }
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return fail(error),
    };
    match store.backup(id, name) {
        Ok(dir) => json!({ "ok": true, "dir": dir.to_string_lossy() }),
        Err(error) => fail(format!("Не удалось сделать копию: {error}")),
    }
}

#[tauri::command]
async fn campaign_export(state: State<'_, AppState>) -> Result<Value, String> {
    let Some(campaign) = current_campaign(&state) else {
        return Ok(fail("Сначала откройте кампанию"));
    };
    let id = campaign
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let name = campaign
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if let Err(error) = persist_before_copy(&state) {
        return Ok(fail(format!("Не удалось экспортировать кампанию: {error}")));
    }

    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Куда сохранить кампанию")
            .pick_folder()
    })
    .await
    .ok()
    .flatten();
    let Some(target) = picked else {
        return Ok(json!({ "ok": false, "canceled": true }));
    };

    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    match store.export_to(&id, &name, &target) {
        Ok(dir) => Ok(json!({ "ok": true, "dir": dir.to_string_lossy() })),
        Err(error) => Ok(fail(format!("Не удалось экспортировать кампанию: {error}"))),
    }
}

#[tauri::command]
async fn campaign_import(state: State<'_, AppState>) -> Result<Value, String> {
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Выберите папку кампании")
            .pick_folder()
    })
    .await
    .ok()
    .flatten();
    let Some(source) = picked else {
        return Ok(json!({ "ok": false, "canceled": true }));
    };
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    match store.import_from(&source) {
        Ok(campaign) => Ok(json!({ "ok": true, "campaign": meta(&campaign) })),
        Err(error) => Ok(fail(format!("Не удалось импортировать кампанию: {error}"))),
    }
}

#[tauri::command]
fn campaign_backup_info(state: State<'_, AppState>) -> Value {
    let Some(campaign) = current_campaign(&state) else {
        return fail("Сначала откройте кампанию");
    };
    let id = campaign.get("id").and_then(Value::as_str).unwrap_or("");
    let name = campaign.get("name").and_then(Value::as_str).unwrap_or("");
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return fail(error),
    };
    match store.list_backups(id, name) {
        Ok(copies) => {
            let items: Vec<Value> = copies
                .iter()
                .map(|copy| {
                    let millis = copy.get("mtimeMs").and_then(Value::as_u64).unwrap_or(0);
                    json!({
                        "name": copy.get("name").cloned().unwrap_or(Value::Null),
                        "at": iso_from_millis(millis)
                    })
                })
                .collect();
            let last = items
                .first()
                .and_then(|item| item.get("at"))
                .cloned()
                .unwrap_or(Value::Null);
            json!({ "ok": true, "count": items.len(), "lastAt": last, "items": items })
        }
        Err(error) => fail(error.to_string()),
    }
}

fn iso_from_millis(millis: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp_millis(millis as i64)
        .map(|time| time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .unwrap_or_default()
}

#[tauri::command(rename_all = "camelCase")]
async fn campaign_restore_backup(
    state: State<'_, AppState>,
    id: String,
    backup_name: String,
) -> Result<Value, String> {
    let current_id = current_campaign(&state)
        .as_ref()
        .and_then(|campaign| campaign.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if current_id.as_deref() != Some(id.as_str()) {
        return Ok(fail("Сначала откройте кампанию"));
    }

    let was_hosting = state.host.lock().unwrap().is_some();
    if was_hosting {
        stop_host(&state).await;
    }
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    if let Err(error) = store.restore_backup(&id, &backup_name) {
        return Ok(fail(format!("Не удалось восстановить копию: {error}")));
    }
    match store.load(&id) {
        Ok(campaign) => {
            *state.current.lock().unwrap() = Some(campaign);
            Ok(json!({ "ok": true, "wasHosting": was_hosting }))
        }
        Err(error) => Ok(fail(format!("Не удалось восстановить копию: {error}"))),
    }
}

#[tauri::command]
async fn campaign_delete(state: State<'_, AppState>, id: String) -> Result<Value, String> {
    let current_id = current_campaign(&state)
        .as_ref()
        .and_then(|campaign| campaign.get("id"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if current_id.as_deref() == Some(id.as_str()) {
        stop_host(&state).await;
        *state.current.lock().unwrap() = None;
    }
    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    match store.remove(&id) {
        Ok(()) => Ok(json!({ "ok": true })),
        Err(error) => Ok(fail(error.to_string())),
    }
}

#[tauri::command]
async fn host_start(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<Value, String> {
    let Some(campaign) = current_campaign(&state) else {
        return Ok(fail("Сначала создайте или загрузите кампанию"));
    };
    stop_host(&state).await;

    let store = match state.store() {
        Ok(store) => store,
        Err(error) => return Ok(fail(error)),
    };
    let host_key = random_key();
    let port = net::find_free_port(32100, 128).await;
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", port)).await {
        Ok(listener) => listener,
        Err(error) => return Ok(fail(format!("Не удалось занять порт: {error}"))),
    };
    let port = match listener.local_addr() {
        Ok(addr) => addr.port(),
        Err(error) => return Ok(fail(error.to_string())),
    };

    let table = Table::new(
        RoomState::new(campaign),
        host_key.clone(),
        Some(Arc::new(store)),
    );
    let (layer, io) = create_io(table.clone());
    let renderer = renderer_dir(&app);
    let router = create_app(table.clone(), &renderer.to_string_lossy()).layer(layer);

    let (shutdown, mut receiver) = watch::channel(false);
    let server_table = table.clone();
    let handle = tokio::spawn(async move {
        let server = axum::serve(listener, router).with_graceful_shutdown(async move {
            let _ = receiver.changed().await;
        });
        if let Err(error) = server.await {
            eprintln!("[server] {error}");
        }
        let _ = server_table.save_if_dirty();
    });

    spawn_autosave(
        state.autosave_seconds.clone(),
        table.clone(),
        app.clone(),
        shutdown.subscribe(),
    );

    let local_url = format!("http://127.0.0.1:{port}");
    *state.host.lock().unwrap() = Some(Host {
        port,
        local_url: local_url.clone(),
        host_key: host_key.clone(),
        shutdown,
        handle,
        table,
        io,
    });

    Ok(json!({
        "ok": true,
        "port": port,
        "localUrl": local_url,
        "hostKey": host_key,
        "campaign": current_campaign(&state).as_ref().map(meta).unwrap_or(Value::Null)
    }))
}

#[tauri::command]
async fn host_stop(state: State<'_, AppState>) -> Result<Value, String> {
    stop_host(&state).await;
    Ok(json!({ "ok": true }))
}

#[tauri::command]
fn host_status(state: State<'_, AppState>) -> Value {
    let guard = state.host.lock().unwrap();
    let Some(host) = guard.as_ref() else {
        return json!({ "running": false });
    };
    json!({
        "running": true,
        "port": host.port,
        "localUrl": host.local_url,
        "hostKey": host.host_key,
        "campaign": current_campaign(&state).as_ref().map(meta).unwrap_or(Value::Null)
    })
}

async fn ensure_p2p_host(state: &AppState) -> Result<(String, String), String> {
    if let Some(host) = state.p2p_host.lock().unwrap().as_ref() {
        return Ok((p2p::endpoint_id(&host.endpoint), host.ticket.clone()));
    }
    let port = {
        let host = state.host.lock().unwrap();
        let Some(host) = host.as_ref() else {
            return Err("Сначала запустите стол.".to_string());
        };
        host.port
    };
    let host = p2p::start_host(port).await?;
    let id = p2p::endpoint_id(&host.endpoint);
    let ticket = host.ticket.clone();
    *state.p2p_host.lock().unwrap() = Some(host);
    Ok((id, ticket))
}

#[tauri::command]
async fn iroh_start(state: State<'_, AppState>) -> Result<Value, String> {
    match ensure_p2p_host(&state).await {
        Ok((id, ticket)) => Ok(json!({ "ok": true, "id": id, "ticket": ticket })),
        Err(error) => Ok(fail(error)),
    }
}

#[tauri::command]
async fn iroh_create_ticket(state: State<'_, AppState>) -> Result<Value, String> {
    match ensure_p2p_host(&state).await {
        Ok((id, ticket)) => Ok(json!({ "ok": true, "id": id, "ticket": ticket })),
        Err(error) => Ok(fail(error)),
    }
}

#[tauri::command]
async fn iroh_stop(state: State<'_, AppState>) -> Result<Value, String> {
    let host = state.p2p_host.lock().unwrap().take();
    if let Some(host) = host {
        p2p::close_host(host).await;
    }
    let tunnel = state.p2p_tunnel.lock().unwrap().take();
    if let Some(tunnel) = tunnel {
        p2p::close_tunnel(tunnel).await;
    }
    Ok(json!({ "ok": true }))
}

#[tauri::command(rename_all = "camelCase")]
async fn iroh_tunnel_client(
    state: State<'_, AppState>,
    ticket_str: String,
) -> Result<Value, String> {
    let previous = state.p2p_tunnel.lock().unwrap().take();
    if let Some(tunnel) = previous {
        p2p::close_tunnel(tunnel).await;
    }
    match p2p::start_tunnel(&ticket_str).await {
        Ok(tunnel) => {
            let local_port = tunnel.local_port;
            *state.p2p_tunnel.lock().unwrap() = Some(tunnel);
            Ok(json!({ "ok": true, "localPort": local_port }))
        }
        Err(error) => Ok(fail(format!("P2P не удалось: {error}"))),
    }
}

#[tauri::command]
async fn iroh_tunnel_stop(state: State<'_, AppState>) -> Result<Value, String> {
    let tunnel = state.p2p_tunnel.lock().unwrap().take();
    if let Some(tunnel) = tunnel {
        p2p::close_tunnel(tunnel).await;
    }
    Ok(json!({ "ok": true }))
}

#[tauri::command(rename_all = "camelCase")]
fn iroh_ticket_qr(ticket_str: String) -> Value {
    match p2p::qr_data_url(&ticket_str) {
        Ok(data_url) => json!({ "ok": true, "dataUrl": data_url }),
        Err(error) => fail(error),
    }
}

#[tauri::command]
fn shell_open_data_dir(state: State<'_, AppState>) -> Value {
    let _ = std::fs::create_dir_all(&state.data_dir);
    match open::that(&state.data_dir) {
        Ok(()) => json!({ "ok": true, "dir": state.data_dir.to_string_lossy() }),
        Err(error) => fail(error.to_string()),
    }
}

#[tauri::command]
fn shell_open_backups(state: State<'_, AppState>) -> Value {
    let dir = state.data_dir.join("backups");
    let _ = std::fs::create_dir_all(&dir);
    match open::that(&dir) {
        Ok(()) => json!({ "ok": true, "dir": dir.to_string_lossy() }),
        Err(error) => fail(error.to_string()),
    }
}

#[tauri::command]
fn shell_open_external(url: String) -> Value {
    if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:")) {
        return fail("Ссылка должна начинаться с http://, https:// или mailto:");
    }
    match open::that(&url) {
        Ok(()) => json!({ "ok": true }),
        Err(error) => fail(error.to_string()),
    }
}

fn open_context_window(
    app: &tauri::AppHandle,
    state: &AppState,
    kind: &str,
    page: &str,
    bridge: &str,
    title: String,
    context: Value,
) -> Value {
    let id = uuid::Uuid::new_v4().to_string();
    state.contexts.lock().unwrap().insert(id.clone(), context);

    let script = format!(
        "window.{bridge} = {{ isElectron: true, getContext: () => window.__TAURI__.core.invoke('window_context', {{ id: '{id}' }}) }};"
    );
    let label = format!("{kind}-{id}");
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(page.into()))
        .title(title)
        .icon(
            tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
                .expect("embedded window icon"),
        )
        .and_then(|builder| {
            builder
                .inner_size(900.0, 760.0)
                .min_inner_size(520.0, 420.0)
                .initialization_script(&script)
                .build()
        });
    match built {
        Ok(_) => json!({ "ok": true }),
        Err(error) => fail(error.to_string()),
    }
}

#[tauri::command]
fn window_context(state: State<'_, AppState>, id: String) -> Value {
    state
        .contexts
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .unwrap_or(Value::Null)
}

#[tauri::command]
fn sheet_open_window(app: tauri::AppHandle, state: State<'_, AppState>, request: Value) -> Value {
    let mut context = request.clone();
    let is_dm = context.get("role").and_then(Value::as_str) == Some("dm");
    if is_dm {
        context["playerId"] = json!("dm");
        if let Some(host) = state.host.lock().unwrap().as_ref() {
            context["hostKey"] = json!(host.host_key);
            context["url"] = json!(host.local_url);
        }
    }
    let title = context
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Лист персонажа")
        .to_string();
    open_context_window(
        &app,
        &state,
        "sheet",
        "sheet-window.html",
        "sheetWindow",
        title,
        context,
    )
}

#[tauri::command]
fn journal_open_window(app: tauri::AppHandle, state: State<'_, AppState>, request: Value) -> Value {
    let mut context = request.clone();
    context["playerId"] = json!("dm");
    if let Some(host) = state.host.lock().unwrap().as_ref() {
        context["hostKey"] = json!(host.host_key);
        context["url"] = json!(host.local_url);
    }
    let title = context
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Журнал кампании")
        .to_string();
    open_context_window(
        &app,
        &state,
        "journal",
        "journal-window.html",
        "journalWindow",
        title,
        context,
    )
}

#[tauri::command]
async fn adventure_pick_file(state: State<'_, AppState>) -> Result<Value, String> {
    if state.host.lock().unwrap().is_none() {
        return Ok(fail("Сначала запустите стол."));
    }
    let picked = tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Выберите файл приключения")
            .add_filter("Приключения Foundry", &["json", "zip"])
            .pick_file()
    })
    .await
    .ok()
    .flatten();
    let Some(path) = picked else {
        return Ok(json!({ "ok": false, "canceled": true }));
    };
    *state.adventure_path.lock().unwrap() = Some(path.clone());
    Ok(json!({ "ok": true, "filePath": path.to_string_lossy() }))
}

#[tauri::command]
async fn adventure_inspect(state: State<'_, AppState>, file_path: String) -> Result<Value, String> {
    if state.host.lock().unwrap().is_none() {
        return Ok(fail("Сначала запустите стол."));
    }
    if !path_was_picked(&state, &file_path) {
        return Ok(fail("Файл нужно выбрать в окне выбора приключения."));
    }
    let data = match std::fs::read(&file_path) {
        Ok(data) => data,
        Err(error) => return Ok(fail(error.to_string())),
    };
    let maps = {
        let host = state.host.lock().unwrap();
        let Some(host) = host.as_ref() else {
            return Ok(fail("Сначала запустите стол."));
        };
        let room = host.table.room.lock().unwrap();
        room.maps.clone()
    };
    let context = json!({ "campaignMaps": maps });
    match dnd_importer::inspect(
        &dnd_importer::Input {
            data: &data,
            path: Some(&file_path),
        },
        &context,
    ) {
        Ok(preview) => Ok(json!({ "ok": true, "preview": preview })),
        Err(error) => Ok(fail(error.to_string())),
    }
}

fn path_was_picked(state: &AppState, path: &str) -> bool {
    state
        .adventure_path
        .lock()
        .unwrap()
        .as_ref()
        .is_some_and(|picked| picked.to_string_lossy() == path)
}

#[tauri::command(rename_all = "camelCase")]
async fn adventure_import(
    state: State<'_, AppState>,
    file_path: String,
    include: Value,
) -> Result<Value, String> {
    if state.host.lock().unwrap().is_none() {
        return Ok(fail("Сначала запустите стол."));
    }
    if !path_was_picked(&state, &file_path) {
        return Ok(fail("Файл нужно выбрать в окне выбора приключения."));
    }
    let selected: Vec<Value> = include
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| json!(item.as_str().unwrap_or("")))
                .collect()
        })
        .unwrap_or_default();
    if selected.is_empty() {
        return Ok(fail("Вы не выбрали ни одного раздела."));
    }
    let data = match std::fs::read(&file_path) {
        Ok(data) => data,
        Err(error) => return Ok(fail(error.to_string())),
    };

    let (maps, table, io) = {
        let host = state.host.lock().unwrap();
        let Some(host) = host.as_ref() else {
            return Ok(fail("Сначала запустите стол."));
        };
        let maps = host.table.room.lock().unwrap().maps.clone();
        (maps, host.table.clone(), host.io.clone())
    };

    let context = json!({ "campaignMaps": maps });
    let options = json!({ "include": selected });
    let output = match dnd_importer::run_import(
        &dnd_importer::Input {
            data: &data,
            path: Some(&file_path),
        },
        &options,
        &context,
    ) {
        Ok(output) => output,
        Err(error) => return Ok(fail(error.to_string())),
    };

    let patch = output.get("patch").cloned().unwrap_or(Value::Null);
    let report = output.get("report").cloned().unwrap_or(Value::Null);

    let (broadcast_maps, added_tokens, adventure) = {
        let mut room = table.room.lock().unwrap();
        let mut next_maps = room.maps.clone();
        if let Some(slots) = patch.get("maps").and_then(Value::as_object) {
            for (kind, slot) in slots {
                next_maps[kind] = room.normalize_map_slot(slot);
            }
        }
        room.maps = next_maps;

        let limit = dnd_core::sheet::LIMITS.max_tokens as usize;
        let mut added = Vec::new();
        if let Some(tokens) = patch.get("tokens").and_then(Value::as_array) {
            for token in tokens {
                if room.tokens.len() >= limit {
                    break;
                }
                let token = room.normalize_token(token);
                room.tokens.push(token.clone());
                added.push(token);
            }
        }

        if let Some(adventure) = patch.get("adventure") {
            room.adventure = Some(normalize_adventure(adventure));
        }
        room.campaign["version"] = json!(2);

        let mut broadcast = Vec::new();
        if let Some(slots) = patch.get("maps").and_then(Value::as_object) {
            for (kind, slot) in slots {
                broadcast.push(json!({
                    "type": kind,
                    "items": slot.get("items").cloned().unwrap_or(Value::Array(Vec::new())),
                    "activeId": slot.get("activeId").cloned().unwrap_or(Value::Null),
                    "activeMap": room.active_map
                }));
            }
        }
        let adventure = room.adventure.clone();
        (broadcast, added, adventure)
    };

    table.mark_dirty();
    let _ = table.save_if_dirty();

    for message in broadcast_maps {
        let _ = io
            .within("table")
            .except("dm-sockets")
            .emit("mapUpdated", &message)
            .await;
        let _ = io.within("dm-sockets").emit("mapUpdated", &message).await;
    }
    for token in added_tokens {
        let full = {
            let room = table.room.lock().unwrap();
            room.token_for_viewer(&token, true)
        };
        let _ = io.within("dm-sockets").emit("tokenCreated", &full).await;
        let visible = table
            .room
            .lock()
            .unwrap()
            .is_token_visible_to_players(&token);
        if visible {
            let player = {
                let room = table.room.lock().unwrap();
                room.token_for_viewer(&token, false)
            };
            let _ = io
                .within("table")
                .except("dm-sockets")
                .emit("tokenCreated", &player)
                .await;
        }
    }

    Ok(json!({ "ok": true, "report": report, "adventure": adventure }))
}

fn tray_image() -> tauri::image::Image<'static> {
    tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
        .expect("embedded tray icon")
}

fn show_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "Открыть", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Выход", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    TrayIconBuilder::new()
        .icon(tray_image())
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_window(app),
            "quit" => {
                if let Some(state) = app.try_state::<AppState>() {
                    flush_on_exit(&state);
                }
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_window(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            show_window(app);
        }))
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            app_config,
            app_quit,
            presence_update,
            app_autosave_interval,
            campaign_list,
            campaign_create,
            campaign_load,
            campaign_current,
            campaign_save,
            campaign_backup,
            campaign_export,
            campaign_import,
            campaign_backup_info,
            campaign_restore_backup,
            campaign_delete,
            host_start,
            host_stop,
            host_status,
            iroh_start,
            iroh_create_ticket,
            iroh_stop,
            iroh_tunnel_client,
            iroh_tunnel_stop,
            iroh_ticket_qr,
            shell_open_data_dir,
            shell_open_backups,
            shell_open_external,
            window_context,
            sheet_open_window,
            journal_open_window,
            adventure_pick_file,
            adventure_inspect,
            adventure_import
        ])
        .setup(|app| {
            let window =
                WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                    .title("D&D Стол")
                    .icon(
                        tauri::image::Image::from_bytes(include_bytes!("../icons/128x128.png"))
                            .expect("embedded window icon"),
                    )?
                    .inner_size(1280.0, 800.0)
                    .min_inner_size(960.0, 640.0)
                    .initialization_script(include_str!("bridge.js"))
                    .build()?;
            let _ = window;
            setup_tray(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .build(tauri::generate_context!())
        .expect("не удалось собрать приложение");

    app.run(|app, event| {
        if let tauri::RunEvent::ExitRequested { .. } = event {
            if let Some(state) = app.try_state::<AppState>() {
                flush_on_exit(&state);
            }
        }
    });
}
