//! Ресурсы на стороне модуля: владение, протокол «открой мне это» и
//! потоковое чтение.
//!
//! Ресурс не обязан лежать в памяти — за его id может стоять файл на диске
//! (fs отдаёт именно такой), и тогда чтение диапазона читает с диска ровно
//! запрошенное окно. Модулю знать это не нужно: он получает обычный
//! `Read + Seek` и отдаёт его любому парсеру (png, tiff, zip, netcdf…),
//! а память тратится на одно окно, а не на весь ресурс.
//!
//! Вся дисциплина владения собрана здесь: `OwnedResource` (RAII), `hand_off`
//! и `grant_*_or_free` (передача и делегирование с освобождением при отказе).
//! Прямые вызовы `abi::resource_free`/`resource_grant_*` в прикладном коде — признак
//! того, что обряд переписан заново: ровно так копии и расходились формой.

use std::io::{self, BufRead, Read, Seek, SeekFrom};

use crate::proto::core::{ResourceHandle, ResourceOpened};

// ── Владение ───────────────────────────────────────────────────

/// RAII-владелец ресурса хоста: Drop освобождает ресурс через ABI.
/// Намеренно не Clone: освобождает ресурс ровно один владелец.
///
/// Разделяемое владение нужно только GPU-объектам (шейдеры, пайплайны) —
/// у них свои типизированные id в `crate::graphics`; байтовым регионам
/// хватает единоличного владельца.
pub struct OwnedResource {
    handle: ResourceHandle,
}

impl OwnedResource {
    pub fn new(handle: ResourceHandle) -> Self {
        Self { handle }
    }

    pub fn handle(&self) -> ResourceHandle { self.handle.clone() }
    pub fn id(&self) -> u64 { self.handle.id }
    pub fn size(&self) -> u64 { self.handle.size }

    /// Владение по голому id — для протоколов, где размер ресурса не несёт
    /// смысла (буферы, текстуры): на хосте они и живут как чистые id.
    pub fn from_raw_id(id: u64) -> Self {
        Self { handle: ResourceHandle { id, size: 0 } }
    }

    /// Снимает RAII-обёртку, отдавая голый handle: владение переходит
    /// вызывающему, Drop не срабатывает. Это единственный способ передать
    /// обёрнутый ресурс дальше по протоколу (transfer, ответ заказчику) —
    /// без него `OwnedResource` и `hand_off` были несовместимы, и модули
    /// держали голые id, освобождая их вручную.
    pub fn into_handle(self) -> ResourceHandle {
        let handle = self.handle.clone();
        std::mem::forget(self);
        handle
    }

    /// Передаёт владение заказчику: тот же обряд, что `hand_off` для голого
    /// handle, — при отказе ресурс освобождён, при успехе освобождать его
    /// больше некому (им владеет `owner`).
    pub fn hand_off(self, owner: &str) -> Result<ResourceHandle, String> {
        hand_off(self.into_handle(), owner)
    }
}

impl Drop for OwnedResource {
    fn drop(&mut self) {
        crate::abi::resource_free(self.handle.id);
    }
}

impl AsRef<ResourceHandle> for OwnedResource {
    fn as_ref(&self) -> &ResourceHandle { &self.handle }
}

// ── Протокол «открой мне это» ──────────────────────────────────
//
// Отвечают открытым ресурсом четверо, и все одним `core.ResourceOpened`
// (см. core.proto): `fs` и `network` со стороны платформы, `data-provider` и
// `data-library` — чужими руками. Обряд вокруг него один: узнать заказчика,
// дождаться ресурса, передать ему владение, ответить.
//
// Здесь собрана его wasm-половина; у нативных `fs` и `network` своё зеркало
// (`host/util/src/resource.rs`) — до SDK им не дотянуться, — и сходятся эти
// двое раскладкой ответа. Собран он в двух местах, а не в шести: разойдясь,
// копии разошлись бы и текстами отказов, а по ним заказчик отличает «нет
// такого» от «не отдам». Передают владение и те, кто отвечает своим типом
// (`tile-cache`, `image-tiler` — тайлом), — им из обряда нужна середина.

/// Заказчик текущего события — тот, кому уйдёт владение открытым ресурсом.
///
/// `Err` — событие опубликовал хост: у него нет модульной идентичности, а
/// значит `resource_transfer` некому адресовать. Отдать ресурс «в никуда» нельзя,
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
    accept_parts(opened.handle.clone(), &opened.error)
}

/// Тот же разбор, что `accept`, но по частям — для ответов, которые несут
/// рядом с ресурсом ещё что-нибудь своё: обряд «error пуст? → handle есть?»
/// один, какой бы формы ни был ответ.
///
/// Зовёт её один `accept`: других ответов с парой «handle и error» в дереве
/// нет. Тайл, скажем, несёт рядом с ресурсом свои поля, но об ошибке говорит
/// не сам, а терминальным ответом — и разбирается поэтому иначе. Открыта она
/// на такой ответ: заведись он, разбирать его будут отсюда, а не заново.
pub fn accept_parts(handle: Option<ResourceHandle>, error: &str) -> Result<ResourceHandle, String> {
    if !error.is_empty() {
        return Err(error.to_string());
    }
    handle.ok_or_else(|| "ответ без handle".to_string())
}

/// Передаёт владение ресурсом заказчику.
///
/// При отказе ресурс освобождается здесь же: иначе он остался бы висеть на
/// открывшем, которому он уже не нужен, — а заказчик про него не узнает и
/// освободить не сможет.
pub fn hand_off(handle: ResourceHandle, owner: &str) -> Result<ResourceHandle, String> {
    if crate::abi::resource_transfer(handle.id, owner) {
        return Ok(handle);
    }
    crate::abi::resource_free(handle.id);
    Err(format!("не удалось передать ресурс сервису '{}'", owner))
}

/// Выдаёт сервису право чтения ресурса. При отказе ресурс освобождается
/// здесь же — та же форма «действие, при отказе — освободи», что у
/// `hand_off`: владельцу ресурс после неудачного делегирования всё равно
/// не нужен, а без хелпера free рядом с каждым grant переписывался руками
/// (и тексты ошибок расходились по модулям).
pub fn grant_read_or_free(id: u64, service: &str) -> Result<(), String> {
    if crate::abi::resource_grant_read(id, service) {
        return Ok(());
    }
    crate::abi::resource_free(id);
    Err(format!("не удалось выдать чтение ресурса {} сервису '{}'", id, service))
}

/// Выдаёт сервису право записи — так владелец окна делегирует рендереру
/// свою текстуру-цель. Форма отказа та же, что у `grant_read_or_free`.
pub fn grant_write_or_free(id: u64, service: &str) -> Result<(), String> {
    if crate::abi::resource_grant_write(id, service) {
        return Ok(());
    }
    crate::abi::resource_free(id);
    Err(format!("не удалось выдать запись ресурса {} сервису '{}'", id, service))
}

/// Собирает ответ на «открой мне это» — удача и неудача одной формы.
///
/// Публикует его модуль сам, своим стабом (`crate::emit::on_open_result`), и
/// передаёт туда же корреляцию запроса: SDK топиков модуля не знает и знать
/// не должен, иначе исходящая связь перестала бы быть объявленной в его
/// schema.yaml.
pub fn opened(result: Result<ResourceHandle, String>) -> ResourceOpened {
    let (handle, error) = match result {
        Ok(handle) => (Some(handle), String::new()),
        Err(error) => (None, error),
    };
    ResourceOpened { handle, error }
}

/// Ответ, которого мы не ждём. На разделяемом топике ответов
/// (`fs/on_read_result` слушают и data-library, и tile-cache, и data-browser)
/// это чужой ответ: broadcast-конвенция шины доставляет его всем подписчикам, и
/// незнакомая корреляция — норма, а не рассогласование. Лог поэтому debug, а не
/// warn: warn выл бы на каждый чужой ответ.
///
/// Ресурс в таком ответе не трогается — ни разбирается, ни освобождается.
/// Освобождать по «не наш» нельзя, и проверка владения на хосте от этого не
/// спасает: ресурс чужого ответа к этому мигу вполне может быть уже нашим — его
/// передал нам тот, чей это ответ, и обе доставки одного и того же открытия
/// разбираем мы. Так и ломался просмотр скачанного файла: fs отвечал
/// библиотеке, библиотека тем же ходом передавала ресурс нам, а мы, разбирая ту
/// же рассылку как чужую, освобождали его — и собственный ответ приезжал с
/// мёртвым дескриптором.
///
/// Свой ресурс, которому не нашлось дела, освобождает [`release`] — там, где
/// учёт есть и о ненадобности известно.
pub fn discard(topic: &str, _opened: ResourceOpened) {
    log::debug!(target: "handlers", "{} не наш: {}", topic, crate::abi::correlation());
}

/// Освобождает ресурс, которому не нашлось дела: ответ учтённый и владение
/// уже наше, но показывать некому — вкладку закрыли, запрос вытеснили.
/// Именованный обряд вместо трюка `drop(OwnedResource::new(...))` на
/// call-сайте.
pub fn release(handle: ResourceHandle) {
    crate::abi::resource_free(handle.id);
}

/// Сквозное звено «открой мне это»: принять открытое и передать владение
/// заказчику, собрав ему ответ той же формы. Так отвечают открывающие чужими
/// руками: data-library — файлом у fs, data-provider — подписав и спросив
/// network. При отказе на любом шаге ресурс уже освобождён составными
/// частями, а в ответе — текст причины.
pub fn relay(opened: &ResourceOpened, owner: &str) -> ResourceOpened {
    self::opened(accept(opened).and_then(|handle| hand_off(handle, owner)))
}

/// Окно чтения. Компромисс между числом ABI-вызовов и памятью: гигабайтный
/// файл при 256 КБ окна — это тысячи вызовов, что на фоне декодирования
/// незаметно.
pub(crate) const WINDOW: u64 = 256 * 1024;

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
        let data = crate::abi::resource_read(self.id, self.pos, size).map_err(|e| {
            io::Error::new(io::ErrorKind::Other,
                format!("resource {}: чтение {} байт со смещения {}: {}", self.id, size, self.pos, e))
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

/// Читатель уже буферизован своим окном, поэтому `BufRead` он реализует сам, а
/// не через обёртку `BufReader`: та завела бы второй буфер поверх первого и
/// копировала бы окно в него целиком.
impl BufRead for ResourceReader {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if !self.fill()? {
            return Ok(&[]);
        }
        let from = (self.pos - self.window_at) as usize;
        Ok(&self.window[from..])
    }

    fn consume(&mut self, amt: usize) {
        // Клампим по размеру ресурса: контракт `BufRead` запрещает потреблять
        // больше отданного, но нарушение здесь — тихий выход за конец, а не
        // ошибка, поэтому границу держим сами.
        self.pos = (self.pos + amt as u64).min(self.len);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake;

    /// Читатель над смонтированными байтами читает их окнами ровно по одному
    /// разу — [0, len) без перехлёста и дыр, последнее окно короткое.
    #[test]
    fn the_reader_walks_a_resource_in_windows_exactly_once() {
        fake::install();
        let tail = 123u64;
        let bytes: Vec<u8> = (0..2 * WINDOW + tail).map(|i| (i % 251) as u8).collect();
        let handle = fake::mount(bytes.clone());

        let mut reader = ResourceReader::new(handle.id, handle.size);
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert_eq!(got, bytes);

        let windows: Vec<(u64, u64)> = fake::reads().iter().map(|r| (r.offset, r.size)).collect();
        assert_eq!(windows, vec![(0, WINDOW), (WINDOW, WINDOW), (2 * WINDOW, tail)]);
    }

    /// Окно начинается там, где было чтение, а не по выровненной границе:
    /// прыжок вперёд внутри окна к хосту не ходит, прыжок назад за его начало
    /// — ходит, и новое окно начинается с места прыжка.
    #[test]
    fn a_window_starts_where_the_read_did() {
        fake::install();
        let handle = fake::mount(vec![1u8; (3 * WINDOW) as usize]);
        let mut reader = ResourceReader::new(handle.id, handle.size);
        let mut byte = [0u8; 1];

        reader.seek(SeekFrom::Start(10)).unwrap();
        reader.read_exact(&mut byte).unwrap();
        reader.seek(SeekFrom::Start(WINDOW)).unwrap();
        reader.read_exact(&mut byte).unwrap();
        assert_eq!(fake::reads().len(), 1, "оба байта из окна [10, 10 + WINDOW)");

        reader.seek(SeekFrom::Start(5)).unwrap();
        reader.read_exact(&mut byte).unwrap();
        let windows: Vec<(u64, u64)> = fake::reads().iter().map(|r| (r.offset, r.size)).collect();
        assert_eq!(windows, vec![(10, WINDOW), (5, WINDOW)]);
    }

    /// Владелец освобождает ресурс ровно раз — после него у хоста ничего не
    /// остаётся.
    #[test]
    fn an_owned_resource_is_freed_on_drop() {
        fake::install();
        let handle = fake::mount(vec![0u8; 16]);
        assert_eq!(fake::leaked(), vec![handle.id]);
        drop(OwnedResource::new(handle));
        assert!(fake::leaked().is_empty());
    }
}
