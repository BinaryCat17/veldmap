//! Ключи thread-local для однопоточного wasm-модуля — вместо musl'овских.
//!
//! Модуль — реактор WASI без `_start`: дескриптор главного потока его libc
//! никто не поднимает, а `pthread_key_delete` musl обходит через него кольцо
//! потоков и без кольца не возвращается. Ключи нужны самой std — ими она
//! регистрирует деструктор всякого `thread_local!`, и первое такое место в
//! модуле (декодер JPEG 2000) вешало инстанс. Поток один, ключ — ячейка
//! таблицы. Определения перекрывают libc при линковке; клей модуля ссылается
//! на них, чтобы они попали в модуль. Экспорт `_initialize` Rust-cdylib не
//! отдаёт, а конструкторы libc дескриптор не чинят (проверено пробником).

use std::ffi::c_void;

/// Сколько ключей бывает у одного модуля: std берёт единицы.
const KEYS: usize = 64;

static mut USED: [bool; KEYS] = [false; KEYS];
static mut VALUES: [*mut c_void; KEYS] = [std::ptr::null_mut(); KEYS];

/// EAGAIN по нумерации WASI: ключи кончились.
const EAGAIN: i32 = 6;
/// EINVAL по нумерации WASI: такого ключа нет.
const EINVAL: i32 = 28;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_key_create(
    key: *mut u32,
    _destructor: Option<unsafe extern "C" fn(*mut c_void)>,
) -> i32 {
    // SAFETY: поток один, таблица статическая; указатель дал вызывающий.
    unsafe {
        for at in 0..KEYS {
            if !USED[at] {
                USED[at] = true;
                VALUES[at] = std::ptr::null_mut();
                *key = at as u32;
                return 0;
            }
        }
    }
    EAGAIN
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_key_delete(key: u32) -> i32 {
    let Some(at) = slot(key) else { return EINVAL };
    // SAFETY: поток один, таблица статическая.
    unsafe { USED[at] = false };
    0
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_getspecific(key: u32) -> *mut c_void {
    let Some(at) = slot(key) else { return std::ptr::null_mut() };
    // SAFETY: поток один, таблица статическая.
    unsafe { VALUES[at] }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn pthread_setspecific(key: u32, value: *const c_void) -> i32 {
    let Some(at) = slot(key) else { return EINVAL };
    // SAFETY: поток один, таблица статическая.
    unsafe { VALUES[at] = value as *mut c_void };
    0
}

fn slot(key: u32) -> Option<usize> {
    let at = key as usize;
    (at < KEYS).then_some(at)
}

/// Ссылка на определения из клея модуля: иначе линковщик взял бы их из
/// libc, а этот файл остался бы в архиве SDK непрочитанным.
pub fn linked() -> usize {
    std::hint::black_box(
        pthread_key_create as *const () as usize
            + pthread_key_delete as *const () as usize
            + pthread_getspecific as *const () as usize
            + pthread_setspecific as *const () as usize,
    )
}
