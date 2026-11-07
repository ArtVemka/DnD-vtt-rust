//! Realtime table server entrypoint.
//!
//! Environment:
//! - `DND_DATA`      data dir (default `data`), campaigns live in `<dir>/campaigns`
//! - `DND_CAMPAIGN`  campaign id to open; otherwise the newest or a new one
//! - `DND_HOST_KEY`  DM secret; generated and printed when missing
//! - `DND_RENDERER`  renderer folder (default `renderer`)
//! - `DND_PORT`      fixed port; otherwise the reference range 32100..32164

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use dnd_core::campaign::CampaignStore;
use dnd_core::room::RoomState;
use dnd_server::create_app;
use dnd_server::net;
use dnd_server::table::{Table, create_io};
use serde_json::Value;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let data_dir = std::env::var("DND_DATA").unwrap_or_else(|_| "data".to_string());
    let campaigns_root = Path::new(&data_dir).join("campaigns");
    let store = CampaignStore::new(&campaigns_root, None)?;

    let campaign = match std::env::var("DND_CAMPAIGN").ok() {
        Some(id) => store.load(&id)?,
        None => {
            let newest = store
                .list()?
                .first()
                .and_then(|campaign| campaign.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string);
            match newest {
                Some(id) => store.load(&id)?,
                None => store.create("Кампания")?,
            }
        }
    };
    let campaign_id = campaign
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();

    let host_key = std::env::var("DND_HOST_KEY").unwrap_or_else(|_| {
        format!(
            "{}{}",
            dnd_core::content::new_token_id(),
            dnd_core::content::new_token_id()
        )
    });

    let table = Table::new(
        RoomState::new(campaign),
        host_key.clone(),
        Some(Arc::new(store)),
    );
    let (layer, _io) = create_io(table.clone());

    let static_dir = std::env::var("DND_RENDERER").unwrap_or_else(|_| "renderer".to_string());
    let app = create_app(table.clone(), &static_dir).layer(layer);

    let port = match std::env::var("DND_PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
    {
        Some(port) => port,
        None => net::find_free_port(32100, 64).await,
    };
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    println!("LISTENING {}", listener.local_addr()?);
    println!("HOSTKEY {host_key}");
    println!("CAMPAIGN {campaign_id}");

    // Autosave every minute, like the reference desktop shell.
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(60));
        ticker.tick().await;
        loop {
            ticker.tick().await;
            if let Err(error) = table.save_if_dirty() {
                eprintln!("[autosave] {error}");
            }
        }
    });

    axum::serve(listener, app).await?;
    Ok(())
}
