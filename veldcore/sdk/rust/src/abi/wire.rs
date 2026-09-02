//! Кодировка ответа синхронного ABI-вызова — один файл по обе стороны провода.
//!
//! SDK (`veldsdk::abi`) разбирает им то, что собирают хост (`abi.rs` ядра
//! включает этот файл через `#[path]`) и фальшивый хост тестов SDK
//! (`veldsdk::fake`). Две копии сходились бы, только пока кто-то держит их
//! одинаковыми, а фальшивка с собственной копией доказывала бы согласие SDK с
//! собой, а не с хостом.
//!
//! Ответ — байты: первый — тег, дальше либо полезная нагрузка, либо UTF-8
//! текст причины отказа. Возвращается он одним `u64` — «длина ≪ 32 |
//! указатель»: место под байты просит сам гость (`veld_alloc`), указатель
//! лежит в его линейной памяти, а ноль значит «ответа нет вовсе».

/// Тег удачи: дальше полезная нагрузка.
pub const OK: u8 = 0;
/// Тег отказа: дальше текст причины.
pub const ERR: u8 = 1;

/// Кодирует результат: тег и байты за ним. Гостю нужен только разбор; собирают
/// ответ хост и фальшивый хост, которые включают этот файл, — у wasm-цели
/// сборка поэтому лежит без дела.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn tagged(result: Result<Vec<u8>, String>) -> Vec<u8> {
    match result {
        Ok(payload) => {
            let mut buf = Vec::with_capacity(1 + payload.len());
            buf.push(OK);
            buf.extend_from_slice(&payload);
            buf
        }
        Err(why) => {
            let mut buf = Vec::with_capacity(1 + why.len());
            buf.push(ERR);
            buf.extend_from_slice(why.as_bytes());
            buf
        }
    }
}

/// Упаковывает ответ, лежащий по `ptr` длиной `len`.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub fn pack(ptr: u64, len: u64) -> u64 {
    (len << 32) | ptr
}

/// Разбирает упакованную пару в `(ptr, len)`; `None` — ответа нет. Хост
/// только пакует, разбирает SDK — отсюда `allow(dead_code)`, как у
/// `log_level.rs`.
#[allow(dead_code)]
pub fn unpack(packed: u64) -> Option<(u64, u64)> {
    if packed == 0 {
        return None;
    }
    Some((packed & 0xFFFF_FFFF, packed >> 32))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Пара переживает упаковку, а ноль — это «нет ответа», не пустой ответ.
    #[test]
    fn a_pair_survives_packing_and_zero_means_nothing() {
        assert_eq!(unpack(pack(0x1000, 300)), Some((0x1000, 300)));
        assert_eq!(unpack(0), None);
    }

    /// Тег — первый байт, и он один на удачу и на отказ.
    #[test]
    fn the_tag_is_the_first_byte() {
        assert_eq!(tagged(Ok(vec![7, 8])), vec![OK, 7, 8]);
        assert_eq!(tagged(Err("нет".to_string())), [&[ERR][..], "нет".as_bytes()].concat());
    }
}
