//! Foundry VTT adventure import. Port of `server/importer/`.
//!
//! Наружу торчат только две функции — предпросмотр и импорт. Всё остальное
//! (адаптеры, ядро) — деталь внутри.

pub mod adapters;
pub mod core;
pub mod errors;
mod js;

use std::sync::{Arc, Once};

use serde_json::Value;

pub use core::pipeline::Input;
use core::pipeline::{inspect as pipeline_inspect, register, run_import as pipeline_run_import};
use errors::ImportError;

static REGISTER_FOUNDRY: Once = Once::new();

fn ensure_foundry() {
    REGISTER_FOUNDRY.call_once(|| {
        register(Arc::new(adapters::foundry::Foundry)).expect("foundry adapter is valid");
    });
}

/// Смотрит файл и говорит, что внутри и что будет с каждой сущностью.
pub fn inspect(input: &Input<'_>, context: &Value) -> Result<Value, ImportError> {
    ensure_foundry();
    pipeline_inspect(input, context)
}

/// Импортирует выбранное.
pub fn run_import(
    input: &Input<'_>,
    options: &Value,
    context: &Value,
) -> Result<Value, ImportError> {
    ensure_foundry();
    pipeline_run_import(input, options, context)
}
