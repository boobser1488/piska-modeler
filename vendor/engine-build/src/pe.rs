//! Чтение значка из готового `.exe` / `.dll` (PE): им проверяют, что значок действительно
//! попал в файл, — по таблице ресурсов, как это делает проводник.

use crate::ico::IconImage;
use crate::Error;

const RT_ICON: u32 = 3;
const RT_GROUP_ICON: u32 = 14;

struct Pe<'a> {
    bytes: &'a [u8],
    /// (RVA, размер в памяти, смещение в файле, размер в файле)
    sections: Vec<(u32, u32, u32, u32)>,
    resource_rva: u32,
    resource_size: u32,
}

fn bad(what: &str) -> Error {
    Error::BadPe(what.to_string())
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, Error> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| bad("the file ends inside a header"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, Error> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| bad("the file ends inside a header"))
}

impl<'a> Pe<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Pe<'a>, Error> {
        if bytes.get(..2) != Some(b"MZ") {
            return Err(bad("not a Windows executable (no `MZ` header)"));
        }
        let pe = u32_at(bytes, 0x3c)? as usize;
        if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err(bad("not a PE file (no `PE` signature)"));
        }
        let sections_count = u16_at(bytes, pe + 6)? as usize;
        let optional_size = u16_at(bytes, pe + 20)? as usize;
        let optional = pe + 24;
        let directories = match u16_at(bytes, optional)? {
            0x10b => optional + 96,  // PE32
            0x20b => optional + 112, // PE32+
            _ => return Err(bad("unknown optional header magic")),
        };
        // Каталог данных №2 — ресурсы.
        let resource_rva = u32_at(bytes, directories + 2 * 8)?;
        let resource_size = u32_at(bytes, directories + 2 * 8 + 4)?;
        let table = optional + optional_size;
        let mut sections = Vec::new();
        for i in 0..sections_count {
            let at = table + i * 40;
            sections.push((
                u32_at(bytes, at + 12)?,
                u32_at(bytes, at + 8)?,
                u32_at(bytes, at + 20)?,
                u32_at(bytes, at + 16)?,
            ));
        }
        Ok(Pe { bytes, sections, resource_rva, resource_size })
    }

    fn file_offset(&self, rva: u32) -> Result<usize, Error> {
        self.sections
            .iter()
            .find(|&&(va, virtual_size, _, raw_size)| rva >= va && rva < va + virtual_size.max(raw_size))
            .map(|&(va, _, raw, _)| (raw + (rva - va)) as usize)
            .ok_or_else(|| bad("a resource points outside every section"))
    }

    /// Записи каталога ресурсов по смещению от начала `.rsrc`: `(id, смещение, подкаталог?)`.
    fn entries(&self, root: usize, dir: u32) -> Result<Vec<(u32, u32, bool)>, Error> {
        let at = root + dir as usize;
        let named = u16_at(self.bytes, at + 12)? as usize;
        let ids = u16_at(self.bytes, at + 14)? as usize;
        let mut out = Vec::new();
        for i in 0..named + ids {
            let entry = at + 16 + i * 8;
            let name = u32_at(self.bytes, entry)?;
            let target = u32_at(self.bytes, entry + 4)?;
            out.push((name, target & 0x7fff_ffff, target & 0x8000_0000 != 0));
        }
        Ok(out)
    }

    /// Данные первого ресурса типа `kind` с именем `name` (любой язык).
    fn resource(&self, kind: u32, name: Option<u32>) -> Result<Option<&'a [u8]>, Error> {
        if self.resource_rva == 0 || self.resource_size == 0 {
            return Ok(None);
        }
        let root = self.file_offset(self.resource_rva)?;
        let Some(&(_, kind_dir, true)) = self.entries(root, 0)?.iter().find(|e| e.0 == kind) else {
            return Ok(None);
        };
        let names = self.entries(root, kind_dir)?;
        let Some(&(_, name_dir, true)) = names.iter().find(|e| name.is_none_or(|n| e.0 == n)) else {
            return Ok(None);
        };
        let Some(&(_, data_entry, false)) = self.entries(root, name_dir)?.first() else {
            return Ok(None);
        };
        let at = root + data_entry as usize;
        let rva = u32_at(self.bytes, at)?;
        let size = u32_at(self.bytes, at + 4)? as usize;
        let start = self.file_offset(rva)?;
        self.bytes
            .get(start..start + size)
            .map(Some)
            .ok_or_else(|| bad("a resource runs past the end of the file"))
    }
}

/// Значок исполняемого файла: образы первой группы `RT_GROUP_ICON`.
/// `Ok(None)` — в файле нет значка.
pub fn read_exe_icon(exe: &[u8]) -> Result<Option<Vec<IconImage>>, Error> {
    let pe = Pe::parse(exe)?;
    let Some(group) = pe.resource(RT_GROUP_ICON, None)? else {
        return Ok(None);
    };
    let count = u16_at(group, 4)? as usize;
    let mut images = Vec::new();
    for i in 0..count {
        let entry = 6 + i * 14;
        let width = *group.get(entry).ok_or_else(|| bad("the icon group is truncated"))?;
        let height = *group.get(entry + 1).ok_or_else(|| bad("the icon group is truncated"))?;
        let bits = u16_at(group, entry + 6)?;
        let id = u16_at(group, entry + 12)?;
        let data = pe
            .resource(RT_ICON, Some(u32::from(id)))?
            .ok_or_else(|| bad("the icon group names an image that is not in the file"))?;
        images.push(IconImage {
            width: if width == 0 { 256 } else { u32::from(width) },
            height: if height == 0 { 256 } else { u32::from(height) },
            bits_per_pixel: bits,
            data: data.to_vec(),
        });
    }
    Ok(Some(images))
}
