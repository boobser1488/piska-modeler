//! Пиксельные картинки текстур: RGBA8, копирование при записи, PNG и base64.
//!
//! Пиксели лежат за `Arc`: копия документа для истории правок стоит несколько указателей,
//! а настоящая копия пикселей делается, только когда в картинку рисуют.

use std::sync::Arc;

use crate::ModelError;

/// Цвет RGBA, по байту на канал (sRGB, как в PNG).
pub type Rgba = [u8; 4];

/// Прозрачный пиксель — то, что оставляет ластик.
pub const CLEAR: Rgba = [0, 0, 0, 0];

/// Картинка RGBA8; (0, 0) — левый верхний угол.
#[derive(Clone, PartialEq, Eq)]
pub struct Image {
    width: u32,
    height: u32,
    pixels: Arc<Vec<Rgba>>,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Image({}x{})", self.width, self.height)
    }
}

impl Image {
    /// Картинка, залитая одним цветом.
    pub fn new(width: u32, height: u32, fill: Rgba) -> Self {
        let (width, height) = (width.max(1), height.max(1));
        Self {
            width,
            height,
            pixels: Arc::new(vec![fill; (width * height) as usize]),
        }
    }

    pub fn from_pixels(width: u32, height: u32, pixels: Vec<Rgba>) -> Result<Self, ModelError> {
        if width == 0 || height == 0 || pixels.len() != (width * height) as usize {
            return Err(ModelError::Format(format!(
                "an image of {width}x{height} needs {} pixels, got {}",
                width * height,
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            pixels: Arc::new(pixels),
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    pub fn pixels(&self) -> &[Rgba] {
        &self.pixels
    }

    /// Пиксели для записи (копия, если их ещё держит история правок).
    pub fn pixels_mut(&mut self) -> &mut Vec<Rgba> {
        Arc::make_mut(&mut self.pixels)
    }

    /// Те же ли это байты в памяти (а не просто равные) — для кэшей интерфейса.
    pub fn shares_pixels(&self, other: &Image) -> bool {
        Arc::ptr_eq(&self.pixels, &other.pixels)
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && (x as u32) < self.width && (y as u32) < self.height
    }

    pub fn get(&self, x: i32, y: i32) -> Option<Rgba> {
        self.contains(x, y)
            .then(|| self.pixels[(y as u32 * self.width + x as u32) as usize])
    }

    /// Записать пиксель; за пределами картинки — ничего. Возвращает, изменился ли он.
    pub fn set(&mut self, x: i32, y: i32, color: Rgba) -> bool {
        if !self.contains(x, y) {
            return false;
        }
        let index = (y as u32 * self.width + x as u32) as usize;
        if self.pixels[index] == color {
            return false;
        }
        self.pixels_mut()[index] = color;
        true
    }

    /// Новый размер холста: старые пиксели остаются на месте (слева сверху), новое — `fill`.
    pub fn resized_canvas(&self, width: u32, height: u32, fill: Rgba) -> Image {
        let mut out = Image::new(width, height, fill);
        out.blit(self, 0, 0, 0, 0, self.width, self.height);
        out
    }

    /// Масштаб «ближайшим соседом» (пиксели остаются резкими).
    pub fn scaled_nearest(&self, width: u32, height: u32) -> Image {
        let (width, height) = (width.max(1), height.max(1));
        let mut pixels = Vec::with_capacity((width * height) as usize);
        for y in 0..height {
            let sy = (y * self.height / height).min(self.height - 1);
            for x in 0..width {
                let sx = (x * self.width / width).min(self.width - 1);
                pixels.push(self.pixels[(sy * self.width + sx) as usize]);
            }
        }
        Image {
            width,
            height,
            pixels: Arc::new(pixels),
        }
    }

    /// Скопировать прямоугольник `w×h` из `source` (от `sx, sy`) в эту картинку (в `dx, dy`).
    /// Части за краями обеих картинок отбрасываются.
    #[allow(clippy::too_many_arguments)]
    pub fn blit(&mut self, source: &Image, sx: i32, sy: i32, dx: i32, dy: i32, w: u32, h: u32) {
        for row in 0..h as i32 {
            for col in 0..w as i32 {
                if let Some(color) = source.get(sx + col, sy + row) {
                    if self.contains(dx + col, dy + row) {
                        let index = ((dy + row) as u32 * self.width + (dx + col) as u32) as usize;
                        if self.pixels[index] != color {
                            self.pixels_mut()[index] = color;
                        }
                    }
                }
            }
        }
    }

    /// Вырезать прямоугольник в новую картинку.
    pub fn crop(&self, x: i32, y: i32, w: u32, h: u32) -> Image {
        let mut out = Image::new(w, h, CLEAR);
        out.blit(self, x, y, 0, 0, w, h);
        out
    }

    /// Закодировать в PNG.
    pub fn to_png(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        let raw: Vec<u8> = self.pixels.iter().flatten().copied().collect();
        let encoder = image::codecs::png::PngEncoder::new(&mut bytes);
        image::ImageEncoder::write_image(encoder, &raw, self.width, self.height, image::ExtendedColorType::Rgba8)
            .expect("encoding PNG into memory cannot fail");
        bytes
    }

    /// Прочитать PNG (или другой формат, который понимает крейт `image`: JPEG).
    pub fn from_png(bytes: &[u8]) -> Result<Image, ModelError> {
        let decoded = image::load_from_memory(bytes)
            .map_err(|e| ModelError::Format(format!("cannot decode the texture image: {e}")))?
            .to_rgba8();
        let (width, height) = decoded.dimensions();
        let pixels = decoded.pixels().map(|p| p.0).collect();
        Image::from_pixels(width, height, pixels)
    }

    /// Картинка крейта `image` (для сохранения в файл и сравнения в тестах).
    pub fn to_rgba_image(&self) -> image::RgbaImage {
        let raw: Vec<u8> = self.pixels.iter().flatten().copied().collect();
        image::RgbaImage::from_raw(self.width, self.height, raw).expect("size matches")
    }
}

/// HEX-запись цвета: `#rrggbb` или `#rrggbbaa` (прозрачность — только если не 255).
pub fn to_hex(color: Rgba) -> String {
    if color[3] == 255 {
        format!("#{:02x}{:02x}{:02x}", color[0], color[1], color[2])
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", color[0], color[1], color[2], color[3])
    }
}

/// Разобрать `#rgb`, `#rrggbb` или `#rrggbbaa` (решётка необязательна).
pub fn parse_hex(text: &str) -> Option<Rgba> {
    let text = text.trim().trim_start_matches('#');
    let digit = |i: usize| u8::from_str_radix(text.get(i..i + 1)?, 16).ok();
    let byte = |i: usize| u8::from_str_radix(text.get(i..i + 2)?, 16).ok();
    match text.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255]),
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

/// Умножить цвет на оттенок (покомпонентно, включая прозрачность).
pub fn multiply(a: Rgba, b: Rgba) -> Rgba {
    let m = |x: u8, y: u8| ((x as u32 * y as u32 + 127) / 255) as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2]), m(a[3], b[3])]
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Base64 (стандартный алфавит, с `=`) — PNG внутри текстового `.pmodel`.
pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(BASE64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn base64_decode(text: &str) -> Result<Vec<u8>, ModelError> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in text.bytes() {
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            b' ' | b'\n' | b'\r' | b'\t' => continue,
            other => {
                return Err(ModelError::Format(format!(
                    "the embedded texture is damaged: `{}` is not base64",
                    other as char
                )))
            }
        };
        acc = acc << 6 | value as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trip_matches_the_standard() {
        assert_eq!(base64_encode(b"Man"), "TWFu");
        assert_eq!(base64_encode(b"Ma"), "TWE=");
        assert_eq!(base64_encode(b"M"), "TQ==");
        for len in 0..40u8 {
            let data: Vec<u8> = (0..len).map(|i| i.wrapping_mul(37).wrapping_add(5)).collect();
            assert_eq!(base64_decode(&base64_encode(&data)).unwrap(), data);
        }
        assert!(base64_decode("ab$c").is_err());
    }

    #[test]
    fn png_round_trip_and_copy_on_write() {
        let mut image = Image::new(5, 3, [10, 20, 30, 255]);
        image.set(4, 2, [1, 2, 3, 4]);
        let back = Image::from_png(&image.to_png()).unwrap();
        assert_eq!(back, image);
        let copy = image.clone();
        assert!(copy.shares_pixels(&image));
        image.set(0, 0, CLEAR);
        assert!(!copy.shares_pixels(&image), "writing makes a private copy");
        assert_eq!(copy.get(0, 0), Some([10, 20, 30, 255]));
    }

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_hex("f80"), Some([255, 136, 0, 255]));
        assert_eq!(parse_hex("#00000080"), Some([0, 0, 0, 128]));
        assert_eq!(parse_hex("zz"), None);
        assert_eq!(to_hex([255, 128, 0, 255]), "#ff8000");
        assert_eq!(to_hex([0, 0, 0, 128]), "#00000080");
    }
}
