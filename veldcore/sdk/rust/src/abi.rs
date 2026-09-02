//! ABI-мост в хост: extern-декларации и safe-обёртки. Единственный
//! синхронный слой SDK — вызовы в состояние хоста (память, graphics,
//! энтропия) и отправка событий в шину (fire-and-forget). Конфига среди них
//! нет: он уезжает модулю целиком в `init`.

use crate::proto::core::EventEnvelope;
use prost::Message;

// Уровень лога на проводе и кодировка ответа: файлы включены и в ядро хоста
// (через `#[path]`), чтобы число и его смысл были одним кодом по обе стороны.
mod log_level;
pub(crate) mod wire;

// ── ABI микроядра Veld ─────────────────────────────────────────

// Имя wasm-модуля импортов объявлено явно: хост регистрирует эти функции
// именно в "env" (см. host-side `abi.rs::register`), и подразумеваемое
// умолчание здесь — совпадение, на которое опираться нельзя.
#[cfg(target_arch = "wasm32")]
mod host {
    #[link(wasm_import_module = "env")]
    unsafe extern "C" {
        pub fn veld_host_publish(ptr: u64, len: u64);
        pub fn veld_host_log(level: u64, target_ptr: u64, target_len: u64, ptr: u64, len: u64);
        pub fn veld_random_bytes(ptr: u64, len: u64);
        pub fn veld_resource_create(ptr: u64, len: u64) -> u64;
        pub fn veld_graphics_execute(ptr: u64, len: u64) -> u64;

        pub fn veld_resource_write(id: u64, offset: u64, ptr: u64, len: u64) -> u64;
        pub fn veld_resource_upload_image(id: u64, ptr: u64, len: u64) -> u64;
        pub fn veld_resource_read(id: u64, offset: u64, size: u64) -> u64;

        pub fn veld_resource_texture_size(id: u64) -> u64;

        pub fn veld_task_kill(ptr: u64, len: u64) -> u64;

        pub fn veld_resource_alloc_buffer(size: u64, usage: u64, mapped: u64) -> u64;
        pub fn veld_resource_alloc_cpu(size: u64) -> u64;
        pub fn veld_resource_alloc_texture(width: u64, height: u64, format: u64, usage: u64) -> u64;
        pub fn veld_resource_transfer(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
        pub fn veld_resource_grant_read(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
        pub fn veld_resource_grant_write(region_id: u64, name_ptr: u64, name_len: u64) -> u64;
        pub fn veld_resource_free(region_id: u64) -> u64;

        pub fn veld_input_len() -> u64;
        pub fn veld_input_copy(p: u64, n: u64);
        pub fn veld_output_set(p: u64, n: u64);
    }
}

// Нативные заглушки хостовых функций — для `cargo test`: юнит-тесты чистой
// логики линкуются нативно, где хоста нет. Без фальшивого хоста
// (`crate::fake::install`) они ведут себя как хост, у которого кончилось всё:
// аллокации не выдаются, ресурсы не находятся, публикации уходят в никуда.
// Поднятый на потоке фальшивый хост отвечает вместо них — из своих ресурсов и
// в свои журналы. Единственное, что живёт здесь само, — энтропия: она
// детерминированная и ненулевая, потому что на ней стоит уникальность
// корреляций, которую тесты и проверяют.
//
// `unsafe fn`, как и настоящие импорты: call-сайты не отличают заглушку от
// хоста ни формой, ни требованиями.
#[cfg(not(target_arch = "wasm32"))]
mod host {
    #![allow(clippy::missing_safety_doc)]

    use crate::fake;

    unsafe fn bytes<'a>(ptr: u64, len: u64) -> &'a [u8] {
        if len == 0 {
            return &[];
        }
        unsafe { std::slice::from_raw_parts(ptr as *const u8, len as usize) }
    }

    unsafe fn text<'a>(ptr: u64, len: u64) -> &'a str {
        std::str::from_utf8(unsafe { bytes(ptr, len) }).unwrap_or("")
    }

    pub unsafe fn veld_host_publish(ptr: u64, len: u64) {
        fake::publish(unsafe { bytes(ptr, len) });
    }

    pub unsafe fn veld_host_log(level: u64, tp: u64, tl: u64, p: u64, l: u64) {
        fake::log(super::log_level::from_wire(level), unsafe { text(tp, tl) }, unsafe { text(p, l) });
    }

    pub unsafe fn veld_random_bytes(ptr: u64, len: u64) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEED: AtomicU64 = AtomicU64::new(0x9E37_79B9_7F4A_7C15);
        let bytes = unsafe { std::slice::from_raw_parts_mut(ptr as *mut u8, len as usize) };
        for chunk in bytes.chunks_mut(8) {
            let value = SEED.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed).to_le_bytes();
            chunk.copy_from_slice(&value[..chunk.len()]);
        }
    }

    pub unsafe fn veld_resource_create(_ptr: u64, _len: u64) -> u64 { fake::unsupported("собирать GPU-объекты") }
    pub unsafe fn veld_graphics_execute(_ptr: u64, _len: u64) -> u64 { fake::unsupported("рисовать") }
    pub unsafe fn veld_resource_write(id: u64, offset: u64, ptr: u64, len: u64) -> u64 {
        fake::resource_write(id, offset, unsafe { bytes(ptr, len) })
    }
    pub unsafe fn veld_resource_upload_image(_id: u64, _ptr: u64, _len: u64) -> u64 { fake::unsupported("заливать текстуры") }
    pub unsafe fn veld_resource_read(id: u64, offset: u64, size: u64) -> u64 { fake::resource_read(id, offset, size) }
    pub unsafe fn veld_resource_texture_size(_id: u64) -> u64 { 0 }
    pub unsafe fn veld_task_kill(ptr: u64, len: u64) -> u64 { fake::task_kill(unsafe { text(ptr, len) }) }
    pub unsafe fn veld_resource_alloc_buffer(size: u64, _usage: u64, _mapped: u64) -> u64 { fake::alloc_bytes(size) }
    pub unsafe fn veld_resource_alloc_cpu(size: u64) -> u64 { fake::alloc_bytes(size) }
    pub unsafe fn veld_resource_alloc_texture(_w: u64, _h: u64, _f: u64, _u: u64) -> u64 { fake::alloc_opaque() }
    pub unsafe fn veld_resource_transfer(id: u64, np: u64, nl: u64) -> u64 { fake::transfer(id, unsafe { text(np, nl) }) }
    pub unsafe fn veld_resource_grant_read(id: u64, np: u64, nl: u64) -> u64 { fake::grant_read(id, unsafe { text(np, nl) }) }
    pub unsafe fn veld_resource_grant_write(id: u64, np: u64, nl: u64) -> u64 { fake::grant_write(id, unsafe { text(np, nl) }) }
    pub unsafe fn veld_resource_free(id: u64) -> u64 { fake::free(id) }
    pub unsafe fn veld_input_len() -> u64 { 0 }
    pub unsafe fn veld_input_copy(_p: u64, _n: u64) {}
    pub unsafe fn veld_output_set(_p: u64, _n: u64) {}
}

use host::*;

// ── Экспорты памяти wasm ───────────────────────────────────────

// Выделение/освобождение памяти wasm для ответов хоста: хост пишет
// результаты синхронных ABI-вызовов через veld_alloc (см. host
// write_response_back). vec![0u8; size] гарантирует capacity == size,
// иначе from_raw_parts в veld_free_wasm был бы UB.
#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
#[unsafe(no_mangle)]
pub extern "C" fn veld_alloc(size: u64) -> u64 {
    let mut buf: Vec<u8> = vec![0u8; size as usize];
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr as u64
}

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn veld_free_wasm(ptr: u64, size: u64) {
    let _ = unsafe { Vec::from_raw_parts(ptr as *mut u8, size as usize, size as usize) };
}

// Нативно указатель обязан влезать в 32 бита упаковки — как в wasm, — и
// нативная куча для этого не годится: место под ответы выдаёт арена
// фальшивого хоста, а «указатель» — смещение в ней.
#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub extern "C" fn veld_alloc(size: u64) -> u64 {
    crate::fake::arena::alloc(size)
}

#[cfg(not(target_arch = "wasm32"))]
#[doc(hidden)]
pub unsafe extern "C" fn veld_free_wasm(ptr: u64, size: u64) {
    crate::fake::arena::free(ptr, size)
}

/// Байты гостя за «указателем» ответа: в wasm это адрес, нативно — смещение
/// в арене фальшивого хоста.
#[cfg(target_arch = "wasm32")]
fn guest_address(ptr: u64) -> *const u8 {
    ptr as *const u8
}

#[cfg(not(target_arch = "wasm32"))]
fn guest_address(ptr: u64) -> *const u8 {
    crate::fake::arena::address(ptr)
}

// ── Шина событий ───────────────────────────────────────────────

/// Забирает сырой буфер хоста: упакованная пара → байты, ноль → `None`.
/// Память под ответ хост выделил через `veld_alloc`; здесь она освобождается.
unsafe fn take_host_bytes(packed: u64) -> Option<Vec<u8>> {
    let (ptr, len) = wire::unpack(packed)?;
    let bytes = unsafe { std::slice::from_raw_parts(guest_address(ptr), len as usize) }.to_vec();
    unsafe { veld_free_wasm(ptr, len) };
    Some(bytes)
}

/// Разбирает ответ хоста по общей кодировке (`wire.rs`): тег удачи — дальше
/// нагрузка, тег отказа — дальше текст причины.
unsafe fn take_host_response(packed: u64, what: &str) -> anyhow::Result<Vec<u8>> {
    let buf = unsafe { take_host_bytes(packed) }.ok_or_else(|| anyhow::anyhow!("{} не выполнился", what))?;
    match buf.split_first() {
        Some((&wire::OK, payload)) => Ok(payload.to_vec()),
        Some((&wire::ERR, msg)) => Err(anyhow::anyhow!(String::from_utf8_lossy(msg).into_owned())),
        _ => Err(anyhow::anyhow!("{}: ответ хоста испорчен", what)),
    }
}

/// Создаёт ресурс по дескриптору (`graphics::ResourceRequest`): шейдер,
/// пайплайн, сэмплер, view, bind group и её layout.
///
/// Сосед `resource_alloc_*`, а не отдельная подсистема: и то, и другое кладёт
/// запись в один реестр, и освобождается всё одним `resource_free`. Разница
/// только в форме аргументов — здесь вложенный дескриптор, там скаляры.
pub fn resource_create(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_resource_create(payload.as_ptr() as u64, payload.len() as u64);
        take_host_response(packed, "resource_create")
    }
}

pub fn graphics_execute(payload: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_graphics_execute(payload.as_ptr() as u64, payload.len() as u64);
        take_host_response(packed, "graphics_execute")
    }
}

// ── Доступ к байтам ресурса ────────────────────────────────────

/// Записывает байты по смещению. Только для байтовых носителей — текстуре
/// нужен [`resource_upload_image`].
pub fn resource_write(id: u64, offset: u64, data: &[u8]) -> anyhow::Result<()> {
    unsafe {
        let packed = veld_resource_write(id, offset, data.as_ptr() as u64, data.len() as u64);
        take_host_response(packed, "resource_write").map(|_| ())
    }
}

/// Заливает изображение в текстуру целиком: смещения у неё нет, а данные
/// обязаны покрывать её всю. Отдельно от [`resource_write`] потому, что от
/// записи по смещению здесь не остаётся ни одного параметра.
pub fn resource_upload_image(id: u64, data: &[u8]) -> anyhow::Result<()> {
    unsafe {
        let packed = veld_resource_upload_image(id, data.as_ptr() as u64, data.len() as u64);
        take_host_response(packed, "resource_upload_image").map(|_| ())
    }
}

/// Копирует диапазон в собственную память вызывающего, а не отдаёт указатель:
/// на этом стоит граница доступа и защита от гонок.
///
/// Чтение за концом — не ошибка, а короткий (возможно пустой) буфер: читатель
/// идёт окнами, и последнее окно почти всегда неполное. Ошибка — это «нет
/// прав», «нет ресурса» или «носитель не читается по смещению», и текст
/// причины приезжает вместе с ней.
pub fn resource_read(id: u64, offset: u64, size: u64) -> anyhow::Result<Vec<u8>> {
    unsafe {
        let packed = veld_resource_read(id, offset, size);
        take_host_response(packed, "resource_read")
    }
}

/// Размеры текстуры в пикселях. `None` — регион не текстура, не найден или
/// доступ запрещён. Нужно тому, кто рисует чужую текстуру: вписать её в
/// отведённое место можно только зная её пропорции.
pub fn resource_texture_size(id: u64) -> Option<(u32, u32)> {
    let packed = unsafe { veld_resource_texture_size(id) };
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

// ── Выделение ресурсов ─────────────────────────────────────────

/// `usage`/`mapped` — семантика wgpu: маска `BufferUsages` и запрос
/// mapped_at_creation (запись пойдёт в отображённый диапазон, минуя очередь).
pub fn resource_alloc_buffer(size: u64, usage: u32, mapped: bool) -> Option<u64> {
    let id = unsafe { veld_resource_alloc_buffer(size, usage as u64, mapped as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Обычная память хоста, не буфер GPU: для байт, которым нечего делать в
/// рендеринге (например, файл через fs/on_write). Сосед `resource_alloc_buffer`,
/// а не его частный случай — wgpu-семантика usage/mapped здесь не применима.
pub fn resource_alloc_cpu(size: u64) -> Option<u64> {
    let id = unsafe { veld_resource_alloc_cpu(size) };
    if id == 0 { None } else { Some(id) }
}

/// Текстура — непрозрачный ресурс: читается не по смещению, а рисованием.
/// `None` — размеры не по силам устройству.
pub fn resource_alloc_texture(width: u32, height: u32, format: i32, usage: u32) -> Option<u64> {
    let id = unsafe { veld_resource_alloc_texture(width as u64, height as u64, format as u64, usage as u64) };
    if id == 0 { None } else { Some(id) }
}

/// Передаёт владение другому сервису. Низкий уровень ABI: прикладной код
/// пользуется `resource::hand_off`, который при отказе освобождает ресурс.
pub(crate) fn resource_transfer(region_id: u64, service: &str) -> bool {
    unsafe { veld_resource_transfer(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Право чтения другому сервису (только от владельца). Низкий уровень ABI:
/// прикладной код — `resource::grant_read_or_free`.
pub(crate) fn resource_grant_read(region_id: u64, service: &str) -> bool {
    unsafe { veld_resource_grant_read(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Право записи другому сервису (только от владельца). Низкий уровень ABI:
/// прикладной код — `resource::grant_write_or_free`.
pub(crate) fn resource_grant_write(region_id: u64, service: &str) -> bool {
    unsafe { veld_resource_grant_write(region_id, service.as_ptr() as u64, service.len() as u64) != 0 }
}

/// Освобождает ресурс.
/// Низкий уровень ABI: прикладной код пользуется `OwnedResource` (RAII)
/// и обрядами `resource::*`; прямой вызов — признак переписанного вручную
/// обряда.
pub(crate) fn resource_free(region_id: u64) -> bool {
    unsafe { veld_resource_free(region_id) != 0 }
}

// ── Логи ───────────────────────────────────────────────────────

/// Транспорт для HostLogger: прикладной код пишет макросами крейта `log`.
/// `target` — подсистема записи; хост дополнит его именем плагина
/// (`veldmap::<plugin>::<target>`) и отфильтрует.
#[doc(hidden)]
pub fn log(level: log::Level, target: &str, message: &str) {
    unsafe {
        veld_host_log(
            log_level::to_wire(level),
            target.as_ptr() as u64, target.len() as u64,
            message.as_ptr() as u64, message.len() as u64,
        );
    }
}

// ── Системные помощники ────────────────────────────────────────

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

// ── Контекст вызова ────────────────────────────────────────────

// Конверт обрабатываемого сейчас события — то, что известно о вызове помимо
// payload (заполняется handle_event из конверта, который кодирует хост, —
// полям можно доверять). Wasm однопоточный, и там хватает статики; нативно
// тесты бегут потоками, и контекст, как и фальшивый хост, живёт на потоке —
// иначе один тест читал бы конверт другого.
#[cfg(target_arch = "wasm32")]
mod context {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static PUBLISHER: Mutex<String> = Mutex::new(String::new());
    static CORRELATION: Mutex<String> = Mutex::new(String::new());
    static INTERMEDIATE: AtomicBool = AtomicBool::new(false);

    pub fn set(publisher: String, correlation: String, intermediate: bool) {
        *PUBLISHER.lock().unwrap() = publisher;
        *CORRELATION.lock().unwrap() = correlation;
        INTERMEDIATE.store(intermediate, Ordering::Relaxed);
    }
    pub fn publisher() -> String { PUBLISHER.lock().unwrap().clone() }
    pub fn correlation() -> String { CORRELATION.lock().unwrap().clone() }
    pub fn intermediate() -> bool { INTERMEDIATE.load(Ordering::Relaxed) }
}

#[cfg(not(target_arch = "wasm32"))]
mod context {
    use std::cell::{Cell, RefCell};

    thread_local! {
        static PUBLISHER: RefCell<String> = const { RefCell::new(String::new()) };
        static CORRELATION: RefCell<String> = const { RefCell::new(String::new()) };
        static INTERMEDIATE: Cell<bool> = const { Cell::new(false) };
    }

    pub fn set(publisher: String, correlation: String, intermediate: bool) {
        PUBLISHER.with(|p| *p.borrow_mut() = publisher);
        CORRELATION.with(|c| *c.borrow_mut() = correlation);
        INTERMEDIATE.with(|i| i.set(intermediate));
    }
    pub fn publisher() -> String { PUBLISHER.with(|p| p.borrow().clone()) }
    pub fn correlation() -> String { CORRELATION.with(|c| c.borrow().clone()) }
    pub fn intermediate() -> bool { INTERMEDIATE.with(|i| i.get()) }
}

/// Заполняется handle_event сгенерированного клея — читается через
/// [`event_publisher`], [`correlation`] и [`reply_is_intermediate`].
#[doc(hidden)]
pub fn set_event_context(publisher: String, correlation: String, intermediate: bool) {
    context::set(publisher, correlation, intermediate);
}

/// Придёт ли по этой корреляции ещё ответ: текущее событие объявлено в схеме
/// исполнителя промежуточным (прогресс), и обмен им не кончается.
///
/// `false` не означает «обмен кончился»: у топика может не быть пары вовсе, а
/// терминальный ответ заказчик вправе продолжить следующим обменом с той же
/// корреляцией. Надёжен здесь только `true` — на нём снимать запрос с учёта
/// заведомо рано.
pub fn reply_is_intermediate() -> bool {
    context::intermediate()
}

/// Общая жалоба таблиц ожидания на преждевременное снятие с учёта.
///
/// Не паника и не отказ: ответ уже пришёл, и уронить обработчик значит
/// потерять приехавший в нём ресурс — то же, от чего предостерегаем. Записью
/// в лог ошибка перестаёт быть невидимой, а поведение остаётся прежним.
pub(crate) fn warn_if_intermediate(method: &str) {
    if reply_is_intermediate() {
        crate::log::warn!(
            target: "veldsdk",
            "{} на промежуточном ответе: по корреляции {} придёт ещё один, \
             и опознать его будет уже нечем",
            method, correlation());
    }
}

/// Имя сервиса, опубликовавшего текущее событие ("" — хост).
/// Для авторизации: сравнивайте с ожидаемым отправителем.
pub fn event_publisher() -> String {
    context::publisher()
}

/// Терминальный ответ договорил хост, а не прислал исполнитель: у события
/// нет отправителя. Правило для тех, кто ждёт ответа, — `crate::reply::undelivered`;
/// там же и почему на прочих событиях это спрашивать нельзя.
pub(crate) fn answered_by_host() -> bool {
    event_publisher().is_empty()
}

/// Корреляция текущего события: у запроса — id, которым заказчик его пометил
/// (в ответ его надо вернуть тем же), у ответа — id нашего запроса, по
/// которому он ищется в таблице ожиданий. "" — топик не объявлен парой
/// `replies_to`, и корреспондента у события нет.
///
/// Едет в конверте, а не полем доменного сообщения: иначе её обязан был бы
/// объявить каждый контракт.
pub fn correlation() -> String {
    context::correlation()
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

// ── Публикация в шину ──────────────────────────────────────────

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
        log::error!(target: "sdk", "Негодный топик: {}", topic);
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
