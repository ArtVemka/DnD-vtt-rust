//! Валидация: проверки уровня кампании, а не источника.
//!
//! Смысл в том, чтобы до записи в кампанию было видно, что именно не влезет и
//! сколько это займёт места. Ничего не отбрасывает молча — только помечает.

use crate::core::normalize::Entity;
use dnd_core::sheet::LIMITS;

/// Сколько суммарно байт картинок считаем приемлемым.
pub const MAX_TOTAL_IMAGE_BYTES: usize = 200 * 1024 * 1024;

pub struct Validation {
    pub warnings: Vec<String>,
    pub image_bytes: usize,
    /// Порядок первого появления вида, как у объекта в JS.
    pub counts: Vec<(String, usize)>,
}

impl Validation {
    pub fn count(&self, kind: &str) -> usize {
        self.counts
            .iter()
            .find(|(name, _)| name == kind)
            .map(|(_, count)| *count)
            .unwrap_or(0)
    }
}

fn mentions_foundry(reason: &str) -> bool {
    reason.to_lowercase().contains("файлах foundry")
}

pub fn validate(entities: &[Entity]) -> Validation {
    let mut warnings = Vec::new();
    let mut counts: Vec<(String, usize)> = Vec::new();
    let mut image_bytes = 0usize;

    for entity in entities {
        match counts.iter_mut().find(|(kind, _)| *kind == entity.kind) {
            Some((_, count)) => *count += 1,
            None => counts.push((entity.kind.clone(), 1)),
        }
        if let Some(image) = &entity.image {
            image_bytes += image.bytes;
        }
        if entity.image.is_none() && entity.kind == "scene" {
            let detail = match &entity.image_error {
                Some(error) if !error.path.is_empty() => format!(" ({})", error.path),
                _ => String::new(),
            };
            let hint = match &entity.image_error {
                Some(error) if mentions_foundry(&error.reason) => {
                    " Выгрузите приключение из Foundry через «Экспорт приключения» — тогда карты едут вместе с json."
                }
                _ => "",
            };
            let name = if entity.name.is_empty() { &entity.source_id } else { &entity.name };
            warnings.push(format!(
                "У сцены «{name}» нет картинки{detail} — она попадёт в игру пустой.{hint}"
            ));
        }
    }

    if image_bytes > MAX_TOTAL_IMAGE_BYTES {
        let megabytes = (image_bytes as f64 / 1024.0 / 1024.0).round();
        warnings.push(format!(
            "Картинок на {megabytes} МБ — это много. Лишнее лучше не импортировать, иначе папка кампании разрастётся."
        ));
    }

    let caps = [
        ("journal", LIMITS.max_adventure_notes, "заметок"),
        ("item", LIMITS.max_adventure_items, "предметов"),
        ("rolltable", LIMITS.max_adventure_tables, "таблиц"),
        ("actor", LIMITS.max_adventure_actors, "персонажей"),
    ];
    for (kind, cap, noun) in caps {
        if let Some((_, count)) = counts.iter().find(|(name, _)| name == kind) {
            if *count as i64 > cap {
                warnings.push(format!(
                    "{noun} больше, чем помещается в кампанию ({count} из {cap}) — лишние не сохранятся."
                ));
            }
        }
    }

    Validation { warnings, image_bytes, counts }
}
