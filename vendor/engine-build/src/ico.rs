//! Формат `.ico`: разбор каталога и сборка из PNG.

use std::io::Cursor;

use image::{imageops::FilterType, ImageFormat, RgbaImage};

use crate::Error;

/// Размеры, которые Windows запрашивает у значка: заголовок окна, панель задач,
/// проводник (от мелких списков до «огромных значков»).
pub const ICO_SIZES: [u32; 7] = [16, 24, 32, 48, 64, 128, 256];

/// Один образ значка: готовые байты (PNG или BMP) и описание из каталога.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconImage {
    /// Ширина в пикселях (256 в каталоге записывается нулём — здесь уже 256).
    pub width: u32,
    pub height: u32,
    pub bits_per_pixel: u16,
    pub data: Vec<u8>,
}

impl IconImage {
    pub fn is_png(&self) -> bool {
        self.data.starts_with(b"\x89PNG\r\n\x1a\n")
    }
}

fn u16_at(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// Разобрать `.ico`: каталог и данные каждого образа.
pub fn parse_ico(bytes: &[u8]) -> Result<Vec<IconImage>, Error> {
    let bad = || Error::BadIco("the file is truncated or is not an .ico (expected a 6-byte header and a directory)".into());
    if u16_at(bytes, 0).ok_or_else(bad)? != 0 || u16_at(bytes, 2).ok_or_else(bad)? != 1 {
        return Err(Error::BadIco("the header says this is not an icon (.ico) file".into()));
    }
    let count = u16_at(bytes, 4).ok_or_else(bad)? as usize;
    if count == 0 {
        return Err(Error::BadIco("the icon file has no images".into()));
    }
    let mut images = Vec::with_capacity(count);
    for i in 0..count {
        let entry = 6 + 16 * i;
        let width = *bytes.get(entry).ok_or_else(bad)?;
        let height = *bytes.get(entry + 1).ok_or_else(bad)?;
        let bits = u16_at(bytes, entry + 6).ok_or_else(bad)?;
        let size = u32_at(bytes, entry + 8).ok_or_else(bad)? as usize;
        let offset = u32_at(bytes, entry + 12).ok_or_else(bad)? as usize;
        let data = bytes.get(offset..offset.checked_add(size).ok_or_else(bad)?).ok_or_else(bad)?;
        images.push(IconImage {
            width: if width == 0 { 256 } else { u32::from(width) },
            height: if height == 0 { 256 } else { u32::from(height) },
            bits_per_pixel: bits,
            data: data.to_vec(),
        });
    }
    Ok(images)
}

/// Собрать `.ico` из образов (PNG-данные, Windows Vista и новее).
pub fn write_ico(images: &[IconImage]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for image in images {
        out.push(if image.width >= 256 { 0 } else { image.width as u8 });
        out.push(if image.height >= 256 { 0 } else { image.height as u8 });
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&image.bits_per_pixel.to_le_bytes());
        out.extend_from_slice(&(image.data.len() as u32).to_le_bytes());
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += image.data.len();
    }
    for image in images {
        out.extend_from_slice(&image.data);
    }
    out
}

/// Многоразмерный `.ico` из одного PNG (свой значок игры): уменьшение Lanczos,
/// картинка вписывается в квадрат по центру.
pub fn ico_from_png(png: &[u8]) -> Result<Vec<u8>, Error> {
    let source = image::load_from_memory_with_format(png, ImageFormat::Png)
        .map_err(|e| Error::BadPng(e.to_string()))?
        .to_rgba8();
    let mut images = Vec::new();
    for size in ICO_SIZES {
        let scale = size as f32 / source.width().max(source.height()).max(1) as f32;
        let fit_w = ((source.width() as f32 * scale).round() as u32).clamp(1, size);
        let fit_h = ((source.height() as f32 * scale).round() as u32).clamp(1, size);
        let fitted = image::imageops::resize(&source, fit_w, fit_h, FilterType::Lanczos3);
        let mut canvas = RgbaImage::new(size, size);
        let x = (size - fit_w) / 2;
        let y = (size - fit_h) / 2;
        image::imageops::overlay(&mut canvas, &fitted, i64::from(x), i64::from(y));
        let mut data = Cursor::new(Vec::new());
        canvas
            .write_to(&mut data, ImageFormat::Png)
            .map_err(|e| Error::BadPng(e.to_string()))?;
        images.push(IconImage { width: size, height: size, bits_per_pixel: 32, data: data.into_inner() });
    }
    Ok(write_ico(&images))
}
