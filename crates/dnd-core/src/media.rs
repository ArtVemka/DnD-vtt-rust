//! Upload validation for images and audio. Port of `server/image.js` and
//! `server/audio.js`.
//!
//! Format is decided by file signature, never by the MIME the client wrote.
//! Unsupported formats are recognized too, so the error names what the user
//! actually picked.

use base64::Engine;

use crate::sheet::LIMITS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaFormat {
    pub ext: Option<&'static str>,
    pub label: Option<&'static str>,
    pub supported: bool,
}

impl MediaFormat {
    const NONE: MediaFormat = MediaFormat {
        ext: None,
        label: None,
        supported: false,
    };

    fn supported(ext: &'static str, label: &'static str) -> Self {
        MediaFormat {
            ext: Some(ext),
            label: Some(label),
            supported: true,
        }
    }

    fn unsupported(ext: &'static str, label: &'static str) -> Self {
        MediaFormat {
            ext: Some(ext),
            label: Some(label),
            supported: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedMedia {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
}

fn at(buffer: &[u8], range: std::ops::Range<usize>) -> &[u8] {
    buffer.get(range).unwrap_or_default()
}

fn ascii_at(buffer: &[u8], range: std::ops::Range<usize>) -> &[u8] {
    at(buffer, range)
}

/// `sniffImageFormat`.
pub fn sniff_image_format(buffer: &[u8]) -> MediaFormat {
    if buffer.len() >= 8 && at(buffer, 0..4) == [0x89, 0x50, 0x4e, 0x47] {
        return MediaFormat::supported("png", "PNG");
    }
    if buffer.len() >= 3 && at(buffer, 0..3) == [0xff, 0xd8, 0xff] {
        return MediaFormat::supported("jpg", "JPEG");
    }
    if buffer.len() >= 12 && ascii_at(buffer, 0..4) == b"RIFF" && ascii_at(buffer, 8..12) == b"WEBP"
    {
        return MediaFormat::supported("webp", "WEBP");
    }
    if buffer.len() >= 6 && matches!(ascii_at(buffer, 0..6), b"GIF87a" | b"GIF89a") {
        return MediaFormat::unsupported("gif", "GIF");
    }
    if buffer.len() >= 2 && buffer[0] == 0x42 && buffer[1] == 0x4d {
        return MediaFormat::unsupported("bmp", "BMP");
    }
    if buffer.len() >= 4
        && ((buffer[0] == 0x49 && buffer[1] == 0x49 && buffer[2] == 0x2a)
            || (buffer[0] == 0x4d && buffer[1] == 0x4d && buffer[2] == 0x00))
    {
        return MediaFormat::unsupported("tiff", "TIFF");
    }
    if buffer.len() >= 12 && ascii_at(buffer, 4..8) == b"ftyp" {
        let brand = ascii_at(buffer, 8..12);
        if matches!(
            brand,
            b"heic" | b"heix" | b"hevc" | b"hevx" | b"mif1" | b"msf1"
        ) {
            return MediaFormat::unsupported("heic", "HEIC (так фотографирует iPhone)");
        }
        if matches!(brand, b"avif" | b"avis") {
            return MediaFormat::unsupported("avif", "AVIF");
        }
    }
    let head = String::from_utf8_lossy(at(buffer, 0..buffer.len().min(400)))
        .trim()
        .to_lowercase();
    if head.starts_with("<?xml") || head.starts_with("<svg") {
        return MediaFormat::unsupported("svg", "SVG");
    }
    if head.starts_with("%pdf") {
        return MediaFormat::unsupported("pdf", "PDF");
    }
    MediaFormat::NONE
}

/// `sniffAudioFormat`.
pub fn sniff_audio_format(buffer: &[u8]) -> MediaFormat {
    if buffer.len() >= 3 && ascii_at(buffer, 0..3) == b"ID3" {
        return MediaFormat::supported("mp3", "MP3");
    }
    // Bare MPEG frame: 11 set bits in a row on the frame boundary.
    if buffer.len() >= 2 && buffer[0] == 0xff && (buffer[1] & 0xe0) == 0xe0 {
        return MediaFormat::supported("mp3", "MP3");
    }
    if buffer.len() >= 4 && ascii_at(buffer, 0..4) == b"OggS" {
        return MediaFormat::supported("ogg", "OGG");
    }
    if buffer.len() >= 12 && ascii_at(buffer, 0..4) == b"RIFF" && ascii_at(buffer, 8..12) == b"WAVE"
    {
        return MediaFormat::supported("wav", "WAV");
    }
    if buffer.len() >= 4 && ascii_at(buffer, 0..4) == b"fLaC" {
        return MediaFormat::supported("flac", "FLAC");
    }
    if buffer.len() >= 12 && ascii_at(buffer, 4..8) == b"ftyp" {
        return MediaFormat::supported("m4a", "M4A");
    }
    if buffer.len() >= 4 && at(buffer, 0..4) == [0x1a, 0x45, 0xdf, 0xa3] {
        return MediaFormat::supported("webm", "WEBM");
    }
    if buffer.len() >= 12
        && ascii_at(buffer, 0..4) == b"FORM"
        && matches!(ascii_at(buffer, 8..12), b"AIFF" | b"AIFC")
    {
        return MediaFormat::unsupported("aiff", "AIFF");
    }
    MediaFormat::NONE
}

fn megabytes(max_bytes: usize) -> f64 {
    (max_bytes as f64 / 1024.0 / 1024.0).round()
}

fn decode_base64(body: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(body))
        .ok()
}

struct DataUrl<'a> {
    mime: String,
    body: &'a str,
}

fn parse_data_url<'a>(value: &'a str, kind: &str) -> Option<DataUrl<'a>> {
    let trimmed = value.trim();
    let prefix = format!("data:{kind}/");
    if trimmed.len() < prefix.len() || !trimmed[..prefix.len()].eq_ignore_ascii_case(&prefix) {
        return None;
    }
    let comma = trimmed.find(',')?;
    let meta = &trimmed[prefix.len()..comma];
    let mut parts = meta.split(';');
    let mime = parts.next()?.to_ascii_lowercase();
    if parts.next() != Some("base64") || parts.next().is_some() {
        return None;
    }
    Some(DataUrl {
        mime,
        body: &trimmed[comma + 1..],
    })
}

/// `decodeImage(dataUrl, maxBytes)`.
pub fn decode_image(data_url: Option<&str>, max_bytes: usize) -> Result<DecodedMedia, String> {
    let Some(value) = data_url else {
        return Err("Изображение не передано".to_string());
    };
    if value.is_empty() {
        return Err("Изображение не передано".to_string());
    }
    let Some(parsed) = parse_data_url(value, "image") else {
        return Err("Поддерживаются только изображения PNG, JPG и WEBP".to_string());
    };
    if !matches!(parsed.mime.as_str(), "png" | "jpeg" | "jpg" | "webp") {
        return Err("Поддерживаются только изображения PNG, JPG и WEBP".to_string());
    }
    let Some(bytes) = decode_base64(parsed.body) else {
        return Err("Файл пустой".to_string());
    };
    if bytes.is_empty() {
        return Err("Файл пустой".to_string());
    }
    if bytes.len() > max_bytes {
        return Err(format!("Файл больше {} МБ", megabytes(max_bytes)));
    }
    let format = sniff_image_format(&bytes);
    if !format.supported {
        if let Some(label) = format.label {
            return Err(format!(
                "Формат {label} не поддерживается. Сохраните изображение как PNG, JPG или WEBP."
            ));
        }
        return Err("Файл не является изображением PNG, JPG или WEBP.".to_string());
    }
    Ok(DecodedMedia {
        bytes,
        ext: format.ext.expect("supported image has extension"),
    })
}

/// `decodeAudio(dataUrl)`: audio uploads have their own ceiling.
pub fn decode_audio(data_url: Option<&str>) -> Result<DecodedMedia, String> {
    let Some(value) = data_url else {
        return Err("Звук не передан".to_string());
    };
    if value.is_empty() {
        return Err("Звук не передан".to_string());
    }
    let Some(parsed) = parse_data_url(value, "audio") else {
        return Err("Поддерживаются только аудио MP3, OGG, WAV, M4A, WEBM и FLAC".to_string());
    };
    if !matches!(
        parsed.mime.as_str(),
        "mpeg" | "mp3" | "ogg" | "wav" | "x-wav" | "mp4" | "m4a" | "x-m4a" | "webm" | "flac"
    ) {
        return Err("Поддерживаются только аудио MP3, OGG, WAV, M4A, WEBM и FLAC".to_string());
    }
    let Some(bytes) = decode_base64(parsed.body) else {
        return Err("Файл пустой".to_string());
    };
    if bytes.is_empty() {
        return Err("Файл пустой".to_string());
    }
    if bytes.len() > LIMITS.max_audio_bytes as usize {
        return Err(format!(
            "Файл больше {} МБ",
            megabytes(LIMITS.max_audio_bytes as usize)
        ));
    }
    let format = sniff_audio_format(&bytes);
    if !format.supported {
        if let Some(label) = format.label {
            return Err(format!(
                "Формат {label} не поддерживается. Сохраните звук как MP3, OGG, WAV, M4A, WEBM или FLAC."
            ));
        }
        return Err("Файл не является аудио MP3, OGG, WAV, M4A, WEBM или FLAC.".to_string());
    }
    Ok(DecodedMedia {
        bytes,
        ext: format.ext.expect("supported audio has extension"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX_IMAGE_BYTES: usize = LIMITS.max_image_bytes as usize;
    const MAX_AVATAR_BYTES: usize = LIMITS.max_avatar_bytes as usize;

    const PNG: &[u8] = &[0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
    const JPEG: &[u8] = &[0xff, 0xd8, 0xff, 0xe0];
    const WEBP: &[u8] = b"RIFF\0\0\0\0WEBP";
    const GIF: &[u8] = b"GIF89a";
    const BMP: &[u8] = b"BM";
    const TIFF: &[u8] = b"II*\0";
    const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\">";
    const PDF: &[u8] = b"%PDF-1.7";
    const MP3: &[u8] = b"ID3\x04";
    const OGG: &[u8] = b"OggS";
    const WAV: &[u8] = b"RIFF\0\0\0\0WAVE";
    const FLAC: &[u8] = b"fLaC";
    const M4A: &[u8] = b"\0\0\0\x20ftypM4A ";
    const WEBM: &[u8] = &[0x1a, 0x45, 0xdf, 0xa3];
    const AIFF: &[u8] = b"FORM\0\0\0\0AIFF";

    fn data_url(kind: &str, mime: &str, bytes: &[u8]) -> String {
        format!(
            "data:{kind}/{mime};base64,{}",
            base64::engine::general_purpose::STANDARD.encode(bytes)
        )
    }

    #[test]
    fn sniffs_supported_images() {
        assert_eq!(sniff_image_format(PNG).ext, Some("png"));
        assert_eq!(sniff_image_format(JPEG).ext, Some("jpg"));
        assert_eq!(sniff_image_format(WEBP).ext, Some("webp"));
        assert!(sniff_image_format(PNG).supported);
    }

    #[test]
    fn names_unsupported_images() {
        let gif = sniff_image_format(GIF);
        assert_eq!(gif.label, Some("GIF"));
        assert!(!gif.supported);
        assert_eq!(sniff_image_format(BMP).label, Some("BMP"));
        assert_eq!(sniff_image_format(TIFF).label, Some("TIFF"));
        assert_eq!(sniff_image_format(SVG).label, Some("SVG"));
        assert_eq!(sniff_image_format(PDF).label, Some("PDF"));

        let heic = sniff_image_format(b"\0\0\0\x18ftypheic\0\0\0\0");
        assert_eq!(heic.label, Some("HEIC (так фотографирует iPhone)"));
        let avif = sniff_image_format(b"\0\0\0\x18ftypavif\0\0\0\0");
        assert_eq!(avif.label, Some("AVIF"));
        assert_eq!(sniff_image_format(b"not an image"), MediaFormat::NONE);
    }

    #[test]
    fn decodes_png_data_url() {
        let url = data_url("image", "png", PNG);
        let decoded = decode_image(Some(&url), MAX_IMAGE_BYTES).expect("png");
        assert_eq!(decoded.ext, "png");
        assert_eq!(decoded.bytes, PNG);
    }

    #[test]
    fn rejects_bad_image_input() {
        assert_eq!(
            decode_image(None, MAX_IMAGE_BYTES).unwrap_err(),
            "Изображение не передано"
        );
        assert_eq!(
            decode_image(Some(""), MAX_IMAGE_BYTES).unwrap_err(),
            "Изображение не передано"
        );
        assert_eq!(
            decode_image(Some("data:text/plain;base64,QQ=="), MAX_IMAGE_BYTES).unwrap_err(),
            "Поддерживаются только изображения PNG, JPG и WEBP"
        );
        assert_eq!(
            decode_image(Some(&data_url("image", "png", GIF)), MAX_IMAGE_BYTES).unwrap_err(),
            "Формат GIF не поддерживается. Сохраните изображение как PNG, JPG или WEBP."
        );
        assert_eq!(
            decode_image(Some(&data_url("image", "png", b"plain")), MAX_IMAGE_BYTES).unwrap_err(),
            "Файл не является изображением PNG, JPG или WEBP."
        );
    }

    #[test]
    fn enforces_image_size() {
        let url = data_url("image", "png", &[0u8; 32]);
        let error = decode_image(Some(&url), 16).unwrap_err();
        assert_eq!(error, "Файл больше 0 МБ");
        assert_eq!(megabytes(MAX_AVATAR_BYTES), 2.0);
    }

    #[test]
    fn sniffs_audio_formats() {
        assert_eq!(sniff_audio_format(MP3).ext, Some("mp3"));
        assert_eq!(sniff_audio_format(OGG).ext, Some("ogg"));
        assert_eq!(sniff_audio_format(WAV).ext, Some("wav"));
        assert!(sniff_audio_format(FLAC).supported);
        assert_eq!(sniff_audio_format(FLAC).ext, Some("flac"));
        assert_eq!(sniff_audio_format(FLAC).label, Some("FLAC"));
        assert!(sniff_audio_format(M4A).supported);
        assert_eq!(sniff_audio_format(M4A).ext, Some("m4a"));
        assert!(sniff_audio_format(WEBM).supported);
        assert_eq!(sniff_audio_format(WEBM).ext, Some("webm"));
        assert!(!sniff_audio_format(AIFF).supported);
        assert_eq!(sniff_audio_format(AIFF).label, Some("AIFF"));
    }

    #[test]
    fn decodes_audio_data_url() {
        let url = data_url("audio", "wav", WAV);
        let decoded = decode_audio(Some(&url)).expect("wav");
        assert_eq!(decoded.ext, "wav");
        assert_eq!(
            decode_audio(Some(&data_url("audio", "flac", FLAC)))
                .expect("flac")
                .ext,
            "flac"
        );
        assert_eq!(
            decode_audio(Some(&data_url("audio", "wav", AIFF))).unwrap_err(),
            "Формат AIFF не поддерживается. Сохраните звук как MP3, OGG, WAV, M4A, WEBM или FLAC."
        );
        assert_eq!(decode_audio(None).unwrap_err(), "Звук не передан");
    }
}
