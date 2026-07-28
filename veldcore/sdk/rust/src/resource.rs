//! Ресурсы на стороне модуля: протокол «открой мне это» и потоковое чтение.
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

use crate::proto::core::{ResourceHandle, ResourceOpened};

// ── Протокол «открой мне это» ──────────────────────────────────
//
// Ответ у всех открывающих один — `core.ResourceOpened` (см. core.proto), и
// обряд вокруг него тоже один: узнать заказчика, дождаться ресурса, передать
// ему владение, ответить. Он собран здесь, потому что был написан заново в
// каждом модуле, который что-то открывает (data-library, data-provider,
// image-loader) — вплоть до совпадающих текстов ошибок, которые при этом
// успели разойтись формулировками.

/// Заказчик текущего события — тот, кому уйдёт владение открытым ресурсом.
///
/// `Err` — событие опубликовал хост: у него нет модульной идентичности, а
/// значит `arena_transfer` некому адресовать. Отдать ресурс «в никуда» нельзя,
/// поэтому это отказ, а не предупреждение.
pub fn requester(topic: &str) -> Result<String, String> {
    let owner = crate::abi::event_publisher();
    if owner.is_empty() {
        return Err(format!("{} пришёл от хоста: ресурс передать некому", topic));
    }
    Ok(owner)
}

/// Ресурс из ответа «открой мне это». `Err` — открыть не удалось: либо
/// производитель сообщил ошибку, либо ответил пустым handle.
pub fn accept(opened: &ResourceOpened) -> Result<ResourceHandle, String> {
    if !opened.error.is_empty() {
        return Err(opened.error.clone());
    }
    opened.handle.clone().ok_or_else(|| "ответ без handle".to_string())
}

/// Передаёт владение ресурсом заказчику.
///
/// При отказе ресурс освобождается здесь же: иначе он остался бы висеть на
/// открывшем, которому он уже не нужен, — а заказчик про него не узнает и
/// освободить не сможет.
pub fn hand_off(handle: ResourceHandle, owner: &str) -> Result<ResourceHandle, String> {
    if crate::abi::arena_transfer(handle.id, owner) {
        return Ok(handle);
    }
    crate::abi::arena_free(handle.id);
    Err(format!("не удалось передать ресурс сервису '{}'", owner))
}

/// Собирает ответ на «открой мне это» — удача и неудача одной формы.
///
/// Публикует его модуль сам, своим стабом (`crate::emit::on_open_result`):
/// SDK топиков модуля не знает и знать не должен, иначе исходящая связь
/// перестала бы быть объявленной в его schema.yaml.
pub fn opened(result: Result<ResourceHandle, String>, correlation_id: String) -> ResourceOpened {
    let (handle, error) = match result {
        Ok(handle) => (Some(handle), String::new()),
        Err(error) => (None, error),
    };
    ResourceOpened { handle, error, correlation_id }
}

/// Ответ, которого никто не ждал: ресурс в нём всё равно наш, поэтому
/// освобождаем — рассогласование должно стоить строчки в логе, а не утечки.
pub fn discard(topic: &str, opened: ResourceOpened) {
    log::warn!(target: "handlers", "{} без учтённого запроса: {}", topic, opened.correlation_id);
    if let Some(handle) = opened.handle {
        crate::abi::arena_free(handle.id);
    }
}

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
