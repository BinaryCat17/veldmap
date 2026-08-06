//! ABI-мост в хост: extern-декларации и safe-обёртки. Единственный
//! синхронный слой SDK — вызовы в состояние хоста (память, graphics,
//! конфиг, энтропия) и отправка событий в шину (fire-and-forget).

use crate::proto::core::EventEnvelope;
use prost::Message;

// ── VELD MICROKERNEL ABI ───────────────────────────────────────

extern "C" {
    fn veld_host_publish(ptr: u64, len: u64);
    fn veld_host_log(level: u64, target_ptr: u64, target_len: u64, ptr: u64, len: u64);
    fn veld_get_config(ptr: u64, len: u64) -> u64;
    fn veld_random_bytes(ptr: u64, len: u64);
    fn veld_graphics_create_resource(ptr: u64, len: u64) -> u64;
    fn veld_graphics_execute(ptr: u64, len: u64) -> u64;

    fn veld_memory_write(id: u64, offset: u64, ptr: u64, len: u64);
    fn veld_memory_read(id: u64, offset: u64, size: u64) -> u64;

    fn veld_memory_texture_size(id: u64) -> u64;

    fn veld_task_kill(ptr: u64, len: u64) -> u64;

    fn veld_memory_alloc_buffer(size: u64, usage: u64, mapped: u64) -> u64;
    fn veld_memory_alloc_cpu(size: u64) -> u64;
    fn veld_memory_alloc_texture(width: u64, height: u64, format: u64, usage: u64) -> u64;
    fn veld_memory_transfer(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
    fn veld_memory_grant_read(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
    fn veld_memory_grant_write(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
    fn veld_memory_revoke(region_id: u64) -> u64;
    fn veld_memory_free(region_id: u64) -> u64;

    fn veld_input_len() -> u64;
    fn veld_input_copy(p: u64, n: u64);
    fn veld_output_set(p: u64, n: u64);
}

// ── WASM MEMORY EXPORTS ────────────────────────────────────────

// Выделение/освобождение памяти wasm для ответов хоста: хост пишет
// результаты синхронных ABI-вызовов через veld_alloc (см. host
// write_response_back). vec![0u8; size] гарантирует capacity == size,
// иначе from_raw_parts в veld_free_wasm был бы UB.
#[doc(hidden)]
#[no_mangle]
pub extern "C" fn veld_alloc(size: u64) -> u64 {
    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as u64
}

#[doc(hidden)]
#[no_mangle]
pub unsafe extern "C" fn veld_free_wasm(ptr: u64, size: u64) {
    let _ = Vec::from_raw_parts(ptr as *mut u8, size as usize, size as usize);
}

// ── Шина событий ───────────────────────────────────────────────

/// Забирает сырой буфер хоста: (len << 32 | ptr) → байты, 0 → None.
/// Память под ответ хост выделил через veld_alloc; здесь она освобождается.
unsafe fn take_host_bytes(packed: u64) -> Option<Vec<u8>> {
    if packed == 0 {
        return None;
    }
    let ptr = (packed & 0xFFFF_FFFF) as *mut u8;
    let len = (packed >> 32) as usize;
    let bytes = std::slice::from_raw_parts(ptr, len).to_vec();
    veld_free_wasm(ptr as u64, len as u64);
    Some(bytes)
}

/// Распаковывает ответ хоста: (len << 32 | ptr) → байты, первый байт —
/// тег (0 = успех, дальше payload; 1 = ошибка, дальше UTF-8 текст),
/// см. host-side `abi.rs::tagged_response`.
unsafe fn take_host_response(packed: u64, what: &str) -> anyhow::Result<Vec<u8>> {
    let buf = take_host_bytes(packed).ok_or_else(|| anyhow::anyhow!("{} failed", what))?;
    match buf.split_first() {
        Some((0, payload)) => Ok(payload.to_vec()),
        Some((1, msg)) => Err(anyhow::anyhow!(String::from_utf8_lossy(msg).into_owned())),
        _ => Err(anyhow::anyhow!("{}: malformed response", what)),
    }
}

pub fn graphics_create_resource(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_graphics_create_resource(payload.as_ptr() as u64, payload.len() as u64);
        take_host_response(packed, "Graphics create_resource")
    }
}

pub fn graphics_execute(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_graphics_execute(payload.as_ptr() as u64, payload.len() as u64);
        take_host_response(packed, "Graphics execute")
    }
}

// ── Memory data access ─────────────────────────────────────────

/// Write data into a memory region
pub fn arena_write(id: u64, offset: u64, data: &[u8]) {
    unsafe { veld_memory_write(id, offset, data.as_ptr() as u64, data.len() as u64); }
}

/// Read a byte range from a memory region into the caller's own wasm memory
/// (not a raw pointer — enforces the access boundary, avoids data races).
/// `None` — region not found, access denied, or read out of bounds.
pub fn arena_read(id: u64, offset: u64, size: u64) -> Option<Vec<u8>> {
    unsafe {
        let packed = veld_memory_read(id, offset, size);
        take_host_bytes(packed)
    }
}

/// Размеры текстуры в пикселях. `None` — регион не текстура, не найден или
/// доступ запрещён. Нужно тому, кто рисует чужую текстуру: вписать её в
/// отведённое место можно только зная её пропорции.
pub fn arena_texture_size(id: u64) -> Option<(u32, u32)> {
    let packed = unsafe { veld_memory_texture_size(id) };
    if packed == 0 { return None; }
    Some(((packed >> 32) as u32, packed as u32))
}

// ── Фоновые задачи ─────────────────────────────────────────────
// Вся система задач со стороны модуля: убить операцию. Заводить и закрывать
// её нечем — учёт ведёт диспетчер по самим публикациям (dispatcher::account).

#[doc(hidden)]
/// Убивает операцию: исполнителя снимают на месте, ничего не доделывая.
/// `false` — убивать было нечего (кончилась сама либо не была отменяемой).
///
/// Звать напрямую не нужно: у каждого отменяемого вызова есть свой стаб
/// `crate::cancel::<сервис>::<топик>`, и только у него — убить то, что
/// отменяемым не объявлено, нельзя по построению.
pub fn task_kill(task_id: &str) -> bool {
    unsafe { veld_task_kill(task_id.as_ptr() as u64, task_id.len() as u64) != 0 }
}

// ── Memory management ──────────────────────────────────────────

/// Allocate a GPU buffer region in the resource registry
pub fn arena_alloc_buffer(size: u64, usage: u32, mapped: bool) -> Option<u64> {
    let id = unsafe { veld_memory_alloc_buffer(size, usage as u64, mapped as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Allocate a plain CPU-backed memory region — not a GPU buffer. For moving
/// bytes to the host that have nothing to do with rendering (e.g. a file
/// through fs/on_write): `arena_alloc_buffer`'s usage/mapped params are wgpu
/// semantics that don't apply here, so this is a sibling, not that function
/// made more generic.
pub fn arena_alloc_cpu(size: u64) -> Option<u64> {
    let id = unsafe { veld_memory_alloc_cpu(size) };
    if id == 0 { None } else { Some(id) }
}

/// Allocate a GPU texture region in the resource registry
pub fn arena_alloc_texture(width: u32, height: u32, format: i32, usage: u32) -> Option<u64> {
    let id = unsafe { veld_memory_alloc_texture(width as u64, height as u64, format as u64, usage as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Transfer ownership of a region to another service (zero-copy).
/// Низкий уровень ABI: прикладной код пользуется `resource::hand_off`,
/// который при отказе освобождает ресурс.
pub(crate) fn arena_transfer(region_id: u64, service: &str) -> bool {
    unsafe { veld_memory_transfer(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Grant read access to another service (owner only).
/// Низкий уровень ABI: прикладной код — `resource::grant_read_or_free`.
pub(crate) fn arena_grant_read(region_id: u64, service: &str) -> bool {
    unsafe { veld_memory_grant_read(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Grant write access to another service (owner only).
/// Низкий уровень ABI: прикладной код — `resource::grant_write_or_free`.
pub(crate) fn arena_grant_write(region_id: u64, service: &str) -> bool {
    unsafe { veld_memory_grant_write(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Revoke all external access to a region
pub fn arena_revoke(region_id: u64) -> bool {
    unsafe { veld_memory_revoke(region_id) != 0 }
}

/// Free a resource region.
/// Низкий уровень ABI: прикладной код пользуется `OwnedResource` (RAII)
/// и обрядами `resource::*`; прямой вызов — признак переписанного вручную
/// обряда.
pub(crate) fn arena_free(region_id: u64) -> bool {
    unsafe { veld_memory_free(region_id) != 0 }
}

// ── Logging ────────────────────────────────────────────────────

/// Транспорт для HostLogger: прикладной код пишет макросами крейта `log`.
/// `target` — подсистема записи; хост дополнит его именем плагина
/// (`veldmap::<plugin>::<target>`) и отфильтрует.
#[doc(hidden)]
pub fn log(level: log::Level, target: &str, message: &str) {
    let level_u64 = match level {
        log::Level::Error => 4u64, log::Level::Warn => 3u64, log::Level::Info => 2u64,
        log::Level::Debug => 1u64, _ => 0u64,
    };
    unsafe {
        veld_host_log(
            level_u64,
            target.as_ptr() as u64, target.len() as u64,
            message.as_ptr() as u64, message.len() as u64,
        );
    }
}

// ── System helpers ─────────────────────────────────────────────

/// Значение из конфига модуля (инжектирован хостом при загрузке).
/// None — ключа нет.
pub fn get_config(key: &str) -> Option<String> {
    unsafe {
        let packed = veld_get_config(key.as_ptr() as u64, key.len() as u64);
        take_host_bytes(packed).and_then(|b| String::from_utf8(b).ok())
    }
}

/// UUID v4 из хостовой энтропии: у wasm нет своего источника случайности.
pub fn generate_id() -> String {
    let mut b = [0u8; 16];
    unsafe { veld_random_bytes(b.as_mut_ptr() as u64, b.len() as u64) };
    b[6] = (b[6] & 0x0f) | 0x40; // версия 4
    b[8] = (b[8] & 0x3f) | 0x80; // вариант RFC 4122
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

// ── Call context ───────────────────────────────────────────────

/// Конверт обрабатываемого сейчас события — то, что известно о вызове помимо
/// payload (заполняется handle_event из конверта, который кодирует хост, —
/// полям можно доверять). Wasm однопоточный, поэтому простого Mutex хватает.
static EVENT_PUBLISHER: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static EVENT_CORRELATION: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Заполняется handle_event сгенерированного клея — читается через
/// [`event_publisher`] и [`correlation`].
#[doc(hidden)]
pub fn set_event_context(publisher: String, correlation: String) {
    *EVENT_PUBLISHER.lock().unwrap() = publisher;
    *EVENT_CORRELATION.lock().unwrap() = correlation;
}

/// Имя сервиса, опубликовавшего текущее событие ("" — хост).
/// Для авторизации: сравнивайте с ожидаемым отправителем.
pub fn event_publisher() -> String {
    EVENT_PUBLISHER.lock().unwrap().clone()
}

/// Корреляция текущего события: у запроса — id, которым заказчик его пометил
/// (в ответ его надо вернуть тем же), у ответа — id нашего запроса, по
/// которому он ищется в таблице ожиданий. "" — топик не объявлен парой
/// `replies_to`, и корреспондента у события нет.
///
/// Раньше это же значение приезжало полем доменного сообщения, из-за чего
/// каждый контракт обязан был его объявить, а прикладной код — протащить.
pub fn correlation() -> String {
    EVENT_CORRELATION.lock().unwrap().clone()
}

/// Вход текущего вызова хоста (конверт события, конфиг при init).
/// Читает сгенерированный клей — модуль получает уже разобранный аргумент.
#[doc(hidden)]
pub fn load_input() -> Vec<u8> {
    unsafe {
        let len = veld_input_len();
        if len == 0 { return Vec::new(); }
        let mut buf = vec![0u8; len as usize];
        veld_input_copy(buf.as_mut_ptr() as u64, len);
        buf
    }
}

/// Результат текущего вызова хоста (например, список подписок).
/// Пишет сгенерированный клей.
#[doc(hidden)]
pub fn store_output(data: Vec<u8>) {
    unsafe { veld_output_set(data.as_ptr() as u64, data.len() as u64); }
}

// ── Pub/Sub ────────────────────────────────────────────────────

/// Единственная форма общения между сервисами: fire-and-forget событие в
/// шину. Не часть публичного API: прикладные модули используют
/// сгенерированные стабы (crate::emit::* для своих выходов, crate::calls::*
/// для объявленных зависимостей). Строковый топик здесь — это нормально: он
/// существует только внутри стабов.
/// `correlation` — пусто у топиков, не объявленных парой `replies_to`; иначе
/// id запроса (у ответа — тот, что пришёл с запросом). `target` — пусто при
/// broadcast; непустой доставляется только подписчику с этим именем и
/// используется для output'ов с `targeted: true` (адресат известен лишь в
/// рантайме — например, ui-service рассылает события владельцам виджетов).
/// Что чем заполнять, решает не автор модуля, а схема: стабы подставляют
/// сюда константы или свои аргументы.
#[doc(hidden)]
pub fn publish(topic: &str, payload: Vec<u8>, correlation: &str, target: &str) {
    let parts: Vec<&str> = topic.splitn(2, '/').collect();
    if parts.len() != 2 {
        log::error!(target: "sdk", "Invalid topic: {}", topic);
        return;
    }
    // publisher не заполняется: его штампует хост при доставке, а в исходящем
    // сообщении игнорирует — иначе паблишер мог бы назваться чужим именем.
    let request = EventEnvelope {
        service: parts[0].to_string(), method: parts[1].to_string(), payload,
        publisher: String::new(),
        correlation_id: correlation.to_string(),
        target: target.to_string(),
    };
    let req_buf = request.encode_to_vec();
    unsafe { veld_host_publish(req_buf.as_ptr() as u64, req_buf.len() as u64); }
}
