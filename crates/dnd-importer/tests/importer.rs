use std::io::Write;

use dnd_importer::Input;
use dnd_importer::adapters::zip::{normalize_asset_path, read_zip_json_and_assets};
use dnd_importer::core::map::{MapContext, map_to_campaign, plan_for};
use dnd_importer::core::normalize::{Entity, Image, normalize, to_entity};
use dnd_importer::core::validate::validate;
use dnd_importer::errors::{E_NO_ADAPTER, ImportError};
use dnd_importer::{inspect, run_import};
use serde_json::{Value, json};

/// Минимальный настоящий PNG.
const PNG_BASE64: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn png_data_url() -> String {
    format!("data:image/png;base64,{PNG_BASE64}")
}

fn png_bytes() -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(PNG_BASE64)
        .expect("valid png base64")
}

/// Данные, которые картинкой не являются: ни одна сигнатура не совпадает.
fn fake_data_url() -> String {
    use base64::Engine;
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode("<html>это не картинка</html>")
    )
}

fn empty_maps() -> Value {
    json!({
        "world": { "items": [], "activeId": null },
        "battle": { "items": [], "activeId": null },
        "sketch": { "items": [], "activeId": null },
    })
}

fn fresh_context() -> Value {
    json!({ "campaignMaps": empty_maps() })
}

fn merge(base: &mut Value, patch: Value) {
    if let (Some(base), Value::Object(patch)) = (base.as_object_mut(), patch) {
        for (key, value) in patch {
            base.insert(key, value);
        }
    }
}

/// Приключение в том виде, в каком его отдаёт выгрузка Foundry.
fn make_adventure(patch: Value) -> Value {
    let mut base = json!({
        "_id": "adventure-test",
        "type": "Adventure",
        "name": "Пещера Кобольда",
        "images": {},
        "scenes": [
            {
                "_id": "scene-1",
                "type": "Scene",
                "name": "Вход в пещеру",
                "img": png_data_url(),
                "width": 4000,
                "height": 2000,
                "tokens": [
                    { "_id": "token-1", "name": "Кобольд", "x": 500, "y": 250, "width": 100, "height": 100 },
                    { "_id": "token-2", "name": "Факел", "x": 1000, "y": 250, "width": 60, "height": 60 }
                ]
            },
            {
                "_id": "scene-2",
                "type": "Scene",
                "name": "Зал сокровищ",
                "img": png_data_url(),
                "width": 3000,
                "height": 3000
            },
            { "_id": "scene-no-image", "type": "Scene", "name": "Пустая комната" }
        ],
        "journal": [
            { "_id": "journal-1", "type": "JournalEntry", "name": "Записка хранителя", "text": { "html": "<p>Не открывай дверь</p>" } },
            { "_id": "journal-2", "type": "JournalEntry", "name": "Схема", "text": { "html": "<p>Слева — выход</p>" } }
        ],
        "actors": [
            {
                "_id": "actor-1",
                "type": "Actor",
                "name": "Вор Рамза",
                "img": png_data_url(),
                "system": {
                    "attributes": {
                        "ac": { "value": 15 },
                        "hp": { "max": 22 },
                        "movement": { "walk": 30, "climb": 15 }
                    },
                    "abilities": {
                        "str": { "value": 14 }, "dex": { "value": 16 }, "con": { "value": 12 },
                        "int": { "value": 10 }, "wis": { "value": 8 }, "cha": { "value": 13 }
                    },
                    "details": { "cr": 0.5 }
                }
            },
            { "_id": "actor-2", "type": "Actor", "name": "Знахарка" }
        ],
        "items": [{ "_id": "item-1", "type": "Item", "name": "Ключ от двери" }],
        "macro": [{ "_id": "macro-1", "type": "Macro", "name": "Взрыв", "command": "boom()" }],
        // Неизвестный тип: должен быть пропущен, а не сломать импорт.
        "_somethingElse": [{ "_id": "x-1", "type": "Mystery" }]
    });
    merge(&mut base, patch);
    base
}

/// Приключение в той форме, в какой его пишет настоящий Foundry.
fn make_real_adventure(patch: Value) -> Value {
    let mut base = json!({
        "_id": "real-adventure",
        "name": "Янтарный храм",
        "actors": [
            { "_id": "ra1", "name": "Берсерк", "type": "npc", "img": "systems/dnd5e/tokens/humanoid/Berserker.webp" }
        ],
        "items": [{ "_id": "ri1", "name": "Том понимания", "type": "equipment" }],
        "journal": [
            {
                "_id": "rj1",
                "name": "Янтарный храм",
                "pages": [
                    {
                        "_id": "rp1",
                        "name": "Обзор",
                        "type": "text",
                        "text": { "format": 1, "content": "<p>Тайное общество волшебников построило храм.</p>" }
                    },
                    {
                        "_id": "rp2",
                        "name": "Цена",
                        "type": "text",
                        "text": { "format": 1, "content": "<p>Вопрос стоит воспоминания.</p>" }
                    }
                ]
            }
        ],
        "scenes": [
            {
                "_id": "rs1",
                "name": "Янтарный храм, верхний уровень",
                "width": 2700,
                "height": 3000,
                "backgroundColor": "#000000",
                "background": { "src": "assets/upper_level.png", "offsetX": 0, "offsetY": 0, "scaleX": 1, "scaleY": 1 },
                "grid": { "size": 50, "type": 1, "distance": 5, "units": "ft" },
                "tokens": [
                    {
                        "_id": "rt1",
                        "name": "Берсерк",
                        "x": 1350,
                        "y": 1500,
                        "width": 0.5,
                        "height": 0.5,
                        "texture": { "src": "assets/berserker.png" }
                    },
                    {
                        "_id": "rt2",
                        "name": "",
                        "actorId": "ra1",
                        "x": 2000,
                        "y": 2000,
                        "width": 0.5,
                        "height": 0.5,
                        "delta": { "name": "Берсерк-2" }
                    }
                ]
            }
        ],
        "tables": [],
        "macros": []
    });
    merge(&mut base, patch);
    base
}

fn as_file(value: &Value) -> Vec<u8> {
    serde_json::to_vec(value).expect("serializable adventure")
}

fn inspect_json(value: &Value, context: &Value) -> Result<Value, ImportError> {
    let data = as_file(value);
    inspect(&Input { data: &data, path: Some("приключение.json") }, context)
}

fn run_json(
    value: &Value,
    include: Value,
    context: &Value,
) -> Result<Value, ImportError> {
    let data = as_file(value);
    run_import(
        &Input { data: &data, path: Some("приключение.json") },
        &json!({ "include": include }),
        context,
    )
}

fn plan<'a>(preview: &'a Value, source_id: &str) -> &'a Value {
    preview["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["sourceId"] == source_id)
        .unwrap_or_else(|| panic!("plan {source_id} not found"))
}

fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        writer.start_file(*name, options).expect("start file");
        writer.write_all(data).expect("write entry");
    }
    writer.finish().expect("finish zip").into_inner()
}

fn zip_context() -> Value {
    fresh_context()
}

#[test]
fn preview_finds_every_supported_kind() {
    let preview = inspect_json(&make_adventure(json!({})), &fresh_context()).expect("preview");
    let mut kinds: Vec<&str> = preview["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .map(|plan| plan["kind"].as_str().expect("kind"))
        .collect();
    kinds.sort_unstable();
    kinds.dedup();
    assert_eq!(kinds, ["actor", "item", "journal", "macro", "scene"]);
    assert_eq!(preview["sourceTitle"], "Пещера Кобольда");
    assert_eq!(preview["total"], 9, "3 сцены, 2 заметки, 2 актёра, 1 предмет, 1 макрос");
}

#[test]
fn macro_is_visible_but_not_selectable() {
    let preview = inspect_json(&make_adventure(json!({})), &fresh_context()).expect("preview");
    let macro_plan = preview["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["kind"] == "macro")
        .expect("macro plan");
    assert_eq!(macro_plan["selectable"], false);
    assert_eq!(macro_plan["selected"], false);
    assert!(macro_plan["note"].as_str().expect("note").contains("Макрос"));
}

#[test]
fn scene_without_image_is_not_importable_and_explains_why() {
    let preview = inspect_json(&make_adventure(json!({})), &fresh_context()).expect("preview");
    let empty = plan(&preview, "scene-no-image");
    assert_eq!(empty["selectable"], false);
    assert_eq!(empty["selected"], false);
    assert!(empty["note"].as_str().expect("note").to_lowercase().contains("картинк"));
}

#[test]
fn scene_with_broken_image_is_not_importable_and_warns() {
    let mut adventure = make_adventure(json!({}));
    adventure["scenes"][0]["img"] = Value::String(fake_data_url());
    let preview = inspect_json(&adventure, &fresh_context()).expect("preview");
    assert_eq!(plan(&preview, "scene-1")["selectable"], false);
    assert!(
        preview["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|warning| warning.as_str().is_some_and(|text| text.to_lowercase().contains("картинк")))
    );
    let error = run_json(&adventure, json!(["scene-1"]), &fresh_context()).expect_err("must reject");
    assert!(error.message.contains("Не выбрано ни одной сущности"));
}

#[test]
fn unknown_document_is_skipped_with_reason() {
    let file = json!([
        { "_id": "mystery-1", "type": "Mystery", "name": "Непонятно что" },
        { "_id": "item-1", "type": "Item", "name": "Ключ от двери" }
    ]);
    let preview = inspect_json(&file, &fresh_context()).expect("preview");
    assert_eq!(preview["plans"].as_array().expect("plans").len(), 1);
    assert_eq!(preview["plans"][0]["name"], "Ключ от двери");
    let skipped = preview["skipped"].as_array().expect("skipped");
    assert_eq!(skipped.len(), 1);
    assert!(skipped[0]["reason"].as_str().expect("reason").to_lowercase().contains("неизвестн"));
}

#[test]
fn file_with_nothing_known_is_rejected() {
    let file = json!([{ "_id": "mystery-1", "type": "Mystery", "name": "Непонятно что" }]);
    let error = inspect_json(&file, &fresh_context()).expect_err("must reject");
    assert_eq!(error.code, "empty");
    assert!(error.message.to_lowercase().contains("нет документов"));
}

#[test]
fn scene_takes_a_free_map_slot() {
    let preview = inspect_json(&make_adventure(json!({})), &fresh_context()).expect("preview");
    let scene = plan(&preview, "scene-1");
    assert_eq!(scene["selectable"], true);
    assert_eq!(scene["target"], "world");
    assert!(scene["targetLabel"].as_str().expect("label").contains("Общая"));
}

#[test]
fn occupied_world_slot_falls_back_to_battle() {
    let context = json!({
        "campaignMaps": {
            "world": { "items": [{ "id": "map-1", "url": "/files/a.png", "name": "Уже есть" }], "activeId": null },
            "battle": { "items": [], "activeId": null },
            "sketch": { "items": [], "activeId": null },
        }
    });
    let preview = inspect_json(&make_adventure(json!({})), &context).expect("preview");
    let scene = plan(&preview, "scene-1");
    assert_eq!(scene["selectable"], true);
    assert_eq!(scene["target"], "battle");
}

#[test]
fn map_is_imported_with_image_url_and_scene_name() {
    let result = run_json(&make_adventure(json!({})), json!(["scene-1"]), &fresh_context()).expect("import");
    let maps = result["patch"]["maps"]["world"]["items"].as_array().expect("maps");
    assert_eq!(maps.len(), 1);
    assert_eq!(maps[0]["url"], png_data_url());
    assert_eq!(maps[0]["name"], "Вход в пещеру");
}

#[test]
fn scene_tokens_become_tokens_with_relative_coordinates() {
    let result = run_json(&make_adventure(json!({})), json!(["scene-1"]), &fresh_context()).expect("import");
    let tokens = result["patch"]["tokens"].as_array().expect("tokens");
    assert_eq!(tokens.len(), 2);
    let kobold = tokens.iter().find(|token| token["name"] == "Кобольд").expect("kobold");
    assert_eq!(kobold["mapType"], "world");
    let x = kobold["x"].as_f64().expect("x");
    let y = kobold["y"].as_f64().expect("y");
    assert!(x > 0.0 && x < 1.0, "x must be a fraction: {x}");
    assert!(y > 0.0 && y < 1.0, "y must be a fraction: {y}");
}

#[test]
fn html_in_notes_is_stripped() {
    let result = run_json(&make_adventure(json!({})), json!(["journal-1"]), &fresh_context()).expect("import");
    let notes = result["patch"]["adventure"]["notes"].as_array().expect("notes");
    let note = notes.iter().find(|note| note["sourceId"] == "journal-1").expect("note");
    assert_eq!(note["text"], "Не открывай дверь");
}

#[test]
fn only_selected_documents_are_imported() {
    let result = run_json(&make_adventure(json!({})), json!(["journal-1", "item-1"]), &fresh_context()).expect("import");
    let adventure = &result["patch"]["adventure"];
    assert_eq!(adventure["notes"].as_array().expect("notes").len(), 1);
    assert_eq!(adventure["items"].as_array().expect("items").len(), 1);
    assert_eq!(adventure["actors"].as_array().expect("actors").len(), 0);
    assert_eq!(result["patch"]["maps"]["world"]["items"].as_array().expect("maps").len(), 0);
    assert_eq!(result["patch"]["tokens"].as_array().expect("tokens").len(), 0);
    assert_eq!(result["report"]["applied"].as_array().expect("applied").len(), 2);
}

#[test]
fn actor_goes_to_the_adventure_reference_with_a_statblock() {
    let result = run_json(&make_adventure(json!({})), json!(["actor-1"]), &fresh_context()).expect("import");
    let actors = result["patch"]["adventure"]["actors"].as_array().expect("actors");
    let actor = actors.iter().find(|actor| actor["sourceId"] == "actor-1").expect("actor");
    assert_eq!(actor["name"], "Вор Рамза");
    assert_eq!(actor["image"], png_data_url());
    assert_eq!(actor["statblock"]["ac"], 15);
    assert_eq!(actor["statblock"]["hp"], 22);
    assert_eq!(actor["statblock"]["abilities"]["str"], 14);
    assert!(actor["statblock"]["speed"].as_str().expect("speed").contains("30"));
    assert_eq!(result["patch"]["tokens"].as_array().expect("tokens").len(), 0);
}

#[test]
fn empty_selection_is_rejected() {
    let error = run_json(&make_adventure(json!({})), json!([]), &fresh_context()).expect_err("must reject");
    assert_eq!(error.code, "nothing-selected");
    assert!(error.message.contains("Не выбрано ни одной сущности"));
}

#[test]
fn zip_import_works_like_json() {
    let data = as_file(&make_adventure(json!({})));
    let zip = make_zip(&[("adventure.json", &data)]);
    let preview = inspect(&Input { data: &zip, path: Some("приключение.zip") }, &zip_context()).expect("preview");
    assert_eq!(preview["sourceTitle"], "Пещера Кобольда");
    let scenes = preview["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .filter(|plan| plan["kind"] == "scene")
        .count();
    assert_eq!(scenes, 3);
}

#[test]
fn summary_uses_russian_plurals() {
    let one = run_json(&make_adventure(json!({})), json!(["journal-1"]), &fresh_context()).expect("import");
    assert!(one["report"]["summary"].as_str().expect("summary").contains("1 заметка."));

    let journals: Vec<Value> = (0..4)
        .map(|index| json!({ "_id": format!("j{index}"), "type": "JournalEntry", "name": format!("Записка {index}") }))
        .collect();
    let four = run_json(
        &make_adventure(json!({ "journal": journals })),
        json!(["j0", "j1", "j2", "j3"]),
        &fresh_context(),
    )
    .expect("import");
    assert!(four["report"]["summary"].as_str().expect("summary").contains("4 заметки."));

    let journals: Vec<Value> = (0..5)
        .map(|index| json!({ "_id": format!("k{index}"), "type": "JournalEntry", "name": format!("Записка {index}") }))
        .collect();
    let five = run_json(
        &make_adventure(json!({ "journal": journals })),
        json!(["k0", "k1", "k2", "k3", "k4"]),
        &fresh_context(),
    )
    .expect("import");
    assert!(five["report"]["summary"].as_str().expect("summary").contains("5 заметок."));
}

#[test]
fn real_export_reads_scene_background_from_the_archive() {
    let adventure = as_file(&make_real_adventure(json!({})));
    let png = png_bytes();
    let zip = make_zip(&[
        ("adventure.json", &adventure),
        ("assets/upper_level.png", &png),
    ]);
    let preview = inspect(&Input { data: &zip, path: Some("храм.zip") }, &zip_context()).expect("preview");
    let scene = preview["plans"]
        .as_array()
        .expect("plans")
        .iter()
        .find(|plan| plan["kind"] == "scene")
        .expect("scene");
    assert_eq!(scene["selectable"], true, "{}", scene["note"]);

    let result = run_import(
        &Input { data: &zip, path: Some("храм.zip") },
        &json!({ "include": ["rs1"] }),
        &zip_context(),
    )
    .expect("import");
    let map = &result["patch"]["maps"]["world"]["items"][0];
    assert!(map["url"].as_str().expect("url").starts_with("data:image/png;base64,"));
}

#[test]
fn real_export_builds_journal_text_from_pages() {
    let adventure = as_file(&make_real_adventure(json!({})));
    let png = png_bytes();
    let zip = make_zip(&[
        ("adventure.json", &adventure),
        ("assets/upper_level.png", &png),
    ]);
    let result = run_import(
        &Input { data: &zip, path: Some("храм.zip") },
        &json!({ "include": ["rj1"] }),
        &zip_context(),
    )
    .expect("import");
    let notes = result["patch"]["adventure"]["notes"].as_array().expect("notes");
    let note = notes.iter().find(|note| note["sourceId"] == "rj1").expect("note");
    let text = note["text"].as_str().expect("text");
    assert!(text.contains("Тайное общество волшебников"));
    assert!(text.contains("воспоминания"));
    assert!(!text.contains("<p>"));
}

#[test]
fn real_export_tokens_take_the_name_from_delta_and_bind_to_their_map() {
    let adventure = as_file(&make_real_adventure(json!({})));
    let png = png_bytes();
    let zip = make_zip(&[
        ("adventure.json", &adventure),
        ("assets/upper_level.png", &png),
        ("assets/berserker.png", &png),
    ]);
    let result = run_import(
        &Input { data: &zip, path: Some("храм.zip") },
        &json!({ "include": ["rs1"] }),
        &zip_context(),
    )
    .expect("import");
    let tokens = result["patch"]["tokens"].as_array().expect("tokens");
    assert_eq!(tokens.len(), 2);
    let named = tokens.iter().find(|token| token["name"] == "Берсерк-2").expect("delta name");
    let x = named["x"].as_f64().expect("x");
    let y = named["y"].as_f64().expect("y");
    assert!((x - 2000.25 / 2700.0).abs() < 0.001, "x = {x}");
    assert!((y - 2000.25 / 3000.0).abs() < 0.001, "y = {y}");
    let map_id = &result["patch"]["maps"]["world"]["items"][0]["id"];
    for token in tokens {
        assert_eq!(&token["mapId"], map_id, "token {} belongs to its map", token["name"]);
    }
}

#[test]
fn foundry_file_path_is_explained_not_silently_dropped() {
    let adventure = make_real_adventure(json!({
        "scenes": [{
            "_id": "rs1",
            "name": "Янтарный храм",
            "width": 2700,
            "height": 3000,
            "background": { "src": "modules/что-то/карта.webp" }
        }]
    }));
    let preview = inspect_json(&adventure, &fresh_context()).expect("preview");
    let scene = plan(&preview, "rs1");
    assert_eq!(scene["selectable"], false);
    assert!(scene["note"].as_str().expect("note").contains("Foundry"));
    let warnings = preview["warnings"].as_array().expect("warnings");
    let warning = warnings
        .iter()
        .find(|warning| warning.as_str().is_some_and(|text| text.contains("Янтарный храм")))
        .expect("warning");
    assert!(warning.as_str().expect("warning").contains("modules/что-то/карта.webp"));
}

#[test]
fn scene_walls_become_map_walls_with_fractions() {
    let adventure = make_adventure(json!({
        "scenes": [{
            "_id": "scene-walls",
            "type": "Scene",
            "name": "Стены",
            "img": png_data_url(),
            "width": 4000,
            "height": 2000,
            "walls": [
                { "c": [0, 0, 2000, 1000], "d": 1, "ds": 1 },
                { "c": [10, 20, 30, 40], "window": true }
            ]
        }]
    }));
    let result = run_json(&adventure, json!(["scene-walls"]), &fresh_context()).expect("import");
    let map = &result["patch"]["maps"]["world"]["items"][0];
    let walls = map["walls"].as_array().expect("walls");
    assert_eq!(walls.len(), 2);
    assert_eq!(walls[0]["x1"], 0.0);
    assert_eq!(walls[0]["x2"], 0.5);
    assert_eq!(walls[0]["y2"], 0.5);
    assert_eq!(walls[0]["door"]["open"], true);
    assert_eq!(walls[0]["door"]["locked"], false);
    assert_eq!(walls[1]["window"], true);
}

#[test]
fn rolltable_rows_are_mapped_from_raw_results() {
    let entity = Entity {
        kind: "rolltable".to_string(),
        source_id: "t1".to_string(),
        name: "Случайная встреча".to_string(),
        tags: Vec::new(),
        image: None,
        image_error: None,
        children: Vec::new(),
        raw: Some(json!({
            "formula": "1d6",
            "description": "описание",
            "results": [
                { "text": "Волк", "range": [1, 2], "weight": 1 },
                { "label": "Медведь", "weight": 2 }
            ]
        })),
    };
    let plan = plan_for(&entity, &[]);
    let mapped = map_to_campaign(
        &[plan],
        MapContext {
            campaign_maps: &empty_maps(),
            entities: &[entity],
            source_title: "Таблицы",
            source_id: "json",
        },
    );
    let tables = mapped.adventure["tables"].as_array().expect("tables");
    assert_eq!(tables.len(), 1);
    let table = &tables[0];
    assert_eq!(table["name"], "Случайная встреча");
    assert_eq!(table["formula"], "1d6");
    assert_eq!(table["description"], "описание");
    let rows = table["rows"].as_array().expect("rows");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["label"], "Волк");
    assert_eq!(rows[0]["min"], 1);
    assert_eq!(rows[0]["max"], 2);
    assert_eq!(rows[0]["weight"], 1);
    assert_eq!(rows[1]["label"], "Медведь");
    assert_eq!(rows[1]["weight"], 2);
    assert!(rows[1].get("min").is_none(), "range отсутствует — min не пишется");
    assert!(rows[1].get("max").is_none(), "range отсутствует — max не пишется");
}

#[test]
fn adventure_caps_produce_a_warning() {
    let journals: Vec<Value> = (0..301)
        .map(|index| json!({ "_id": format!("j{index}"), "type": "JournalEntry", "name": format!("Записка {index}") }))
        .collect();
    let preview = inspect_json(&make_adventure(json!({ "journal": journals })), &fresh_context()).expect("preview");
    let warning = preview["warnings"]
        .as_array()
        .expect("warnings")
        .iter()
        .find(|warning| warning.as_str().is_some_and(|text| text.contains("заметок")))
        .expect("cap warning");
    assert!(warning.as_str().expect("warning").contains("301 из 300"));
}

#[test]
fn image_bytes_cap_produces_a_warning() {
    let entity = Entity {
        kind: "actor".to_string(),
        source_id: "a".to_string(),
        name: "A".to_string(),
        tags: Vec::new(),
        image: Some(Image {
            data: String::new(),
            mime: "image/png".to_string(),
            bytes: 201 * 1024 * 1024,
        }),
        image_error: None,
        children: Vec::new(),
        raw: None,
    };
    let validation = validate(&[entity]);
    assert!(validation.image_bytes > 0);
    assert!(
        validation
            .warnings
            .iter()
            .any(|warning| warning.starts_with("Картинок на 201 МБ")),
        "{:?}",
        validation.warnings
    );
}

#[test]
fn normalization_drops_unidentifiable_and_unknown_entities() {
    let result = normalize(&[
        json!({}),
        json!({ "sourceId": "x", "kind": "Mystery", "name": "Что-то" }),
        json!({ "sourceId": "ok", "kind": "item", "name": "Меч" }),
    ]);
    assert_eq!(result.entities.len(), 1);
    assert_eq!(result.entities[0].source_id, "ok");
    assert_eq!(result.skipped.len(), 2);
    assert_eq!(result.skipped[0].reason, "нет ни идентификатора, ни названия");
    assert_eq!(result.skipped[1].reason, "неизвестный вид «Mystery»");
}

#[test]
fn entity_name_and_tags_are_truncated() {
    let tags: Vec<Value> = (0..40).map(|index| json!(format!("tag{index}"))).collect();
    let entity = to_entity(&json!({
        "kind": "actor",
        "sourceId": "a",
        "name": "A".repeat(500),
        "tags": tags,
        "image": "   ",
        "children": [{ "kind": "actor", "sourceId": "child", "name": "Дитя" }],
    }));
    assert_eq!(entity.name.encode_utf16().count(), 200);
    assert_eq!(entity.tags.len(), 32);
    assert!(entity.image.is_none());
    assert_eq!(entity.image_error.expect("image error").reason, "пусто");
    assert_eq!(entity.children.len(), 1);
}

#[test]
fn foundry_image_path_is_recorded_in_the_error() {
    let entity = to_entity(&json!({
        "kind": "scene",
        "sourceId": "s",
        "name": "Сцена",
        "image": "./modules/maps/amber.webp",
    }));
    assert!(entity.image.is_none());
    let error = entity.image_error.expect("image error");
    assert!(error.reason.contains("файлах Foundry"));
    assert_eq!(error.path, "./modules/maps/amber.webp");
}

#[test]
fn zip_bomb_is_stopped_by_the_entry_ceiling() {
    let zeros = vec![0u8; 70 * 1024 * 1024];
    let zip = make_zip(&[("adventure.json", &zeros)]);
    assert!(read_zip_json_and_assets(&zip).is_err(), "распаковка обязана прерваться");
}

#[test]
fn empty_archive_is_rejected() {
    let zip = make_zip(&[]);
    let error = dnd_importer::adapters::foundry::from_zip_buffer(&zip).expect_err("empty archive");
    assert_eq!(error.code, "empty");
    assert_eq!(error.message, "В архиве нет документов, которые можно импортировать.");
}

#[test]
fn broken_zip_is_rejected() {
    let mut broken = vec![0x50, 0x4b, 0x03, 0x04];
    broken.extend_from_slice(&[0u8; 40]);
    let error = dnd_importer::adapters::foundry::from_zip_buffer(&broken).expect_err("broken zip");
    assert_eq!(error.code, "empty");
}

#[test]
fn unknown_file_gets_no_adapter_error() {
    let error = inspect(&Input { data: b"hello", path: None }, &fresh_context()).expect_err("no adapter");
    assert_eq!(error.code, E_NO_ADAPTER);
}

#[test]
fn zip_reader_keeps_only_json_and_images() {
    let zip = make_zip(&[
        ("adventure.json", br#"{"name":"x"}"#),
        ("assets/map.png", PNG_BASE64.as_bytes()),
        ("assets/theme.mp3", b"not an image"),
    ]);
    let contents = read_zip_json_and_assets(&zip).expect("zip");
    assert_eq!(contents.files.len(), 1);
    assert_eq!(contents.files[0].0, "adventure.json");
    assert_eq!(contents.assets.len(), 1);
    assert_eq!(contents.assets[0].0, "assets/map.png");
}

#[test]
fn asset_paths_are_normalized() {
    assert_eq!(normalize_asset_path(".\\assets\\Map.PNG"), "assets/map.png");
    assert_eq!(normalize_asset_path("./assets/map.png"), "assets/map.png");
    assert_eq!(normalize_asset_path("assets/map.png"), "assets/map.png");
}

#[test]
fn json_detection_skips_bom_and_whitespace() {
    use dnd_importer::adapters::foundry::{looks_like_json, looks_like_zip};
    assert!(looks_like_json(b"\xef\xbb\xbf  {\"a\":1}"));
    assert!(looks_like_json(b"[\n{]"));
    assert!(!looks_like_json(b"hello"));
    assert!(looks_like_zip(&[0x50, 0x4b, 0x03, 0x04, 0]));
    assert!(!looks_like_zip(b"PK"));
}
