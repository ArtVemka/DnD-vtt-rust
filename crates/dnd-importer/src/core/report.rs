//! Отчёт и сводка для предпросмотра.
//!
//! Пользователь должен до импорта видеть, что именно он запускает, и после
//! импорта — что получилось. Это два разных формата одного и того же счёта.

use serde_json::{Value, json};

use crate::core::map::Plan;
use crate::core::normalize::{Entity, Skipped, kind_label};
use crate::js;

pub struct PreviewInput<'a> {
    pub source_id: &'a str,
    pub source_title: &'a str,
    pub entities: &'a [Entity],
    pub skipped: &'a [Skipped],
    pub warnings: &'a [String],
    pub plans: &'a [Plan],
}

/// Сводка для предпросмотра.
pub fn build_preview(data: PreviewInput<'_>) -> Value {
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for entity in data.entities {
        match counts.iter_mut().find(|(kind, _)| *kind == entity.kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((&entity.kind, 1)),
        }
    }

    let selectable = data.plans.iter().filter(|plan| plan.selectable).count();

    json!({
        "sourceId": data.source_id,
        "sourceTitle": data.source_title,
        "counts": counts
            .iter()
            .map(|(kind, count)| json!({
                "kind": kind,
                "label": kind_label(kind),
                "count": count,
            }))
            .collect::<Vec<_>>(),
        "total": data.entities.len(),
        "selectable": selectable,
        "warnings": data.warnings,
        "skipped": data.skipped.iter().map(Skipped::to_value).collect::<Vec<_>>(),
        "plans": data.plans.iter().map(Plan::to_value).collect::<Vec<_>>(),
    })
}

pub struct ReportInput<'a> {
    pub source_title: &'a str,
    pub applied: &'a [Value],
    pub skipped: &'a [Value],
    pub warnings: &'a [String],
}

/// Итоговый отчёт после импорта.
pub fn build_report(input: ReportInput<'_>) -> Value {
    json!({
        "sourceTitle": input.source_title,
        "applied": input.applied,
        "skipped": input.skipped,
        "warnings": input.warnings,
        "summary": summarise(input.applied),
    })
}

/// Склоняет существительное по числу: 1 карта, 2 карты, 5 карт.
/// Без этого итог импорта читается как «1 карт», и это сразу бросается в глаза.
pub fn plural(count: usize, one: &str, few: &str, many: &str) -> String {
    let mod100 = count % 100;
    let mod10 = mod100 % 10;
    if (11..=14).contains(&mod100) {
        return many.to_string();
    }
    if mod10 == 1 {
        return one.to_string();
    }
    if (2..=4).contains(&mod10) {
        return few.to_string();
    }
    many.to_string()
}

pub fn summarise(applied: &[Value]) -> String {
    let nouns = [
        ("scene", "карта", "карты", "карт"),
        ("journal", "заметка", "заметки", "заметок"),
        ("item", "предмет", "предмета", "предметов"),
        ("rolltable", "таблица", "таблицы", "таблиц"),
        ("actor", "персонаж", "персонажа", "персонажей"),
    ];
    let mut parts = Vec::new();
    for (kind, one, few, many) in nouns {
        let total = applied
            .iter()
            .filter(|item| js::get(item, "kind").and_then(Value::as_str) == Some(kind))
            .count();
        if total > 0 {
            parts.push(format!("{total} {}", plural(total, one, few, many)));
        }
    }
    if parts.is_empty() {
        "Импортировать было нечего.".to_string()
    } else {
        format!("Импортировано: {}.", parts.join(", "))
    }
}
