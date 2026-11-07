//! Чтение ZIP через crate `zip`.
//!
//! Понимаются два метода сжатия — «без сжатия» и deflate — этого хватает для
//! модулей Foundry. Остальное честно отвергается, а не молча распаковывается
//! как попало.
//!
//! Это НЕ защита от вредоносных архивов: «zip-бомба» (огромный размер при
//! маленьком файле) ограничена лимитом на суммарный распакованный объём и
//! потолком на одну запись.

use std::io::Read;

use zip::CompressionMethod;

/// Сколько данных распаковываем суммарно — защита от zip-бомбы.
pub const MAX_TOTAL_BYTES: usize = 400 * 1024 * 1024;

/// Потолок на одну запись. Суммарного лимита мало: одна запись может
/// раздуться на гигабайты, и память под неё выделится ещё до проверки суммы.
pub const MAX_ENTRY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Default)]
pub struct ZipContents {
    pub files: Vec<(String, Vec<u8>)>,
    pub assets: Vec<(String, Vec<u8>)>,
}

fn lower_ends_with(text: &str, suffix: &str) -> bool {
    text.len() >= suffix.len() && text[text.len() - suffix.len()..].eq_ignore_ascii_case(suffix)
}

fn is_json_entry(name: &str) -> bool {
    lower_ends_with(name, ".json")
}

/// Расширения картинок, которые умеет наша игра. Остальное не берём: в
/// выгрузке Foundry рядом с картами лежат шрифты, звуки и json, и тащить их
/// в память незачем.
fn is_image_entry(name: &str) -> bool {
    [".png", ".jpg", ".jpeg", ".webp", ".gif"]
        .iter()
        .any(|suffix| lower_ends_with(name, suffix))
}

fn compression_number(method: CompressionMethod) -> String {
    match method {
        CompressionMethod::Stored => "0".to_string(),
        CompressionMethod::Deflated => "8".to_string(),
        other => format!("{other:?}"),
    }
}

/// Читает из архива json и картинки за один проход.
///
/// Каталог читается один раз, а результат раскладывается по двум спискам:
/// сначала json, с картинками, чтобы при нехватке памяти пострадали именно они.
pub fn read_zip_json_and_assets(buffer: &[u8]) -> Result<ZipContents, String> {
    let mut contents = ZipContents::default();
    // У JS-разбора нечитаемый каталог (нет EOCD) давал пустой список, а не
    // исключение: решение «архив пуст» принимает адаптер.
    let Ok(mut archive) = zip::ZipArchive::new(std::io::Cursor::new(buffer)) else {
        return Ok(contents);
    };
    let count = archive.len();
    let mut total = 0usize;

    for index in 0..count {
        let mut entry = archive.by_index(index).map_err(|error| error.to_string())?;
        // Внутри архива пути всегда со слэшами — так их записывают упаковщики.
        // Windows-переменная отдаёт обратные, поэтому приводим к одному виду,
        // иначе поиск «packs/…» ломается на этой платформе.
        let name = entry.name().replace('\\', "/");
        let is_json = is_json_entry(&name);
        let is_image = is_image_entry(&name);
        if !is_json && !is_image {
            continue;
        }

        let method = entry.compression();
        let data = match method {
            CompressionMethod::Stored => {
                let mut data = Vec::new();
                entry
                    .read_to_end(&mut data)
                    .map_err(|error| error.to_string())?;
                data
            }
            CompressionMethod::Deflated => {
                // maxOutputLength не даёт одной записи съесть память целиком.
                let limit = MAX_ENTRY_BYTES as u64;
                let mut data = Vec::new();
                (&mut entry)
                    .take(limit + 1)
                    .read_to_end(&mut data)
                    .map_err(|error| error.to_string())?;
                if data.len() as u64 > limit {
                    return Err("Архив распаковывается в слишком большой объём".to_string());
                }
                data
            }
            other => {
                return Err(format!(
                    "Неподдерживаемый метод сжатия в архиве: {}",
                    compression_number(other)
                ));
            }
        };
        total += data.len();
        if total > MAX_TOTAL_BYTES {
            return Err("Архив распаковывается в слишком большой объём".to_string());
        }
        if is_json {
            contents.files.push((name, data));
        } else {
            contents.assets.push((name, data));
        }
    }

    Ok(contents)
}

/// Приводит путь внутри архива к виду, по которому его ищет документ.
/// Foundry пишет `assets/карта.webp`, но встречаются и `./assets/карта.webp`,
/// и обратные слэши из выгрузок с Windows.
pub fn normalize_asset_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    path.to_lowercase()
}
