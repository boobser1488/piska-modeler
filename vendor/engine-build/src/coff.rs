//! Ресурсы Windows без `rc.exe`: готовый COFF-объект с секциями `.rsrc$01` / `.rsrc$02`.
//!
//! Это тот же формат, что выдаёт связка `rc.exe` + `cvtres.exe`: линкер сам склеивает
//! такие секции в `.rsrc` исполняемого файла. Объект содержит значок как два вида
//! ресурсов — `RT_ICON` (образы) и `RT_GROUP_ICON` (каталог образов), их читают
//! проводник, панель задач и `LoadIcon`.
//!
//! Раскладка `.rsrc$01` — дерево каталогов «тип → имя → язык → запись данных»;
//! адреса данных в записях — RVA, поэтому на каждую запись приходится перемещение
//! `ADDR32NB` относительно секции `.rsrc$02`, где лежат сами байты.

use crate::ico::IconImage;
use crate::Error;

/// Целевая архитектура объекта.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X86,
    X64,
    Arm64,
}

impl Arch {
    /// Из `CARGO_CFG_TARGET_ARCH`.
    pub fn from_cargo(name: &str) -> Option<Arch> {
        match name {
            "x86" => Some(Arch::X86),
            "x86_64" => Some(Arch::X64),
            "aarch64" => Some(Arch::Arm64),
            _ => None,
        }
    }

    fn machine(self) -> u16 {
        match self {
            Arch::X86 => 0x014c,
            Arch::X64 => 0x8664,
            Arch::Arm64 => 0xaa64,
        }
    }

    /// Тип перемещения «32-битный RVA».
    fn rva_relocation(self) -> u16 {
        match self {
            Arch::X86 => 7,   // IMAGE_REL_I386_DIR32NB
            Arch::X64 => 3,   // IMAGE_REL_AMD64_ADDR32NB
            Arch::Arm64 => 2, // IMAGE_REL_ARM64_ADDR32NB
        }
    }
}

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;
const SUBDIR: u32 = 0x8000_0000;

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}
fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Каталог ресурсов только с числовыми идентификаторами: `(id, смещение, подкаталог?)`.
fn directory(out: &mut Vec<u8>, entries: &[(u32, u32, bool)]) {
    put32(out, 0); // Characteristics
    put32(out, 0); // TimeDateStamp: ноль, объект воспроизводим
    put16(out, 0); // MajorVersion
    put16(out, 0); // MinorVersion
    put16(out, 0); // NumberOfNamedEntries
    put16(out, entries.len() as u16); // NumberOfIdEntries
    for &(id, offset, subdir) in entries {
        put32(out, id);
        put32(out, if subdir { offset | SUBDIR } else { offset });
    }
}

fn pad4(data: &mut Vec<u8>) {
    while !data.len().is_multiple_of(4) {
        data.push(0);
    }
}

/// COFF-объект с ресурсом значка (`RT_GROUP_ICON` №1 и `RT_ICON` №1…n, язык нейтральный).
pub fn icon_resource_object(images: &[IconImage], arch: Arch) -> Result<Vec<u8>, Error> {
    if images.is_empty() || images.len() > 255 {
        return Err(Error::BadIco(format!("an icon needs 1 to 255 images, got {}", images.len())));
    }
    let n = images.len();

    // ---- .rsrc$02: данные ресурсов (каждый кусок выровнен по 4 байта).
    let mut data = Vec::new();
    let mut icon_data = Vec::new(); // (смещение, размер)
    for image in images {
        icon_data.push((data.len() as u32, image.data.len() as u32));
        data.extend_from_slice(&image.data);
        pad4(&mut data);
    }
    // GRPICONDIR: заголовок и по 14 байт на образ (в конце — id ресурса RT_ICON).
    let group_offset = data.len() as u32;
    put16(&mut data, 0);
    put16(&mut data, 1);
    put16(&mut data, n as u16);
    for (i, image) in images.iter().enumerate() {
        data.push(if image.width >= 256 { 0 } else { image.width as u8 });
        data.push(if image.height >= 256 { 0 } else { image.height as u8 });
        data.extend_from_slice(&[0, 0]); // число цветов, зарезервировано
        put16(&mut data, 1); // planes
        put16(&mut data, image.bits_per_pixel);
        put32(&mut data, image.data.len() as u32);
        put16(&mut data, (i + 1) as u16);
    }
    let group_size = data.len() as u32 - group_offset;
    pad4(&mut data);

    // ---- .rsrc$01: дерево каталогов. Смещения считаем заранее.
    let root = 0u32;
    let icon_dir = root + 16 + 2 * 8;
    let group_dir = icon_dir + 16 + n as u32 * 8;
    let lang_dirs = group_dir + 16 + 8; // n + 1 каталогов языка по 24 байта
    let entries_at = lang_dirs + (n as u32 + 1) * 24; // n + 1 записей данных по 16 байт

    let mut tree = Vec::new();
    directory(&mut tree, &[(RT_ICON, icon_dir, true), (RT_GROUP_ICON, group_dir, true)]);
    let icon_entries: Vec<(u32, u32, bool)> =
        (0..n as u32).map(|i| (i + 1, lang_dirs + i * 24, true)).collect();
    directory(&mut tree, &icon_entries);
    directory(&mut tree, &[(1, lang_dirs + n as u32 * 24, true)]);
    for i in 0..=n as u32 {
        directory(&mut tree, &[(0, entries_at + i * 16, false)]);
    }
    debug_assert_eq!(tree.len() as u32, entries_at);
    let mut relocations = Vec::new(); // смещение поля OffsetToData в .rsrc$01
    for &(offset, size) in &icon_data {
        relocations.push(tree.len() as u32);
        put32(&mut tree, offset); // RVA = начало .rsrc$02 + offset (допишет линкер)
        put32(&mut tree, size);
        put32(&mut tree, 0); // CodePage
        put32(&mut tree, 0);
    }
    relocations.push(tree.len() as u32);
    put32(&mut tree, group_offset);
    put32(&mut tree, group_size);
    put32(&mut tree, 0);
    put32(&mut tree, 0);

    // ---- COFF-файл.
    const HEADER: usize = 20;
    const SECTION_HEADER: usize = 40;
    let raw1 = HEADER + 2 * SECTION_HEADER;
    let reloc1 = raw1 + tree.len();
    let raw2 = reloc1 + relocations.len() * 10;
    let symbols = raw2 + data.len();
    let flags: u32 = 0x4030_0040; // данные, инициализированные | чтение | выравнивание 4

    let mut out = Vec::new();
    put16(&mut out, arch.machine());
    put16(&mut out, 2); // секций
    put32(&mut out, 0); // время
    put32(&mut out, symbols as u32);
    put32(&mut out, 4); // символов (2 секции × (символ + вспомогательная запись))
    put16(&mut out, 0);
    put16(&mut out, 0);

    for (name, size, raw, reloc, count) in [
        (b".rsrc$01", tree.len(), raw1, reloc1, relocations.len()),
        (b".rsrc$02", data.len(), raw2, 0, 0),
    ] {
        out.extend_from_slice(name);
        put32(&mut out, 0); // VirtualSize
        put32(&mut out, 0); // VirtualAddress
        put32(&mut out, size as u32);
        put32(&mut out, raw as u32);
        put32(&mut out, reloc as u32);
        put32(&mut out, 0);
        put16(&mut out, count as u16);
        put16(&mut out, 0);
        put32(&mut out, flags);
    }

    out.extend_from_slice(&tree);
    for &at in &relocations {
        put32(&mut out, at);
        put32(&mut out, 2); // индекс символа секции .rsrc$02
        put16(&mut out, arch.rva_relocation());
    }
    out.extend_from_slice(&data);

    for (name, section, size, relocs) in [
        (b".rsrc$01", 1i16, tree.len(), relocations.len()),
        (b".rsrc$02", 2i16, data.len(), 0),
    ] {
        out.extend_from_slice(name);
        put32(&mut out, 0); // Value
        out.extend_from_slice(&section.to_le_bytes());
        put16(&mut out, 0); // Type
        out.push(3); // IMAGE_SYM_CLASS_STATIC
        out.push(1); // одна вспомогательная запись
        put32(&mut out, size as u32); // aux: Length
        put16(&mut out, relocs as u16);
        put16(&mut out, 0);
        put32(&mut out, 0); // CheckSum
        put16(&mut out, section as u16);
        out.extend_from_slice(&[0; 4]); // Selection + выравнивание до 18 байт
    }
    put32(&mut out, 4); // таблица строк: только поле длины
    Ok(out)
}
