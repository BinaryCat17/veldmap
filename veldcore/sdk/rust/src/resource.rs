//! Потоковое чтение ресурса: `Read + Seek` поверх `arena_read`.
//!
//! Ресурс не обязан лежать в памяти — за его id может стоять файл на диске
//! (fs отдаёт именно такой), и тогда чтение диапазона читает с диска ровно
//! запрошенное окно. Модулю знать это не нужно: он получает обычный
//! `Read + Seek` и отдаёт его любому парсеру (png, tiff, zip, netcdf…),
//! а память тратится на одно окно, а не на весь ресурс.
//!
//! Владение ресурсом читатель не берёт: освобождает его тот, кому он
//! принадлежит (обычно через `OwnedResource`).

use std::io::{self, Read, Seek, SeekFrom};

/// Окно чтения. Компромисс между числом ABI-вызовов и памятью: гигабайтный
/// файл при 256 КБ окна — это тысячи вызовов, что на фоне декодирования
/// незаметно.
const WINDOW: u64 = 256 * 1024;

pub struct ResourceReader {
    id: u64,
    len: u64,
    pos: u64,
    /// Прочитанное окно и смещение, с которого оно начинается.
    window: Vec<u8>,
    window_at: u64,
}

impl ResourceReader {
    /// `len` — размер ресурса (ResourceHandle.size).
    pub fn new(id: u64, len: u64) -> Self {
        Self { id, len, pos: 0, window: Vec::new(), window_at: 0 }
    }

    pub fn len(&self) -> u64 { self.len }
    pub fn is_empty(&self) -> bool { self.len == 0 }

    /// Подтягивает окно, покрывающее текущую позицию. Ok(false) — позиция
    /// за концом ресурса.
    fn fill(&mut self) -> io::Result<bool> {
        if self.pos >= self.len { return Ok(false); }
        let covered = self.pos >= self.window_at
            && self.pos < self.window_at + self.window.len() as u64;
        if covered { return Ok(true); }

        let size = WINDOW.min(self.len - self.pos);
        let data = crate::abi::arena_read(self.id, self.pos, size).ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other,
                format!("resource {}: чтение {} байт со смещения {} не удалось", self.id, size, self.pos))
        })?;
        if data.is_empty() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof,
                format!("resource {}: пустое чтение на смещении {}", self.id, self.pos)));
        }
        self.window_at = self.pos;
        self.window = data;
        Ok(true)
    }
}

impl Read for ResourceReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || !self.fill()? { return Ok(0); }
        let from = (self.pos - self.window_at) as usize;
        let n = buf.len().min(self.window.len() - from);
        buf[..n].copy_from_slice(&self.window[from..from + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for ResourceReader {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => n as i64,
            SeekFrom::End(n) => self.len as i64 + n,
            SeekFrom::Current(n) => self.pos as i64 + n,
        };
        if target < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek до начала ресурса"));
        }
        // Позиция за концом допустима (как у файла) — чтение оттуда вернёт 0.
        self.pos = target as u64;
        Ok(self.pos)
    }
}
