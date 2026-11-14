//! Картинки RGBA8: чтение и запись PNG, `data:`-URI.

use crate::{base64, limits, Error};

/// Картинка RGBA, 8 бит на канал, строки сверху вниз.
#[derive(Clone, PartialEq, Eq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` байт.
    pub pixels: Vec<u8>,
}

impl std::fmt::Debug for RgbaImage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RgbaImage({}x{})", self.width, self.height)
    }
}

impl RgbaImage {
    pub fn new(width: u32, height: u32, fill: [u8; 4]) -> Self {
        let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
        for _ in 0..width as usize * height as usize {
            pixels.extend_from_slice(&fill);
        }
        Self { width, height, pixels }
    }

    /// Пиксель; за пределами — прозрачный.
    pub fn get(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.pixels[i], self.pixels[i + 1], self.pixels[i + 2], self.pixels[i + 3]]
    }

    pub fn set(&mut self, x: u32, y: u32, color: [u8; 4]) {
        if x < self.width && y < self.height {
            let i = (y as usize * self.width as usize + x as usize) * 4;
            self.pixels[i..i + 4].copy_from_slice(&color);
        }
    }

    /// Пиксель по UV (0…1, (0, 0) — левый верхний угол), ближайший.
    pub fn sample(&self, uv: [f32; 2]) -> [u8; 4] {
        let x = (uv[0] * self.width as f32).floor().clamp(0.0, self.width.saturating_sub(1) as f32) as u32;
        let y = (uv[1] * self.height as f32).floor().clamp(0.0, self.height.saturating_sub(1) as f32) as u32;
        self.get(x, y)
    }

    /// Часть картинки (обрезается по краям).
    pub fn crop(&self, x: u32, y: u32, width: u32, height: u32) -> RgbaImage {
        let width = width.min(self.width.saturating_sub(x));
        let height = height.min(self.height.saturating_sub(y));
        let mut out = RgbaImage::new(width, height, [0; 4]);
        for row in 0..height {
            let from = ((y + row) as usize * self.width as usize + x as usize) * 4;
            let to = row as usize * width as usize * 4;
            out.pixels[to..to + width as usize * 4].copy_from_slice(&self.pixels[from..from + width as usize * 4]);
        }
        out
    }

    /// Скопировать `source` в эту картинку с левым верхним углом в (`x`, `y`).
    pub fn blit(&mut self, source: &RgbaImage, x: u32, y: u32) {
        for row in 0..source.height {
            for column in 0..source.width {
                self.set(x + column, y + row, source.get(column, row));
            }
        }
    }
}

/// Разобрать PNG (любой тип цвета и глубина → RGBA8). Размеры проверяются по
/// [`limits::MAX_TEXTURE_SIDE`] до выделения памяти под пиксели.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, Error> {
    let side = limits::MAX_TEXTURE_SIDE as usize;
    let mut decoder = png::Decoder::new_with_limits(std::io::Cursor::new(bytes), png::Limits { bytes: side * side * 4 + (1 << 20) });
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| Error(format!("the PNG image is damaged ({e})")))?;
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > limits::MAX_TEXTURE_SIDE || height > limits::MAX_TEXTURE_SIDE {
        return Err(Error(format!(
            "the image is {width}x{height}; textures must be 1..{} pixels on each side",
            limits::MAX_TEXTURE_SIDE
        )));
    }
    let size = reader.output_buffer_size().ok_or_else(|| Error("the PNG image is too large".to_string()))?;
    let mut buffer = vec![0u8; size];
    let info = reader.next_frame(&mut buffer).map_err(|e| Error(format!("the PNG image is damaged ({e})")))?;
    let data = &buffer[..info.buffer_size().min(buffer.len())];
    let channels = match info.color_type {
        png::ColorType::Grayscale => 1,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        png::ColorType::Indexed => return Err(Error("the PNG palette could not be expanded".to_string())),
    };
    if info.bit_depth != png::BitDepth::Eight {
        return Err(Error("the PNG image has an unsupported bit depth".to_string()));
    }
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    for row in 0..height as usize {
        let line = data.get(row * info.line_size..row * info.line_size + width as usize * channels);
        let line = line.ok_or_else(|| Error("the PNG image is truncated".to_string()))?;
        for p in line.chunks_exact(channels) {
            pixels.extend_from_slice(&match channels {
                1 => [p[0], p[0], p[0], 255],
                2 => [p[0], p[0], p[0], p[1]],
                3 => [p[0], p[1], p[2], 255],
                _ => [p[0], p[1], p[2], p[3]],
            });
        }
    }
    Ok(RgbaImage { width, height, pixels })
}

/// Записать картинку в PNG.
pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, image.width.max(1), image.height.max(1));
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut pixels = image.pixels.clone();
        pixels.resize(image.width.max(1) as usize * image.height.max(1) as usize * 4, 0);
        // Запись в память не отказывает: размеры и длина данных согласованы выше.
        let mut writer = encoder.write_header().expect("PNG header for an in-memory image");
        writer.write_image_data(&pixels).expect("PNG data of the right length");
        writer.finish().expect("PNG end of an in-memory image");
    }
    out
}

/// Байты из `data:`-URI (`data:image/png;base64,…`).
pub fn decode_data_uri(uri: &str) -> Result<Vec<u8>, Error> {
    let rest = uri.strip_prefix("data:").ok_or_else(|| Error("the embedded data is not a data: URI".to_string()))?;
    let (header, payload) = rest.split_once(',').ok_or_else(|| Error("the data: URI has no `,`".to_string()))?;
    if !header.ends_with(";base64") {
        return Err(Error(format!("the data: URI `data:{header}` is not base64-encoded")));
    }
    base64::decode(payload)
}

/// `data:image/png;base64,…` для картинки.
pub fn png_data_uri(image: &RgbaImage) -> String {
    format!("data:image/png;base64,{}", base64::encode(&encode_png(image)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_round_trip_and_hostile_input() {
        let mut image = RgbaImage::new(3, 2, [0, 0, 0, 0]);
        image.set(2, 1, [10, 20, 30, 40]);
        image.set(0, 0, [255, 0, 0, 255]);
        let back = decode_png(&encode_png(&image)).unwrap();
        assert_eq!(back, image);
        assert_eq!(decode_png(&decode_data_uri(&png_data_uri(&image)).unwrap()).unwrap(), image);
        assert!(decode_png(b"not a png").unwrap_err().0.contains("damaged"));
        let png = encode_png(&image);
        assert!(decode_png(&png[..png.len() / 2]).is_err(), "truncated");
        // Заголовок PNG с размером 100000×100000: отказ до выделения памяти.
        let mut huge = png.clone();
        huge[16..20].copy_from_slice(&100_000u32.to_be_bytes());
        huge[20..24].copy_from_slice(&100_000u32.to_be_bytes());
        assert!(decode_png(&huge).is_err());
        assert!(decode_data_uri("data:image/png,abc").is_err());
        assert_eq!(image.sample([0.99, 0.99]), [10, 20, 30, 40]);
        assert_eq!(image.crop(2, 1, 5, 5).pixels, vec![10, 20, 30, 40]);
    }
}
