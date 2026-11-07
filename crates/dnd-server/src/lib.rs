//! Realtime table server: axum + Socket.IO + campaign storage.
//!
//! The reference (E:\DnD\server) stays the behaviour oracle; parity is checked
//! by `tools/` scripts against the exact renderer client.

pub mod auth;
pub mod handlers;
pub mod net;
pub mod table;

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde_json::{Value, json};
use tower_http::cors::CorsLayer;
use tower_http::services::ServeDir;

use table::Table;

pub fn create_app(table: Arc<Table>, static_dir: &str) -> Router {
    Router::new()
        .route("/", get(root))
        .route("/api/info", get(info))
        .route("/files/{name}", get(files))
        .fallback_service(ServeDir::new(static_dir))
        .layer(CorsLayer::permissive())
        .with_state(table)
}

async fn root(State(table): State<Arc<Table>>) -> Response {
    let name = {
        let room = table.room.lock().unwrap();
        room.campaign
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("Кампания")
            .to_string()
    };
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        format!("Стол D&D «{name}» работает.\nПодключайтесь через приложение.\n"),
    )
        .into_response()
}

async fn info(State(table): State<Arc<Table>>) -> Response {
    let (id, name, players) = {
        let room = table.room.lock().unwrap();
        (
            room.campaign.get("id").cloned().unwrap_or(Value::Null),
            room.campaign.get("name").cloned().unwrap_or(Value::Null),
            room.players.len(),
        )
    };
    axum::Json(json!({
        "ok": true,
        "campaign": { "id": id, "name": name },
        "players": players
    }))
    .into_response()
}

/// Serves an uploaded map, token image or sound from the campaign folder.
async fn files(State(table): State<Arc<Table>>, Path(name): Path<String>) -> Response {
    let Some(store) = &table.store else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(campaign_id) = table.campaign_id() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(path) = store.file_path(&campaign_id, &name) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, mime_for(&name)),
                (header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

fn mime_for(name: &str) -> &'static str {
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}
