//! Конвейер импорта.
//!
//! Модуль ничего не знает ни про Electron, ни про файловую систему: всё чтение
//! делает адаптер, а конвейер только связывает шаги. Это позволяет проверять
//! импорт обычными тестами, без запуска приложения.
//!
//! Ядро не мутирует кампанию: на выходе лежит патч, который вызывающий код
//! решает применить или отбросить целиком.

use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};

use crate::core::map::{MapContext, build_plans, map_to_campaign};
use crate::core::normalize::normalize;
use crate::core::report::{PreviewInput, ReportInput, build_preview, build_report};
use crate::core::validate::validate;
use crate::errors::{E_EMPTY, E_NO_ADAPTER, ImportError, import_error};
use crate::js;

/// Входные данные: файл читает вызывающий код, ядро в файловую систему не ходит.
pub struct Input<'a> {
    pub data: &'a [u8],
    pub path: Option<&'a str>,
}

#[derive(Debug)]
pub struct Extracted {
    pub source_id: String,
    pub source_title: String,
    pub entities: Vec<Value>,
}

pub trait Adapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn can_handle(&self, input: &Input<'_>) -> bool;
    fn extract(&self, input: &Input<'_>) -> Result<Extracted, ImportError>;
}

static ADAPTERS: OnceLock<Mutex<Vec<Arc<dyn Adapter>>>> = OnceLock::new();

fn registry() -> &'static Mutex<Vec<Arc<dyn Adapter>>> {
    ADAPTERS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Регистрирует адаптер источника.
pub fn register(adapter: Arc<dyn Adapter>) -> Result<(), ImportError> {
    if adapter.id().is_empty() {
        return Err(import_error(
            "adapter",
            "Адаптер должен иметь id, canHandle и extract",
        ));
    }
    let mut adapters = registry().lock().expect("adapter registry is not poisoned");
    match adapters.iter_mut().find(|item| item.id() == adapter.id()) {
        Some(existing) => *existing = adapter,
        None => adapters.push(adapter),
    }
    Ok(())
}

pub fn find_adapter(input: &Input<'_>) -> Result<Arc<dyn Adapter>, ImportError> {
    let adapters = registry().lock().expect("adapter registry is not poisoned");
    let found = adapters
        .iter()
        .find(|adapter| adapter.can_handle(input))
        .cloned();
    drop(adapters);
    found.ok_or_else(|| {
        import_error(
            E_NO_ADAPTER,
            "Не удалось понять, что это за файл. Поддерживаются выгрузки Foundry VTT.",
        )
    })
}

/// Шаги 1–4: источник → адаптер → нормализация → валидация → планы.
/// Результат уходит в интерфейс: список сущностей с тем, что будет с каждой.
pub fn inspect(input: &Input<'_>, context: &Value) -> Result<Value, ImportError> {
    let adapter = find_adapter(input)?;
    let raw = adapter.extract(input)?;
    let normalized = normalize(&raw.entities);
    if normalized.entities.is_empty() {
        // Файл разобрали, но переносить из него нечего. Молчаливый пустой
        // предпросмотр выглядел бы как «сломалось что-то в программе».
        return Err(import_error(
            E_EMPTY,
            "В файле нет документов, которые можно импортировать.",
        ));
    }
    let validation = validate(&normalized.entities);
    let campaign_maps = js::get(context, "campaignMaps").unwrap_or(&Value::Null);
    let built = build_plans(&normalized.entities, campaign_maps);
    Ok(build_preview(PreviewInput {
        source_id: &raw.source_id,
        source_title: &raw.source_title,
        entities: &normalized.entities,
        skipped: &normalized.skipped,
        warnings: &validation.warnings,
        plans: &built.plans,
    }))
}

/// Шаги 5–8: выбор пользователя → маппинг → патч → отчёт.
///
/// Если что-то выбранное не удалось применить, импорт отменяется целиком:
/// полуприменённая кампания хуже, чем никакая.
pub fn run_import(
    input: &Input<'_>,
    options: &Value,
    context: &Value,
) -> Result<Value, ImportError> {
    let adapter = find_adapter(input)?;
    let raw = adapter.extract(input)?;
    let normalized = normalize(&raw.entities);
    let validation = validate(&normalized.entities);
    let campaign_maps = js::get(context, "campaignMaps").unwrap_or(&Value::Null);
    let built = build_plans(&normalized.entities, campaign_maps);

    let include: Vec<&str> = js::as_array(js::get(options, "include"))
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let chosen: Vec<_> = built
        .plans
        .iter()
        .filter(|plan| plan.selectable && include.contains(&plan.source_id.as_str()))
        .map(|plan| {
            let mut chosen = plan.clone();
            chosen.selected = true;
            chosen
        })
        .collect();

    if chosen.is_empty() {
        return Err(import_error(
            "nothing-selected",
            "Не выбрано ни одной сущности для импорта.",
        ));
    }

    let mapped = map_to_campaign(
        &chosen,
        MapContext {
            campaign_maps,
            entities: &normalized.entities,
            source_title: &raw.source_title,
            source_id: &raw.source_id,
        },
    );

    let mut skipped: Vec<Value> = normalized
        .skipped
        .iter()
        .map(|item| item.to_value())
        .collect();
    skipped.extend(mapped.skipped);

    let report = build_report(ReportInput {
        source_title: &raw.source_title,
        applied: &mapped.applied,
        skipped: &skipped,
        warnings: &validation.warnings,
    });

    Ok(json!({
        "patch": {
            "maps": mapped.maps,
            "tokens": mapped.tokens,
            "adventure": mapped.adventure,
        },
        "report": report,
    }))
}
