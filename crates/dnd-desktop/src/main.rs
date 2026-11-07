//! Desktop shell: hosts the table server in-process and shows the renderer.
//!
//! The webview loads the embedded renderer over Tauri's asset protocol. Players
//! on the network still get the same files from `<url>/` because the server
//! serves the renderer folder copied next to the executable.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use dnd_core::campaign::CampaignStore;
use dnd_core::room::RoomState;
use dnd_server::net;
use dnd_server::table::{create_io, Table};
use dnd_server::create_app;
use serde_json::{json, Value};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, State, WebviewUrl, WebviewWindowBuilder};
use tokio::sync::watch;

/// Data lives where the Electron version kept it, so existing campaigns open.
fn default_data_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("DND_DATA") {
        return PathBuf::from(dir);
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
}

struct AppState {
    data_dir: PathBuf,
    campaigns_dir: PathBuf,
    current: Mutex<Option<Value>>,
    host: Mutex<Option<Host>>,
    autosave_seconds: Arc<AtomicU64>,
}

impl AppState {
    fn new() -> Self {
        let data_dir = default_data_dir();
        Self {
            campaigns_dir: data_dir.join("campaigns"),
            data_dir,
            current: Mutex::new(None),
            host: Mutex::new(None),
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

fn spawn_autosave(seconds: Arc<AtomicU64>, table: Arc<Table>, mut shutdown: watch::Receiver<bool>) {
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
                        if let Err(error) = table.save_if_dirty() {
                            eprintln!("[autosave] {error}");
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
        "portable": false,
        "platform": "win32",
        "canUpdate": false,
        "isElectron": true
    })
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
    let (layer, _io) = create_io(table.clone());
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

fn tray_image() -> tauri::image::Image<'static> {
    let size = 32u32;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            let color = if dx * dx + dy * dy <= 14 * 14 {
                [201, 111, 74, 255]
            } else {
                [0, 0, 0, 0]
            };
            rgba.extend_from_slice(&color);
        }
    }
    tauri::image::Image::new_owned(rgba, size, size)
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
        .manage(AppState::new())
        .invoke_handler(tauri::generate_handler![
            app_config,
            app_quit,
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
            shell_open_data_dir,
            shell_open_backups,
            shell_open_external
        ])
        .setup(|app| {
            let window = WebviewWindowBuilder::new(
                app,
                "main",
                WebviewUrl::App("index.html".into()),
            )
            .title("D&D Стол")
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
